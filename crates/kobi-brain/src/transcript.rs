//! Envoi incrémental de la transcription : chaque message à Claude ne porte
//! que les segments qu'il n'a pas encore vus.
//!
//! [`Outbox`] garde les segments finaux de la réunion et un curseur sur le
//! dernier envoyé. Préparer un message ([`Outbox::ask`],
//! [`Outbox::notes_update`], [`Outbox::report`]) avance le curseur et rend
//! une [`Mark`] : si le tour échoue avant que Claude ait lu le message,
//! [`Outbox::rewind`] remet les segments dans la file, pour qu'aucun ne soit
//! oublié.

use crate::brain::{Ask, NotesUpdate, ReportRequest};

/// Fenêtre mise en évidence avant une question (dossier de conception :
/// « la question et les 2 min qui la précèdent »).
pub const FOCUS_SECONDS: f64 = 120.0;

/// Origine d'un segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Speaker {
    /// L'utilisateur, au micro.
    Moi,
    /// Les autres participants, par l'audio système.
    Eux,
}

impl Speaker {
    fn label(self) -> &'static str {
        match self {
            Speaker::Moi => "moi",
            Speaker::Eux => "eux",
        }
    }
}

/// Segment final de transcription.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Début, en secondes depuis le début de la réunion.
    pub start: f64,
    pub speaker: Speaker,
    pub text: String,
}

/// Position du curseur avant un envoi, pour l'annuler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use = "à garder pour rewind si le tour échoue"]
pub struct Mark(usize);

/// Segments de la réunion et curseur du dernier envoyé.
#[derive(Debug, Default)]
pub struct Outbox {
    lines: Vec<Line>,
    sent: usize,
}

impl Outbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un segment final. Les segments des deux sources arrivent
    /// entrelacés : ils sont rangés par début, mais jamais avant le curseur
    /// (un segment en retard sur ce qui est déjà parti est envoyé avec le
    /// prochain message plutôt que perdu).
    pub fn push(&mut self, line: Line) {
        let at = self.lines[self.sent..]
            .iter()
            .rposition(|l| l.start <= line.start)
            .map_or(self.sent, |i| self.sent + i + 1);
        self.lines.insert(at, line);
    }

    /// Nombre de segments pas encore envoyés.
    pub fn unsent(&self) -> usize {
        self.lines.len() - self.sent
    }

    /// Interpellation posée à `at` secondes : segments pas encore vus, les
    /// 2 minutes précédentes marquées `▶`.
    pub fn ask(&mut self, question: &str, notes: &str, at: f64) -> (Ask, Mark) {
        let (transcript, mark) = self.take(Some(at - FOCUS_SECONDS));
        let ask = Ask {
            question: question.to_owned(),
            transcript,
            notes: notes.to_owned(),
        };
        (ask, mark)
    }

    /// Bloc de notes : segments pas encore vus.
    pub fn notes_update(&mut self, notes: &str) -> (NotesUpdate, Mark) {
        let (transcript, mark) = self.take(None);
        let update = NotesUpdate {
            transcript,
            notes: notes.to_owned(),
            ..Default::default()
        };
        (update, mark)
    }

    /// Compte rendu : fin de transcription pas encore vue.
    pub fn report(&mut self, notes: &str, pending_actions: Vec<String>) -> (ReportRequest, Mark) {
        let (transcript, mark) = self.take(None);
        let report = ReportRequest {
            transcript,
            notes: notes.to_owned(),
            pending_actions,
        };
        (report, mark)
    }

    /// Le message n'a pas été lu (tour en erreur avant envoi, process
    /// planté) : ses segments repartiront avec le suivant.
    pub fn rewind(&mut self, mark: Mark) {
        self.sent = self.sent.min(mark.0);
    }

    fn take(&mut self, focus_from: Option<f64>) -> (String, Mark) {
        let mark = Mark(self.sent);
        let chunk = &self.lines[self.sent..];
        let mut text = String::new();
        for line in chunk {
            let focus = focus_from.is_some_and(|from| line.start >= from);
            text.push_str(&format!(
                "{}[{} {}] {}\n",
                if focus { "▶ " } else { "" },
                timestamp(line.start),
                line.speaker.label(),
                line.text
            ));
        }
        if let Some(from) = focus_from {
            // La fenêtre commence avant ce morceau : Claude a déjà vu le début.
            let earlier = self.lines[..self.sent].iter().any(|l| l.start >= from);
            if earlier {
                text.push_str(&format!(
                    "(Les 2 minutes avant la question commencent à [{}], dans la transcription déjà envoyée.)\n",
                    timestamp(from.max(0.0))
                ));
            } else if chunk.iter().any(|l| l.start >= from) {
                text.push_str("(▶ : les 2 minutes avant la question.)\n");
            }
        }
        self.sent = self.lines.len();
        (text, mark)
    }
}

/// `[mm:ss]`, ou `[h:mm:ss]` au-delà d'une heure.
fn timestamp(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(start: f64, text: &str) -> Line {
        Line {
            start,
            speaker: if (start as u64).is_multiple_of(2) {
                Speaker::Moi
            } else {
                Speaker::Eux
            },
            text: text.into(),
        }
    }

    /// Textes des segments présents dans un message.
    fn texts(transcript: &str) -> Vec<String> {
        transcript
            .lines()
            .map(|l| l.strip_prefix("▶ ").unwrap_or(l))
            .filter(|l| l.starts_with('['))
            .filter_map(|l| l.split_once("] ").map(|(_, t)| t.to_owned()))
            .collect()
    }

    #[test]
    fn horodatage() {
        assert_eq!(timestamp(0.0), "00:00");
        assert_eq!(timestamp(754.9), "12:34");
        assert_eq!(timestamp(3725.0), "1:02:05");
    }

    #[test]
    fn seulement_ce_qui_n_a_pas_ete_vu() {
        let mut out = Outbox::new();
        out.push(line(1.0, "a"));
        out.push(line(2.0, "b"));
        let (update, _) = out.notes_update("");
        assert_eq!(update.transcript, "[00:01 eux] a\n[00:02 moi] b\n");
        out.push(line(3.0, "c"));
        let (ask, _) = out.ask("quoi ?", "- notes", 3.5);
        assert_eq!(texts(&ask.transcript), ["c"]);
        assert_eq!(ask.notes, "- notes");
        assert_eq!(out.unsent(), 0);
        let (report, _) = out.report("", vec![]);
        assert_eq!(report.transcript, "");
    }

    #[test]
    fn deux_minutes_avant_la_question_en_evidence() {
        let mut out = Outbox::new();
        out.push(line(10.0, "ancien"));
        out.push(line(200.0, "récent"));
        let (ask, _) = out.ask("et donc ?", "", 250.0);
        assert_eq!(
            ask.transcript,
            "[00:10 moi] ancien\n▶ [03:20 moi] récent\n(▶ : les 2 minutes avant la question.)\n"
        );
    }

    #[test]
    fn fenetre_deja_envoyee_signalee_sans_renvoi() {
        let mut out = Outbox::new();
        out.push(line(200.0, "vu au bloc de notes"));
        let _ = out.notes_update("");
        out.push(line(240.0, "nouveau"));
        let (ask, _) = out.ask("et donc ?", "", 250.0);
        assert_eq!(texts(&ask.transcript), ["nouveau"]);
        assert!(ask
            .transcript
            .contains("commencent à [02:10], dans la transcription déjà envoyée"));
    }

    #[test]
    fn rewind_apres_un_tour_rate() {
        let mut out = Outbox::new();
        out.push(line(1.0, "a"));
        let (_, mark) = out.ask("q1", "", 2.0);
        out.push(line(3.0, "b"));
        // Le process a planté : le message n'a pas été lu.
        out.rewind(mark);
        let (ask, _) = out.ask("q1 bis", "", 4.0);
        assert_eq!(texts(&ask.transcript), ["a", "b"]);
    }

    #[test]
    fn segment_en_retard_range_par_debut_sans_passer_avant_le_curseur() {
        let mut out = Outbox::new();
        out.push(line(5.0, "eux 5"));
        out.push(line(3.0, "moi 3"));
        let (u, _) = out.notes_update("");
        assert_eq!(texts(&u.transcript), ["moi 3", "eux 5"]);
        // Arrive après l'envoi mais commence avant : part au message suivant.
        out.push(line(4.0, "moi 4 en retard"));
        out.push(line(6.0, "eux 6"));
        let (u, _) = out.notes_update("");
        assert_eq!(texts(&u.transcript), ["moi 4 en retard", "eux 6"]);
    }

    /// Générateur pseudo-aléatoire déterministe (pas de dépendance).
    struct Rng(u64);
    impl Rng {
        fn next(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    #[test]
    fn aucun_segment_envoye_deux_fois_aucun_oublie() {
        for seed in 1..=200u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut out = Outbox::new();
            let mut pushed = Vec::new();
            let mut received: Vec<String> = Vec::new();
            let mut t = 0.0;
            for i in 0..300 {
                match rng.next(10) {
                    // Segments, parfois en retard de quelques secondes.
                    0..=5 => {
                        t += rng.next(5) as f64;
                        let start = (t - rng.next(4) as f64).max(0.0);
                        let text = format!("s{i}");
                        pushed.push(text.clone());
                        out.push(line(start, &text));
                    }
                    6 => received.extend(texts(&out.notes_update("").0.transcript)),
                    7 => received.extend(texts(&out.ask("?", "", t).0.transcript)),
                    // Tour raté : rien n'a été lu, on rembobine.
                    8 => {
                        let (_, mark) = out.ask("?", "", t);
                        out.rewind(mark);
                    }
                    _ => {}
                }
            }
            received.extend(texts(&out.report("", vec![]).0.transcript));
            let mut sorted = received.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(
                sorted.len(),
                received.len(),
                "graine {seed} : segment envoyé deux fois"
            );
            let mut expected = pushed.clone();
            expected.sort();
            assert_eq!(sorted, expected, "graine {seed} : segment oublié");
        }
    }
}

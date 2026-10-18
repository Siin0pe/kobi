//! Écoute continue du micro (KOB-132) : Nemotron transcrit en local, en
//! permanence tant que Kobi n'est pas en pause, et [`Ecouteur`] repère
//! « Kobi » dans la transcription. Rien ne sort de la machine et rien n'est
//! écrit : seule la question part chez Claude, par le fil d'interpellation.
//!
//! Le fil garde le micro ouvert et le modèle chargé. Le raccourci clavier
//! reste un déclencheur de secours : il lui envoie [`Commande::Raccourci`].
//!
//! `KOBI_DIAG=1` journalise le niveau du micro et les segments à part
//! ([`crate::diag`]) ; `KOBI_ECOUTE_WAV=<fichier.wav>` remplace le micro par
//! ce fichier, joué en temps réel puis suivi de silence (essais sans voix).

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kobi_audio::{AudioFrame, Capture, CaptureOptions};
use kobi_session::etat::Evenement;
use kobi_stt::nemotron::NemotronEngine;
use kobi_stt::SttEngine;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::diag::Diag;
use crate::interpellation::{charger_stt, erreur, EVENEMENT_QUESTION};
use crate::mot_cle::{Action, Ecouteur};
use crate::porte::Porte;
use crate::session::{appliquer, etat};

/// Après un échec d'ouverture du micro, nouvel essai après ce délai.
const REESSAI_MICRO: Duration = Duration::from_secs(5);
/// Période du bilan de charge dans le journal.
const BILAN: Duration = Duration::from_secs(300);

/// Ce que l'application demande au fil d'écoute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commande {
    /// Raccourci clavier : la phrase suivante est une question.
    Raccourci,
    /// Met l'écoute en pause, ou la reprend.
    BasculerPause,
}

/// Canal vers le fil d'écoute.
pub struct Ecoute(Mutex<Sender<Commande>>);

/// Lance le fil d'écoute ; les questions repérées partent sur `questions`.
pub fn installer<R: Runtime>(app: &AppHandle<R>, questions: Sender<String>) {
    let (tx, rx) = mpsc::channel();
    app.manage(Ecoute(Mutex::new(tx)));
    let app = app.clone();
    std::thread::Builder::new()
        .name("ecoute".into())
        .spawn(move || Fil::new(app, questions).boucle(rx))
        .expect("fil d'écoute");
}

/// Envoie une commande au fil d'écoute.
pub fn commander<R: Runtime>(app: &AppHandle<R>, commande: Commande) -> Result<(), String> {
    app.state::<Ecoute>()
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .send(commande)
        .map_err(|e| e.to_string())
}

/// D'où vient l'audio.
enum Entree {
    Micro(Capture),
    /// Fichier injecté (`KOBI_ECOUTE_WAV`), joué par un fil à part.
    Fichier(Receiver<AudioFrame>),
}

impl Entree {
    fn frames(&self) -> &Receiver<AudioFrame> {
        match self {
            Entree::Micro(capture) => capture.frames(),
            Entree::Fichier(rx) => rx,
        }
    }

    /// Ouvre le fichier injecté, ou le micro par défaut ; renvoie aussi sa
    /// description pour le journal.
    fn ouvrir() -> Result<(Entree, String), String> {
        if let Some(wav) = std::env::var_os("KOBI_ECOUTE_WAV") {
            let wav = PathBuf::from(wav);
            let samples = kobi_stt::wav::read_wav_16k_mono(&wav)
                .map_err(|e| format!("{} : {e}", wav.display()))?;
            let description = format!(
                "fichier {} ({:.1} s, 16 kHz mono)",
                wav.display(),
                samples.len() as f64 / kobi_stt::SAMPLE_RATE as f64
            );
            return Ok((Entree::Fichier(jouer(samples)), description));
        }
        let capture = Capture::start(&CaptureOptions {
            sources: vec![kobi_audio::Source::Moi],
            mic: None,
            output: None,
        })
        .map_err(|e| e.to_string())?;
        let description = capture
            .sources()
            .iter()
            .map(|s| {
                format!(
                    "micro « {} » ({} Hz, {} canaux, {:?}), rééchantillonné en 16 kHz mono",
                    s.device, s.sample_rate, s.channels, s.sample_format
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok((Entree::Micro(capture), description))
    }
}

/// Joue des échantillons 16 kHz en temps réel, par trames de 100 ms, puis du
/// silence tant que le récepteur existe.
fn jouer(samples: Vec<f32>) -> Receiver<AudioFrame> {
    const TRAME: usize = 1_600;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let silence = vec![0.0; TRAME];
        let mut morceaux = samples.chunks(TRAME);
        let debut = Instant::now();
        for i in 0u64.. {
            let morceau = morceaux.next().unwrap_or(&silence);
            let t = Duration::from_millis(100 * i);
            if let Some(attente) = t.checked_sub(debut.elapsed()) {
                std::thread::sleep(attente);
            }
            let frame = AudioFrame {
                source: kobi_audio::Source::Moi,
                samples: morceau.to_vec(),
                t,
            };
            if tx.send(frame).is_err() {
                return;
            }
        }
    });
    rx
}

/// Transcrit une trame et en déduit les actions de l'écoute. La porte ne
/// laisse passer vers Nemotron que la parole ; l'écoute voit passer le temps
/// à chaque trame. `t` est la fin de la trame en temps du micro (celui du
/// moteur repart de zéro à chaque question).
fn transcrire(
    stt: &mut NemotronEngine,
    porte: &mut Porte,
    ecouteur: &mut Ecouteur,
    diag: &mut Option<Diag>,
    frame: kobi_stt::AudioFrame,
    t: f64,
) -> Vec<Action> {
    if let Some(d) = diag {
        d.trame(frame.t, &frame.samples);
    }
    let mut actions = ecouteur.tick(t);
    let etait_ouverte = porte.ouverte();
    let sorties = porte.trame(frame.t, frame.samples);
    if let (Some(d), true) = (diag.as_mut(), porte.ouverte() != etait_ouverte) {
        d.note(if porte.ouverte() {
            "porte ouverte : parole"
        } else {
            "porte fermée : silence"
        });
    }
    for (debut, samples) in sorties {
        let frame = kobi_stt::AudioFrame {
            source: kobi_stt::Source::Moi,
            samples,
            t: debut,
        };
        match stt.push(&frame) {
            Ok(segments) => {
                for s in segments {
                    let suite = ecouteur.segment(&s.text, s.is_final, t);
                    if let Some(d) = diag {
                        d.segment(&s.text, s.is_final, &suite);
                    }
                    actions.extend(suite);
                }
            }
            Err(e) => tracing::warn!(erreur = %e, "écoute continue : transcription"),
        }
    }
    actions
}

struct Fil<R: Runtime> {
    app: AppHandle<R>,
    questions: Sender<String>,
    stt: Result<NemotronEngine, String>,
    entree: Option<Entree>,
    diag: Option<Diag>,
    porte: Porte,
    ecouteur: Ecouteur,
    en_pause: bool,
    /// Fin de l'audio reçu, en secondes depuis l'ouverture du micro.
    t: f64,
    /// Charge : audio transcrit et temps passé à le transcrire.
    audio: f64,
    calcul: Duration,
    bilan: Instant,
}

impl<R: Runtime> Fil<R> {
    fn new(app: AppHandle<R>, questions: Sender<String>) -> Self {
        Self {
            app,
            questions,
            stt: charger_stt(),
            entree: None,
            diag: Diag::ouvrir(),
            porte: Porte::new(kobi_stt::SAMPLE_RATE),
            ecouteur: Ecouteur::default(),
            en_pause: false,
            t: 0.0,
            audio: 0.0,
            calcul: Duration::ZERO,
            bilan: Instant::now(),
        }
    }

    fn note(&mut self, texte: &str) {
        if let Some(d) = &mut self.diag {
            d.note(texte);
        }
    }

    fn boucle(mut self, rx: Receiver<Commande>) {
        if let Err(e) = &self.stt {
            tracing::warn!(erreur = %e, "écoute continue impossible : Nemotron absent");
            let e = e.clone();
            self.note(&format!("Nemotron absent : {e}"));
        }
        loop {
            while let Ok(commande) = rx.try_recv() {
                self.commande(commande);
            }
            // En pause (Kobi ou la réunion), ni micro ni transcription.
            if self.en_pause || etat(&self.app).etat == "pause" || self.stt.is_err() {
                self.fermer_micro();
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(commande) => self.commande(commande),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            if self.entree.is_none() && !self.ouvrir_micro() {
                match rx.recv_timeout(REESSAI_MICRO) {
                    Ok(commande) => self.commande(commande),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            self.trame();
        }
    }

    fn ouvrir_micro(&mut self) -> bool {
        match Entree::ouvrir() {
            Ok((entree, description)) => {
                if let Ok(stt) = self.stt.as_mut() {
                    stt.reset();
                }
                self.ecouteur = Ecouteur::default();
                self.porte = Porte::new(kobi_stt::SAMPLE_RATE);
                self.t = 0.0;
                self.entree = Some(entree);
                tracing::info!(entree = %description, "écoute continue : micro ouvert");
                if let Some(d) = &mut self.diag {
                    d.entree(&description);
                }
                true
            }
            Err(e) => {
                tracing::warn!(erreur = %e, "écoute continue : micro indisponible");
                self.note(&format!("micro indisponible : {e}"));
                false
            }
        }
    }

    fn fermer_micro(&mut self) {
        if self.entree.take().is_some() {
            tracing::info!("écoute continue : micro fermé");
            self.note("micro fermé");
        }
    }

    fn commande(&mut self, commande: Commande) {
        match commande {
            Commande::BasculerPause => {
                self.en_pause = !self.en_pause;
                tracing::info!(pause = self.en_pause, "écoute continue");
            }
            Commande::Raccourci => {
                self.note("raccourci clavier");
                if self.en_pause {
                    self.en_pause = false;
                    tracing::info!(pause = false, "écoute continue : reprise par le raccourci");
                }
                if self.stt.is_err() {
                    self.stt = charger_stt();
                }
                let probleme = match &self.stt {
                    Err(e) => Some(e.clone()),
                    Ok(_) => None,
                }
                .or_else(|| {
                    (self.entree.is_none() && !self.ouvrir_micro())
                        .then(|| "micro indisponible".to_string())
                });
                if let Some(message) = probleme {
                    let app = self.app.clone();
                    std::thread::spawn(move || erreur(&app, "Micro ou transcription", &message));
                    return;
                }
                // La phrase en cours est oubliée : la suivante est la question.
                if let Ok(stt) = self.stt.as_mut() {
                    stt.reset();
                }
                let actions = self.ecouteur.raccourci(self.t);
                self.executer(actions);
            }
        }
    }

    fn trame(&mut self) {
        let Some(entree) = &self.entree else { return };
        let frame = match entree.frames().recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => frame,
            Err(RecvTimeoutError::Timeout) => return,
            Err(RecvTimeoutError::Disconnected) => {
                tracing::warn!("écoute continue : le micro s'est arrêté");
                self.note("le micro s'est arrêté");
                self.entree = None;
                return;
            }
        };
        let Ok(stt) = self.stt.as_mut() else { return };
        let frame = kobi_stt::AudioFrame {
            source: kobi_stt::Source::Moi,
            samples: frame.samples,
            t: frame.t.as_secs_f64(),
        };
        self.t = frame.end();
        self.audio += frame.samples.len() as f64 / kobi_stt::SAMPLE_RATE as f64;
        let debut = Instant::now();
        let t = self.t;
        let actions = transcrire(
            stt,
            &mut self.porte,
            &mut self.ecouteur,
            &mut self.diag,
            frame,
            t,
        );
        self.calcul += debut.elapsed();
        self.executer(actions);

        if self.bilan.elapsed() >= BILAN {
            tracing::info!(
                audio_s = self.audio as u64,
                calcul_s = self.calcul.as_secs_f64() as u64,
                rtf = format!("{:.3}", self.calcul.as_secs_f64() / self.audio.max(1.0)),
                "écoute continue : charge"
            );
            self.audio = 0.0;
            self.calcul = Duration::ZERO;
            self.bilan = Instant::now();
        }
    }

    fn executer(&mut self, actions: Vec<Action>) {
        for action in actions {
            match action {
                Action::Interpeller => match appliquer(&self.app, &[Evenement::Interpellation]) {
                    Ok(_) => tracing::info!("Kobi interpellé"),
                    Err(refus) => {
                        tracing::info!(%refus, "interpellation ignorée");
                        self.note(&format!("interpellation refusée par la session : {refus}"));
                        self.ecouteur.refusee();
                        return;
                    }
                },
                Action::Question(q) => {
                    let _ = self.app.emit(EVENEMENT_QUESTION, &q);
                }
                Action::Envoyer(q) => {
                    // La question est partie : la transcription repart d'un flux neuf.
                    if let Ok(stt) = self.stt.as_mut() {
                        stt.reset();
                    }
                    tracing::info!(longueur = q.len(), "question transcrite");
                    match appliquer(&self.app, &[Evenement::QuestionEnvoyee]) {
                        Ok(_) => {
                            let _ = self.app.emit(EVENEMENT_QUESTION, &q);
                            let _ = self.questions.send(q);
                        }
                        Err(refus) => self.note(&format!("question refusée : {refus}")),
                    }
                }
                Action::Abandonner => {
                    tracing::info!("interpellation sans question");
                    let _ = appliquer(&self.app, &[Evenement::PasDeQuestion]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpellation::modele_nemotron;
    use std::path::Path;

    /// Rejoue un WAV dans Nemotron et l'écoute comme le fil le fait avec le
    /// micro (trames de 10 ms, puis 3 s de silence). Renvoie les actions et
    /// les segments finals.
    fn rejouer(
        stt: &mut NemotronEngine,
        diag: &mut Option<Diag>,
        wav: &Path,
    ) -> (Vec<Action>, Vec<String>) {
        let samples = kobi_stt::wav::read_wav_16k_mono(wav).unwrap();
        stt.reset();
        let mut ecouteur = Ecouteur::default();
        let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
        let (mut actions, mut finals) = (Vec::new(), Vec::new());
        let silence = vec![0.0; 48_000];
        for (i, morceau) in samples.chunks(160).chain(silence.chunks(160)).enumerate() {
            let frame = kobi_stt::AudioFrame {
                source: kobi_stt::Source::Moi,
                samples: morceau.to_vec(),
                t: i as f64 * 0.01,
            };
            let t = frame.end();
            let suite = transcrire(stt, &mut porte, &mut ecouteur, diag, frame, t);
            if let Some(d) = diag {
                finals.extend(d.finals());
            }
            for a in suite {
                if matches!(a, Action::Envoyer(_)) {
                    stt.reset();
                }
                actions.push(a);
            }
        }
        (actions, finals)
    }

    /// Un WAV : `KOBI_ECOUTE_WAV=<fichier.wav> cargo test -p kobi-app
    /// --release -- --ignored wav_injecte --nocapture` ;
    /// `KOBI_ECOUTE_ATTENDU=rien` pour un fichier qui ne doit rien déclencher.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_WAV et le modèle Nemotron"]
    fn wav_injecte() {
        assert!(modele_nemotron().is_some(), "modèle Nemotron absent");
        let wav = PathBuf::from(std::env::var_os("KOBI_ECOUTE_WAV").expect("KOBI_ECOUTE_WAV"));
        let mut stt = charger_stt().unwrap();
        let (actions, finals) = rejouer(&mut stt, &mut Diag::ouvrir(), &wav);
        println!("{finals:#?}\n{actions:#?}");
        let envoyees = actions
            .iter()
            .filter(|a| matches!(a, Action::Envoyer(_)))
            .count();
        match std::env::var("KOBI_ECOUTE_ATTENDU").as_deref() {
            Ok("rien") => assert_eq!(envoyees, 0),
            _ => assert!(envoyees > 0, "aucune question envoyée"),
        }
    }

    /// Un corpus : `pos*.wav` doivent envoyer une question, `neg*.wav` ne
    /// rien déclencher. `KOBI_ECOUTE_CORPUS=<dossier> cargo test -p kobi-app
    /// --release -- --ignored corpus --nocapture`.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_CORPUS et le modèle Nemotron"]
    fn corpus() {
        let dir =
            PathBuf::from(std::env::var_os("KOBI_ECOUTE_CORPUS").expect("KOBI_ECOUTE_CORPUS"));
        let mut stt = charger_stt().unwrap();
        let mut fichiers: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        fichiers.sort();
        let (mut pos, mut pos_ok, mut fausses) = (0, 0, 0);
        for wav in &fichiers {
            let nom = wav.file_name().unwrap().to_string_lossy().into_owned();
            let (actions, finals) = rejouer(&mut stt, &mut Some(Diag::memoire()), wav);
            let envoyees: Vec<_> = actions
                .iter()
                .filter_map(|a| match a {
                    Action::Envoyer(q) => Some(q.as_str()),
                    _ => None,
                })
                .collect();
            let interpellations = actions
                .iter()
                .filter(|a| **a == Action::Interpeller)
                .count();
            if nom.starts_with("pos") {
                pos += 1;
                pos_ok += usize::from(!envoyees.is_empty());
                println!("{nom}: {finals:?} -> {envoyees:?}");
            } else {
                fausses += interpellations;
                // Transcription complète, pour juger de sa qualité.
                std::fs::write(wav.with_extension("txt"), finals.join("\n")).unwrap();
                for f in finals
                    .iter()
                    .filter(|f| crate::mot_cle::reperer(f).is_some())
                {
                    println!("{nom}: fausse détection « {f} »");
                }
                println!(
                    "{nom}: {} segments, {interpellations} interpellations",
                    finals.len()
                );
            }
        }
        println!("BILAN positifs {pos_ok}/{pos}, fausses interpellations {fausses}");
    }
}

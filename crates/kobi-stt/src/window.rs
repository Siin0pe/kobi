//! Transcription en flux par fenêtres glissantes, pour les moteurs qui ne
//! savent transcrire qu'un morceau d'audio complet (Whisper).
//!
//! À chaque pas (1 s par défaut), le dernier morceau d'audio non validé, au
//! plus une fenêtre (5 s), est retranscrit. Les mots sur lesquels deux
//! hypothèses successives s'accordent sont validés (« accord local ») et
//! sortent du morceau suivant ; les autres restent provisoires. L'énoncé
//! devient final après une pause (énergie sous un seuil) ou au-delà d'une
//! longueur maximale.

use std::collections::HashMap;

use crate::{AudioFrame, Result, Segment, Source, SttEngine, SAMPLE_RATE};

/// Mot reconnu, horodaté en secondes depuis le début de la piste.
#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

/// Reconnaissance d'un morceau d'audio complet.
pub trait Recognizer {
    /// Transcrit `samples` (16 kHz mono) ; les horodatages des mots sont
    /// relatifs au début de `samples`. `prompt` reprend la fin du texte déjà
    /// validé, pour la continuité.
    fn transcribe(&mut self, samples: &[f32], prompt: &str) -> Result<Vec<Word>>;

    /// Voir [`SttEngine::build_info`].
    fn build_info(&self) -> String {
        String::new()
    }
}

/// Réglages de la fenêtre glissante.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    /// Durée maximale du morceau retranscrit, en secondes.
    pub window: f64,
    /// Audio reçu entre deux transcriptions, en secondes.
    pub step: f64,
    /// Énergie RMS sous laquelle un bloc de 100 ms est un silence.
    pub silence_rms: f32,
    /// Silence qui clôt un énoncé, en secondes.
    pub final_pause: f64,
    /// Longueur au-delà de laquelle la partie validée devient finale.
    pub max_utterance: f64,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            window: 5.0,
            step: 1.0,
            silence_rms: 0.01,
            final_pause: 0.8,
            max_utterance: 20.0,
        }
    }
}

#[derive(Debug, Default)]
struct Stream {
    /// Audio depuis `buf_t0`.
    buf: Vec<f32>,
    buf_t0: f64,
    /// Fin de l'audio reçu.
    end: f64,
    /// Audio reçu depuis la dernière transcription, en secondes.
    since_step: f64,
    /// Fin du dernier mot validé ou finalisé : le morceau retranscrit
    /// commence là.
    anchor: f64,
    committed: Vec<Word>,
    tentative: Vec<Word>,
    last_partial: String,
}

impl Stream {
    fn index(&self, t: f64) -> usize {
        (((t - self.buf_t0) * SAMPLE_RATE as f64).round().max(0.0) as usize).min(self.buf.len())
    }

    /// Vrai si un bloc de 100 ms de `[from, to]` dépasse le seuil.
    fn voiced(&self, from: f64, to: f64, threshold: f32) -> bool {
        let (lo, hi) = (self.index(from), self.index(to));
        self.buf[lo..hi]
            .chunks(SAMPLE_RATE as usize / 10)
            .any(|block| rms(block) >= threshold)
    }

    fn trim_before(&mut self, t: f64) {
        let cut = self.index(t);
        if cut > 0 {
            self.buf.drain(..cut);
            self.buf_t0 += cut as f64 / SAMPLE_RATE as f64;
        }
    }
}

/// Énergie RMS d'un bloc.
pub fn rms(block: &[f32]) -> f32 {
    if block.is_empty() {
        return 0.0;
    }
    (block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32).sqrt()
}

/// Forme comparable d'un mot : minuscules, lettres et chiffres seulement.
fn key(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Nombre de mots en tête sur lesquels deux hypothèses s'accordent.
pub fn agreement(previous: &[Word], current: &[Word]) -> usize {
    previous
        .iter()
        .zip(current)
        .take_while(|(a, b)| key(&a.text) == key(&b.text))
        .count()
}

fn segment(words: &[Word], source: Source, is_final: bool) -> Option<Segment> {
    Some(Segment {
        start: words.first()?.start,
        end: words.last()?.end,
        source,
        text: words
            .iter()
            .map(|w| w.text.trim())
            .collect::<Vec<_>>()
            .join(" "),
        is_final,
    })
}

/// Moteur en flux construit sur un [`Recognizer`].
pub struct SlidingWindow<R> {
    name: String,
    recognizer: R,
    config: WindowConfig,
    streams: HashMap<Source, Stream>,
}

impl<R: Recognizer> SlidingWindow<R> {
    pub fn new(name: impl Into<String>, recognizer: R, config: WindowConfig) -> Self {
        Self {
            name: name.into(),
            recognizer,
            config,
            streams: HashMap::new(),
        }
    }

    pub fn recognizer(&self) -> &R {
        &self.recognizer
    }

    fn process(&mut self, source: Source, flush: bool) -> Result<Vec<Segment>> {
        let cfg = &self.config;
        let s = self.streams.get_mut(&source).expect("source connue");
        let now = s.end;
        let mut out = Vec::new();

        let start = s.anchor.max(now - cfg.window).max(s.buf_t0);
        let pending = !s.committed.is_empty() || !s.tentative.is_empty();
        let voiced = s.voiced(start, now, cfg.silence_rms);

        if !voiced && !pending {
            // Rien à transcrire : la fenêtre avance sur le silence.
            s.anchor = s.anchor.max(now - 0.3);
            s.trim_before(s.anchor);
            return Ok(out);
        }

        if voiced {
            let (lo, hi) = (s.index(start), s.index(now));
            let mut chunk = s.buf[lo..hi].to_vec();
            // Whisper décode mal un morceau de moins d'une seconde.
            chunk.resize(chunk.len().max(SAMPLE_RATE as usize), 0.0);
            let prompt = s
                .committed
                .iter()
                .rev()
                .take(20)
                .rev()
                .map(|w| w.text.trim())
                .collect::<Vec<_>>()
                .join(" ");
            let mut hyp: Vec<Word> = self
                .recognizer
                .transcribe(&chunk, &prompt)?
                .into_iter()
                .map(|w| Word {
                    start: w.start + start,
                    end: (w.end + start).min(now),
                    text: w.text,
                })
                .filter(|w| w.end > s.anchor + 0.05 && !key(&w.text).is_empty())
                .collect();

            let n = agreement(&s.tentative, &hyp);
            let mut rest = hyp.split_off(n);
            s.committed.extend(hyp);
            // Fenêtre pleine sans accord : on valide le début pour avancer.
            if n == 0 && now - start >= cfg.window - 1e-6 {
                let cut = rest
                    .iter()
                    .take_while(|w| w.end <= start + cfg.step)
                    .count()
                    .max(usize::from(!rest.is_empty()));
                s.committed.extend(rest.drain(..cut));
            }
            s.tentative = rest;
        }
        // Sans voix dans la fenêtre, l'hypothèse précédente reste valable.
        if let Some(w) = s.committed.last() {
            s.anchor = s.anchor.max(w.end);
        }

        let silent_tail = !s.voiced(now - cfg.final_pause, now, cfg.silence_rms);
        let utterance = s
            .committed
            .first()
            .zip(s.committed.last())
            .map_or(0.0, |(a, b)| b.end - a.start);

        if flush || silent_tail {
            let mut words = std::mem::take(&mut s.committed);
            words.append(&mut s.tentative);
            if let Some(w) = words.last() {
                s.anchor = s.anchor.max(w.end);
            }
            out.extend(segment(&words, source, true));
            s.last_partial.clear();
        } else if utterance >= cfg.max_utterance {
            let words = std::mem::take(&mut s.committed);
            out.extend(segment(&words, source, true));
            s.last_partial.clear();
        }

        let mut visible = s.committed.clone();
        visible.extend(s.tentative.iter().cloned());
        if let Some(partial) = segment(&visible, source, false) {
            if partial.text != s.last_partial {
                s.last_partial = partial.text.clone();
                out.push(partial);
            }
        }

        s.trim_before(s.anchor.max(now - cfg.window));
        Ok(out)
    }
}

impl<R: Recognizer> SttEngine for SlidingWindow<R> {
    fn name(&self) -> &str {
        &self.name
    }

    fn build_info(&self) -> String {
        self.recognizer.build_info()
    }

    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>> {
        let step = self.config.step;
        let s = self.streams.entry(frame.source).or_default();
        if s.buf.is_empty() && s.end == 0.0 {
            s.buf_t0 = frame.t;
            s.anchor = frame.t;
        }
        s.buf.extend_from_slice(&frame.samples);
        s.end = frame.end();
        s.since_step += frame.samples.len() as f64 / SAMPLE_RATE as f64;
        if s.since_step + 1e-9 < step {
            return Ok(Vec::new());
        }
        s.since_step = 0.0;
        self.process(frame.source, false)
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        let mut sources: Vec<Source> = self.streams.keys().copied().collect();
        sources.sort_by_key(|s| *s as u8);
        let mut out = Vec::new();
        for source in sources {
            out.extend(self.process(source, true)?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reconnaisseur scripté : connaît les mots de la piste et renvoie ceux
    /// entièrement contenus dans le morceau, à partir de leur position.
    /// Le dernier mot du morceau est d'abord mal reconnu, comme Whisper sur
    /// un mot coupé.
    struct Scripted {
        words: Vec<Word>,
        /// Début du morceau, connu par la taille des appels successifs.
        calls: Vec<(usize, String)>,
        clock: std::rc::Rc<std::cell::Cell<f64>>,
    }

    impl Recognizer for Scripted {
        fn transcribe(&mut self, samples: &[f32], prompt: &str) -> Result<Vec<Word>> {
            self.calls.push((samples.len(), prompt.to_string()));
            let end = self.clock.get();
            let start = end - samples.len() as f64 / SAMPLE_RATE as f64;
            let mut out: Vec<Word> = self
                .words
                .iter()
                .filter(|w| w.start >= start - 1e-6 && w.end <= end + 1e-6)
                .map(|w| Word {
                    text: w.text.clone(),
                    start: w.start - start,
                    end: w.end - start,
                })
                .collect();
            if let Some(last) = out.last_mut() {
                if end - (last.end + start) < 0.3 {
                    last.text.push('e');
                }
            }
            Ok(out)
        }
    }

    fn w(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.into(),
            start,
            end,
        }
    }

    /// Joue `secs` d'audio : parole (0,1) sur les intervalles des mots,
    /// silence ailleurs, en trames de 20 ms.
    fn play(engine: &mut SlidingWindow<Scripted>, words: &[Word], secs: f64) -> Vec<Segment> {
        let frame = (SAMPLE_RATE / 50) as usize;
        let n = (secs * 50.0) as usize;
        let mut out = Vec::new();
        for k in 0..n {
            let t = k as f64 * 0.02;
            let voiced = words
                .iter()
                .any(|w| t + 0.01 >= w.start && t + 0.01 <= w.end);
            let f = AudioFrame {
                source: Source::Moi,
                samples: vec![if voiced { 0.1 } else { 0.0 }; frame],
                t,
            };
            engine.recognizer.clock.set(f.end());
            out.extend(engine.push(&f).unwrap());
        }
        out.extend(engine.finish().unwrap());
        out
    }

    fn engine(words: &[Word]) -> SlidingWindow<Scripted> {
        SlidingWindow::new(
            "test",
            Scripted {
                words: words.to_vec(),
                calls: Vec::new(),
                clock: Default::default(),
            },
            WindowConfig::default(),
        )
    }

    #[test]
    fn agreement_ignores_case_and_punctuation() {
        let a = [
            w("Bonjour,", 0.0, 0.4),
            w("Kobi", 0.5, 0.9),
            w("ça", 1.0, 1.2),
        ];
        let b = [
            w("bonjour", 0.0, 0.4),
            w("Kobi.", 0.5, 0.9),
            w("sa", 1.0, 1.2),
        ];
        assert_eq!(agreement(&a, &b), 2);
        assert_eq!(agreement(&a, &[]), 0);
    }

    #[test]
    fn stable_words_are_committed_and_finalized_after_a_pause() {
        let words = [
            w("Kobi,", 0.2, 0.6),
            w("combien", 0.7, 1.2),
            w("de", 1.3, 1.4),
            w("tickets", 1.5, 2.1),
            w("ouverts", 2.2, 2.8),
            w("Merci", 5.0, 5.5),
        ];
        let mut e = engine(&words);
        let out = play(&mut e, &words, 7.0);

        let finals: Vec<_> = out.iter().filter(|s| s.is_final).collect();
        assert_eq!(finals.len(), 2, "{out:#?}");
        assert_eq!(finals[0].text, "Kobi, combien de tickets ouverts");
        assert_eq!((finals[0].start, finals[0].end), (0.2, 2.8));
        assert_eq!(finals[1].text, "Merci");
        // Des partiels précèdent le premier final.
        assert!(out.iter().take_while(|s| !s.is_final).count() >= 2);
        // Le silence entre les énoncés n'est pas transcrit à chaque pas.
        assert!(
            e.recognizer().calls.len() <= 7,
            "{:?}",
            e.recognizer().calls
        );
    }

    #[test]
    fn long_speech_keeps_moving_with_a_bounded_window() {
        // 12 s de parole continue, un mot toutes les 0,5 s.
        let words: Vec<Word> = (0..24)
            .map(|i| w(&format!("m{i}"), i as f64 * 0.5, i as f64 * 0.5 + 0.45))
            .collect();
        let mut e = engine(&words);
        let out = play(&mut e, &words, 13.0);
        let finals: String = out
            .iter()
            .filter(|s| s.is_final)
            .map(|s| s.text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        let expected: Vec<String> = (0..24).map(|i| format!("m{i}")).collect();
        assert_eq!(finals, expected.join(" "));
        // Aucun morceau ne dépasse la fenêtre (5 s, ou 1 s de remplissage).
        let max = e.recognizer().calls.iter().map(|c| c.0).max().unwrap();
        assert!(max <= 5 * SAMPLE_RATE as usize + 1, "{max}");
        // Le texte validé sert d'amorce.
        assert!(e.recognizer().calls.iter().any(|c| c.1.starts_with("m")));
    }

    #[test]
    fn rms_of_blocks() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[0.1; 100]) - 0.1).abs() < 1e-6);
    }
}

//! Détection d'activité vocale : début et fin de parole par source.
//!
//! [`Endpointer`] transforme une probabilité de parole par trame (32 ms) en
//! événements : début de parole, puis fin de parole après `min_silence` de
//! silence (0,8 s par défaut). C'est la fin de phrase qui déclenche l'envoi
//! d'une question à Claude. Il est indépendant du modèle et testé sans lui.
//!
//! [`SileroVad`] (feature `silero`) donne ces probabilités avec Silero VAD v5
//! (ONNX, licence MIT), par ONNX Runtime chargé à l'exécution :
//! `ORT_DYLIB_PATH`, sinon le dossier de l'exécutable, sinon le chargeur du
//! système. [`Vad`] combine les deux pour une source.

use std::time::Duration;

/// Trame d'analyse : 512 échantillons à 16 kHz (32 ms), celle de Silero v5.
pub const FRAME: usize = 512;
const FRAME_S: f64 = FRAME as f64 / crate::SAMPLE_RATE as f64;

/// Réglages de la segmentation.
#[derive(Debug, Clone)]
pub struct VadOptions {
    /// Probabilité à partir de laquelle une trame est de la parole.
    pub threshold: f32,
    /// En dessous, une trame est du silence (hystérésis) ; entre les deux,
    /// l'état ne change pas.
    pub neg_threshold: f32,
    /// Silence qui clôt une phrase.
    pub min_silence: Duration,
    /// Parole plus courte : ignorée (claquement, toux).
    pub min_speech: Duration,
}

impl Default for VadOptions {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            neg_threshold: 0.35,
            min_silence: Duration::from_millis(800),
            min_speech: Duration::from_millis(250),
        }
    }
}

/// Événement de parole d'une source. Instants en secondes depuis l'origine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VadEvent {
    /// Début de parole (début de la première trame de parole).
    SpeechStart { t: f64 },
    /// Fin de parole : `end` est la fin de la dernière trame de parole,
    /// `detected` l'instant où le silence a été jugé assez long.
    SpeechEnd { start: f64, end: f64, detected: f64 },
}

/// Segmentation d'une suite de probabilités par trame.
#[derive(Debug, Clone)]
pub struct Endpointer {
    options: VadOptions,
    /// Trames reçues.
    frames: u64,
    /// Parole en cours : début, et fin de la dernière trame de parole.
    speech: Option<(f64, f64)>,
    /// Début confirmé (assez long) ?
    confirmed: bool,
}

impl Endpointer {
    pub fn new(options: VadOptions) -> Self {
        Self {
            options,
            frames: 0,
            speech: None,
            confirmed: false,
        }
    }

    /// Ajoute la probabilité de la trame suivante ; `t0` est l'instant du
    /// début de la première trame.
    pub fn push(&mut self, t0: f64, prob: f32) -> Option<VadEvent> {
        let start = t0 + self.frames as f64 * FRAME_S;
        let end = start + FRAME_S;
        self.frames += 1;
        let (threshold, neg, min_silence) = (
            self.options.threshold,
            self.options.neg_threshold,
            self.options.min_silence.as_secs_f64(),
        );
        match self.speech {
            None if prob >= threshold => {
                self.speech = Some((start, end));
                self.confirmed = false;
                self.confirm()
            }
            None => None,
            // En parole, toute trame au-dessus du seuil bas la prolonge.
            Some((s, _)) if prob >= neg => {
                self.speech = Some((s, end));
                self.confirm()
            }
            Some((s, last)) if end - last >= min_silence - 1e-9 => {
                self.speech = None;
                self.confirmed.then_some(VadEvent::SpeechEnd {
                    start: s,
                    end: last,
                    detected: end,
                })
            }
            Some(_) => None,
        }
    }

    /// Émet le début de parole quand elle dure assez.
    fn confirm(&mut self) -> Option<VadEvent> {
        let (s, last) = self.speech?;
        if !self.confirmed && last - s >= self.options.min_speech.as_secs_f64() - 1e-9 {
            self.confirmed = true;
            return Some(VadEvent::SpeechStart { t: s });
        }
        None
    }

    /// Fin du flux : clôt la parole en cours.
    pub fn finish(&mut self, t0: f64) -> Option<VadEvent> {
        let (s, last) = self.speech.take()?;
        self.confirmed.then_some(VadEvent::SpeechEnd {
            start: s,
            end: last,
            detected: t0 + self.frames as f64 * FRAME_S,
        })
    }
}

#[cfg(feature = "silero")]
pub use silero::{SileroVad, Vad};

#[cfg(feature = "silero")]
mod silero {
    use std::path::{Path, PathBuf};

    use anyhow::{anyhow, Context, Result};
    use ort::session::Session;
    use ort::value::Tensor;

    use super::{Endpointer, VadEvent, VadOptions, FRAME};
    use crate::{AudioFrame, Source, SAMPLE_RATE};

    /// Échantillons de la trame précédente joints à chaque trame (Silero v5).
    const CONTEXT: usize = 64;

    /// Nom de la bibliothèque partagée d'ONNX Runtime sur cette plateforme.
    const LIBRARY: &str = if cfg!(windows) {
        "onnxruntime.dll"
    } else if cfg!(target_os = "macos") {
        "libonnxruntime.dylib"
    } else {
        "libonnxruntime.so"
    };

    /// Charge ONNX Runtime une fois pour tout le processus.
    fn ensure_runtime() -> Result<()> {
        use std::sync::OnceLock;
        static LOADED: OnceLock<Result<(), String>> = OnceLock::new();
        LOADED
            .get_or_init(|| {
                let beside = std::env::current_exe()
                    .ok()
                    .and_then(|e| e.parent().map(|d| d.join(LIBRARY)))
                    .filter(|p| p.is_file());
                let path = std::env::var_os("ORT_DYLIB_PATH")
                    .filter(|p| !p.is_empty())
                    .map(PathBuf::from)
                    .or(beside)
                    .unwrap_or_else(|| PathBuf::from(LIBRARY));
                ort::init_from(&path)
                    .map(|env| {
                        env.commit();
                    })
                    .map_err(|e| {
                        format!(
                            "ONNX Runtime introuvable ({}) : {e}. Définir ORT_DYLIB_PATH",
                            path.display()
                        )
                    })
            })
            .clone()
            .map_err(|e| anyhow!(e))
    }

    /// Les erreurs d'ort ne sont pas `Send` : texte seulement.
    fn ort_err<R>(e: ort::Error<R>) -> anyhow::Error {
        anyhow!(e.to_string())
    }

    /// Silero VAD v5 : probabilité de parole par trame de 32 ms.
    pub struct SileroVad {
        session: Session,
        state: Vec<f32>,
        context: Vec<f32>,
    }

    impl SileroVad {
        pub fn load(model: &Path) -> Result<Self> {
            ensure_runtime()?;
            let session = Session::builder()
                .map_err(ort_err)?
                .with_intra_threads(1)
                .map_err(ort_err)?
                .commit_from_file(model)
                .map_err(ort_err)
                .with_context(|| format!("modèle VAD {}", model.display()))?;
            Ok(Self {
                session,
                state: vec![0.0; 2 * 128],
                context: vec![0.0; CONTEXT],
            })
        }

        /// Probabilité de parole d'une trame de [`FRAME`] échantillons.
        pub fn prob(&mut self, frame: &[f32]) -> Result<f32> {
            assert_eq!(frame.len(), FRAME, "trame de 512 échantillons");
            let mut input = Vec::with_capacity(CONTEXT + FRAME);
            input.extend_from_slice(&self.context);
            input.extend_from_slice(frame);
            self.context.copy_from_slice(&frame[FRAME - CONTEXT..]);
            let outputs = self.session.run(ort::inputs![
                "input" => Tensor::from_array(([1usize, CONTEXT + FRAME], input)).map_err(ort_err)?,
                "state" => Tensor::from_array(([2usize, 1, 128], self.state.clone())).map_err(ort_err)?,
                "sr" => Tensor::from_array(([1usize], vec![SAMPLE_RATE as i64])).map_err(ort_err)?,
            ])
            .map_err(ort_err)?;
            let (_, prob) = outputs["output"]
                .try_extract_tensor::<f32>()
                .map_err(ort_err)?;
            let (_, state) = outputs["stateN"]
                .try_extract_tensor::<f32>()
                .map_err(ort_err)?;
            self.state.copy_from_slice(state);
            Ok(prob[0])
        }

        pub fn reset(&mut self) {
            self.state.fill(0.0);
            self.context.fill(0.0);
        }
    }

    /// VAD d'une source : découpe les blocs en trames, Silero puis
    /// [`Endpointer`].
    pub struct Vad {
        source: Source,
        model: SileroVad,
        endpointer: Endpointer,
        pending: Vec<f32>,
        /// Instant du premier échantillon reçu.
        t0: Option<f64>,
    }

    impl Vad {
        pub fn new(source: Source, model: SileroVad, options: VadOptions) -> Self {
            Self {
                source,
                model,
                endpointer: Endpointer::new(options),
                pending: Vec::new(),
                t0: None,
            }
        }

        /// Blocs d'une seule source, contigus.
        pub fn push(&mut self, frame: &AudioFrame) -> Result<Vec<VadEvent>> {
            debug_assert_eq!(frame.source, self.source);
            let t0 = *self.t0.get_or_insert(frame.t.as_secs_f64());
            self.pending.extend_from_slice(&frame.samples);
            let mut events = Vec::new();
            let mut used = 0;
            while self.pending.len() - used >= FRAME {
                let p = self.model.prob(&self.pending[used..used + FRAME])?;
                used += FRAME;
                events.extend(self.endpointer.push(t0, p));
            }
            self.pending.drain(..used);
            Ok(events)
        }

        pub fn finish(&mut self) -> Option<VadEvent> {
            self.endpointer.finish(self.t0.unwrap_or(0.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probabilités : `speech` trames de parole, entourées de silence.
    fn run(probs: &[f32]) -> Vec<VadEvent> {
        let mut e = Endpointer::new(VadOptions::default());
        let mut out: Vec<VadEvent> = probs.iter().filter_map(|&p| e.push(0.0, p)).collect();
        out.extend(e.finish(0.0));
        out
    }

    fn frames(n: usize, p: f32) -> Vec<f32> {
        vec![p; n]
    }

    #[test]
    fn phrase_puis_silence() {
        // 1 s de silence, 2 s de parole (62 trames), 1,5 s de silence.
        let probs = [frames(31, 0.05), frames(62, 0.9), frames(47, 0.05)].concat();
        let ev = run(&probs);
        assert_eq!(ev.len(), 2, "{ev:?}");
        let VadEvent::SpeechStart { t } = ev[0] else {
            panic!()
        };
        assert!((t - 31.0 * FRAME_S).abs() < 1e-9);
        let VadEvent::SpeechEnd {
            start,
            end,
            detected,
        } = ev[1]
        else {
            panic!()
        };
        assert!((start - t).abs() < 1e-9);
        assert!((end - 93.0 * FRAME_S).abs() < 1e-9);
        // Détectée 0,8 s après la fin de la parole, à une trame près.
        assert!(
            (detected - end - 0.8).abs() <= FRAME_S,
            "{}",
            detected - end
        );
    }

    #[test]
    fn pause_courte_ne_coupe_pas() {
        // 0,5 s de pause (< 0,8 s) entre deux morceaux : une seule phrase.
        let probs = [
            frames(20, 0.9),
            frames(16, 0.05),
            frames(20, 0.9),
            frames(40, 0.05),
        ]
        .concat();
        let ev = run(&probs);
        assert_eq!(ev.len(), 2, "{ev:?}");
        let VadEvent::SpeechEnd { end, .. } = ev[1] else {
            panic!()
        };
        assert!((end - 56.0 * FRAME_S).abs() < 1e-9);
    }

    #[test]
    fn claquement_ignore() {
        // 3 trames (96 ms < 250 ms) puis silence : rien.
        let probs = [frames(3, 0.95), frames(40, 0.0)].concat();
        assert!(run(&probs).is_empty());
    }

    #[test]
    fn hysteresis_entre_les_seuils() {
        // Après un début franc, 0,4 (entre 0,35 et 0,5) prolonge la parole.
        let probs = [frames(10, 0.9), frames(10, 0.4), frames(40, 0.1)].concat();
        let ev = run(&probs);
        let VadEvent::SpeechEnd { end, .. } = ev[1] else {
            panic!()
        };
        assert!((end - 20.0 * FRAME_S).abs() < 1e-9, "{end}");
    }

    #[test]
    fn seuil_de_silence_reglable() {
        let mut e = Endpointer::new(VadOptions {
            min_silence: Duration::from_millis(320),
            ..Default::default()
        });
        let probs = [frames(20, 0.9), frames(12, 0.05)].concat();
        let ev: Vec<_> = probs.iter().filter_map(|&p| e.push(10.0, p)).collect();
        let VadEvent::SpeechEnd { end, detected, .. } = ev[1] else {
            panic!("{ev:?}")
        };
        assert!((end - (10.0 + 20.0 * FRAME_S)).abs() < 1e-9);
        assert!((detected - end - 0.32).abs() <= FRAME_S);
    }

    #[test]
    fn fin_de_flux_clot_la_parole() {
        let ev = run(&frames(20, 0.9));
        assert!(matches!(ev[1], VadEvent::SpeechEnd { .. }));
    }
}

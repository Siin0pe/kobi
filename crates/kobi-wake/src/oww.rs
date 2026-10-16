//! Détecteur openWakeWord (KOB-26).
//!
//! Même chaîne que la bibliothèque Python openWakeWord : toutes les 80 ms
//! (1 280 échantillons), le mel de ces échantillons et des 480 précédents
//! donne 8 trames de 32 bandes ; les 76 dernières trames donnent un vecteur
//! d'embedding de 96 valeurs ; les 16 derniers vecteurs (environ 2 s
//! d'audio) donnent le score du modèle « Kobi ». Les modèles
//! `melspectrogram.onnx` et `embedding_model.onnx` sont ceux d'openWakeWord
//! v0.5.1 ; le modèle « Kobi » est entraîné par `bench/wake/train_oww.py`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use ort::session::Session;
use ort::value::TensorRef;

use crate::{Trigger, WakeDetected, WakeDetector, WakeError, SAMPLE_RATE};

const CHUNK: usize = 1280; // 80 ms
const CONTEXT: usize = 480; // échantillons précédents rendus au mel
const MEL_BINS: usize = 32;
const MEL_FRAMES_PER_CHUNK: usize = 8;
const MEL_WINDOW: usize = 76;
const EMB_DIM: usize = 96;
const MODEL_FRAMES: usize = 16;

/// Emplacement des modèles et réglages du détecteur.
#[derive(Debug, Clone)]
pub struct OwwConfig {
    pub melspectrogram: PathBuf,
    pub embedding: PathBuf,
    pub model: PathBuf,
    pub threshold: f32,
    pub debounce_s: f64,
    /// Threads ONNX Runtime par modèle.
    pub threads: usize,
}

impl OwwConfig {
    /// `features_dir` contient `melspectrogram.onnx` et `embedding_model.onnx`.
    pub fn new(features_dir: &Path, model: &Path) -> Self {
        Self {
            melspectrogram: features_dir.join("melspectrogram.onnx"),
            embedding: features_dir.join("embedding_model.onnx"),
            model: model.to_path_buf(),
            threshold: 0.5,
            debounce_s: 1.5,
            threads: 1,
        }
    }
}

pub struct OpenWakeWord {
    mel: Session,
    emb: Session,
    model: Session,
    framer: Framer,
    mel_frames: MelFrames,
    embeddings: VecDeque<[f32; EMB_DIM]>,
    trigger: Trigger,
}

impl OpenWakeWord {
    pub fn new(config: &OwwConfig) -> Result<Self, WakeError> {
        Ok(Self {
            mel: session(&config.melspectrogram, config.threads)?,
            emb: session(&config.embedding, config.threads)?,
            model: session(&config.model, config.threads)?,
            framer: Framer::default(),
            mel_frames: MelFrames::default(),
            embeddings: VecDeque::with_capacity(MODEL_FRAMES),
            trigger: Trigger::new(config.threshold, config.debounce_s),
        })
    }

    /// Ajoute des échantillons et renvoie un score (instant en secondes,
    /// score) toutes les 80 ms, dès que 16 vecteurs sont disponibles.
    pub fn push_scores(&mut self, samples: &[i16]) -> Result<Vec<(f64, f32)>, WakeError> {
        self.framer.push(samples);
        let mut scores = Vec::new();
        while let Some(window) = self.framer.next_window() {
            let frames = self.melspectrogram(&window)?;
            self.mel_frames.extend(&frames);
            let embedding = self.embed(&self.mel_frames.window())?;
            if self.embeddings.len() == MODEL_FRAMES {
                self.embeddings.pop_front();
            }
            self.embeddings.push_back(embedding);
            if self.embeddings.len() == MODEL_FRAMES {
                let t = self.framer.consumed as f64 / f64::from(SAMPLE_RATE);
                scores.push((t, self.score()?));
            }
        }
        Ok(scores)
    }

    fn melspectrogram(&mut self, window: &[f32]) -> Result<Vec<[f32; MEL_BINS]>, WakeError> {
        let input = TensorRef::from_array_view(([1usize, window.len()], window)).map_err(ort_err)?;
        let outputs = self.mel.run(ort::inputs![input]).map_err(ort_err)?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>().map_err(ort_err)?;
        if data.len() != MEL_FRAMES_PER_CHUNK * MEL_BINS {
            return Err(bad_output("melspectrogram", data.len()));
        }
        // Même transformée que openWakeWord : rapproche le mel ONNX de celui
        // du modèle TensorFlow d'origine.
        Ok(data
            .as_chunks::<MEL_BINS>()
            .0
            .iter()
            .map(|c| c.map(|v| v / 10.0 + 2.0))
            .collect())
    }

    fn embed(&mut self, frames: &[f32]) -> Result<[f32; EMB_DIM], WakeError> {
        let input =
            TensorRef::from_array_view(([1usize, MEL_WINDOW, MEL_BINS, 1], frames)).map_err(ort_err)?;
        let outputs = self.emb.run(ort::inputs![input]).map_err(ort_err)?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>().map_err(ort_err)?;
        <[f32; EMB_DIM]>::try_from(data).map_err(|_| bad_output("embedding", data.len()))
    }

    fn score(&mut self) -> Result<f32, WakeError> {
        let x: Vec<f32> = self.embeddings.iter().flatten().copied().collect();
        let input = TensorRef::from_array_view(([1usize, MODEL_FRAMES, EMB_DIM], &x[..])).map_err(ort_err)?;
        let outputs = self.model.run(ort::inputs![input]).map_err(ort_err)?;
        let (_, data) = outputs[0].try_extract_tensor::<f32>().map_err(ort_err)?;
        data.first().copied().ok_or_else(|| bad_output("kobi", 0))
    }
}

impl WakeDetector for OpenWakeWord {
    fn name(&self) -> &'static str {
        "openwakeword"
    }

    fn push(&mut self, samples: &[i16]) -> Result<Vec<WakeDetected>, WakeError> {
        Ok(self
            .push_scores(samples)?
            .into_iter()
            .filter_map(|(t, s)| self.trigger.feed(t, s))
            .collect())
    }

    fn reset(&mut self) {
        self.framer = Framer::default();
        self.mel_frames = MelFrames::default();
        self.embeddings.clear();
        self.trigger.reset();
    }
}

fn session(path: &Path, threads: usize) -> Result<Session, WakeError> {
    if !path.exists() {
        return Err(WakeError::MissingModel(path.to_path_buf()));
    }
    Session::builder()
        .map_err(ort_err)?
        .with_intra_threads(threads)
        .map_err(ort_err)?
        .with_inter_threads(1)
        .map_err(ort_err)?
        .commit_from_file(path)
        .map_err(ort_err)
}

fn ort_err(e: impl std::fmt::Display) -> WakeError {
    WakeError::Ort(e.to_string())
}

fn bad_output(model: &'static str, len: usize) -> WakeError {
    WakeError::BadOutput {
        model,
        detail: format!("{len} valeurs"),
    }
}

/// Découpe le flux en blocs de 1 280 échantillons, chacun précédé des 480
/// échantillons d'avant (silence au début du flux).
#[derive(Debug)]
struct Framer {
    history: [i16; CONTEXT],
    pending: Vec<i16>,
    consumed: u64,
}

impl Default for Framer {
    fn default() -> Self {
        Self {
            history: [0; CONTEXT],
            pending: Vec::with_capacity(2 * CHUNK),
            consumed: 0,
        }
    }
}

impl Framer {
    fn push(&mut self, samples: &[i16]) {
        self.pending.extend_from_slice(samples);
    }

    fn next_window(&mut self) -> Option<Vec<f32>> {
        if self.pending.len() < CHUNK {
            return None;
        }
        let chunk: Vec<i16> = self.pending.drain(..CHUNK).collect();
        let window = self
            .history
            .iter()
            .chain(&chunk)
            .map(|&s| f32::from(s))
            .collect();
        self.history.copy_from_slice(&chunk[CHUNK - CONTEXT..]);
        self.consumed += CHUNK as u64;
        Some(window)
    }
}

/// Les 76 dernières trames de mel, initialisées à 1 comme dans openWakeWord.
#[derive(Debug)]
struct MelFrames(VecDeque<[f32; MEL_BINS]>);

impl Default for MelFrames {
    fn default() -> Self {
        Self(std::iter::repeat_n([1.0; MEL_BINS], MEL_WINDOW).collect())
    }
}

impl MelFrames {
    fn extend(&mut self, frames: &[[f32; MEL_BINS]]) {
        for f in frames {
            self.0.pop_front();
            self.0.push_back(*f);
        }
    }

    /// Fenêtre [76, 32] aplatie, prête pour le modèle d'embedding.
    fn window(&self) -> Vec<f32> {
        self.0.iter().flatten().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn framer_emits_one_window_per_80_ms_with_context() {
        let mut f = Framer::default();
        let samples: Vec<i16> = (0..3000).map(|i| i as i16).collect();
        f.push(&samples[..1000]);
        assert!(f.next_window().is_none(), "moins de 80 ms");
        f.push(&samples[1000..]);
        let w1 = f.next_window().unwrap();
        assert_eq!(w1.len(), CONTEXT + CHUNK);
        assert!(w1[..CONTEXT].iter().all(|&s| s == 0.0), "silence avant le flux");
        assert_eq!(w1[CONTEXT], 0.0);
        assert_eq!(w1[CONTEXT + CHUNK - 1], 1279.0);
        let w2 = f.next_window().unwrap();
        assert_eq!(w2[0], 800.0, "les 480 derniers échantillons du bloc précédent");
        assert_eq!(w2[CONTEXT], 1280.0);
        assert!(f.next_window().is_none(), "reste 440 échantillons");
        assert_eq!(f.consumed, 2560);
    }

    #[test]
    fn mel_frames_keep_the_last_76() {
        let mut m = MelFrames::default();
        assert!(m.window().iter().all(|&v| v == 1.0));
        let frames: Vec<[f32; MEL_BINS]> = (0..80).map(|i| [i as f32; MEL_BINS]).collect();
        m.extend(&frames);
        let w = m.window();
        assert_eq!(w.len(), MEL_WINDOW * MEL_BINS);
        assert_eq!(w[0], 4.0);
        assert_eq!(w[w.len() - 1], 79.0);
    }

    #[test]
    fn missing_model_is_reported() {
        let cfg = OwwConfig::new(Path::new("introuvable"), Path::new("introuvable/kobi.onnx"));
        assert!(matches!(OpenWakeWord::new(&cfg), Err(WakeError::MissingModel(_))));
    }
}

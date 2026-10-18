//! Détecteur rustpotter (KOB-27), candidat 2, pur Rust.
//!
//! rustpotter calcule des MFCC toutes les 10 ms et lit l'audio par trames de
//! 30 ms (480 échantillons à 16 kHz) ; un petit réseau (« wakeword model »,
//! entraîné par [`train`]) classe la fenêtre de MFCC et lui donne un score
//! entre 0 et 1. rustpotter confirme lui-même une détection après `min_scores`
//! fenêtres au-dessus du seuil ; [`Trigger`] ajoute l'anti-rebond commun aux candidats.

use std::collections::HashMap;
use std::path::Path;

use rustpotter::{
    ModelType, Rustpotter, RustpotterConfig, SampleFormat, WakewordLoad, WakewordModel,
    WakewordModelTrain, WakewordModelTrainOptions, WakewordSave,
};

use crate::{Trigger, WakeDetected, WakeDetector, WakeError, SAMPLE_RATE};

/// Nom du mot-clé dans le modèle (étiquette `[kobi]` des fichiers d'entraînement).
pub const LABEL: &str = "kobi";

/// Réglages du détecteur.
#[derive(Debug, Clone)]
pub struct RustpotterOptions {
    /// Score minimal d'une fenêtre pour compter comme détection partielle.
    pub threshold: f32,
    /// Détections partielles nécessaires pour confirmer.
    pub min_scores: usize,
    pub debounce_s: f64,
}

impl Default for RustpotterOptions {
    fn default() -> Self {
        Self {
            threshold: 0.5,
            min_scores: 5,
            debounce_s: 1.5,
        }
    }
}

pub struct RustpotterDetector {
    detector: Rustpotter,
    frame: usize,
    pending: Vec<f32>,
    /// Échantillons traités depuis le dernier reset.
    received: usize,
    trigger: Trigger,
}

impl RustpotterDetector {
    /// Charge un modèle enregistré par [`save`].
    pub fn load(model: &Path, options: &RustpotterOptions) -> Result<Self, WakeError> {
        if !model.is_file() {
            return Err(WakeError::MissingModel(model.to_path_buf()));
        }
        let wakeword = WakewordModel::load_from_file(&model.to_string_lossy())
            .map_err(WakeError::Rustpotter)?;
        Self::new(wakeword, options)
    }

    pub fn new(wakeword: WakewordModel, options: &RustpotterOptions) -> Result<Self, WakeError> {
        let mut config = RustpotterConfig::default();
        config.fmt.sample_rate = SAMPLE_RATE as usize;
        config.fmt.sample_format = SampleFormat::F32;
        config.fmt.channels = 1;
        config.detector.threshold = options.threshold;
        config.detector.avg_threshold = 0.0;
        config.detector.min_scores = options.min_scores;
        let mut detector = Rustpotter::new(&config).map_err(WakeError::Rustpotter)?;
        detector
            .add_wakeword_model(LABEL, wakeword)
            .map_err(WakeError::Rustpotter)?;
        let frame = detector.get_samples_per_frame();
        Ok(Self {
            detector,
            frame,
            pending: Vec::new(),
            received: 0,
            // Le seuil est déjà appliqué par rustpotter.
            trigger: Trigger::new(0.0, options.debounce_s),
        })
    }
}

impl WakeDetector for RustpotterDetector {
    fn name(&self) -> &'static str {
        "rustpotter"
    }

    fn push(&mut self, samples: &[i16]) -> Result<Vec<WakeDetected>, WakeError> {
        self.pending
            .extend(samples.iter().map(|&s| f32::from(s) / 32768.0));
        let mut out = Vec::new();
        let mut used = 0;
        while self.pending.len() - used >= self.frame {
            let chunk = self.pending[used..used + self.frame].to_vec();
            used += self.frame;
            self.received += self.frame;
            if let Some(d) = self.detector.process_samples(chunk) {
                let t = self.received as f64 / f64::from(SAMPLE_RATE);
                out.extend(self.trigger.feed(t, d.score));
            }
        }
        self.pending.drain(..used);
        Ok(out)
    }

    fn reset(&mut self) {
        self.detector.reset();
        self.pending.clear();
        self.received = 0;
        self.trigger.reset();
    }
}

/// Réglages d'entraînement (voir `WakewordModelTrainOptions` de rustpotter).
#[derive(Debug, Clone)]
pub struct TrainOptions {
    /// `tiny`, `small`, `medium` ou `large`.
    pub model: String,
    pub learning_rate: f64,
    pub epochs: usize,
    /// Évaluation sur le jeu de test toutes les `test_epochs` époques.
    pub test_epochs: usize,
    pub mfcc_size: u16,
}

impl Default for TrainOptions {
    fn default() -> Self {
        Self {
            model: "small".into(),
            learning_rate: 0.017,
            epochs: 1500,
            test_epochs: 100,
            mfcc_size: 16,
        }
    }
}

/// Un extrait d'entraînement : audio 16 kHz mono et mot-clé ou non.
pub struct Sample {
    pub name: String,
    pub audio: Vec<i16>,
    pub wake: bool,
}

/// Entraîne un modèle. Les extraits doivent avoir la même durée : rustpotter
/// prend la plus longue comme taille de fenêtre et complète les autres par
/// des zéros.
pub fn train(
    options: &TrainOptions,
    train: &[Sample],
    test: &[Sample],
) -> Result<WakewordModel, WakeError> {
    let m_type = match options.model.as_str() {
        "tiny" => ModelType::Tiny,
        "small" => ModelType::Small,
        "medium" => ModelType::Medium,
        "large" => ModelType::Large,
        m => {
            return Err(WakeError::Rustpotter(format!(
                "type de modèle inconnu : {m}"
            )))
        }
    };
    let opts = WakewordModelTrainOptions::new(
        m_type,
        options.learning_rate,
        options.epochs,
        options.test_epochs,
        options.mfcc_size,
    );
    WakewordModel::train_from_buffers(opts, wav_buffers(train)?, wav_buffers(test)?, None)
        .map_err(|e| WakeError::Rustpotter(e.to_string()))
}

/// Enregistre un modèle pour [`RustpotterDetector::load`].
pub fn save(model: &WakewordModel, path: &Path) -> Result<(), WakeError> {
    model
        .save_to_file(&path.to_string_lossy())
        .map_err(WakeError::Rustpotter)
}

/// Fichiers WAV en mémoire, nommés comme rustpotter l'attend : l'étiquette
/// entre crochets (`[kobi]…`), sans crochets pour les négatifs.
fn wav_buffers(samples: &[Sample]) -> Result<HashMap<String, Vec<u8>>, WakeError> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut out = HashMap::with_capacity(samples.len());
    for s in samples {
        let mut bytes = std::io::Cursor::new(Vec::new());
        let mut w = hound::WavWriter::new(&mut bytes, spec)
            .map_err(|e| WakeError::Rustpotter(e.to_string()))?;
        for &x in &s.audio {
            w.write_sample(x)
                .map_err(|e| WakeError::Rustpotter(e.to_string()))?;
        }
        w.finalize()
            .map_err(|e| WakeError::Rustpotter(e.to_string()))?;
        let name = if s.wake {
            format!("[{LABEL}]{}", s.name)
        } else {
            s.name.replace(['[', ']'], "_")
        };
        out.insert(name, bytes.into_inner());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffers_are_labelled_by_name() {
        let samples = [
            Sample {
                name: "a.wav".into(),
                audio: vec![0; 160],
                wake: true,
            },
            Sample {
                name: "b[x].wav".into(),
                audio: vec![0; 160],
                wake: false,
            },
        ];
        let map = wav_buffers(&samples).unwrap();
        assert!(map.contains_key("[kobi]a.wav"));
        assert!(map.contains_key("b_x_.wav"), "pas d'étiquette parasite");
        let reader = hound::WavReader::new(std::io::Cursor::new(&map["[kobi]a.wav"])).unwrap();
        assert_eq!(reader.spec().sample_rate, SAMPLE_RATE);
        assert_eq!(reader.len(), 160);
    }

    #[test]
    fn missing_model_is_reported() {
        let err = RustpotterDetector::load(Path::new("/nulle/part.rpw"), &Default::default());
        assert!(matches!(err, Err(WakeError::MissingModel(_))));
    }

    /// 1 s de son : un « bip » à 880 Hz pour le mot-clé, du bruit sinon.
    fn clip(name: &str, wake: bool, seed: u32) -> Sample {
        let mut x = seed;
        let audio = (0..SAMPLE_RATE as usize)
            .map(|i| {
                if wake {
                    let t = i as f32 / SAMPLE_RATE as f32;
                    ((t * 880.0 * std::f32::consts::TAU).sin() * 12_000.0) as i16
                } else {
                    x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                    ((x >> 16) as i16) / 8
                }
            })
            .collect();
        Sample {
            name: name.into(),
            audio,
            wake,
        }
    }

    /// Passe par candle 0.2 (patché, voir third_party/candle-core-0.2.2) :
    /// initialisation aléatoire, entraînement, sauvegarde, rechargement et
    /// détection en flux.
    #[test]
    fn train_save_load_and_push() {
        let set = |n: u32| -> Vec<Sample> {
            (0..n)
                .flat_map(|i| {
                    [
                        clip(&format!("p{i}.wav"), true, i),
                        clip(&format!("n{i}.wav"), false, 100 + i),
                    ]
                })
                .collect()
        };
        let opts = TrainOptions {
            model: "tiny".into(),
            epochs: 20,
            test_epochs: 10,
            ..Default::default()
        };
        let model = train(&opts, &set(3), &set(1)).unwrap();
        let path = std::env::temp_dir().join(format!("kobi-wake-{}.rpw", std::process::id()));
        save(&model, &path).unwrap();
        let loaded = RustpotterDetector::load(&path, &Default::default());
        std::fs::remove_file(&path).unwrap();
        let mut det = loaded.unwrap();
        assert_eq!(det.name(), "rustpotter");
        det.push(&clip("flux", true, 0).audio).unwrap();
        det.reset();
        assert!(det.push(&[0; 100]).unwrap().is_empty());
    }

    #[test]
    fn unknown_model_type_is_an_error() {
        let opts = TrainOptions {
            model: "geant".into(),
            ..Default::default()
        };
        assert!(train(&opts, &[], &[]).is_err());
    }
}

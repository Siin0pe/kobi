//! Voix Piper/VITS par sherpa-onnx (paquets `vits-piper-*` : modèle,
//! `tokens.txt`, `espeak-ng-data`), en CPU.

use std::path::{Path, PathBuf};

use sherpa_onnx::{
    GenerationConfig, OfflineTts, OfflineTtsConfig, OfflineTtsModelConfig,
    OfflineTtsVitsModelConfig,
};

use crate::{Erreur, Resultat, Synthese};

/// Paquet de la voix française par défaut.
pub const VOIX_PAR_DEFAUT: &str = "vits-piper-fr_FR-siwis-medium";

#[derive(Debug, Clone)]
pub struct PiperConfig {
    /// Dossier du paquet (`vits-piper-…`).
    pub dossier: PathBuf,
    pub threads: i32,
    /// Vitesse de parole (1 = celle du modèle).
    pub vitesse: f32,
}

impl PiperConfig {
    pub fn new(dossier: impl Into<PathBuf>) -> Self {
        Self {
            dossier: dossier.into(),
            threads: 2,
            vitesse: 1.0,
        }
    }
}

/// Premier `.onnx` du paquet.
fn modele(dossier: &Path) -> Option<PathBuf> {
    let mut onnx: Vec<PathBuf> = std::fs::read_dir(dossier)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "onnx"))
        .collect();
    onnx.sort();
    onnx.into_iter().next()
}

pub struct Piper {
    tts: OfflineTts,
    vitesse: f32,
}

impl Piper {
    pub fn new(config: &PiperConfig) -> Resultat<Self> {
        let d = &config.dossier;
        let absent = |quoi: &str| Erreur::VoixAbsente(format!("{quoi} dans {}", d.display()));
        let model = modele(d).ok_or_else(|| absent("aucun .onnx"))?;
        let tokens = d.join("tokens.txt");
        let data = d.join("espeak-ng-data");
        if !tokens.is_file() {
            return Err(absent("pas de tokens.txt"));
        }
        if !data.is_dir() {
            return Err(absent("pas d'espeak-ng-data"));
        }
        let chemin = |p: &Path| Some(p.to_string_lossy().into_owned());
        let tts_config = OfflineTtsConfig {
            model: OfflineTtsModelConfig {
                vits: OfflineTtsVitsModelConfig {
                    model: chemin(&model),
                    tokens: chemin(&tokens),
                    data_dir: chemin(&data),
                    ..Default::default()
                },
                num_threads: config.threads,
                provider: Some("cpu".into()),
                ..Default::default()
            },
            // Le parleur découpe déjà en phrases.
            max_num_sentences: 1,
            silence_scale: 0.2,
            ..Default::default()
        };
        let tts = OfflineTts::create(&tts_config)
            .ok_or_else(|| Erreur::Synthese(format!("sherpa-onnx refuse {}", model.display())))?;
        Ok(Self {
            tts,
            vitesse: config.vitesse,
        })
    }
}

impl Synthese for Piper {
    fn frequence(&self) -> u32 {
        self.tts.sample_rate() as u32
    }

    fn synthetiser(&mut self, phrase: &str) -> Resultat<Vec<f32>> {
        let generation = GenerationConfig {
            speed: self.vitesse,
            ..Default::default()
        };
        let audio = self
            .tts
            .generate_with_config(phrase, &generation, None::<fn(&[f32], f32) -> bool>)
            .ok_or_else(|| Erreur::Synthese(format!("rien de généré pour « {phrase} »")))?;
        Ok(audio.samples().to_vec())
    }
}

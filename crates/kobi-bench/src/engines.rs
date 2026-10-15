//! Moteurs de transcription à bibliothèques natives, construits par nom.
//!
//! Ils ne sont compilés qu'avec leur feature (`nemotron` pour le CPU,
//! `nemotron-shared` pour les bibliothèques sherpa-onnx CUDA) ; [`NATIVE`]
//! liste ceux du binaire.

use std::path::{Path, PathBuf};

use kobi_stt::SttEngine;

/// Moteurs natifs compilés dans ce binaire.
#[cfg(any(feature = "nemotron", feature = "nemotron-shared"))]
pub const NATIVE: &[&str] = &["nemotron-160ms", "nemotron-560ms"];
#[cfg(not(any(feature = "nemotron", feature = "nemotron-shared")))]
pub const NATIVE: &[&str] = &[];

/// Réglages communs ; chaque moteur ignore ceux qui ne le concernent pas.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    /// Dossier des modèles, où chercher le paquet du moteur.
    pub models: PathBuf,
    /// Paquet imposé, à la place de la recherche dans `models`.
    pub model: Option<PathBuf>,
    /// Langue forcée (`fr`, `en`…), détection automatique sinon.
    pub language: Option<String>,
    pub threads: i32,
    /// `cpu` ou `cuda`.
    pub device: String,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            models: PathBuf::from("models"),
            model: None,
            language: None,
            threads: 4,
            device: "cpu".into(),
        }
    }
}

/// Paquet sherpa-onnx de Nemotron pour une taille de bloc : int8 sur CPU,
/// fp32 sur GPU (les opérateurs int8 n'ont pas de noyau CUDA). En cas de
/// plusieurs dates, la plus récente.
pub fn nemotron_package(models: &Path, chunk: &str, device: &str) -> Result<PathBuf, String> {
    let prefix = format!("sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-{chunk}-");
    let want_int8 = device != "cuda";
    let mut found: Vec<PathBuf> = std::fs::read_dir(models)
        .map_err(|e| format!("{} : {e}", models.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(&prefix))
                .is_some_and(|rest| rest.starts_with("int8-") == want_int8)
        })
        .collect();
    found.sort();
    found.pop().ok_or_else(|| {
        format!(
            "paquet {prefix}{}<date> introuvable dans {}",
            if want_int8 { "int8-" } else { "" },
            models.display()
        )
    })
}

/// Construit le moteur natif `name` (`nemotron-160ms`…).
#[cfg_attr(
    not(any(feature = "nemotron", feature = "nemotron-shared")),
    allow(unused_variables)
)]
pub fn build(name: &str, opts: &EngineOptions) -> Result<Box<dyn SttEngine>, String> {
    #[cfg(any(feature = "nemotron", feature = "nemotron-shared"))]
    if let Some(chunk) = name.strip_prefix("nemotron-") {
        use kobi_stt::nemotron::{NemotronConfig, NemotronEngine};
        let dir = match &opts.model {
            Some(dir) => dir.clone(),
            None => nemotron_package(&opts.models, chunk, &opts.device)?,
        };
        let mut config = NemotronConfig::new(dir);
        config.language = opts.language.clone();
        config.num_threads = opts.threads;
        config.provider = opts.device.parse()?;
        return Ok(Box::new(
            NemotronEngine::new(config).map_err(|e| e.to_string())?,
        ));
    }
    Err(format!(
        "moteur natif inconnu ou non compilé : {name} (compilés : {})",
        if NATIVE.is_empty() {
            "aucun, voir les features".to_string()
        } else {
            NATIVE.join(", ")
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_int8_on_cpu_and_fp32_on_cuda() {
        let dir = std::env::temp_dir().join(format!("kobi-engines-{}", std::process::id()));
        let p = "sherpa-onnx-nemotron-3.5-asr-streaming-0.6b";
        for d in [
            format!("{p}-160ms-int8-2026-06-11"),
            format!("{p}-160ms-2026-06-11"),
            format!("{p}-560ms-int8-2026-06-11"),
            format!("{p}-560ms-int8-2026-09-01"),
        ] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let name = |c, d| {
            nemotron_package(&dir, c, d)
                .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        };
        assert_eq!(
            name("160ms", "cpu").unwrap(),
            format!("{p}-160ms-int8-2026-06-11")
        );
        assert_eq!(
            name("160ms", "cuda").unwrap(),
            format!("{p}-160ms-2026-06-11")
        );
        assert_eq!(
            name("560ms", "cpu").unwrap(),
            format!("{p}-560ms-int8-2026-09-01")
        );
        assert!(name("560ms", "cuda").is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unknown_engine_is_an_error() {
        assert!(build("nope", &EngineOptions::default()).is_err());
    }
}

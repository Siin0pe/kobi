//! Moteurs de transcription à bibliothèques natives, construits par nom.
//!
//! Ils ne sont compilés qu'avec leur feature (`nemotron` pour le CPU,
//! `nemotron-shared` pour les bibliothèques sherpa-onnx CUDA, `voxtral` et
//! `voxtral-cuda` pour transcribe.cpp,
//! `whisper` et `whisper-cuda` pour whisper.cpp) ; [`native`] liste ceux du binaire.

use std::path::{Path, PathBuf};

use kobi_stt::SttEngine;

#[cfg(any(feature = "nemotron", feature = "nemotron-shared"))]
const NEMOTRON: &[&str] = &["nemotron-160ms", "nemotron-560ms"];
#[cfg(not(any(feature = "nemotron", feature = "nemotron-shared")))]
const NEMOTRON: &[&str] = &[];

/// Voxtral Realtime : `voxtral` = délai par défaut (6 tokens, 480 ms),
/// `voxtral-dN` = N tokens de 80 ms.
#[cfg(feature = "voxtral")]
const VOXTRAL: &[&str] = &["voxtral", "voxtral-d3", "voxtral-d12"];
#[cfg(not(feature = "voxtral"))]
const VOXTRAL: &[&str] = &[];

#[cfg(feature = "whisper")]
const WHISPER: &[&str] = &["whisper"];
#[cfg(not(feature = "whisper"))]
const WHISPER: &[&str] = &[];

/// Moteurs natifs compilés dans ce binaire.
pub fn native() -> Vec<&'static str> {
    [NEMOTRON, VOXTRAL, WHISPER].concat()
}

/// Délai en tokens de 80 ms d'un nom `voxtral` ou `voxtral-dN`.
pub fn voxtral_delay(name: &str) -> Option<i32> {
    match name.strip_prefix("voxtral")? {
        "" => Some(6),
        rest => rest.strip_prefix("-d")?.parse().ok(),
    }
}

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
            threads: 3,
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
    not(any(
        feature = "nemotron",
        feature = "nemotron-shared",
        feature = "voxtral",
        feature = "whisper"
    )),
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
    // Whisper large-v3-turbo, fenêtre glissante de 5 s, pas de 1 s.
    #[cfg(feature = "whisper")]
    if name == "whisper" {
        use kobi_stt::whisper::{whisper_engine, WhisperConfig};
        if opts.device == "cuda" && !cfg!(feature = "whisper-cuda") {
            return Err("whisper sur cuda : compiler avec la feature whisper-cuda".into());
        }
        let mut config = WhisperConfig::new(match &opts.model {
            Some(model) => model.clone(),
            None => opts.models.join("whisper").join("ggml-large-v3-turbo.bin"),
        });
        config.language = opts.language.clone();
        config.gpu = opts.device == "cuda";
        config.threads = opts.threads;
        let (engine, _) = whisper_engine(&config).map_err(|e| e.to_string())?;
        return Ok(Box::new(engine));
    }
    #[cfg(feature = "voxtral")]
    if let Some(delay) = voxtral_delay(name) {
        use kobi_stt::voxtral::{Device, VoxtralEngine, VoxtralOptions};
        let model = opts.model.clone().unwrap_or_else(|| {
            opts.models
                .join("voxtral-mini-4b-realtime-2602")
                .join("Voxtral-Mini-4B-Realtime-2602-Q8_0.gguf")
        });
        let device = match opts.device.as_str() {
            "cpu" => Device::Cpu,
            "cuda" => Device::Cuda,
            other => return Err(format!("matériel inconnu pour Voxtral : {other}")),
        };
        let mut options = VoxtralOptions::new(model, device);
        options.delay_tokens = delay;
        options.threads = opts.threads;
        return Ok(Box::new(
            VoxtralEngine::load(options).map_err(|e| e.to_string())?,
        ));
    }
    let native = native();
    Err(format!(
        "moteur natif inconnu ou non compilé : {name} (compilés : {})",
        if native.is_empty() {
            "aucun, voir les features".to_string()
        } else {
            native.join(", ")
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
    fn voxtral_delay_from_name() {
        assert_eq!(voxtral_delay("voxtral"), Some(6));
        assert_eq!(voxtral_delay("voxtral-d12"), Some(12));
        assert_eq!(voxtral_delay("voxtral-x"), None);
        assert_eq!(voxtral_delay("nemotron-160ms"), None);
    }

    #[test]
    fn unknown_engine_is_an_error() {
        assert!(build("nope", &EngineOptions::default()).is_err());
    }
}

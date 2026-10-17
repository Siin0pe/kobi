//! Moteurs de transcription à bibliothèques natives, construits par nom.
//!
//! Ils ne sont compilés qu'avec leur feature (`nemotron` pour le CPU,
//! `nemotron-shared` pour les bibliothèques sherpa-onnx CUDA, `kyutai` pour
//! candle, `voxtral` et
//! `voxtral-cuda` pour transcribe.cpp,
//! `whisper` et `whisper-cuda` pour whisper.cpp) ; [`native`] liste ceux du binaire.

use std::path::{Path, PathBuf};

use kobi_stt::SttEngine;

/// Nemotron : `nemotron-<bloc>` = paquet int8 sur CPU, fp32 sur GPU ;
/// suffixe `-int8` ou `-fp32` pour imposer la précision.
#[cfg(any(feature = "nemotron", feature = "nemotron-shared"))]
const NEMOTRON: &[&str] = &[
    "nemotron-160ms",
    "nemotron-560ms",
    "nemotron-160ms-fp32",
    "nemotron-560ms-fp32",
];
#[cfg(not(any(feature = "nemotron", feature = "nemotron-shared")))]
const NEMOTRON: &[&str] = &[];

/// Voxtral Realtime : `voxtral` = GGUF Q8_0, délai par défaut (6 tokens,
/// 480 ms) ; `-q4` = GGUF Q4_K_M (plus léger pour le CPU) ; `-dN` = N tokens
/// de 80 ms.
#[cfg(feature = "voxtral")]
const VOXTRAL: &[&str] = &[
    "voxtral",
    "voxtral-d3",
    "voxtral-d12",
    "voxtral-q4",
    "voxtral-q4-d3",
    "voxtral-q4-d12",
];
#[cfg(not(feature = "voxtral"))]
const VOXTRAL: &[&str] = &[];

/// Kyutai STT 1B en_fr, poids du dépôt (bf16 lus en f32) ou, via
/// [`EngineOptions::model`], un fichier de poids quantifiés (`.gguf`).
#[cfg(feature = "kyutai")]
const KYUTAI: &[&str] = &["kyutai"];
#[cfg(not(feature = "kyutai"))]
const KYUTAI: &[&str] = &[];

/// Whisper : `whisper` = large-v3-turbo, `whisper-small` et `whisper-base` =
/// modèles multilingues plus petits.
#[cfg(feature = "whisper")]
const WHISPER: &[&str] = &["whisper", "whisper-small", "whisper-base"];
#[cfg(not(feature = "whisper"))]
const WHISPER: &[&str] = &[];

/// Moteurs natifs compilés dans ce binaire.
pub fn native() -> Vec<&'static str> {
    [KYUTAI, NEMOTRON, VOXTRAL, WHISPER].concat()
}

/// Fichier ggml d'un nom `whisper` ou `whisper-<taille>`.
pub fn whisper_model(name: &str) -> Option<&'static str> {
    match name {
        "whisper" => Some("ggml-large-v3-turbo.bin"),
        "whisper-small" => Some("ggml-small.bin"),
        "whisper-base" => Some("ggml-base.bin"),
        _ => None,
    }
}

/// Variante Voxtral d'un nom `voxtral[-q4][-dN]` : quantification du GGUF
/// et délai en tokens de 80 ms.
pub fn voxtral_variant(name: &str) -> Option<(&'static str, i32)> {
    let rest = name.strip_prefix("voxtral")?;
    let (quant, rest) = match rest.strip_prefix("-q4") {
        Some(rest) => ("Q4_K_M", rest),
        None => ("Q8_0", rest),
    };
    let delay = match rest {
        "" => 6,
        rest => rest.strip_prefix("-d")?.parse().ok()?,
    };
    Some((quant, delay))
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
            threads: crate::protocol::DEFAULT_THREADS as i32,
            device: "cpu".into(),
        }
    }
}

/// Taille de bloc et précision d'un nom `nemotron-<bloc>[-int8|-fp32]`. Sans
/// suffixe : int8 sur CPU, fp32 sur GPU (les opérateurs int8 n'ont pas de
/// noyau CUDA dans onnxruntime).
pub fn nemotron_variant<'a>(name: &'a str, device: &str) -> Option<(&'a str, bool)> {
    let rest = name.strip_prefix("nemotron-")?;
    let (chunk, int8) = match rest.rsplit_once('-') {
        Some((chunk, "int8")) => (chunk, true),
        Some((chunk, "fp32")) => (chunk, false),
        Some(_) => return None,
        None => (rest, device != "cuda"),
    };
    chunk.strip_suffix("ms")?.parse::<u32>().ok()?;
    Some((chunk, int8))
}

/// Paquet sherpa-onnx de Nemotron pour une taille de bloc et une précision.
/// En cas de plusieurs dates, la plus récente.
pub fn nemotron_package(models: &Path, chunk: &str, want_int8: bool) -> Result<PathBuf, String> {
    let prefix = format!("sherpa-onnx-nemotron-3.5-asr-streaming-0.6b-{chunk}-");
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
        feature = "whisper",
        feature = "kyutai"
    )),
    allow(unused_variables)
)]
pub fn build(name: &str, opts: &EngineOptions) -> Result<Box<dyn SttEngine>, String> {
    #[cfg(any(feature = "nemotron", feature = "nemotron-shared"))]
    if let Some((chunk, int8)) = nemotron_variant(name, &opts.device) {
        use kobi_stt::nemotron::{NemotronConfig, NemotronEngine};
        let dir = match &opts.model {
            Some(dir) => dir.clone(),
            None => nemotron_package(&opts.models, chunk, int8)?,
        };
        let mut config = NemotronConfig::new(dir);
        config.language = opts.language.clone();
        config.num_threads = opts.threads;
        config.provider = opts.device.parse()?;
        return Ok(Box::new(
            NemotronEngine::new(config).map_err(|e| e.to_string())?,
        ));
    }
    // Whisper, fenêtre glissante de 5 s, pas de 1 s.
    #[cfg(feature = "whisper")]
    if let Some(file) = whisper_model(name) {
        use kobi_stt::whisper::{whisper_engine, WhisperConfig};
        if opts.device == "cuda" && !cfg!(feature = "whisper-cuda") {
            return Err("whisper sur cuda : compiler avec la feature whisper-cuda".into());
        }
        let mut config = WhisperConfig::new(match &opts.model {
            Some(model) => model.clone(),
            None => opts.models.join("whisper").join(file),
        });
        config.language = opts.language.clone();
        config.gpu = opts.device == "cuda";
        config.threads = opts.threads;
        let (engine, _) = whisper_engine(&config).map_err(|e| e.to_string())?;
        return Ok(Box::new(engine));
    }
    // Kyutai : dossier du dépôt Hugging Face ; `--model` désigne soit ce
    // dossier, soit un fichier de poids (`.safetensors`, `.gguf`) placé dedans.
    #[cfg(feature = "kyutai")]
    if name == "kyutai" {
        use kobi_stt::kyutai::{KyutaiEngine, KyutaiOptions};
        if opts.device != "cpu" {
            return Err("kyutai : seul le CPU est compilé".into());
        }
        let mut options = KyutaiOptions::new(opts.models.join("kyutai-stt-1b-en_fr-candle"));
        if let Some(model) = &opts.model {
            if model.is_dir() {
                options.model_dir = model.clone();
            } else {
                if let Some(dir) = model.parent() {
                    options.model_dir = dir.to_path_buf();
                }
                options.weights = Some(model.clone());
            }
        }
        return Ok(Box::new(
            KyutaiEngine::load(&options).map_err(|e| e.to_string())?,
        ));
    }
    #[cfg(feature = "voxtral")]
    if let Some((quant, delay)) = voxtral_variant(name) {
        use kobi_stt::voxtral::{Device, VoxtralEngine, VoxtralOptions};
        let model = opts.model.clone().unwrap_or_else(|| {
            opts.models
                .join("voxtral-mini-4b-realtime-2602")
                .join(format!("Voxtral-Mini-4B-Realtime-2602-{quant}.gguf"))
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
    fn nemotron_names_give_chunk_and_precision() {
        assert_eq!(
            nemotron_variant("nemotron-160ms", "cpu"),
            Some(("160ms", true))
        );
        assert_eq!(
            nemotron_variant("nemotron-160ms", "cuda"),
            Some(("160ms", false))
        );
        assert_eq!(
            nemotron_variant("nemotron-560ms-fp32", "cpu"),
            Some(("560ms", false))
        );
        assert_eq!(
            nemotron_variant("nemotron-560ms-int8", "cuda"),
            Some(("560ms", true))
        );
        assert_eq!(nemotron_variant("nemotron-560ms-q4", "cpu"), None);
        assert_eq!(nemotron_variant("nemotron-x", "cpu"), None);
        assert_eq!(nemotron_variant("whisper", "cpu"), None);
    }

    #[test]
    fn picks_the_package_of_the_requested_precision() {
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
            name("160ms", true).unwrap(),
            format!("{p}-160ms-int8-2026-06-11")
        );
        assert_eq!(
            name("160ms", false).unwrap(),
            format!("{p}-160ms-2026-06-11")
        );
        assert_eq!(
            name("560ms", true).unwrap(),
            format!("{p}-560ms-int8-2026-09-01")
        );
        assert!(name("560ms", false).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn whisper_model_from_name() {
        assert_eq!(whisper_model("whisper"), Some("ggml-large-v3-turbo.bin"));
        assert_eq!(whisper_model("whisper-base"), Some("ggml-base.bin"));
        assert_eq!(whisper_model("whisper-tiny"), None);
    }

    #[test]
    fn voxtral_variant_from_name() {
        assert_eq!(voxtral_variant("voxtral"), Some(("Q8_0", 6)));
        assert_eq!(voxtral_variant("voxtral-d12"), Some(("Q8_0", 12)));
        assert_eq!(voxtral_variant("voxtral-q4"), Some(("Q4_K_M", 6)));
        assert_eq!(voxtral_variant("voxtral-q4-d3"), Some(("Q4_K_M", 3)));
        assert_eq!(voxtral_variant("voxtral-x"), None);
        assert_eq!(voxtral_variant("nemotron-160ms"), None);
    }

    #[test]
    fn unknown_engine_is_an_error() {
        assert!(build("nope", &EngineOptions::default()).is_err());
    }
}

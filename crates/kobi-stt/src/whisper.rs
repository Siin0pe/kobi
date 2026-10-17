//! Adaptateur Whisper (whisper.cpp via `whisper-rs`) en fenêtres glissantes.
//!
//! Compilé avec la feature `whisper` (CPU) ou `whisper-cuda` (GPU NVIDIA).
//! La logique de flux est dans [`crate::window`] ; ce module ne fait que
//! transcrire un morceau et en extraire les mots horodatés.

use std::path::PathBuf;

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::window::{Recognizer, SlidingWindow, WindowConfig, Word};
use crate::{Result, SttError};

/// Réglages de l'adaptateur Whisper.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    /// Modèle ggml (`ggml-large-v3-turbo.bin`).
    pub model: PathBuf,
    /// Langue forcée (`fr`, `en`) ; détection à chaque morceau sinon.
    pub language: Option<String>,
    /// Calcul sur GPU (effectif seulement avec la feature `whisper-cuda`).
    pub gpu: bool,
    pub threads: i32,
    pub window: WindowConfig,
}

impl WhisperConfig {
    pub fn new(model: impl Into<PathBuf>) -> Self {
        Self {
            model: model.into(),
            language: None,
            gpu: false,
            threads: std::thread::available_parallelism().map_or(4, |n| n.get() as i32),
            window: WindowConfig::default(),
        }
    }
}

/// Transcrit un morceau complet avec whisper.cpp.
pub struct WhisperRecognizer {
    // Garde le contexte en vie tant que l'état l'utilise.
    _ctx: WhisperContext,
    state: WhisperState,
    eot: i32,
    language: Option<String>,
    threads: i32,
    gpu: bool,
}

impl WhisperRecognizer {
    pub fn new(config: &WhisperConfig) -> Result<Self> {
        // Redirige les journaux de whisper.cpp vers `log` (muet par défaut).
        whisper_rs::install_logging_hooks();
        // Avant tout appel à ggml : une bibliothèque compilée avec une
        // instruction absente peut planter (« Illegal instruction ») dès son
        // premier appel, y compris whisper_print_system_info.
        let missing = missing_cpu_features(&compiled_x86_features(), cpu_has);
        if !missing.is_empty() {
            return Err(SttError::Engine(format!(
                "whisper.cpp compilé avec {} que ce processeur n'a pas : recompiler avec \
                 GGML_{}=OFF, puis cargo clean -p whisper-rs-sys (voir .cargo/config.toml)",
                missing.join(", "),
                missing.join("=OFF GGML_")
            )));
        }
        let gpu = config.gpu && cfg!(feature = "whisper-cuda");
        let mut params = WhisperContextParameters::default();
        params.use_gpu(gpu);
        let path = config
            .model
            .to_str()
            .ok_or_else(|| SttError::Engine("chemin du modèle non UTF-8".into()))?;
        let ctx = WhisperContext::new_with_params(path, params)
            .map_err(|e| SttError::Engine(format!("chargement de {path} : {e}")))?;
        let state = ctx
            .create_state()
            .map_err(|e| SttError::Engine(format!("état whisper : {e}")))?;
        Ok(Self {
            eot: ctx.token_eot(),
            _ctx: ctx,
            state,
            language: config.language.clone(),
            threads: config.threads,
            gpu,
        })
    }

    /// `cuda` si le calcul est sur GPU, `cpu` sinon.
    pub fn device(&self) -> &'static str {
        if self.gpu {
            "cuda"
        } else {
            "cpu"
        }
    }
}

/// Instructions CPU actives dans ggml, lues dans `whisper_print_system_info`
/// (« AVX2 = 1 »…), plus l'accélérateur.
pub fn ggml_build_info(system_info: &str, gpu: bool) -> String {
    const CPU: [&str; 8] = [
        "AVX", "AVX2", "FMA", "F16C", "BMI2", "AVX512", "AVX_VNNI", "NEON",
    ];
    let mut flags: Vec<&str> = CPU
        .iter()
        .copied()
        .filter(|f| compiled_with(system_info, f))
        .collect();
    if flags.is_empty() {
        flags.push("scalaire");
    }
    let mut info = format!("ggml {}", flags.join(" "));
    if gpu {
        info.push_str(" + CUDA");
    }
    info
}

/// `true` si ggml annonce l'instruction `flag` (« AVX2 = 1 ») dans
/// `whisper_print_system_info`.
fn compiled_with(system_info: &str, flag: &str) -> bool {
    system_info.contains(&format!("| {flag} = 1")) || system_info.contains(&format!(" {flag} = 1 "))
}

/// Instructions x86 imposées à ggml à la compilation, lues dans les variables
/// `GGML_*` vues par ce crate (`.cargo/config.toml` ou environnement). Vide
/// avec `GGML_NATIVE=ON` : ggml suit alors le processeur de compilation.
pub fn compiled_x86_features() -> Vec<&'static str> {
    let on = |v: Option<&str>| {
        v.is_some_and(|v| matches!(v.to_ascii_uppercase().as_str(), "ON" | "1" | "TRUE" | "YES"))
    };
    if on(option_env!("GGML_NATIVE")) {
        return Vec::new();
    }
    [
        ("AVX", option_env!("GGML_AVX")),
        ("AVX2", option_env!("GGML_AVX2")),
        ("FMA", option_env!("GGML_FMA")),
        ("F16C", option_env!("GGML_F16C")),
        ("BMI2", option_env!("GGML_BMI2")),
        ("AVX512", option_env!("GGML_AVX512")),
    ]
    .into_iter()
    .filter(|(_, v)| on(*v))
    .map(|(f, _)| f)
    .collect()
}

/// Parmi les instructions `compiled`, celles que le processeur n'a pas (`has`
/// renvoie `false`).
pub fn missing_cpu_features(
    compiled: &[&'static str],
    has: impl Fn(&str) -> bool,
) -> Vec<&'static str> {
    compiled.iter().copied().filter(|f| !has(f)).collect()
}

/// Instruction disponible sur ce processeur (toujours vrai hors x86).
fn cpu_has(flag: &str) -> bool {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    {
        match flag {
            "AVX" => std::arch::is_x86_feature_detected!("avx"),
            "AVX2" => std::arch::is_x86_feature_detected!("avx2"),
            "FMA" => std::arch::is_x86_feature_detected!("fma"),
            "F16C" => std::arch::is_x86_feature_detected!("f16c"),
            "BMI2" => std::arch::is_x86_feature_detected!("bmi2"),
            "AVX512" => std::arch::is_x86_feature_detected!("avx512f"),
            _ => true,
        }
    }
    #[cfg(not(any(target_arch = "x86", target_arch = "x86_64")))]
    {
        let _ = flag;
        true
    }
}

impl Recognizer for WhisperRecognizer {
    fn build_info(&self) -> String {
        ggml_build_info(whisper_rs::print_system_info(), self.gpu)
    }

    fn transcribe(&mut self, samples: &[f32], prompt: &str) -> Result<Vec<Word>> {
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(self.language.as_deref().unwrap_or("auto")));
        params.set_n_threads(self.threads);
        params.set_no_context(true);
        params.set_single_segment(true);
        params.set_token_timestamps(true);
        params.set_suppress_blank(true);
        params.set_suppress_nst(true);
        params.set_temperature_inc(0.0);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        if !prompt.is_empty() {
            params.set_initial_prompt(prompt);
        }
        self.state
            .full(params, samples)
            .map_err(|e| SttError::Engine(format!("whisper : {e}")))?;

        // Les mots commencent par une espace ; les octets d'un caractère
        // peuvent être répartis sur deux tokens, d'où l'assemblage en octets.
        let mut words = Vec::new();
        let mut cur: Option<(Vec<u8>, f64, f64)> = None;
        for segment in self.state.as_iter() {
            for i in 0..segment.n_tokens() {
                let Some(token) = segment.get_token(i) else {
                    continue;
                };
                let data = token.token_data();
                if data.id >= self.eot {
                    continue;
                }
                let Ok(bytes) = token.to_bytes() else {
                    continue;
                };
                let (t0, t1) = (data.t0 as f64 / 100.0, data.t1 as f64 / 100.0);
                if bytes.first() == Some(&b' ') || cur.is_none() {
                    if let Some((b, s, e)) = cur.take() {
                        words.push(word(b, s, e));
                    }
                    cur = Some((bytes.to_vec(), t0, t1));
                } else if let Some((b, _, e)) = cur.as_mut() {
                    b.extend_from_slice(bytes);
                    *e = t1.max(*e);
                }
            }
        }
        if let Some((b, s, e)) = cur {
            words.push(word(b, s, e));
        }
        words.retain(|w| !w.text.is_empty());
        Ok(words)
    }
}

fn word(bytes: Vec<u8>, start: f64, end: f64) -> Word {
    Word {
        text: String::from_utf8_lossy(&bytes).trim().to_string(),
        start,
        end: end.max(start),
    }
}

/// Moteur Whisper en flux : fenêtre glissante de 5 s, pas de 1 s.
pub type WhisperEngine = SlidingWindow<WhisperRecognizer>;

/// Construit le moteur Whisper ; renvoie aussi le matériel utilisé.
pub fn whisper_engine(config: &WhisperConfig) -> Result<(WhisperEngine, &'static str)> {
    let recognizer = WhisperRecognizer::new(config)?;
    let device = recognizer.device();
    // Trace de contrôle : les instructions effectivement compilées.
    eprintln!("whisper.cpp : {}", whisper_rs::print_system_info().trim());
    Ok((
        SlidingWindow::new("whisper", recognizer, config.window.clone()),
        device,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::{Track, WavReplayer};
    use crate::wav::read_wav_16k_mono;
    use crate::{Source, SttEngine};

    fn home() -> PathBuf {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(PathBuf::from)
            .unwrap_or_default()
    }

    /// Transcrit les 20 premières secondes du corpus français ; nécessite le
    /// modèle et le corpus dans `~/Kobi-bench` (`cargo test --features
    /// whisper -- --ignored`).
    #[test]
    fn reads_cpu_flags_from_system_info() {
        let info = "WHISPER : COREML = 0 | OPENVINO = 0 | CPU : SSE3 = 1 | SSSE3 = 1 | AVX = 1                     | AVX2 = 1 | F16C = 1 | FMA = 1 | BMI2 = 1 | AVX512 = 0 | REPACK = 1 |";
        assert_eq!(ggml_build_info(info, false), "ggml AVX AVX2 FMA F16C BMI2");
        assert_eq!(
            ggml_build_info(info, true),
            "ggml AVX AVX2 FMA F16C BMI2 + CUDA"
        );
        let scalar = "CPU : SSE3 = 1 | AVX = 0 | AVX2 = 0 | FMA = 0 | F16C = 0 |";
        assert_eq!(ggml_build_info(scalar, false), "ggml scalaire");
    }

    #[test]
    fn missing_cpu_features_lists_compiled_but_absent() {
        // Homelab i5-3470 : AVX et F16C, sans AVX2, FMA ni BMI2.
        let ivy_bridge = |f: &str| matches!(f, "AVX" | "F16C");
        let defaults = ["AVX", "AVX2", "FMA", "F16C", "BMI2"];
        assert_eq!(
            missing_cpu_features(&defaults, ivy_bridge),
            ["AVX2", "FMA", "BMI2"]
        );
        // GGML_AVX2=OFF GGML_FMA=OFF mais BMI2 laissé actif : le cas du plantage.
        assert_eq!(
            missing_cpu_features(&["AVX", "F16C", "BMI2"], ivy_bridge),
            ["BMI2"]
        );
        assert!(missing_cpu_features(&["AVX", "F16C"], ivy_bridge).is_empty());
    }

    #[test]
    fn compiled_features_follow_cargo_config() {
        // .cargo/config.toml impose AVX, AVX2, FMA, F16C et BMI2 (sauf
        // valeur déjà présente dans l'environnement de compilation).
        let compiled = compiled_x86_features();
        for (flag, var) in [
            ("AVX2", option_env!("GGML_AVX2")),
            ("BMI2", option_env!("GGML_BMI2")),
        ] {
            let on = var.is_some_and(|v| v.eq_ignore_ascii_case("on"));
            assert_eq!(compiled.contains(&flag), on, "{flag} = {var:?}");
        }
    }

    #[test]
    #[ignore]
    fn transcribes_the_start_of_the_french_corpus() {
        let model = home().join("Kobi-bench/models/whisper/ggml-large-v3-turbo.bin");
        let wav = home().join("Kobi-bench/corpus/fr/moi.wav");
        let mut config = WhisperConfig::new(model);
        config.language = Some("fr".into());
        config.gpu = true;
        let (mut engine, device) = whisper_engine(&config).unwrap();
        let mut samples = read_wav_16k_mono(wav).unwrap();
        samples.truncate(20 * 16_000);
        let out = WavReplayer::new(vec![Track {
            source: Source::Moi,
            samples,
        }])
        .speed(0.0)
        .run(&mut engine)
        .unwrap();
        let text: Vec<_> = out.finals().map(|s| s.text.clone()).collect();
        println!("{device} {:.1} s : {text:#?}", out.wall_duration);
        assert!(text.join(" ").to_lowercase().contains("bonjour claire"));
        assert_eq!(engine.name(), "whisper");
    }
}

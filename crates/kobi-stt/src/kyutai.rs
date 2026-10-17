//! Adaptateur Kyutai STT 1B en_fr via la crate `moshi` (candle).
//!
//! Le modèle lit des trames de 80 ms d'audio 24 kHz (codec Mimi, 12,5 pas/s)
//! et émet le texte avec un retard fixe de 0,5 s (`audio_delay_seconds`).
//! `moshi::asr` sort des mots horodatés dans le temps de l'audio, pas dans
//! celui du calcul : on les regroupe en segments.
//!
//! - chaque phrase terminée (`.`, `?`, `!`) devient un segment final ;
//! - les mots suivants forment un segment partiel ;
//! - sans nouveau mot pendant [`PAUSE_FINAL`], l'énoncé devient final.
//!
//! Les deux sources partagent un seul modèle en lot de 2 (`moi` = 0,
//! `eux` = 1) : sur CPU, le pas du transformeur est limité par la lecture des
//! poids, qu'un lot de 2 lit une seule fois. Les deux pistes avancent donc
//! ensemble, trame par trame ; une source sans audio depuis plus de
//! [`MAX_LAG`] est complétée par du silence pour ne pas bloquer l'autre.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};

use candle::{Device, Tensor};
use kobi_audio::Resampler;
use moshi::asr::{AsrMsg, State};

use crate::{AudioFrame, Result, Segment, Source, SttEngine, SttError, SAMPLE_RATE};

/// Fréquence de Mimi.
const MODEL_RATE: u32 = 24_000;
/// Échantillons 24 kHz par pas du modèle (80 ms).
const STEP: usize = 1920;
/// Pas du modèle par seconde.
const STEPS_PER_SEC: f64 = 12.5;
/// Durée sans nouveau mot au bout de laquelle l'énoncé devient final, en
/// secondes d'audio.
pub const PAUSE_FINAL: f64 = 1.5;
/// Avance maximale d'une source sur l'autre avant de compléter la source en
/// retard par du silence, en secondes.
pub const MAX_LAG: f64 = 1.0;

const SOURCES: [Source; 2] = [Source::Moi, Source::Eux];

/// Réglages de l'adaptateur.
#[derive(Debug, Clone)]
pub struct KyutaiOptions {
    /// Dossier du dépôt `kyutai/stt-1b-en_fr-candle` (`config.json`, modèle,
    /// Mimi, tokenizer).
    pub model_dir: PathBuf,
    /// Poids du modèle de langue, à la place de `model.safetensors` : un
    /// `.safetensors` ou un `.gguf` quantifié.
    pub weights: Option<PathBuf>,
}

impl KyutaiOptions {
    pub fn new(model_dir: impl Into<PathBuf>) -> Self {
        Self {
            model_dir: model_dir.into(),
            weights: None,
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct SttConfig {
    audio_delay_seconds: f64,
}

/// Champs utiles de `config.json`.
#[derive(Debug, serde::Deserialize)]
struct Config {
    mimi_name: String,
    tokenizer_name: String,
    card: usize,
    text_card: usize,
    dim: usize,
    n_q: usize,
    context: usize,
    max_period: f64,
    num_heads: usize,
    num_layers: usize,
    causal: bool,
    stt_config: SttConfig,
}

impl Config {
    /// Configuration du modèle de langue, reprise de `stt-rs` (Kyutai).
    fn lm_config(&self) -> moshi::lm::Config {
        let transformer = moshi::transformer::Config {
            d_model: self.dim,
            num_heads: self.num_heads,
            num_layers: self.num_layers,
            dim_feedforward: self.dim * 4,
            causal: self.causal,
            norm_first: true,
            bias_ff: false,
            bias_attn: false,
            layer_scale: None,
            context: self.context,
            max_period: self.max_period as usize,
            use_conv_block: false,
            use_conv_bias: true,
            cross_attention: None,
            gating: Some(candle_nn::Activation::Silu),
            norm: moshi::NormType::RmsNorm,
            positional_embedding: moshi::transformer::PositionalEmbedding::Rope,
            conv_layout: false,
            conv_kernel_size: 3,
            kv_repeat: 1,
            max_seq_len: 4096 * 4,
            shared_cross_attn: false,
        };
        moshi::lm::Config {
            transformer,
            depformer: None,
            audio_vocab_size: self.card + 1,
            text_in_vocab_size: self.text_card + 1,
            text_out_vocab_size: self.text_card,
            audio_codebooks: self.n_q,
            conditioners: Default::default(),
            extra_heads: None,
        }
    }
}

/// Moteur Kyutai STT, les deux sources dans un même lot.
pub struct KyutaiEngine {
    name: String,
    state: State,
    pieces: Vec<String>,
    device: Device,
    delay: f64,
    lanes: [Lane; 2],
    /// Pas du modèle déjà calculés.
    steps: usize,
}

/// Audio et texte d'une source.
struct Lane {
    resampler: Resampler,
    /// Échantillons 24 kHz pas encore donnés au modèle ; `pending[0]` est
    /// l'échantillon `steps × STEP` de la piste.
    pending: VecDeque<f32>,
    /// Échantillons 24 kHz reçus (ou complétés), depuis le début de la piste.
    received: usize,
    segmenter: Segmenter,
}

impl KyutaiEngine {
    pub fn load(options: &KyutaiOptions) -> Result<Self> {
        let dir = &options.model_dir;
        let config: Config = serde_json::from_str(
            &std::fs::read_to_string(dir.join("config.json"))
                .map_err(|e| SttError::Engine(format!("{} : {e}", dir.display())))?,
        )
        .map_err(|e| SttError::Engine(format!("config.json : {e}")))?;
        let device = Device::Cpu;
        let weights = options
            .weights
            .clone()
            .unwrap_or_else(|| dir.join("model.safetensors"));
        let quantized = weights.extension().is_some_and(|e| e == "gguf");
        let vb = if quantized {
            moshi::nn::MaybeQuantizedVarBuilder::Quantized(
                candle_transformers::quantized_var_builder::VarBuilder::from_gguf(
                    &weights, &device,
                )
                .map_err(candle_error)?,
            )
        } else {
            // Poids bf16 convertis en f32 : candle n'a pas de produit bf16
            // rapide sur CPU.
            // SAFETY : fichier en lecture seule, non modifié pendant l'exécution.
            moshi::nn::MaybeQuantizedVarBuilder::Real(
                unsafe {
                    candle_nn::VarBuilder::from_mmaped_safetensors(
                        &[&weights],
                        device.bf16_default_to_f32(),
                        &device,
                    )
                }
                .map_err(candle_error)?,
            )
        };
        let lm = moshi::lm::LmModel::new(&config.lm_config(), vb).map_err(candle_error)?;
        let mimi_path = dir.join(&config.mimi_name);
        let mimi = moshi::mimi::load(&mimi_path.to_string_lossy(), Some(config.n_q), &device)
            .map_err(candle_error)?;
        let delay_steps = (config.stt_config.audio_delay_seconds * STEPS_PER_SEC) as usize;
        let state = State::new(SOURCES.len(), delay_steps, 0.0, mimi, lm).map_err(candle_error)?;
        let pieces = load_pieces(&dir.join(&config.tokenizer_name))?;
        let delay = config.stt_config.audio_delay_seconds;
        Ok(Self {
            name: if quantized {
                "kyutai-gguf".into()
            } else {
                "kyutai".into()
            },
            state,
            pieces,
            device,
            delay,
            lanes: SOURCES.map(|source| Lane {
                resampler: Resampler::new(SAMPLE_RATE, MODEL_RATE),
                pending: VecDeque::new(),
                received: 0,
                segmenter: Segmenter::new(source),
            }),
            steps: 0,
        })
    }

    /// Retard du texte sur l'audio, en secondes.
    pub fn delay(&self) -> f64 {
        self.delay
    }

    /// Calcule tous les pas pour lesquels les deux sources ont de l'audio, en
    /// complétant par du silence une source en retard de plus de `max_lag`
    /// échantillons 24 kHz.
    fn run_steps(&mut self, max_lag: usize) -> Result<Vec<Segment>> {
        let mut out = Vec::new();
        loop {
            let ahead = self.lanes.iter().map(|l| l.received).max().unwrap_or(0);
            let next = (self.steps + 1) * STEP;
            if ahead < next {
                break;
            }
            for lane in &mut self.lanes {
                if lane.received < next && ahead - lane.received > max_lag {
                    let missing = next - lane.received;
                    lane.pending.extend(std::iter::repeat_n(0.0, missing));
                    lane.received = next;
                }
            }
            if self.lanes.iter().any(|l| l.received < next) {
                break;
            }
            out.extend(self.step()?);
        }
        Ok(out)
    }

    /// Un pas du modèle : 80 ms de chaque source.
    fn step(&mut self) -> Result<Vec<Segment>> {
        let mut pcm = Vec::with_capacity(STEP * SOURCES.len());
        for lane in &mut self.lanes {
            pcm.extend(lane.pending.drain(..STEP));
        }
        let pcm =
            Tensor::from_vec(pcm, (SOURCES.len(), 1, STEP), &self.device).map_err(candle_error)?;
        let msgs = self
            .state
            .step_pcm(pcm, None, &().into(), |_, _, _| ())
            .map_err(candle_error)?;
        self.steps += 1;
        // Le texte d'un pas porte sur l'audio d'il y a `delay` : les mots
        // finis à cet instant sont horodatés par `moshi::asr`.
        let pos = self.steps as f64 / STEPS_PER_SEC - self.delay;
        let mut out = Vec::new();
        for msg in msgs {
            match msg {
                AsrMsg::Word {
                    tokens,
                    start_time,
                    batch_idx,
                } => {
                    let word = decode(&self.pieces, &tokens);
                    self.lanes[batch_idx]
                        .segmenter
                        .word(&word, start_time, &mut out);
                }
                AsrMsg::EndWord {
                    stop_time,
                    batch_idx,
                } => self.lanes[batch_idx].segmenter.end_word(stop_time),
                AsrMsg::Step { .. } => {}
            }
        }
        for lane in &mut self.lanes {
            lane.segmenter.tick(pos, &mut out);
        }
        Ok(out)
    }
}

impl SttEngine for KyutaiEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>> {
        let lane = &mut self.lanes[frame.source as usize];
        let (samples, _) = lane.resampler.process(&frame.samples);
        // Une source complétée par du silence a pris de l'avance sur son
        // audio : on saute la partie déjà comblée.
        let frame_end = (frame.end() * MODEL_RATE as f64).round() as usize;
        let skip = (lane.received + samples.len())
            .saturating_sub(frame_end)
            .min(samples.len());
        lane.received += samples.len() - skip;
        lane.pending.extend(&samples[skip..]);
        let max_lag = (MAX_LAG * MODEL_RATE as f64) as usize;
        self.run_steps(max_lag)
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        // Silence en fin de piste pour sortir le texte des dernières
        // `delay` secondes, plus une marge d'une seconde (comme `stt-rs`).
        let ahead = self.lanes.iter().map(|l| l.received).max().unwrap_or(0);
        if ahead == 0 {
            return Ok(Vec::new());
        }
        let tail = ((self.delay + 1.0) * MODEL_RATE as f64) as usize;
        let end = (ahead + tail).div_ceil(STEP) * STEP;
        for lane in &mut self.lanes {
            lane.pending
                .extend(std::iter::repeat_n(0.0, end - lane.received));
            lane.received = end;
        }
        let mut out = self.run_steps(0)?;
        for lane in &mut self.lanes {
            lane.segmenter.flush(&mut out);
        }
        Ok(out)
    }

    fn build_info(&self) -> String {
        let mut flags: Vec<&str> = Vec::new();
        for (flag, on) in [
            ("AVX", candle::utils::with_avx()),
            ("F16C", candle::utils::with_f16c()),
        ] {
            if on {
                flags.push(flag);
            }
        }
        let quant = if self.name.ends_with("gguf") {
            "gguf"
        } else {
            "f32"
        };
        if flags.is_empty() {
            format!("candle {quant}, noyaux scalaires (sans target-feature)")
        } else {
            format!("candle {quant} {}", flags.join(" "))
        }
    }
}

fn candle_error(e: candle::Error) -> SttError {
    SttError::Engine(e.to_string())
}

/// Mot d'une source, horodaté dans le temps de la piste.
#[derive(Debug, Clone)]
struct Word {
    text: String,
    start: f64,
    end: Option<f64>,
}

/// Regroupe les mots d'une source en segments partiels et finaux.
#[derive(Debug)]
struct Segmenter {
    source: Source,
    /// Mots de l'énoncé en cours.
    words: Vec<Word>,
    /// Instant (temps de la piste) du dernier mot reçu.
    last_word_at: f64,
}

impl Segmenter {
    fn new(source: Source) -> Self {
        Self {
            source,
            words: Vec::new(),
            last_word_at: 0.0,
        }
    }

    fn word(&mut self, text: &str, start: f64, out: &mut Vec<Segment>) {
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        if let Some(prev) = self.words.last_mut() {
            prev.end.get_or_insert(start);
        }
        self.words.push(Word {
            text: text.to_string(),
            start,
            end: None,
        });
        self.last_word_at = start;
        if text.ends_with(is_terminator) {
            self.emit(true, start, out);
        } else {
            self.emit(false, start, out);
        }
    }

    fn end_word(&mut self, stop: f64) {
        if let Some(last) = self.words.last_mut() {
            last.end.get_or_insert(stop);
            self.last_word_at = self.last_word_at.max(stop);
        }
    }

    /// Finalise l'énoncé après une pause ; `pos` est l'instant de la piste
    /// jusqu'où le modèle a rendu son texte.
    fn tick(&mut self, pos: f64, out: &mut Vec<Segment>) {
        if !self.words.is_empty() && pos - self.last_word_at >= PAUSE_FINAL {
            self.emit(true, pos, out);
        }
    }

    fn flush(&mut self, out: &mut Vec<Segment>) {
        if !self.words.is_empty() {
            let end = self.last_word_at;
            self.emit(true, end, out);
        }
    }

    fn emit(&mut self, is_final: bool, fallback_end: f64, out: &mut Vec<Segment>) {
        let start = self.words[0].start;
        let last = self.words.last().expect("énoncé non vide");
        let end = last.end.unwrap_or(fallback_end).max(start);
        out.push(Segment {
            start,
            end,
            source: self.source,
            text: join_words(&self.words),
            is_final,
        });
        if is_final {
            self.words.clear();
        }
    }
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '?' | '!' | '…')
}

/// Assemble les mots, sans espace avant la ponctuation collée que le modèle
/// sort parfois comme un mot à part.
fn join_words(words: &[Word]) -> String {
    let mut text = String::new();
    for word in words {
        let glued = word.text.starts_with([',', '.', '?', '!', '…', ')']);
        if !text.is_empty() && !glued {
            text.push(' ');
        }
        text.push_str(&word.text);
    }
    text
}

/// Texte d'un mot : pièces SentencePiece concaténées, `▁` = espace.
fn decode(pieces: &[String], tokens: &[u32]) -> String {
    tokens
        .iter()
        .filter_map(|&t| pieces.get(t as usize))
        .map(String::as_str)
        .collect::<String>()
        .replace('\u{2581}', " ")
}

/// Pièces d'un modèle SentencePiece (`.model`), dans l'ordre des identifiants.
///
/// Le fichier est un `ModelProto` protobuf dont le champ 1 répète des
/// `SentencePiece` (champ 1 : texte de la pièce). Lu à la main pour éviter la
/// bibliothèque C++ de SentencePiece : seul le décodage sert ici.
fn load_pieces(path: &Path) -> Result<Vec<String>> {
    let bytes =
        std::fs::read(path).map_err(|e| SttError::Engine(format!("{} : {e}", path.display())))?;
    parse_pieces(&bytes).ok_or_else(|| {
        SttError::Engine(format!(
            "{} : modèle SentencePiece illisible",
            path.display()
        ))
    })
}

fn parse_pieces(bytes: &[u8]) -> Option<Vec<String>> {
    let mut pieces = Vec::new();
    let mut r = ProtoReader(bytes);
    while let Some((field, value)) = r.next_field()? {
        if let (1, Value::Bytes(piece)) = (field, value) {
            let mut inner = ProtoReader(piece);
            let mut text = String::new();
            while let Some((field, value)) = inner.next_field()? {
                if let (1, Value::Bytes(b)) = (field, value) {
                    text = String::from_utf8_lossy(b).into_owned();
                }
            }
            pieces.push(text);
        }
    }
    (!pieces.is_empty()).then_some(pieces)
}

enum Value<'a> {
    Int,
    Bytes(&'a [u8]),
}

/// Lecteur protobuf minimal : varints, longueurs délimitées, 32 et 64 bits.
struct ProtoReader<'a>(&'a [u8]);

impl<'a> ProtoReader<'a> {
    fn varint(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let (&b, rest) = self.0.split_first()?;
            self.0 = rest;
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Some(v);
            }
        }
        None
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    /// `Some(None)` en fin de message, `None` si le message est mal formé.
    fn next_field(&mut self) -> Option<Option<(u64, Value<'a>)>> {
        if self.0.is_empty() {
            return Some(None);
        }
        let key = self.varint()?;
        let value = match key & 7 {
            0 => {
                self.varint()?;
                Value::Int
            }
            1 => {
                self.take(8)?;
                Value::Int
            }
            2 => {
                let len = usize::try_from(self.varint()?).ok()?;
                Value::Bytes(self.take(len)?)
            }
            5 => {
                self.take(4)?;
                Value::Int
            }
            _ => return None,
        };
        Some(Some((key >> 3, value)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pb_bytes(field: u64, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![(field << 3 | 2) as u8, payload.len() as u8];
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn reads_sentencepiece_pieces() {
        let mut model = Vec::new();
        for piece in ["<unk>", "\u{2581}bon", "jour"] {
            let mut sp = pb_bytes(1, piece.as_bytes());
            // score (float, champ 2) et type (varint, champ 3) ignorés
            sp.extend_from_slice(&[2 << 3 | 5, 0, 0, 0, 0, 3 << 3, 1]);
            model.extend(pb_bytes(1, &sp));
        }
        // trainer_spec (champ 2), ignoré
        model.extend(pb_bytes(2, &[1 << 3, 5]));
        let pieces = parse_pieces(&model).unwrap();
        assert_eq!(pieces, ["<unk>", "\u{2581}bon", "jour"]);
        assert_eq!(decode(&pieces, &[1, 2]).trim(), "bonjour");
        assert!(parse_pieces(&[0x0a, 0x05, 1]).is_none());
    }

    fn texts(out: &[Segment]) -> Vec<(bool, &str)> {
        out.iter().map(|s| (s.is_final, s.text.as_str())).collect()
    }

    #[test]
    fn sentences_and_pauses_become_final() {
        let mut seg = Segmenter::new(Source::Eux);
        let mut out = Vec::new();
        seg.word("Bonjour", 0.2, &mut out);
        seg.word("à", 0.6, &mut out);
        seg.word("tous.", 0.8, &mut out);
        seg.end_word(1.1);
        seg.word("Kobi", 2.0, &mut out);
        seg.end_word(2.3);
        seg.tick(3.0, &mut out);
        assert!(out.iter().all(|s| s.source == Source::Eux));
        assert_eq!(
            texts(&out),
            [
                (false, "Bonjour"),
                (false, "Bonjour à"),
                (true, "Bonjour à tous."),
                (false, "Kobi"),
            ]
        );
        let first = &out[2];
        assert_eq!((first.start, first.end), (0.2, 0.8));
        seg.tick(3.9, &mut out);
        let last = out.last().unwrap();
        assert_eq!(
            (last.is_final, last.text.as_str(), last.start, last.end),
            (true, "Kobi", 2.0, 2.3)
        );
        seg.flush(&mut out);
        assert_eq!(out.len(), 5);
    }

    #[test]
    fn glued_punctuation_has_no_space() {
        let words = ["Oui", ",", "Kobi", "?"].map(|t| Word {
            text: t.into(),
            start: 0.0,
            end: None,
        });
        assert_eq!(join_words(&words), "Oui, Kobi?");
    }
}

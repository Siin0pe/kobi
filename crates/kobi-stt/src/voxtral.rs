//! Adaptateur Voxtral Mini 4B Realtime (2602) via transcribe.cpp.
//!
//! Le modèle émet un token par tranche de 80 ms d'audio, avec un délai fixe
//! (`num_delay_tokens`, 6 = 480 ms par défaut). transcribe.cpp expose pour un
//! flux un texte « engagé » (qui ne fait que grandir) et un texte provisoire,
//! sans horodatage par mot. On en tire des segments :
//!
//! - chaque phrase terminée du texte engagé devient un segment final ;
//! - le reste (engagé non final + provisoire) est un segment partiel ;
//! - un texte engagé qui ne bouge plus pendant [`PAUSE_FINAL`] devient final.
//!
//! Horodatage : un mot apparu quand l'audio poussé atteint `p` a fini d'être
//! prononcé vers `p − délai`.
//!
//! Un [`Stream`](transcribe_cpp::Stream) emprunte sa session : chaque source a
//! donc son propre thread qui possède session et flux. transcribe.cpp 0.x
//! n'accepte qu'un flux actif par modèle (`model busy` au second) : chaque
//! source a donc aussi son propre modèle. Le second (pour `moi` et `eux`) est
//! chargé dès [`VoxtralEngine::load`], pour que son chargement ne soit pas
//! compté dans la relecture ; la mémoire du modèle est donc doublée.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use transcribe_cpp::{
    Backend, CommitPolicy, Model, ModelOptions, RunOptions, SessionOptions, StreamExtension,
    StreamOptions, StreamText, VoxtralRealtimeStreamOptions,
};

use crate::{AudioFrame, Result, Segment, Source, SttEngine, SttError, SAMPLE_RATE};

/// Durée sans nouveau texte engagé au bout de laquelle l'énoncé devient final,
/// en secondes d'audio.
pub const PAUSE_FINAL: f64 = 1.5;

/// Matériel de calcul.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Device {
    Cpu,
    Cuda,
}

/// Réglages de l'adaptateur.
#[derive(Debug, Clone)]
pub struct VoxtralOptions {
    /// Fichier GGUF (par exemple `Voxtral-Mini-4B-Realtime-2602-Q8_0.gguf`).
    pub model: PathBuf,
    pub device: Device,
    /// Délai en tokens de 80 ms : 1 à 15, ou 30 (validés par l'éditeur).
    pub delay_tokens: i32,
    /// Intervalle minimal entre deux décodages provisoires, en ms d'audio
    /// (`None` : valeur de transcribe.cpp).
    pub min_decode_interval_ms: Option<i32>,
    /// Threads CPU (0 : valeur de transcribe.cpp).
    pub threads: i32,
}

impl VoxtralOptions {
    pub fn new(model: impl Into<PathBuf>, device: Device) -> Self {
        Self {
            model: model.into(),
            device,
            delay_tokens: 6,
            min_decode_interval_ms: None,
            threads: 0,
        }
    }

    fn delay_secs(&self) -> f64 {
        self.delay_tokens as f64 * 0.08
    }
}

enum Command {
    Feed(Vec<f32>),
    Finish,
}

struct Worker {
    commands: Sender<Command>,
    replies: Receiver<Result<Vec<Segment>>>,
    thread: Option<JoinHandle<()>>,
}

/// Moteur Voxtral Realtime, une session de flux par source.
pub struct VoxtralEngine {
    name: String,
    options: VoxtralOptions,
    model: Model,
    /// Modèles préchargés pour les sources suivantes.
    spares: Vec<Model>,
    workers: HashMap<Source, Worker>,
}

impl VoxtralEngine {
    pub fn load(options: VoxtralOptions) -> Result<Self> {
        let model = load_model(&options)?;
        let caps = model.capabilities().map_err(engine_error)?;
        if !caps.supports_streaming {
            return Err(SttError::Engine(format!(
                "{} ne gère pas le streaming",
                model.arch()
            )));
        }
        let spares = vec![load_model(&options)?];
        Ok(Self {
            name: format!("voxtral-d{}", options.delay_tokens),
            options,
            model,
            spares,
            workers: HashMap::new(),
        })
    }

    /// Backend réellement utilisé, tel que rapporté par transcribe.cpp.
    pub fn backend(&self) -> String {
        self.model.backend()
    }

    fn worker(&mut self, source: Source) -> Result<&mut Worker> {
        if !self.workers.contains_key(&source) {
            // Le modèle principal sert la première source ; les suivantes ont
            // le leur (un seul flux actif par modèle).
            let model = if self.workers.is_empty() {
                self.model.clone()
            } else if let Some(model) = self.spares.pop() {
                model
            } else {
                load_model(&self.options)?
            };
            let worker = spawn_worker(model, self.options.clone(), source);
            self.workers.insert(source, worker);
        }
        Ok(self.workers.get_mut(&source).expect("worker créé"))
    }
}

impl SttEngine for VoxtralEngine {
    fn name(&self) -> &str {
        &self.name
    }

    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>> {
        let worker = self.worker(frame.source)?;
        worker.call(Command::Feed(frame.samples.clone()))
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        let mut out = Vec::new();
        let mut sources: Vec<Source> = self.workers.keys().copied().collect();
        sources.sort_by_key(|s| *s as u8);
        for source in sources {
            let mut worker = self.workers.remove(&source).expect("source connue");
            out.extend(worker.call(Command::Finish)?);
            if let Some(thread) = worker.thread.take() {
                let _ = thread.join();
            }
        }
        Ok(out)
    }
}

impl Worker {
    fn call(&mut self, command: Command) -> Result<Vec<Segment>> {
        self.commands
            .send(command)
            .map_err(|_| SttError::Engine("thread Voxtral arrêté".into()))?;
        self.replies
            .recv()
            .map_err(|_| SttError::Engine("thread Voxtral arrêté".into()))?
    }
}

fn spawn_worker(model: Model, options: VoxtralOptions, source: Source) -> Worker {
    let (commands, command_rx) = mpsc::channel::<Command>();
    let (reply_tx, replies) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name(format!("voxtral-{source:?}"))
        .spawn(move || {
            if let Err(e) = run_worker(&model, &options, source, &command_rx, &reply_tx) {
                // Réponse à la commande en cours, puis erreur pour les suivantes.
                let message = e.to_string();
                let _ = reply_tx.send(Err(e));
                for _ in command_rx.iter() {
                    let _ = reply_tx.send(Err(SttError::Engine(message.clone())));
                }
            }
        })
        .expect("création du thread Voxtral");
    Worker {
        commands,
        replies,
        thread: Some(thread),
    }
}

fn run_worker(
    model: &Model,
    options: &VoxtralOptions,
    source: Source,
    commands: &Receiver<Command>,
    replies: &Sender<Result<Vec<Segment>>>,
) -> Result<()> {
    let mut session = model
        .session_with(&SessionOptions {
            n_threads: options.threads,
            ..SessionOptions::default()
        })
        .map_err(engine_error)?;
    let stream_options = StreamOptions {
        commit_policy: CommitPolicy::Auto,
        family: Some(StreamExtension::VoxtralRealtime(
            VoxtralRealtimeStreamOptions {
                num_delay_tokens: Some(options.delay_tokens),
                min_decode_interval_ms: options.min_decode_interval_ms,
            },
        )),
        ..StreamOptions::default()
    };
    let mut stream = session
        .stream(&RunOptions::default(), &stream_options)
        .map_err(engine_error)?;
    let mut segmenter = Segmenter::new(source, options.delay_secs());
    let mut received = 0usize;

    for command in commands.iter() {
        match command {
            Command::Feed(samples) => {
                received += samples.len();
                let update = stream.feed(&samples).map_err(engine_error)?;
                let pos = received as f64 / SAMPLE_RATE as f64;
                let segments = if update.committed_changed || update.tentative_changed {
                    segmenter.update(&stream.text(), pos, false)
                } else {
                    segmenter.tick(pos)
                };
                if replies.send(Ok(segments)).is_err() {
                    return Ok(());
                }
            }
            Command::Finish => {
                stream.finalize().map_err(engine_error)?;
                let pos = received as f64 / SAMPLE_RATE as f64;
                let text = stream.text();
                // Après finalize, tout le texte est définitif.
                let full = StreamText {
                    committed: text.full.clone(),
                    tentative: String::new(),
                    full: text.full,
                };
                let _ = replies.send(Ok(segmenter.update(&full, pos, true)));
                return Ok(());
            }
        }
    }
    Ok(())
}

fn load_model(options: &VoxtralOptions) -> Result<Model> {
    let backend = match options.device {
        Device::Cpu => Backend::Cpu,
        Device::Cuda => Backend::Cuda,
    };
    Model::load_with(
        &options.model,
        &ModelOptions {
            backend,
            device: None,
        },
    )
    .map_err(engine_error)
}

fn engine_error(e: transcribe_cpp::Error) -> SttError {
    SttError::Engine(e.to_string())
}

/// Découpe le texte d'un flux en segments partiels et finaux.
#[derive(Debug)]
struct Segmenter {
    source: Source,
    delay: f64,
    /// Octets du texte engagé déjà émis en segments finaux.
    finalized: usize,
    /// Début estimé de l'énoncé en cours.
    utterance_start: Option<f64>,
    /// Longueur du texte engagé et position audio de son dernier changement.
    committed_len: usize,
    committed_at: f64,
    last_partial: String,
    /// Dernier texte vu, pour finaliser sur pause sans nouvel appel au modèle.
    last_text: StreamText,
}

impl Segmenter {
    fn new(source: Source, delay: f64) -> Self {
        Self {
            source,
            delay,
            finalized: 0,
            utterance_start: None,
            committed_len: 0,
            committed_at: 0.0,
            last_partial: String::new(),
            last_text: StreamText::default(),
        }
    }

    /// Instant de fin estimé des mots apparus quand l'audio poussé atteint `pos`.
    fn spoken_at(&self, pos: f64) -> f64 {
        (pos - self.delay).max(0.0)
    }

    /// Sans nouveau texte : finalise l'énoncé après une pause.
    fn tick(&mut self, pos: f64) -> Vec<Segment> {
        let pending = self.last_text.committed.len() > self.finalized;
        if pending
            && self.last_text.tentative.trim().is_empty()
            && pos - self.committed_at >= PAUSE_FINAL
        {
            let text = self.last_text.clone();
            let mut out = Vec::new();
            self.finalize_until(&text.committed, text.committed.len(), pos, &mut out);
            return out;
        }
        Vec::new()
    }

    fn update(&mut self, text: &StreamText, pos: f64, last: bool) -> Vec<Segment> {
        let committed = &text.committed;
        if committed.len() < self.finalized || !committed.is_char_boundary(self.finalized) {
            // Le texte engagé ne devrait jamais reculer ; on repart du début si c'est le cas.
            self.finalized = 0;
        }
        if committed.len() != self.committed_len {
            self.committed_len = committed.len();
            self.committed_at = pos;
        }
        self.last_text = text.clone();

        let mut out = Vec::new();
        let cut = if last {
            committed.len()
        } else {
            last_sentence_end(committed, self.finalized, &text.tentative)
        };
        if cut > self.finalized {
            self.finalize_until(committed, cut, pos, &mut out);
        }
        if last {
            return out;
        }

        let partial = format!("{}{}", &committed[self.finalized..], text.tentative);
        let partial = partial.trim();
        if !partial.is_empty() && partial != self.last_partial {
            let end = self.spoken_at(pos);
            let start = *self.utterance_start.get_or_insert(end);
            out.push(Segment {
                start,
                end,
                source: self.source,
                text: partial.to_string(),
                is_final: false,
            });
            self.last_partial = partial.to_string();
        }
        out
    }

    fn finalize_until(&mut self, committed: &str, cut: usize, pos: f64, out: &mut Vec<Segment>) {
        let end = self.spoken_at(pos);
        for sentence in split_sentences(&committed[self.finalized..cut]) {
            let start = self.utterance_start.take().unwrap_or(end).min(end);
            out.push(Segment {
                start,
                end,
                source: self.source,
                text: sentence,
                is_final: true,
            });
        }
        self.finalized = cut;
        self.utterance_start = None;
        self.last_partial.clear();
    }
}

fn is_terminator(c: char) -> bool {
    matches!(c, '.' | '?' | '!' | '…')
}

/// Fin (en octets) de la dernière phrase complète du texte engagé après `from` :
/// un terminateur suivi d'un espace, ou en fin de texte engagé quand le
/// provisoire commence par un espace (ou est vide).
fn last_sentence_end(committed: &str, from: usize, tentative: &str) -> usize {
    let mut cut = from;
    let tail = &committed[from..];
    let mut chars = tail.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !is_terminator(c) {
            continue;
        }
        let after = from + i + c.len_utf8();
        let boundary = match chars.peek() {
            Some((_, next)) => next.is_whitespace(),
            None => tentative.is_empty() || tentative.starts_with(char::is_whitespace),
        };
        if boundary {
            cut = after;
        }
    }
    cut
}

/// Découpe en phrases sur les terminateurs suivis d'un espace.
fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        current.push(c);
        if is_terminator(c) && chars.peek().is_none_or(|n| n.is_whitespace()) {
            let s = current.trim();
            if !s.is_empty() {
                out.push(s.to_string());
            }
            current.clear();
        }
    }
    let s = current.trim();
    if !s.is_empty() {
        out.push(s.to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(committed: &str, tentative: &str) -> StreamText {
        StreamText {
            full: format!("{committed}{tentative}"),
            committed: committed.into(),
            tentative: tentative.into(),
        }
    }

    fn finals(segments: &[Segment]) -> Vec<&str> {
        segments
            .iter()
            .filter(|s| s.is_final)
            .map(|s| s.text.as_str())
            .collect()
    }

    #[test]
    fn phrases_coupees_sur_terminateurs() {
        assert_eq!(
            split_sentences(" Bonjour. Ça va ? Oui… très bien"),
            vec!["Bonjour.", "Ça va ?", "Oui…", "très bien"]
        );
        // Un point dans un nombre ne coupe pas.
        assert_eq!(
            split_sentences("Il fait 3.5 degrés."),
            vec!["Il fait 3.5 degrés."]
        );
    }

    #[test]
    fn fin_de_phrase_attend_la_suite() {
        // « Bonjour. » est suivi d'un espace : phrase complète.
        assert_eq!(last_sentence_end(" Bonjour. Ça", 0, ""), 9);
        // Point en fin de texte engagé, suite provisoire collée : pas encore.
        assert_eq!(last_sentence_end(" Il fait 3.", 0, "5"), 0);
        assert_eq!(last_sentence_end(" Il fait 3.", 0, " Oui"), 11);
    }

    #[test]
    fn partiels_puis_final() {
        let mut s = Segmenter::new(Source::Moi, 0.48);
        let out = s.update(&text("", " Bonjour"), 1.0, false);
        assert_eq!(out.len(), 1);
        assert!(!out[0].is_final);
        assert!((out[0].end - 0.52).abs() < 1e-9);

        let out = s.update(&text(" Bonjour Claire.", " On"), 2.0, false);
        assert_eq!(finals(&out), vec!["Bonjour Claire."]);
        assert!(
            (out[0].start - 0.52).abs() < 1e-9,
            "début = première apparition"
        );
        assert_eq!(out.last().unwrap().text, "On");

        // Fin du flux : le reste devient final.
        let out = s.update(&text(" Bonjour Claire. On commence", ""), 3.0, true);
        assert_eq!(finals(&out), vec!["On commence"]);
    }

    #[test]
    fn pause_finalise_l_enonce() {
        let mut s = Segmenter::new(Source::Eux, 0.48);
        s.update(&text(" d'accord", ""), 1.0, false);
        assert!(s.tick(2.0).is_empty(), "pause trop courte");
        let out = s.tick(2.6);
        assert_eq!(finals(&out), vec!["d'accord"]);
        assert!(s.tick(5.0).is_empty(), "déjà finalisé");
    }

    #[test]
    fn partiel_identique_non_reemis() {
        let mut s = Segmenter::new(Source::Moi, 0.48);
        assert_eq!(s.update(&text("", " un"), 1.0, false).len(), 1);
        assert!(s.update(&text(" un", ""), 1.1, false).is_empty());
    }
}

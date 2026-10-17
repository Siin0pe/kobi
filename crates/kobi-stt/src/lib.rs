//! kobi-stt : interface commune des moteurs de transcription en flux.
//!
//! Un moteur reçoit des trames audio 16 kHz mono ([`AudioFrame`]) et renvoie
//! des [`Segment`] partiels ou finaux. Le [`replay::WavReplayer`] pousse un ou
//! plusieurs WAV au rythme réel (ou accéléré) ; [`fake::FakeEngine`] sert aux
//! tests et à la validation du harnais de benchmark.

pub mod fake;
#[cfg(feature = "kyutai")]
pub mod kyutai;
#[cfg(any(feature = "nemotron", feature = "nemotron-shared"))]
pub mod nemotron;
pub mod replay;
#[cfg(feature = "voxtral")]
pub mod voxtral;
pub mod wav;
#[cfg(feature = "whisper")]
pub mod whisper;
pub mod window;

use serde::{Deserialize, Serialize};

/// Fréquence d'échantillonnage attendue par tous les moteurs.
pub const SAMPLE_RATE: u32 = 16_000;

/// Origine d'une piste audio : le micro de l'utilisateur ou l'audio système.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Micro (`moi.wav`).
    Moi,
    /// Audio système (`eux.wav`).
    Eux,
}

/// Trame audio 16 kHz mono, échantillons `f32` dans [-1, 1].
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    pub source: Source,
    pub samples: Vec<f32>,
    /// Position du premier échantillon dans la piste, en secondes.
    pub t: f64,
}

impl AudioFrame {
    /// Position de la fin de la trame, en secondes.
    pub fn end(&self) -> f64 {
        self.t + self.samples.len() as f64 / SAMPLE_RATE as f64
    }
}

/// Morceau de transcription. Un segment partiel peut être remplacé par un
/// suivant ; un segment final ne change plus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    /// Début, en secondes depuis le début de la piste.
    pub start: f64,
    /// Fin, en secondes depuis le début de la piste.
    pub end: f64,
    pub source: Source,
    pub text: String,
    pub is_final: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SttError {
    #[error("lecture WAV : {0}")]
    Wav(#[from] hound::Error),
    #[error("format audio non pris en charge : {0}")]
    Format(String),
    #[error("moteur : {0}")]
    Engine(String),
    /// Relecture arrêtée : le moteur ne suit pas le temps réel (voir
    /// [`replay::WavReplayer::max_rtf`]).
    #[error(
        "trop lent : {wall:.0} s de calcul pour {audio:.0} s d'audio (facteur temps réel {:.2})",
        wall / audio.max(1e-9)
    )]
    TooSlow { audio: f64, wall: f64 },
}

pub type Result<T> = std::result::Result<T, SttError>;

/// Moteur de transcription en flux.
///
/// Les trames de chaque source arrivent dans l'ordre chronologique ; les
/// trames de sources différentes peuvent être entrelacées.
pub trait SttEngine {
    /// Nom court du moteur, repris dans les rapports.
    fn name(&self) -> &str;

    /// Reçoit une trame et renvoie les segments produits depuis l'appel
    /// précédent.
    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>>;

    /// Fin du flux : renvoie les derniers segments, tous finaux.
    fn finish(&mut self) -> Result<Vec<Segment>>;

    /// Options de compilation qui comptent pour la vitesse (instructions
    /// CPU, accélérateur), reprises dans les rapports. Vide par défaut.
    fn build_info(&self) -> String {
        String::new()
    }
}

impl<E: SttEngine + ?Sized> SttEngine for Box<E> {
    fn name(&self) -> &str {
        (**self).name()
    }

    fn push(&mut self, frame: &AudioFrame) -> Result<Vec<Segment>> {
        (**self).push(frame)
    }

    fn finish(&mut self) -> Result<Vec<Segment>> {
        (**self).finish()
    }

    fn build_info(&self) -> String {
        (**self).build_info()
    }
}

//! kobi-audio : capture du micro et de l'audio système, ramenés en 16 kHz mono
//! sur une même origine de temps.

mod align;
mod capture;
mod convert;
mod input;
mod resample;

use std::time::Duration;

pub use align::{Placement, TrackAligner};
pub use capture::{
    list_devices, Capture, CaptureOptions, DeviceInfo, DeviceList, SourceInfo, StreamError,
};
pub use convert::Converter;
pub use input::AudioInput;
pub use resample::{downmix, Resampler};

/// Fréquence d'échantillonnage de tout l'audio produit par ce crate.
pub const SAMPLE_RATE: u32 = 16_000;

/// Provenance d'un bloc audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Source {
    /// Le micro de l'utilisateur.
    Moi,
    /// L'audio système (les autres participants).
    Eux,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Moi => "moi",
            Source::Eux => "eux",
        }
    }
}

/// Bloc audio 16 kHz mono.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub source: Source,
    pub samples: Vec<f32>,
    /// Instant du premier échantillon, mesuré depuis l'origine commune de la capture.
    /// Négatif impossible : les échantillons antérieurs à l'origine sont écartés.
    pub t: Duration,
}

//! Interface commune des sources audio : la capture cpal ([`Capture`]) et les
//! sources de test (fichier), pour que le reste de Kobi ne dépende pas d'un
//! périphérique.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use crate::{AudioFrame, Capture, Source};

/// Source de blocs 16 kHz mono horodatés depuis une origine commune.
pub trait AudioInput {
    /// Sources produites (`moi`, `eux`).
    fn sources(&self) -> Vec<Source>;

    /// Blocs dans l'ordre de capture ; le canal se ferme quand la source s'arrête.
    fn frames(&self) -> &Receiver<AudioFrame>;

    /// Temps écoulé depuis l'origine.
    fn elapsed(&self) -> Duration;
}

impl AudioInput for Capture {
    fn sources(&self) -> Vec<Source> {
        Capture::sources(self).iter().map(|s| s.source).collect()
    }

    fn frames(&self) -> &Receiver<AudioFrame> {
        Capture::frames(self)
    }

    fn elapsed(&self) -> Duration {
        Capture::elapsed(self)
    }
}

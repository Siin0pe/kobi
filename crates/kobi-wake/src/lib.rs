//! kobi-wake : détection du mot-clé « Kobi ».
//!
//! Chaque détecteur reçoit l'audio 16 kHz mono en morceaux de taille
//! quelconque et signale les détections. Deux candidats sont comparés au J0 :
//! openWakeWord ([`oww::OpenWakeWord`], KOB-26) et rustpotter
//! (`rustpotter::RustpotterDetector`, KOB-27, derrière la fonction
//! `rustpotter`).

pub mod oww;
pub mod runtime;
#[cfg(feature = "rustpotter")]
pub mod rustpotter;

/// Fréquence d'échantillonnage attendue par tous les détecteurs.
pub const SAMPLE_RATE: u32 = 16_000;

/// Une détection du mot-clé.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WakeDetected {
    /// Fin de la fenêtre qui a déclenché, en secondes depuis le début du flux
    /// (ou depuis le dernier [`WakeDetector::reset`]).
    pub t: f64,
    /// Score du détecteur, entre 0 et 1.
    pub score: f32,
}

#[derive(Debug, thiserror::Error)]
pub enum WakeError {
    #[error("modèle introuvable : {0}")]
    MissingModel(std::path::PathBuf),
    #[error("ONNX Runtime : {0}")]
    Ort(String),
    #[error("sortie inattendue du modèle {model} : {detail}")]
    BadOutput { model: &'static str, detail: String },
    #[error("rustpotter : {0}")]
    Rustpotter(String),
}

/// Interface commune aux détecteurs de mot-clé.
pub trait WakeDetector: Send {
    /// Nom court du détecteur (« openwakeword », « rustpotter »).
    fn name(&self) -> &'static str;

    /// Ajoute des échantillons 16 kHz mono et renvoie les détections
    /// survenues dans ce morceau.
    fn push(&mut self, samples: &[i16]) -> Result<Vec<WakeDetected>, WakeError>;

    /// Oublie l'audio déjà reçu (changement de source, sortie de veille).
    fn reset(&mut self);
}

/// Seuil et anti-rebond communs : une détection quand le score atteint le
/// seuil, puis silence pendant `debounce_s` secondes.
#[derive(Debug, Clone)]
pub struct Trigger {
    pub threshold: f32,
    pub debounce_s: f64,
    last: Option<f64>,
}

impl Trigger {
    pub fn new(threshold: f32, debounce_s: f64) -> Self {
        Self {
            threshold,
            debounce_s,
            last: None,
        }
    }

    /// Renvoie une détection si `score` déclenche à l'instant `t`.
    pub fn feed(&mut self, t: f64, score: f32) -> Option<WakeDetected> {
        if score < self.threshold {
            return None;
        }
        if self.last.is_some_and(|last| t - last < self.debounce_s) {
            return None;
        }
        self.last = Some(t);
        Some(WakeDetected { t, score })
    }

    pub fn reset(&mut self) {
        self.last = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trigger_respects_threshold_and_debounce() {
        let mut trig = Trigger::new(0.5, 1.5);
        assert_eq!(trig.feed(0.0, 0.4), None);
        assert!(trig.feed(1.0, 0.9).is_some());
        assert_eq!(trig.feed(2.0, 0.9), None, "dans l'anti-rebond");
        assert!(trig.feed(2.5, 0.6).is_some());
        trig.reset();
        assert!(trig.feed(2.6, 0.6).is_some());
    }
}

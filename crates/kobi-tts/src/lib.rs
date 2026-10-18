//! kobi-tts : la voix de Kobi, en local (KOB-91, KOB-142).
//!
//! - [`texte`] : ce qui se dit d'une réponse écrite, phrase par phrase.
//! - [`Synthese`] : une voix ; [`piper::Piper`] (fonction `piper`) lit les
//!   paquets Piper/VITS de sherpa-onnx, en CPU.
//! - [`Sortie`] : où va le son ; [`sortie::Fichier`] l'écrit en WAV au rythme
//!   de la lecture, [`sortie::Haut_parleur`] (fonction `lecture`) le joue.
//! - [`Parleur`] : un fil qui synthétise la phrase suivante pendant que la
//!   précédente se joue, et s'arrête net sur [`Parleur::interrompre`].

pub mod parleur;
#[cfg(feature = "piper")]
pub mod piper;
pub mod sortie;
pub mod texte;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub use parleur::{Evenement, Parleur};
pub use sortie::Sortie;

#[derive(Debug, thiserror::Error)]
pub enum Erreur {
    #[error("voix introuvable : {0}")]
    VoixAbsente(String),
    #[error("synthèse : {0}")]
    Synthese(String),
    #[error("sortie audio : {0}")]
    Sortie(String),
}

pub type Resultat<T> = Result<T, Erreur>;

/// Une voix : du texte en échantillons mono, entre -1 et 1.
pub trait Synthese: Send {
    fn frequence(&self) -> u32;
    fn synthetiser(&mut self, phrase: &str) -> Resultat<Vec<f32>>;
}

/// Drapeau d'arrêt partagé entre le parleur, la synthèse et la sortie.
#[derive(Debug, Clone, Default)]
pub struct Arret(Arc<AtomicBool>);

impl Arret {
    pub fn arreter(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn arrete(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

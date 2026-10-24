//! Transcription brute en direct, pour la mise au point (KOB-50) : les
//! segments partiels et finaux des deux pistes, tels que le moteur les sort,
//! envoyés au panneau de la section « En direct » de l'app. Rien n'est émis
//! tant que le panneau n'est pas ouvert (`suivre_transcription`).

use std::sync::atomic::{AtomicBool, Ordering};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Runtime};

/// Événement reçu par l'app.
pub const EVENEMENT: &str = "transcription_brute";

/// Le panneau est ouvert quelque part.
static SUIVIE: AtomicBool = AtomicBool::new(false);

/// Une ligne du panneau.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Ligne {
    /// `moi` (micro) ou `eux` (son de l'ordinateur).
    pub source: &'static str,
    pub texte: String,
    /// Segment final ; sinon partiel, remplacé par le suivant.
    pub finale: bool,
    /// Partiel abandonné sans final (nouvel énoncé, pause) : la ligne se
    /// fige telle quelle, la phrase suivante en ouvre une neuve (KOB-274).
    pub abandon: bool,
}

/// Ligne à envoyer, si le panneau suit et que le texte n'est pas vide.
fn ligne(source: kobi_stt::Source, texte: &str, finale: bool, abandon: bool) -> Option<Ligne> {
    let texte = texte.trim();
    if !SUIVIE.load(Ordering::Relaxed) || texte.is_empty() {
        return None;
    }
    Some(Ligne {
        source: match source {
            kobi_stt::Source::Moi => "moi",
            kobi_stt::Source::Eux => "eux",
        },
        texte: texte.to_owned(),
        finale,
        abandon,
    })
}

/// Envoie un segment au panneau, s'il est ouvert.
pub fn publier<R: Runtime>(
    app: &AppHandle<R>,
    source: kobi_stt::Source,
    texte: &str,
    finale: bool,
) {
    if let Some(l) = ligne(source, texte, finale, false) {
        let _ = app.emit(EVENEMENT, l);
    }
}

/// Le partiel `texte`, déjà envoyé, n'aura pas de final : le panneau le
/// fige, marqué abandonné (KOB-274).
pub fn abandonner<R: Runtime>(app: &AppHandle<R>, source: kobi_stt::Source, texte: &str) {
    if let Some(l) = ligne(source, texte, true, true) {
        let _ = app.emit(EVENEMENT, l);
    }
}

/// Le panneau s'ouvre (`true`) ou se ferme (`false`).
#[tauri::command]
pub fn suivre_transcription(actif: bool) {
    SUIVIE.store(actif, Ordering::Relaxed);
    tracing::info!(actif, "transcription brute en direct (mise au point)");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rien tant que le panneau n'est pas ouvert, ni pour un texte vide.
    #[test]
    fn rien_sans_panneau_ouvert() {
        suivre_transcription(false);
        assert_eq!(ligne(kobi_stt::Source::Moi, "bonjour", false, false), None);
        suivre_transcription(true);
        assert_eq!(
            ligne(kobi_stt::Source::Eux, " on commence ", true, false),
            Some(Ligne {
                source: "eux",
                texte: "on commence".into(),
                finale: true,
                abandon: false,
            })
        );
        assert_eq!(ligne(kobi_stt::Source::Moi, "  ", false, false), None);
        suivre_transcription(false);
    }
}

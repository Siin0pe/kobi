//! Raccourci clavier global : secours du mot-clé. Il interpelle Kobi comme
//! « Kobi, … » : la phrase suivante est la question (voir [`crate::ecoute`]).
//!
//! Le raccourci vient de `hotkey` dans `~/Kobi/config.toml`
//! (`Ctrl+Shift+Space` par défaut).

use std::str::FromStr;

use kobi_session::config::Config;
use tauri::{AppHandle, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

/// Raccourci de la config, ou celui par défaut s'il est illisible (avec la
/// raison, à journaliser).
pub fn lire(hotkey: &str) -> (Shortcut, Option<String>) {
    match Shortcut::from_str(hotkey) {
        Ok(s) => (s, None),
        Err(e) => {
            let defaut = Config::default().hotkey;
            let s = Shortcut::from_str(&defaut).expect("raccourci par défaut valide");
            (
                s,
                Some(format!(
                    "hotkey = \"{hotkey}\" illisible ({e}), {defaut} utilisé"
                )),
            )
        }
    }
}

/// Branche le plugin et enregistre le raccourci. Un raccourci déjà pris par
/// une autre application n'empêche pas Kobi de démarrer.
pub fn installer<R: Runtime>(app: &AppHandle<R>, hotkey: &str) -> tauri::Result<()> {
    let (raccourci, probleme) = lire(hotkey);
    if let Some(probleme) = probleme {
        tracing::warn!(%probleme, "raccourci de la config ignoré");
    }
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, _raccourci, evenement| {
                if evenement.state() == ShortcutState::Pressed {
                    // Écoute du micro, transcription, question à Claude (KOB-67).
                    match crate::interpellation::declencher(app) {
                        Ok(()) => tracing::info!("raccourci global : interpellation"),
                        Err(refus) => tracing::info!(%refus, "raccourci global ignoré"),
                    }
                }
            })
            .build(),
    )?;
    if let Err(e) = app.global_shortcut().register(raccourci) {
        tracing::warn!(raccourci = ?raccourci, erreur = %e, "raccourci global non enregistré");
    } else {
        tracing::info!(raccourci = ?raccourci, "raccourci global enregistré");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    #[test]
    fn raccourci_par_defaut_et_reglable() {
        let (defaut, probleme) = lire(&Config::default().hotkey);
        assert!(probleme.is_none());
        assert_eq!(
            defaut,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space)
        );
        let (autre, probleme) = lire("Alt+K");
        assert!(probleme.is_none());
        assert_eq!(autre, Shortcut::new(Some(Modifiers::ALT), Code::KeyK));

        let (repli, probleme) = lire("Ctrl+Truc");
        assert_eq!(repli, defaut);
        assert!(probleme.unwrap().contains("Ctrl+Truc"));
    }
}

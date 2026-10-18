//! Raccourcis clavier globaux, réglables dans `~/Kobi/config.toml` :
//!
//! * `hotkey` (`Ctrl+Shift+Space` par défaut), secours du mot-clé : il
//!   interpelle Kobi comme « Kobi, … », la phrase suivante est la question
//!   (voir [`crate::ecoute`]). Il ne démarre jamais d'enregistrement ;
//! * `hotkey_record` (`Ctrl+Shift+R` par défaut) : démarre l'enregistrement
//!   de la réunion, ou l'arrête s'il tourne (puis compte rendu).

use std::str::FromStr;

use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::chef::{publier, ChefPartage, Message};
use crate::intentions::Intention;

/// Raccourci de la config, ou `defaut` s'il est illisible (avec la raison, à
/// journaliser). `cle` nomme le réglage dans le message.
pub fn lire(cle: &str, valeur: &str, defaut: &str) -> (Shortcut, Option<String>) {
    match Shortcut::from_str(valeur) {
        Ok(s) => (s, None),
        Err(e) => {
            let s = Shortcut::from_str(defaut).expect("raccourci par défaut valide");
            (
                s,
                Some(format!(
                    "{cle} = \"{valeur}\" illisible ({e}), {defaut} utilisé"
                )),
            )
        }
    }
}

/// Raccourcis enregistrés, pour reconnaître celui qui est pressé.
struct Raccourcis {
    interpeller: Shortcut,
    enregistrer: Shortcut,
}

/// Démarre l'enregistrement, ou l'arrête s'il tourne.
fn basculer_enregistrement<R: Runtime>(app: &AppHandle<R>) {
    let actif = app.try_state::<ChefPartage<R>>().is_some_and(|c| {
        c.0.lock()
            .map(|c| c.enregistrement().actif)
            .unwrap_or(false)
    });
    let intention = if actif {
        Intention::TerminerReunion
    } else {
        Intention::DemarrerReunion
    };
    tracing::info!(?intention, "raccourci d'enregistrement");
    publier(app, Message::Commande(intention));
}

/// Branche le plugin et enregistre les deux raccourcis. Un raccourci déjà
/// pris par une autre application n'empêche pas Kobi de démarrer.
pub fn installer<R: Runtime>(
    app: &AppHandle<R>,
    config: &kobi_session::config::Config,
) -> tauri::Result<()> {
    let defaut = kobi_session::config::Config::default();
    let (interpeller, p1) = lire("hotkey", &config.hotkey, &defaut.hotkey);
    let (enregistrer, p2) = lire(
        "hotkey_record",
        &config.hotkey_record,
        &defaut.hotkey_record,
    );
    for probleme in [p1, p2].into_iter().flatten() {
        tracing::warn!(%probleme, "raccourci de la config ignoré");
    }
    app.manage(Raccourcis {
        interpeller,
        enregistrer,
    });
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(|app, raccourci, evenement| {
                if evenement.state() != ShortcutState::Pressed {
                    return;
                }
                let r = app.state::<Raccourcis>();
                if *raccourci == r.enregistrer {
                    basculer_enregistrement(app);
                } else if *raccourci == r.interpeller {
                    match crate::interpellation::declencher(app) {
                        Ok(()) => tracing::info!("raccourci global : interpellation"),
                        Err(refus) => tracing::info!(%refus, "raccourci global ignoré"),
                    }
                }
            })
            .build(),
    )?;
    for raccourci in [interpeller, enregistrer] {
        if let Err(e) = app.global_shortcut().register(raccourci) {
            tracing::warn!(raccourci = ?raccourci, erreur = %e, "raccourci global non enregistré");
        } else {
            tracing::info!(raccourci = ?raccourci, "raccourci global enregistré");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_session::config::Config;
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    #[test]
    fn raccourcis_par_defaut_et_reglables() {
        let c = Config::default();
        let (defaut, probleme) = lire("hotkey", &c.hotkey, &c.hotkey);
        assert!(probleme.is_none());
        assert_eq!(
            defaut,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space)
        );
        let (record, _) = lire("hotkey_record", &c.hotkey_record, &c.hotkey_record);
        assert_eq!(
            record,
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyR)
        );
        assert_ne!(defaut, record);
        let (autre, probleme) = lire("hotkey", "Alt+K", &c.hotkey);
        assert!(probleme.is_none());
        assert_eq!(autre, Shortcut::new(Some(Modifiers::ALT), Code::KeyK));

        let (repli, probleme) = lire("hotkey", "Ctrl+Truc", &c.hotkey);
        assert_eq!(repli, defaut);
        assert!(probleme.unwrap().contains("Ctrl+Truc"));
    }
}

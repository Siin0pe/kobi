//! Raccourci clavier global : secours du mot-clé. Il interpelle Kobi comme
//! « Kobi, … » : la session passe en Écoute, puis revient au calme si aucune
//! question n'arrive dans les 4 s.
//!
//! Le raccourci vient de `hotkey` dans `~/Kobi/config.toml`
//! (`Ctrl+Shift+Space` par défaut).

use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use kobi_session::config::Config;
use kobi_session::etat::Evenement;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

use crate::session::{appliquer, EtatUi};

/// Délai sans question après lequel l'écoute s'arrête.
#[allow(dead_code)] // tests ; le fil d'interpellation fixe son propre délai
pub const SANS_QUESTION: Duration = Duration::from_secs(4);

/// Compteur d'interpellations : seule la dernière peut clore l'écoute.
#[derive(Default)]
pub struct Interpellations(AtomicU64);

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

/// Interpelle Kobi : Écoute, puis retour au calme après `delai` sans
/// question. Hors réunion (le mode assistant arrive au J4), la session
/// refuse et rien ne change.
pub fn interpeller<R: Runtime>(app: &AppHandle<R>, delai: Duration) -> Result<EtatUi, String> {
    let etat = appliquer(app, &[Evenement::Interpellation])?;
    let compteur = app.state::<Interpellations>();
    let numero = compteur.0.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(delai);
        let derniere = app.state::<Interpellations>().0.load(Ordering::SeqCst) == numero;
        // Refusé si la question est déjà partie : seule l'Écoute se clôt.
        if derniere {
            let _ = appliquer(&app, &[Evenement::PasDeQuestion]);
        }
    });
    Ok(etat)
}

/// Branche le plugin et enregistre le raccourci. Un raccourci déjà pris par
/// une autre application n'empêche pas Kobi de démarrer.
pub fn installer<R: Runtime>(app: &AppHandle<R>, hotkey: &str) -> tauri::Result<()> {
    app.manage(Interpellations::default());
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
    use crate::session;
    use tauri::test::{mock_builder, mock_context, noop_assets};
    use tauri::App;
    use tauri_plugin_global_shortcut::{Code, Modifiers};

    fn app() -> App<tauri::test::MockRuntime> {
        let app = session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        app.manage(Interpellations::default());
        app
    }

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

    #[test]
    fn le_raccourci_declenche_l_ecoute_puis_le_calme() {
        let app = app();
        let h = app.handle();
        appliquer(h, &[Evenement::DemarrerReunion]).unwrap();

        let etat = interpeller(h, Duration::from_millis(150)).unwrap();
        assert_eq!(
            (etat.etat, etat.activite, etat.mascotte),
            ("reunion", Some("ecoute"), "listen")
        );

        // Seconde interpellation : c'est elle qui fixe la fin de l'écoute.
        std::thread::sleep(Duration::from_millis(100));
        interpeller(h, Duration::from_millis(150)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            etat_courant(h),
            "ecoute",
            "la première ne coupe pas la seconde"
        );
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(etat_courant(h), "calme");
    }

    #[test]
    fn question_partie_avant_le_delai() {
        let app = app();
        let h = app.handle();
        appliquer(h, &[Evenement::DemarrerReunion]).unwrap();
        interpeller(h, Duration::from_millis(50)).unwrap();
        appliquer(h, &[Evenement::QuestionEnvoyee]).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(etat_courant(h), "reflechit");
    }

    #[test]
    fn hors_reunion_rien_ne_change() {
        let app = app();
        let refus = interpeller(app.handle(), Duration::from_millis(10)).unwrap_err();
        assert!(refus.contains("transition refusée"), "{refus}");
        assert_eq!(session::etat(app.handle()).etat, "veille");
    }

    fn etat_courant(h: &AppHandle<tauri::test::MockRuntime>) -> &'static str {
        session::etat(h).activite.unwrap_or("-")
    }
}

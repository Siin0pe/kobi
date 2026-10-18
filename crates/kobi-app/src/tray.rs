//! Icône et menu dans la barre système : le point d'entrée quand la mascotte
//! est masquée.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Runtime};

/// Événement envoyé à l'interface (et plus tard à la session) pour chaque entrée.
pub const EVENT: &str = "kobi://menu";

/// Entrées du menu, dans l'ordre d'affichage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    StartMeeting,
    Pause,
    OpenNotebook,
    OpenSettings,
    ToggleVoice,
    ToggleMascot,
    Quit,
}

impl MenuAction {
    pub const ALL: [MenuAction; 7] = [
        MenuAction::StartMeeting,
        MenuAction::Pause,
        MenuAction::OpenNotebook,
        MenuAction::OpenSettings,
        MenuAction::ToggleVoice,
        MenuAction::ToggleMascot,
        MenuAction::Quit,
    ];

    /// Identifiant stable, utilisé dans le menu, l'événement et le journal.
    pub fn id(self) -> &'static str {
        match self {
            MenuAction::StartMeeting => "start-meeting",
            MenuAction::Pause => "pause",
            MenuAction::OpenNotebook => "open-notebook",
            MenuAction::OpenSettings => "open-settings",
            MenuAction::ToggleVoice => "voix",
            MenuAction::ToggleMascot => "toggle-mascot",
            MenuAction::Quit => "quit",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MenuAction::StartMeeting => "Démarrer une réunion",
            MenuAction::Pause => "Pause",
            MenuAction::OpenNotebook => "Ouvrir Kobi",
            MenuAction::OpenSettings => "Réglages…",
            MenuAction::ToggleVoice => "Réponse à voix haute / en texte",
            MenuAction::ToggleMascot => "Afficher/masquer Kobi",
            MenuAction::Quit => "Quitter Kobi",
        }
    }

    pub fn from_id(id: &str) -> Option<MenuAction> {
        MenuAction::ALL.into_iter().find(|a| a.id() == id)
    }
}

/// Pose l'icône et son menu.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let items = MenuAction::ALL
        .into_iter()
        .map(|a| MenuItem::with_id(app, a.id(), a.label(), true, None::<&str>))
        .collect::<tauri::Result<Vec<_>>>()?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &items[0], &items[1], &items[2], &items[3], &items[4], &items[5], &separator, &items[6],
        ],
    )?;
    let mut tray = TrayIconBuilder::with_id("kobi")
        .tooltip("Kobi")
        .menu(&menu)
        .on_menu_event(|app, event| handle(app, event.id().as_ref()));
    if let Some(icon) = app.default_window_icon() {
        tray = tray.icon(icon.clone());
    }
    tray.build(app)?;
    Ok(())
}

/// Exécute une entrée du menu : journal (`~/Kobi/logs`), événement vers l'interface, puis
/// l'effet direct quand la coque sait le faire seule.
pub fn handle<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let Some(action) = MenuAction::from_id(id) else {
        tracing::warn!(entree = id, "menu : entrée inconnue");
        return;
    };
    tracing::info!(entree = action.id(), "menu");
    let _ = app.emit(EVENT, action.id());
    match action {
        MenuAction::ToggleMascot => {
            if let Some(window) = app.get_webview_window("main") {
                let visible = window.is_visible().unwrap_or(true);
                let _ = if visible {
                    window.hide()
                } else {
                    window.show()
                };
            }
        }
        // L'app Kobi (KOB-159) : notes, rappels, activité, réglages.
        MenuAction::OpenNotebook => {
            if let Err(e) = crate::app::ouvrir(app, None) {
                tracing::warn!(erreur = %e, "app");
            }
        }
        MenuAction::OpenSettings => {
            if let Err(e) = crate::reglages::ouvrir(app, None) {
                tracing::warn!(erreur = %e, "réglages");
            }
        }
        MenuAction::Quit => app.exit(0),
        // Voix ou texte seul pour le contexte courant (KOB-142).
        MenuAction::ToggleVoice => {
            crate::voix::basculer(app);
        }
        // En réunion, met l'enregistrement en pause (ou le reprend) ; hors
        // réunion, coupe le micro jusqu'au prochain clic (KOB-132).
        MenuAction::Pause => {
            let reunion = app
                .try_state::<crate::chef::ChefPartage<R>>()
                .map(|c| c.0.lock().map(|c| c.enregistrement()).unwrap_or_default())
                .unwrap_or_default();
            if reunion.actif {
                let i = if reunion.pause {
                    crate::intentions::Intention::Reprendre
                } else {
                    crate::intentions::Intention::Pause
                };
                crate::chef::publier(app, crate::chef::Message::Commande(i));
            } else if let Err(e) =
                crate::ecoute::commander(app, crate::ecoute::Commande::BasculerPause)
            {
                tracing::warn!(erreur = %e, "menu : pause de l'écoute impossible");
            }
        }
        // Demande explicite d'enregistrement (KOB-134).
        MenuAction::StartMeeting => crate::chef::publier(
            app,
            crate::chef::Message::Commande(crate::intentions::Intention::DemarrerReunion),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chaque_entree_a_un_identifiant_unique_et_reversible() {
        for a in MenuAction::ALL {
            assert_eq!(MenuAction::from_id(a.id()), Some(a));
        }
        let mut ids: Vec<_> = MenuAction::ALL.iter().map(|a| a.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), MenuAction::ALL.len());
    }

    #[test]
    fn entree_inconnue_ignoree() {
        assert_eq!(MenuAction::from_id("supprimer-tout"), None);
    }

    #[test]
    fn menu_du_ticket() {
        let labels: Vec<_> = MenuAction::ALL.iter().map(|a| a.label()).collect();
        assert_eq!(
            labels,
            [
                "Démarrer une réunion",
                "Pause",
                "Ouvrir Kobi",
                "Réglages…",
                "Réponse à voix haute / en texte",
                "Afficher/masquer Kobi",
                "Quitter Kobi"
            ]
        );
    }
}

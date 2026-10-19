//! Icône et menu dans la barre système : le point d'entrée quand la mascotte
//! est masquée. Mêmes entrées que le menu du clic droit (`menu.rs`).

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Runtime};

use crate::menu::{self, Entree};

/// Pose l'icône et son menu.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let mut items: Vec<Box<dyn IsMenuItem<R>>> = Vec::new();
    for entree in Entree::BARRE_SYSTEME {
        items.push(match entree {
            Some(e) => Box::new(MenuItem::with_id(
                app,
                e.id(),
                e.libelle(),
                true,
                None::<&str>,
            )?),
            None => Box::new(PredefinedMenuItem::separator(app)?),
        });
    }
    let refs: Vec<&dyn IsMenuItem<R>> = items.iter().map(|i| i.as_ref()).collect();
    let menu = Menu::with_items(app, &refs)?;
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

/// Exécute une entrée du menu (refus journalisés par `menu::executer`), sur
/// un fil à part : le menu de la barre système appelle sur le fil principal,
/// où créer la fenêtre de l'app bloque WebView2 sous Windows (KOB-167).
pub fn handle<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let app = app.clone();
    let id = id.to_owned();
    std::thread::spawn(move || {
        let _ = menu::executer(&app, &id);
    });
}

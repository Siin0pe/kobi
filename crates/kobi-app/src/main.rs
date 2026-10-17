//! kobi-app : coque Tauri 2 de Kobi. La mascotte vit dans une petite fenêtre
//! sans bordure, transparente, toujours au premier plan et absente de la barre
//! des tâches, en bas à droite de l'écran ou là où l'utilisateur l'a posée.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod placement;

use std::sync::mpsc::Sender;
use std::sync::Mutex;

use placement::{Rect, Saved};
use tauri::{Manager, PhysicalPosition, WebviewWindow, WindowEvent};

/// Canal vers le fil qui enregistre la position de la fenêtre.
struct PositionSaver(Mutex<Sender<Saved>>);

fn rect(monitor: &tauri::Monitor) -> Rect {
    let area = monitor.work_area();
    Rect {
        x: area.position.x,
        y: area.position.y,
        width: area.size.width,
        height: area.size.height,
    }
}

/// Pose la fenêtre à sa position mémorisée, ou en bas à droite de l'écran principal.
fn place(window: &WebviewWindow, saved: Option<Saved>) -> tauri::Result<()> {
    let reported = window.outer_size()?;
    // Taille déclarée dans tauri.conf.json, en pixels logiques.
    let declared = window
        .app_handle()
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == window.label())
        .map(|w| (w.width, w.height))
        .unwrap_or((410.0, 280.0));
    let primary = window.primary_monitor()?;
    let scale = primary.as_ref().map_or(1.0, |m| m.scale_factor());
    let size = placement::physical_size((reported.width, reported.height), declared, scale);
    let screens: Vec<Rect> = window.available_monitors()?.iter().map(rect).collect();
    let target = match placement::restore(saved, size, &screens) {
        Some(pos) => Some(pos),
        None => primary.map(|m| placement::bottom_right(rect(&m), size, m.scale_factor())),
    };
    if let Some((x, y)) = target {
        window.set_position(PhysicalPosition::new(x, y))?;
    }
    Ok(())
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let window = app
                .get_webview_window("main")
                .expect("fenêtre « main » déclarée dans tauri.conf.json");
            let file = placement::state_file(&app.path().home_dir()?);
            place(&window, placement::load(&file))?;
            app.manage(PositionSaver(Mutex::new(placement::spawn_saver(file))));
            window.show()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::Moved(pos) = event {
                if let Some(saver) = window.try_state::<PositionSaver>() {
                    let saved = Saved { x: pos.x, y: pos.y };
                    let _ = saver.0.lock().map(|tx| tx.send(saved));
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("échec du lancement de Kobi");
}

//! kobi-app : coque Tauri 2 de Kobi. La mascotte vit dans une petite fenêtre
//! sans bordure, transparente, toujours au premier plan et absente de la barre
//! des tâches, en bas à droite de l'écran ou là où l'utilisateur l'a posée.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bienvenue;
mod carnet;
mod cerveau;
mod chef;
mod controle;
mod demarrage;
mod depots;
mod diag;
mod ecoute;
mod ecrivain;
mod intentions;
mod interpellation;
mod lecture;
mod mcp;
mod mcp_config;
mod mot_cle;
mod notes;
mod placement;
mod porte;
mod raccourci;
mod rappels;
mod reglages;
mod session;
mod stockage;
#[cfg(test)]
mod test_ipc;
mod tray;
mod voix;

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

/// Sous Linux sans GPU, le moteur DMA-BUF de WebKitGTK ne peint rien et
/// tourne à vide (constaté sous Xvfb : fenêtre noire, 72 % d'un cœur). On le
/// désactive alors, sauf si l'utilisateur a déjà choisi.
#[cfg(target_os = "linux")]
fn choose_webkit_renderer() {
    let has_gpu = std::fs::read_dir("/dev/dri")
        .map(|d| {
            d.flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("renderD"))
        })
        .unwrap_or(false);
    if !has_gpu && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

/// Sortie de la CLI dans le terminal qui a lancé `kobi` : l'app est un
/// exécutable graphique sous Windows, sans console à elle. Une sortie déjà
/// redirigée (tube de `kobi mcp`, fichier) est gardée telle quelle.
#[cfg(windows)]
fn attacher_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(pid: u32) -> i32;
        fn GetStdHandle(id: u32) -> isize;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    // SAFETY : appels Win32 sans pointeur ; un échec laisse la sortie muette.
    unsafe {
        let sortie = GetStdHandle(STD_OUTPUT_HANDLE);
        if sortie == 0 || sortie == -1 {
            AttachConsole(ATTACH_PARENT_PROCESS);
        }
    }
}

fn main() {
    // `kobi status`, `kobi mcp`… : la CLI de Kobi (KOB-161), qui parle à
    // l'app en cours ; les options de l'app (`--carnet`…) lancent l'app.
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .first()
        .is_some_and(|a| kobi_ctl::cli::est_une_commande(a))
    {
        #[cfg(windows)]
        attacher_console();
        std::process::exit(kobi_ctl::cli::main_avec(&args));
    }
    #[cfg(target_os = "linux")]
    choose_webkit_renderer();
    session::installer(tauri::Builder::default())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            if let Some(dir) = kobi_session::logs::default_dir() {
                if let Err(e) = kobi_session::logs::init(&dir) {
                    eprintln!("journal {} indisponible : {e}", dir.display());
                }
            }
            let config_path = kobi_session::config::default_path();
            let config = match &config_path {
                Some(path) => kobi_session::config::load_or_create(path),
                None => kobi_session::config::Loaded {
                    config: Default::default(),
                    created: false,
                    problem: None,
                },
            };
            if let Some(problem) = &config.problem {
                tracing::warn!(%problem, "config.toml");
            }
            demarrage::installer(app.handle());
            // Interface de contrôle (KOB-161) : `kobi <commande>`, `kobi mcp`.
            controle::installer(app.handle(), config_path.clone());
            stockage::installer(app.handle(), &config.config, config_path.clone());
            // Réponse à voix haute ou en texte (KOB-142) : la voix se charge en arrière-plan.
            voix::installer(app.handle(), &config.config.voice, config_path);
            rappels::installer(app.handle());
            interpellation::installer(
                app.handle(),
                config.config.notes.interval_min,
                config.config.codebases.clone(),
            );
            raccourci::installer(app.handle(), &config.config)?;
            let window = app
                .get_webview_window("main")
                .expect("fenêtre « main » déclarée dans tauri.conf.json");
            let file = placement::state_file(&app.path().home_dir()?);
            place(&window, placement::load(&file))?;
            app.manage(PositionSaver(Mutex::new(placement::spawn_saver(file))));
            window.show()?;
            tray::install(app.handle())?;
            // Après la mascotte : la bienvenue se range à côté d'elle.
            bienvenue::installer(app.handle());
            // `kobi --app` / `--carnet` / `--reglages` / `--bienvenue` : ouvre
            // l'app (sur sa section) au lancement (raccourci du menu
            // Démarrer, essai sous Xvfb).
            for arg in std::env::args().skip(1) {
                let ouverte = match arg.as_str() {
                    "--app" => app::ouvrir(app.handle(), None),
                    "--carnet" => carnet::ouvrir(app.handle()),
                    "--reglages" => reglages::ouvrir(app.handle(), None),
                    "--bienvenue" => bienvenue::ouvrir(app.handle()),
                    _ => continue,
                };
                if let Err(e) = ouverte {
                    tracing::warn!(erreur = %e, arg, "ouverture au lancement");
                }
            }
            // Essai de bout en bout du menu (sans barre système, par exemple en CI
            // sous Xvfb) : chaque entrée est déclenchée une fois, « Quitter » en dernier.
            if std::env::var_os("KOBI_ESSAI_MENU").is_some() {
                let handle = app.handle().clone();
                std::thread::spawn(move || {
                    for id in [
                        "start-meeting",
                        "pause",
                        "toggle-mascot",
                        "toggle-mascot",
                        "quit",
                    ] {
                        std::thread::sleep(std::time::Duration::from_millis(300));
                        let h = handle.clone();
                        let _ = handle.run_on_main_thread(move || tray::handle(&h, id));
                    }
                });
            }
            Ok(())
        })
        .on_window_event(|window, event| {
            if let (WindowEvent::Moved(pos), "main") = (event, window.label()) {
                if let Some(saver) = window.try_state::<PositionSaver>() {
                    let saved = Saved { x: pos.x, y: pos.y };
                    let _ = saver.0.lock().map(|tx| tx.send(saved));
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("échec du lancement de Kobi");
}

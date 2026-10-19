//! Menu du clic droit sous la souris (KOB-202), comme un menu contextuel
//! Windows : il s'ouvre là où l'on a cliqué, n'importe où sur l'écran, dans
//! sa propre petite fenêtre transparente (la mascotte ne bouge pas), et se
//! retourne vers la gauche ou le haut s'il déborde de la zone de travail de
//! l'écran.
//!
//! 1. La mascotte demande [`ouvrir_menu`] avec l'état des libellés.
//! 2. La coque envoie à la fenêtre du menu le pointeur, la zone de travail et
//!    l'échelle de l'écran qui le contient (`menu://ouvrir`).
//! 3. La page du menu (`menu.html`) connaît sa taille : elle calcule sa place
//!    et la demande ([`montrer_menu`]).
//! 4. Choix, Échap ou perte du focus : [`menu_choisi`] cache la fenêtre et
//!    renvoie l'entrée à la mascotte (`menu://choix`), qui l'exécute.

use serde::Serialize;
use serde_json::Value;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime, WebviewUrl,
    WebviewWindowBuilder,
};

/// Libellé de la fenêtre du menu.
pub const FENETRE: &str = "menu";
/// Fenêtre de la mascotte, qui exécute l'entrée choisie.
const MASCOTTE: &str = "main";

/// Vers la page du menu : où s'ouvrir, avec quels libellés.
const EVENEMENT_OUVRIR: &str = "menu://ouvrir";
/// Vers la mascotte : l'entrée choisie.
const EVENEMENT_CHOIX: &str = "menu://choix";
/// Vers la mascotte : le menu est fermé.
const EVENEMENT_FERME: &str = "menu://ferme";

/// Taille de départ ; la page la remplace par la sienne à chaque ouverture.
const TAILLE: (f64, f64) = (300.0, 480.0);

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

/// Zone de travail d'un écran (sans la barre des tâches), en pixels physiques.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Zone {
    pub x: i32,
    pub y: i32,
    pub largeur: u32,
    pub hauteur: u32,
}

#[derive(Debug, Clone, Serialize)]
struct Ouverture {
    pointeur: Point,
    zone: Zone,
    echelle: f64,
    contexte: Value,
}

/// Crée la fenêtre du menu, cachée. Au démarrage, une fois pour toutes :
/// créée à la demande, la première ouverture attendrait sa page, et la créer
/// depuis une commande bloque WebView2 sous Windows (KOB-167).
pub fn installer<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    if app.get_webview_window(FENETRE).is_some() {
        return Ok(());
    }
    WebviewWindowBuilder::new(app, FENETRE, WebviewUrl::App("menu.html".into()))
        .title("Menu de Kobi")
        .inner_size(TAILLE.0, TAILLE.1)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .build()?;
    Ok(())
}

/// Point d'ouverture : le pointeur, ou, au clavier (touche Menu, Maj+F10),
/// le haut de la mascotte.
fn point<R: Runtime>(app: &AppHandle<R>, clavier: bool) -> Result<Point, String> {
    let mascotte = app.get_webview_window(MASCOTTE);
    if clavier {
        if let Some(w) = &mascotte {
            let pos = w.outer_position().map_err(|e| e.to_string())?;
            let taille = w.outer_size().map_err(|e| e.to_string())?;
            return Ok(Point {
                x: pos.x as f64 + taille.width as f64 / 2.0,
                y: pos.y as f64 + taille.height as f64 / 3.0,
            });
        }
    }
    let p = app.cursor_position().map_err(|e| e.to_string())?;
    Ok(Point { x: p.x, y: p.y })
}

/// Ouvre le menu sous le pointeur (ou sur Kobi au clavier) ; `contexte` fixe
/// ses libellés (réunion, pause, mode discret, voix).
#[tauri::command]
pub fn ouvrir_menu<R: Runtime>(
    app: AppHandle<R>,
    contexte: Value,
    clavier: bool,
) -> Result<(), String> {
    if app.get_webview_window(FENETRE).is_none() {
        return Err("fenêtre du menu indisponible".into());
    }
    let pointeur = point(&app, clavier)?;
    // L'écran du pointeur ; à défaut (pointeur entre deux écrans), le principal.
    let ecran = app
        .monitor_from_point(pointeur.x, pointeur.y)
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten())
        .ok_or("aucun écran")?;
    let zone = ecran.work_area();
    let ouverture = Ouverture {
        pointeur,
        zone: Zone {
            x: zone.position.x,
            y: zone.position.y,
            largeur: zone.size.width,
            hauteur: zone.size.height,
        },
        echelle: ecran.scale_factor(),
        contexte,
    };
    tracing::info!(
        x = pointeur.x,
        y = pointeur.y,
        clavier,
        "menu du clic droit"
    );
    app.emit_to(FENETRE, EVENEMENT_OUVRIR, ouverture)
        .map_err(|e| e.to_string())
}

/// Pose la fenêtre du menu (pixels physiques), la montre et lui donne le
/// focus : sa perte (clic ailleurs) le fermera.
#[tauri::command]
pub fn montrer_menu<R: Runtime>(
    app: AppHandle<R>,
    x: i32,
    y: i32,
    largeur: u32,
    hauteur: u32,
) -> Result<(), String> {
    let w = app
        .get_webview_window(FENETRE)
        .ok_or("fenêtre du menu indisponible")?;
    // La position d'abord : l'échelle de la fenêtre suit l'écran où elle est.
    w.set_position(PhysicalPosition::new(x, y))
        .map_err(|e| e.to_string())?;
    // Sous GTK, une fenêtre non redimensionnable garde sa taille : elle le
    // devient le temps de prendre celle du menu. Plus grande, sa partie
    // transparente prendrait des clics (et sortirait de l'écran).
    let _ = w.set_resizable(true);
    let taille = w.set_size(PhysicalSize::new(largeur.max(1), hauteur.max(1)));
    let _ = w.set_resizable(false);
    taille.map_err(|e| e.to_string())?;
    w.show().map_err(|e| e.to_string())?;
    w.set_focus().map_err(|e| e.to_string())
}

/// Menu fermé : choix d'une entrée (exécutée par la mascotte), Échap ou
/// clic ailleurs (`entree` absente).
#[tauri::command]
pub fn menu_choisi<R: Runtime>(app: AppHandle<R>, entree: Option<String>) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(FENETRE) {
        let _ = w.hide();
    }
    if let Some(entree) = &entree {
        tracing::info!(%entree, "menu du clic droit : entrée choisie");
        app.emit_to(MASCOTTE, EVENEMENT_CHOIX, entree)
            .map_err(|e| e.to_string())?;
    }
    app.emit_to(MASCOTTE, EVENEMENT_FERME, ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
    use tauri::{App, Listener};

    fn app() -> App<MockRuntime> {
        mock_builder().build(mock_context(noop_assets())).unwrap()
    }

    fn ecouter(app: &App<MockRuntime>, nom: &str) -> Arc<Mutex<Vec<String>>> {
        let recus = Arc::new(Mutex::new(Vec::new()));
        let r = Arc::clone(&recus);
        app.listen_any(nom, move |e| r.lock().unwrap().push(e.payload().to_owned()));
        recus
    }

    #[test]
    fn sans_fenetre_du_menu_la_mascotte_garde_le_sien() {
        let app = app();
        let e = ouvrir_menu(app.handle().clone(), Value::Null, false).unwrap_err();
        assert!(e.contains("indisponible"), "{e}");
    }

    // Le runtime simulé ne suit pas la visibilité des fenêtres : elle est
    // vérifiée sous Xvfb (fenêtre cachée au démarrage, montrée, recachée).
    #[test]
    fn fenetre_creee_une_seule_fois() {
        let app = app();
        installer(app.handle()).unwrap();
        installer(app.handle()).unwrap();
        let menus = app
            .webview_windows()
            .into_keys()
            .filter(|l| l == FENETRE)
            .count();
        assert_eq!(menus, 1);
    }

    #[test]
    fn choix_renvoye_a_la_mascotte_puis_menu_ferme() {
        let app = app();
        installer(app.handle()).unwrap();
        let choix = ecouter(&app, EVENEMENT_CHOIX);
        let ferme = ecouter(&app, EVENEMENT_FERME);
        menu_choisi(app.handle().clone(), Some("carnet".into())).unwrap();
        menu_choisi(app.handle().clone(), None).unwrap();
        assert_eq!(*choix.lock().unwrap(), ["\"carnet\""]);
        assert_eq!(ferme.lock().unwrap().len(), 2);
    }
}

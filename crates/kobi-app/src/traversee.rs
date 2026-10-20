//! Clics qui traversent (KOB-217) : la fenêtre de Kobi est transparente et
//! plus grande que lui. Hors de Kobi et de ses affichages, la souris passe aux
//! applications en dessous.
//!
//! L'interface envoie les rectangles de ses zones actives (`zones_actives`)
//! et l'état du bouton (`appui_souris`). Un fil sonde le pointeur (environ
//! 25 fois par seconde) et appelle `set_ignore_cursor_events` seulement quand
//! la décision change. La fenêtre garde toujours la souris tant qu'aucune zone
//! n'est connue et pendant un appui : une sélection qui déborde de la bulle ne
//! se casse pas.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Manager, Runtime};

/// Rythme du sondage du pointeur.
const SONDAGE: Duration = Duration::from_millis(40);

/// Rectangle actif, en px logiques de la fenêtre : x, y, largeur, hauteur.
pub type Zone = [f64; 4];

/// Zones reçues de l'interface et état du bouton.
#[derive(Default)]
pub struct Traversee {
    zones: Mutex<Option<Vec<Zone>>>,
    appui: AtomicBool,
}

/// La souris passe-t-elle aux applications en dessous ? Jamais sans zones
/// connues ni pendant un appui ; sinon, seulement hors de toute zone active.
pub fn laisser_passer(zones: Option<&[Zone]>, point: (f64, f64), appui: bool) -> bool {
    let Some(zones) = zones else { return false };
    if appui {
        return false;
    }
    let (x, y) = point;
    !zones
        .iter()
        .any(|[zx, zy, l, h]| x >= *zx && x < zx + l && y >= *zy && y < zy + h)
}

#[tauri::command]
pub fn zones_actives<R: Runtime>(app: AppHandle<R>, zones: Vec<Zone>) {
    if let Some(t) = app.try_state::<Traversee>() {
        *t.zones.lock().unwrap_or_else(|e| e.into_inner()) = Some(zones);
    }
}

#[tauri::command]
pub fn appui_souris<R: Runtime>(app: AppHandle<R>, appui: bool) {
    if let Some(t) = app.try_state::<Traversee>() {
        t.appui.store(appui, Ordering::SeqCst);
    }
}

/// Pointeur en px logiques de la fenêtre « main », s'il est connu.
fn pointeur<R: Runtime>(app: &AppHandle<R>) -> Option<(f64, f64)> {
    let fenetre = app.get_webview_window("main")?;
    let curseur = fenetre.cursor_position().ok()?;
    let origine = fenetre.inner_position().ok()?;
    let echelle = fenetre.scale_factor().ok()?;
    Some((
        (curseur.x - f64::from(origine.x)) / echelle,
        (curseur.y - f64::from(origine.y)) / echelle,
    ))
}

/// Lance le suivi du pointeur.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    app.manage(Traversee::default());
    let app = app.clone();
    let lance = std::thread::Builder::new()
        .name("traversee".into())
        .spawn(move || {
            let mut passe = false;
            loop {
                std::thread::sleep(SONDAGE);
                let Some(t) = app.try_state::<Traversee>() else {
                    continue;
                };
                let Some(point) = pointeur(&app) else {
                    continue;
                };
                let decision = {
                    let zones = t.zones.lock().unwrap_or_else(|e| e.into_inner());
                    laisser_passer(zones.as_deref(), point, t.appui.load(Ordering::SeqCst))
                };
                if decision == passe {
                    continue;
                }
                let Some(fenetre) = app.get_webview_window("main") else {
                    continue;
                };
                match fenetre.set_ignore_cursor_events(decision) {
                    Ok(()) => {
                        passe = decision;
                        tracing::debug!(passe, "souris : traverse la fenêtre de Kobi");
                    }
                    Err(e) => tracing::warn!(erreur = %e, "souris : réglage impossible"),
                }
            }
        });
    if let Err(e) = lance {
        tracing::warn!(erreur = %e, "suivi du pointeur impossible : la fenêtre garde la souris");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KOBI: Zone = [600.0, 320.0, 220.0, 160.0];
    const BULLE: Zone = [40.0, 60.0, 500.0, 240.0];

    #[test]
    fn sans_zones_la_fenetre_garde_la_souris() {
        assert!(!laisser_passer(None, (5.0, 5.0), false));
    }

    #[test]
    fn hors_des_zones_la_souris_passe() {
        let zones = [KOBI, BULLE];
        // Coin vide de la fenêtre transparente.
        assert!(laisser_passer(Some(&zones), (5.0, 5.0), false));
        // Entre la bulle et Kobi.
        assert!(laisser_passer(Some(&zones), (570.0, 200.0), false));
        // Liste vide (rien d'affiché) : tout passe.
        assert!(laisser_passer(Some(&[]), (700.0, 400.0), false));
    }

    #[test]
    fn dans_une_zone_kobi_garde_la_souris() {
        let zones = [KOBI, BULLE];
        assert!(!laisser_passer(Some(&zones), (700.0, 400.0), false));
        assert!(!laisser_passer(Some(&zones), (40.0, 60.0), false));
        // Bord droit et bas exclus : la zone suivante commence là.
        assert!(laisser_passer(Some(&zones), (820.0, 400.0), false));
        assert!(laisser_passer(Some(&zones), (700.0, 480.0), false));
    }

    #[test]
    fn pendant_un_appui_rien_ne_passe() {
        // Une sélection qui sort de la bulle, bouton tenu.
        assert!(!laisser_passer(Some(&[BULLE]), (5.0, 5.0), true));
    }
}

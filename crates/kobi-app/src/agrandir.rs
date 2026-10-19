//! Fenêtre de Kobi agrandie le temps d'une réponse longue (KOB-163) : plutôt
//! que de tasser la bulle dans 410 × 280, la fenêtre gagne de la place vers
//! le côté libre de l'écran (à gauche ou à droite, en haut ou en bas), puis
//! reprend sa taille et sa place. Kobi, lui, ne bouge pas à l'écran :
//! l'interface garde la scène dans son coin, à la même échelle.
//!
//! En deux temps, pour que la scène se range dans son coin avant que la
//! fenêtre change de taille : `agrandir_kobi { grand: true, appliquer: false }`
//! donne le sens, puis `appliquer: true` agrandit. Pendant l'agrandissement,
//! la position n'est pas mémorisée (`window.json` garde la petite fenêtre).

use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Manager, PhysicalPosition, PhysicalSize, Runtime};

use crate::placement::Rect;

/// Place gagnée, en pixels logiques : une colonne de lecture à côté de la
/// mascotte, et assez de hauteur pour une réponse de plusieurs paragraphes.
pub const EN_PLUS_LARGEUR: f64 = 420.0;
pub const HAUTEUR_MIN: f64 = 620.0;

/// Sens de l'agrandissement : vers la gauche ou la droite, le haut ou le bas.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Sens {
    pub gauche: bool,
    pub haut: bool,
}

/// Rectangle de la petite fenêtre, gardé pendant l'agrandissement.
#[derive(Default)]
pub struct Agrandissement(pub Mutex<Option<Rect>>);

impl Agrandissement {
    pub fn actif(&self) -> bool {
        self.0.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

/// Rectangle agrandi depuis `base` dans `zone` (pixels physiques) : on
/// pousse vers le côté où il y a la place, sans sortir de la zone de travail.
pub fn agrandi(base: Rect, zone: Rect, plus_largeur: u32, hauteur: u32) -> (Rect, Sens) {
    let gauche_libre = i64::from(base.x) - i64::from(zone.x);
    let droite_libre =
        i64::from(zone.x) + i64::from(zone.width) - (i64::from(base.x) + i64::from(base.width));
    let gauche = gauche_libre >= i64::from(plus_largeur) || gauche_libre >= droite_libre;
    let largeur = (base.width + plus_largeur).min(zone.width);
    let hauteur = hauteur.max(base.height).min(zone.height);
    let en_plus_h = i64::from(hauteur) - i64::from(base.height);
    let haut_libre = i64::from(base.y) - i64::from(zone.y);
    let bas_libre =
        i64::from(zone.y) + i64::from(zone.height) - (i64::from(base.y) + i64::from(base.height));
    let haut = haut_libre >= en_plus_h || haut_libre >= bas_libre;
    let x = if gauche {
        i64::from(base.x) + i64::from(base.width) - i64::from(largeur)
    } else {
        i64::from(base.x)
    };
    let y = if haut {
        i64::from(base.y) + i64::from(base.height) - i64::from(hauteur)
    } else {
        i64::from(base.y)
    };
    let borne = |v: i64, min: i32, taille: u32, total: u32| {
        v.min(i64::from(min) + i64::from(total) - i64::from(taille))
            .max(i64::from(min)) as i32
    };
    (
        Rect {
            x: borne(x, zone.x, largeur, zone.width),
            y: borne(y, zone.y, hauteur, zone.height),
            width: largeur,
            height: hauteur,
        },
        Sens { gauche, haut },
    )
}

fn calculer<R: Runtime>(
    w: &tauri::WebviewWindow<R>,
    base: Rect,
) -> tauri::Result<Option<(Rect, Sens)>> {
    let Some(ecran) = w.current_monitor()?.or(w.primary_monitor()?) else {
        return Ok(None);
    };
    let zone = ecran.work_area();
    let echelle = ecran.scale_factor();
    let zone = Rect {
        x: zone.position.x,
        y: zone.position.y,
        width: zone.size.width,
        height: zone.size.height,
    };
    let plus = (EN_PLUS_LARGEUR * echelle).round() as u32;
    let hauteur = (HAUTEUR_MIN * echelle).round() as u32;
    Ok(Some(agrandi(base, zone, plus, hauteur)))
}

/// Agrandit la fenêtre de Kobi (`grand`), ou lui rend sa taille et sa place.
/// Sans `appliquer`, donne seulement le sens. `None` : rien à faire (pas
/// d'écran, ou déjà dans cet état).
#[tauri::command]
pub fn agrandir_kobi<R: Runtime>(
    app: AppHandle<R>,
    grand: bool,
    appliquer: bool,
) -> Result<Option<Sens>, String> {
    let w = app
        .get_webview_window("main")
        .ok_or("fenêtre de Kobi absente")?;
    let etat = app.state::<Agrandissement>();
    let err = |e: tauri::Error| e.to_string();
    if !grand {
        let base = etat.0.lock().map_err(|e| e.to_string())?.take();
        if let Some(b) = base {
            w.set_size(PhysicalSize::new(b.width, b.height))
                .map_err(err)?;
            w.set_position(PhysicalPosition::new(b.x, b.y))
                .map_err(err)?;
            tracing::info!("fenêtre rendue à sa taille");
        }
        return Ok(None);
    }
    let mut garde = etat.0.lock().map_err(|e| e.to_string())?;
    let base = match *garde {
        Some(b) => b,
        None => {
            let (p, t) = (
                w.outer_position().map_err(err)?,
                w.outer_size().map_err(err)?,
            );
            Rect {
                x: p.x,
                y: p.y,
                width: t.width,
                height: t.height,
            }
        }
    };
    let Some((cible, sens)) = calculer(&w, base).map_err(err)? else {
        return Ok(None);
    };
    if appliquer && garde.is_none() {
        *garde = Some(base);
        drop(garde);
        w.set_position(PhysicalPosition::new(cible.x, cible.y))
            .map_err(err)?;
        w.set_size(PhysicalSize::new(cible.width, cible.height))
            .map_err(err)?;
        tracing::info!(
            ?sens,
            largeur = cible.width,
            hauteur = cible.height,
            "fenêtre agrandie pour une réponse longue"
        );
    }
    Ok(Some(sens))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZONE: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };

    fn fenetre(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            width: 410,
            height: 280,
        }
    }

    #[test]
    fn en_bas_a_droite_on_pousse_vers_la_gauche_et_le_haut() {
        let (r, sens) = agrandi(fenetre(1494, 744), ZONE, 420, 620);
        assert_eq!(
            sens,
            Sens {
                gauche: true,
                haut: true
            }
        );
        // Le coin bas droit ne bouge pas : Kobi reste à sa place.
        assert_eq!((r.x + r.width as i32, r.y + r.height as i32), (1904, 1024));
        assert_eq!((r.width, r.height), (830, 620));
    }

    #[test]
    fn en_haut_a_gauche_on_pousse_vers_la_droite_et_le_bas() {
        let (r, sens) = agrandi(fenetre(10, 7), ZONE, 420, 620);
        assert_eq!(
            sens,
            Sens {
                gauche: false,
                haut: false
            }
        );
        assert_eq!((r.x, r.y), (10, 7));
    }

    #[test]
    fn en_haut_a_droite_vers_la_gauche_et_le_bas() {
        let (r, sens) = agrandi(fenetre(1230, 7), ZONE, 420, 620);
        assert_eq!(
            sens,
            Sens {
                gauche: true,
                haut: false
            }
        );
        assert_eq!((r.x + r.width as i32, r.y), (1640, 7));
    }

    #[test]
    fn jamais_hors_de_la_zone_de_travail() {
        let petite = Rect {
            x: 0,
            y: 0,
            width: 700,
            height: 500,
        };
        let (r, _) = agrandi(fenetre(200, 100), petite, 420, 620);
        assert!(r.x >= 0 && r.y >= 0);
        assert!(r.x + r.width as i32 <= 700 && r.y + r.height as i32 <= 500);
    }
}

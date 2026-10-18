//! Vue agrandie de la conversation (KOB-169, après KOB-163) : quand il faut
//! lire, la fenêtre de Kobi s'agrandit le temps de la lecture pour contenir
//! un panneau large et lisible, à la taille et à la place réglées (à côté de
//! Kobi, au centre de l'écran, sur la moitié droite), puis reprend sa taille
//! et sa place. Pas de deuxième fenêtre : c'est la fenêtre transparente de
//! Kobi qui couvre Kobi et le panneau. Kobi, lui, ne bouge pas à l'écran :
//! l'interface garde la scène à sa place, à la même échelle, et le panneau
//! ne le recouvre jamais.
//!
//! En deux temps, pour que l'interface place la scène avant que la fenêtre
//! change de taille : `agrandir_kobi { grand: true, appliquer: false, … }`
//! donne la disposition, puis `appliquer: true` agrandit. Pendant
//! l'agrandissement, la position n'est pas mémorisée (`window.json` garde la
//! petite fenêtre).
//!
//! La vue s'ouvre aussi à la demande, hors de l'interface : raccourci
//! `hotkey_view`, « Kobi, montre en grand », `kobi view open` ; la coque émet
//! alors l'événement [`EVENEMENT_VUE`].

use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Runtime};

use crate::placement::Rect;

/// Événement vers l'interface : `{ "action": "ouvrir" | "fermer" | "basculer" }`.
pub const EVENEMENT_VUE: &str = "vue";

/// Taille du panneau par défaut, en pixels logiques.
pub const PANNEAU_PAR_DEFAUT: (f64, f64) = (640.0, 560.0);
/// Marge au bord de l'écran et entre le panneau et Kobi, en pixels logiques.
const MARGE: f64 = 16.0;

/// Où se pose le panneau.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Position {
    /// À côté de Kobi, du côté où il y a la place.
    #[default]
    ACote,
    /// Au centre de l'écran.
    Centre,
    /// Sur la moitié droite de l'écran.
    MoitieDroite,
}

/// Un rectangle en pixels logiques, relatif à la fenêtre agrandie.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Boite {
    pub x: f64,
    pub y: f64,
    pub largeur: f64,
    pub hauteur: f64,
}

/// Disposition de la fenêtre agrandie, pour l'interface : où reste la scène
/// de Kobi (là où était la petite fenêtre) et où se pose le panneau.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Disposition {
    pub kobi: Boite,
    pub panneau: Boite,
}

/// Rectangle de la petite fenêtre, gardé pendant l'agrandissement.
#[derive(Default)]
pub struct Agrandissement(pub Mutex<Option<Rect>>);

impl Agrandissement {
    pub fn actif(&self) -> bool {
        self.0.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

fn droite(r: &Rect) -> i64 {
    i64::from(r.x) + i64::from(r.width)
}
fn bas(r: &Rect) -> i64 {
    i64::from(r.y) + i64::from(r.height)
}
fn croise(a: &Rect, b: &Rect) -> bool {
    i64::from(a.x) < droite(b)
        && i64::from(b.x) < droite(a)
        && i64::from(a.y) < bas(b)
        && i64::from(b.y) < bas(a)
}
fn rect(x: i64, y: i64, l: i64, h: i64) -> Rect {
    Rect {
        x: x as i32,
        y: y as i32,
        width: l.max(1) as u32,
        height: h.max(1) as u32,
    }
}

/// Corps de Kobi (pilule et mot) dans sa petite fenêtre : le panneau ne le
/// recouvre jamais. La partie gauche de la fenêtre, transparente, est celle
/// de ses bulles : le panneau peut s'y poser.
pub fn corps(base: Rect) -> Rect {
    let (l, h) = (i64::from(base.width), i64::from(base.height));
    rect(
        i64::from(base.x) + l * 52 / 100,
        i64::from(base.y) + h * 28 / 100,
        l * 40 / 100,
        h * 68 / 100,
    )
}

/// Panneau (pixels physiques) dans la zone de travail, puis fenêtre qui
/// contient Kobi et le panneau.
pub fn disposer(
    base: Rect,
    zone: Rect,
    panneau: (u32, u32),
    position: Position,
    marge: u32,
) -> (Rect, Rect) {
    let m = i64::from(marge);
    let (zx, zy, zl, zh) = (
        i64::from(zone.x),
        i64::from(zone.y),
        i64::from(zone.width),
        i64::from(zone.height),
    );
    let l = i64::from(panneau.0).min(zl - 2 * m);
    let h = i64::from(panneau.1).min(zh - 2 * m);
    let k = corps(base);
    let borne =
        |v: i64, min: i64, taille: i64, total: i64| v.min(min + total - taille - m).max(min + m);
    let mut p = match position {
        Position::ACote => {
            // Du côté libre : à gauche, le bord droit du panneau là où
            // finissaient les bulles ; sinon à droite du corps de Kobi.
            let fin_bulles = i64::from(base.x) + i64::from(base.width) * 49 / 100;
            let x = if fin_bulles - l >= zx + m {
                fin_bulles - l
            } else {
                droite(&k) + m
            };
            let y = bas(&base) - i64::from(base.height) * 5 / 100 - h;
            rect(borne(x, zx, l, zl), borne(y, zy, h, zh), l, h)
        }
        Position::Centre => rect(zx + (zl - l) / 2, zy + (zh - h) / 2, l, h),
        Position::MoitieDroite => rect(zx + zl / 2, zy + m, zl / 2 - m, zh - 2 * m),
    };
    // Kobi reste visible : un panneau qui le recouvrirait passe à côté de lui,
    // ou se rétrécit jusqu'à son bord.
    let moitie = zx + zl / 2;
    if croise(&p, &k) && position == Position::MoitieDroite && i64::from(k.x) - m - moitie >= 360 {
        // La moitié droite, jusqu'au bord de Kobi.
        p = rect(
            moitie,
            i64::from(p.y),
            i64::from(k.x) - m - moitie,
            i64::from(p.height),
        );
    }
    if croise(&p, &k) {
        let a_gauche = i64::from(k.x) - m - i64::from(p.width);
        let a_droite = droite(&k) + m;
        if a_gauche >= zx + m {
            p.x = a_gauche as i32;
        } else if a_droite + i64::from(p.width) <= zx + zl - m {
            p.x = a_droite as i32;
        } else {
            let place_gauche = i64::from(k.x) - m - (zx + m);
            let place_droite = zx + zl - m - a_droite;
            if place_gauche >= place_droite {
                p = rect(zx + m, i64::from(p.y), place_gauche, i64::from(p.height));
            } else {
                p = rect(a_droite, i64::from(p.y), place_droite, i64::from(p.height));
            }
        }
    }
    let x0 = i64::from(base.x).min(i64::from(p.x)).max(zx);
    let y0 = i64::from(base.y).min(i64::from(p.y)).max(zy);
    let x1 = droite(&base).max(droite(&p)).min(zx + zl);
    let y1 = bas(&base).max(bas(&p)).min(zy + zh);
    (rect(x0, y0, x1 - x0, y1 - y0), p)
}

/// La disposition, en pixels logiques relatifs à la fenêtre agrandie.
pub fn relative(fenetre: Rect, base: Rect, panneau: Rect, echelle: f64) -> Disposition {
    let b = |r: Rect| Boite {
        x: f64::from(r.x - fenetre.x) / echelle,
        y: f64::from(r.y - fenetre.y) / echelle,
        largeur: f64::from(r.width) / echelle,
        hauteur: f64::from(r.height) / echelle,
    };
    Disposition {
        kobi: b(base),
        panneau: b(panneau),
    }
}

/// Agrandit la fenêtre de Kobi (`grand`) pour la vue de la conversation, ou
/// lui rend sa taille et sa place. Sans `appliquer`, donne seulement la
/// disposition. `None` : rien à faire (pas d'écran, ou fenêtre rendue).
#[tauri::command]
pub fn agrandir_kobi<R: Runtime>(
    app: AppHandle<R>,
    grand: bool,
    appliquer: bool,
    largeur: Option<f64>,
    hauteur: Option<f64>,
    position: Option<Position>,
) -> Result<Option<Disposition>, String> {
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
            tracing::info!("vue agrandie fermée, fenêtre rendue à sa taille");
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
    let Some(ecran) = w
        .current_monitor()
        .map_err(err)?
        .or(w.primary_monitor().map_err(err)?)
    else {
        return Ok(None);
    };
    let echelle = ecran.scale_factor();
    let zone = ecran.work_area();
    let zone = Rect {
        x: zone.position.x,
        y: zone.position.y,
        width: zone.size.width,
        height: zone.size.height,
    };
    let taille = (
        (largeur.unwrap_or(PANNEAU_PAR_DEFAUT.0) * echelle).round() as u32,
        (hauteur.unwrap_or(PANNEAU_PAR_DEFAUT.1) * echelle).round() as u32,
    );
    let position = position.unwrap_or_default();
    let (fenetre, panneau) = disposer(
        base,
        zone,
        taille,
        position,
        (MARGE * echelle).round() as u32,
    );
    if appliquer {
        // Déjà agrandie (autre réglage) : on repart de la petite fenêtre.
        if garde.is_none() {
            *garde = Some(base);
        }
        drop(garde);
        w.set_position(PhysicalPosition::new(fenetre.x, fenetre.y))
            .map_err(err)?;
        w.set_size(PhysicalSize::new(fenetre.width, fenetre.height))
            .map_err(err)?;
        tracing::info!(
            ?position,
            largeur = panneau.width,
            hauteur = panneau.height,
            "vue agrandie de la conversation"
        );
    }
    Ok(Some(relative(fenetre, base, panneau, echelle)))
}

/// Ouvre, ferme ou bascule la vue agrandie depuis la coque (raccourci, voix,
/// commande `kobi`) : Kobi se montre et prend le focus pour l'ouvrir.
pub fn vue<R: Runtime>(app: &AppHandle<R>, action: &str) {
    let Some(fenetre) = app.get_webview_window("main") else {
        return;
    };
    if action != "fermer" {
        let _ = fenetre.show();
        let _ = fenetre.unminimize();
        let _ = fenetre.set_focus();
    }
    let _ = fenetre.emit(EVENEMENT_VUE, serde_json::json!({ "action": action }));
    tracing::info!(action, "vue agrandie demandée");
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

    fn kobi(x: i32, y: i32) -> Rect {
        Rect {
            x,
            y,
            width: 410,
            height: 280,
        }
    }

    fn dans(r: &Rect, z: &Rect) -> bool {
        r.x >= z.x && r.y >= z.y && droite(r) <= droite(z) && bas(r) <= bas(z)
    }

    #[test]
    fn a_cote_de_kobi_en_bas_a_droite_le_panneau_se_pose_a_gauche() {
        let base = kobi(1494, 744);
        let (f, p) = disposer(base, ZONE, (640, 560), Position::ACote, 16);
        assert!(!croise(&p, &corps(base)), "Kobi reste visible");
        assert!(droite(&p) <= i64::from(corps(base).x));
        assert!(dans(&p, &ZONE) && dans(&f, &ZONE));
        // La fenêtre contient Kobi, qui ne bouge pas.
        assert!(f.x <= base.x && droite(&f) >= droite(&base) && bas(&f) >= bas(&base));
    }

    #[test]
    fn a_cote_de_kobi_colle_a_gauche_le_panneau_passe_a_droite() {
        let base = kobi(10, 7);
        let (f, p) = disposer(base, ZONE, (640, 560), Position::ACote, 16);
        assert!(i64::from(p.x) >= droite(&corps(base)));
        assert!(!croise(&p, &corps(base)));
        assert!(dans(&f, &ZONE));
    }

    #[test]
    fn au_centre_de_l_ecran() {
        let base = kobi(1494, 744);
        let (_, p) = disposer(base, ZONE, (640, 560), Position::Centre, 16);
        assert_eq!((p.x, p.y), ((1920 - 640) / 2, (1040 - 560) / 2));
    }

    #[test]
    fn moitie_droite_sans_recouvrir_kobi() {
        let base = kobi(1494, 744);
        let (_, p) = disposer(base, ZONE, (640, 560), Position::MoitieDroite, 16);
        assert!(!croise(&p, &corps(base)));
        assert_eq!(p.x, 960, "la moitié droite, rétrécie jusqu'au bord de Kobi");
        assert!(p.x >= 0 && dans(&p, &ZONE));
        // Kobi en haut à gauche : la moitié droite entière.
        let (_, p) = disposer(kobi(10, 7), ZONE, (640, 560), Position::MoitieDroite, 16);
        assert_eq!((p.x, p.width), (960, 944));
    }

    #[test]
    fn jamais_hors_de_la_zone_de_travail() {
        let petite = Rect {
            x: 0,
            y: 0,
            width: 900,
            height: 600,
        };
        for pos in [Position::ACote, Position::Centre, Position::MoitieDroite] {
            let (f, p) = disposer(kobi(400, 300), petite, (860, 720), pos, 16);
            assert!(dans(&f, &petite), "{pos:?}");
            assert!(dans(&p, &petite), "{pos:?}");
        }
    }

    #[test]
    fn disposition_relative_a_la_fenetre() {
        let base = kobi(1494, 744);
        let (f, p) = disposer(base, ZONE, (640, 560), Position::ACote, 16);
        let d = relative(f, base, p, 1.0);
        assert_eq!(d.kobi.x, f64::from(base.x - f.x));
        assert_eq!(d.panneau.largeur, 640.0);
    }
}

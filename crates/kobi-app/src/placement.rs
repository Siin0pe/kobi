//! Position de la fenêtre : en bas à droite au premier lancement, puis là où
//! l'utilisateur l'a laissée (`~/Kobi/window.json`).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Marge entre la fenêtre et le bord de la zone de travail, en pixels logiques.
pub const MARGIN: f64 = 16.0;

/// Délai sans mouvement avant d'écrire la position sur le disque.
const SAVE_DELAY: Duration = Duration::from_millis(500);

/// Un rectangle en pixels physiques.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x
            && y >= self.y
            && i64::from(x) < i64::from(self.x) + i64::from(self.width)
            && i64::from(y) < i64::from(self.y) + i64::from(self.height)
    }
}

/// Position mémorisée, en pixels physiques.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    pub x: i32,
    pub y: i32,
}

/// Coin bas droit de la zone de travail (hors barre des tâches), avec une marge.
pub fn bottom_right(work_area: Rect, window: (u32, u32), scale: f64) -> (i32, i32) {
    let margin = (MARGIN * scale).round() as i64;
    let x = i64::from(work_area.x) + i64::from(work_area.width) - i64::from(window.0) - margin;
    let y = i64::from(work_area.y) + i64::from(work_area.height) - i64::from(window.1) - margin;
    (
        x.max(i64::from(work_area.x)) as i32,
        y.max(i64::from(work_area.y)) as i32,
    )
}

/// Taille physique de la fenêtre. Sous Linux, une fenêtre encore cachée
/// annonce 0 × 0 : on se rabat alors sur la taille déclarée (pixels logiques)
/// multipliée par l'échelle de l'écran.
pub fn physical_size(reported: (u32, u32), declared: (f64, f64), scale: f64) -> (u32, u32) {
    if reported.0 > 0 && reported.1 > 0 {
        return reported;
    }
    (
        (declared.0 * scale).round() as u32,
        (declared.1 * scale).round() as u32,
    )
}

/// Garde la position mémorisée seulement si le centre de la fenêtre tombe sur
/// un écran encore branché.
pub fn restore(saved: Option<Saved>, window: (u32, u32), screens: &[Rect]) -> Option<(i32, i32)> {
    let s = saved?;
    let cx = s.x.saturating_add((window.0 / 2) as i32);
    let cy = s.y.saturating_add((window.1 / 2) as i32);
    screens
        .iter()
        .any(|r| r.contains(cx, cy))
        .then_some((s.x, s.y))
}

pub fn state_file(home: &Path) -> PathBuf {
    home.join("Kobi").join("window.json")
}

pub fn load(path: &Path) -> Option<Saved> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn save(path: &Path, pos: Saved) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec(&pos).map_err(std::io::Error::other)?,
    )?;
    std::fs::rename(tmp, path)
}

/// Écrit la position sur le disque une fois la fenêtre immobile depuis
/// [`SAVE_DELAY`]. Le fil attend sans se réveiller tant que rien ne bouge.
pub fn spawn_saver(path: PathBuf) -> Sender<Saved> {
    let (tx, rx) = mpsc::channel::<Saved>();
    std::thread::Builder::new()
        .name("kobi-window-pos".into())
        .spawn(move || {
            while let Ok(mut pos) = rx.recv() {
                loop {
                    match rx.recv_timeout(SAVE_DELAY) {
                        Ok(next) => pos = next,
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => {
                            let _ = save(&path, pos);
                            return;
                        }
                    }
                }
                if let Err(e) = save(&path, pos) {
                    eprintln!("kobi : position de la fenêtre non enregistrée : {e}");
                }
            }
        })
        .expect("fil d'enregistrement de la position");
    tx
}

impl Rect {
    fn croise(&self, autre: &Rect) -> bool {
        let fin = |r: &Rect| {
            (
                i64::from(r.x) + i64::from(r.width),
                i64::from(r.y) + i64::from(r.height),
            )
        };
        let (a, b) = (fin(self), fin(autre));
        i64::from(self.x) < b.0
            && i64::from(autre.x) < a.0
            && i64::from(self.y) < b.1
            && i64::from(autre.y) < a.1
    }
}

/// Position d'une fenêtre secondaire (bienvenue, Carnet, Réglages) : centrée
/// dans la zone de travail, sauf si elle passerait sous Kobi, toujours au
/// premier plan. Elle se range alors à gauche de Kobi, sinon au-dessus,
/// sinon en haut à gauche.
pub fn a_cote_de_kobi(work_area: Rect, fenetre: (u32, u32), kobi: Rect, scale: f64) -> (i32, i32) {
    let marge = (MARGIN * scale).round() as i64;
    let (l, h) = (i64::from(fenetre.0), i64::from(fenetre.1));
    let (ax, ay) = (i64::from(work_area.x), i64::from(work_area.y));
    let (al, ah) = (i64::from(work_area.width), i64::from(work_area.height));
    let centre = (ax + (al - l) / 2, ay + (ah - h) / 2);
    let pose = |x: i64, y: i64| Rect {
        x: x as i32,
        y: y as i32,
        width: fenetre.0,
        height: fenetre.1,
    };
    let borne = |v: i64, min: i64, taille: i64, total: i64| v.min(min + total - taille).max(min);
    if !pose(centre.0, centre.1).croise(&kobi) {
        return (centre.0 as i32, centre.1 as i32);
    }
    let a_gauche = i64::from(kobi.x) - marge - l;
    if a_gauche >= ax {
        let x = a_gauche.min(centre.0);
        return (x as i32, borne(centre.1, ay, h, ah) as i32);
    }
    let au_dessus = i64::from(kobi.y) - marge - h;
    if au_dessus >= ay {
        return (
            borne(centre.0, ax, l, al) as i32,
            au_dessus.min(centre.1) as i32,
        );
    }
    ((ax + marge) as i32, (ay + marge) as i32)
}

/// Construit une fenêtre secondaire de `taille` (pixels logiques) à côté de
/// la mascotte (fenêtre `main`), sur l'écran de celle-ci ; centrée si la
/// position de Kobi est illisible. La position est donnée au constructeur :
/// sous GTK, la déplacer après coup est écrasé par le centrage à l'affichage.
pub fn a_cote_de_kobi_ou_centre<'a, R: tauri::Runtime, M: tauri::Manager<R>>(
    app: &M,
    constructeur: tauri::WebviewWindowBuilder<'a, R, M>,
    taille: (f64, f64),
) -> tauri::WebviewWindowBuilder<'a, R, M> {
    let lire = || -> tauri::Result<Option<(f64, f64)>> {
        let Some(kobi) = app.get_webview_window("main") else {
            return Ok(None);
        };
        // Juste après `show()`, GTK peut encore annoncer la mascotte cachée
        // et hors de tout écran : sa position, elle, est déjà posée.
        let (pos, t) = (kobi.outer_position()?, kobi.outer_size()?);
        let Some(ecran) = kobi.current_monitor()?.or(kobi.primary_monitor()?) else {
            return Ok(None);
        };
        let zone = ecran.work_area();
        let echelle = ecran.scale_factor();
        let declaree = app
            .config()
            .app
            .windows
            .iter()
            .find(|w| w.label == "main")
            .map(|w| (w.width, w.height))
            .unwrap_or((410.0, 280.0));
        let t = physical_size((t.width, t.height), declaree, echelle);
        let (x, y) = a_cote_de_kobi(
            Rect {
                x: zone.position.x,
                y: zone.position.y,
                width: zone.size.width,
                height: zone.size.height,
            },
            (
                (taille.0 * echelle).round() as u32,
                (taille.1 * echelle).round() as u32,
            ),
            Rect {
                x: pos.x,
                y: pos.y,
                width: t.0,
                height: t.1,
            },
            echelle,
        );
        Ok(Some((f64::from(x) / echelle, f64::from(y) / echelle)))
    };
    match lire() {
        Ok(Some((x, y))) => {
            tracing::info!(x, y, "fenêtre rangée à côté de Kobi");
            constructeur.position(x, y)
        }
        autre => {
            tracing::info!(?autre, "position de Kobi illisible : fenêtre centrée");
            constructeur.center()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FHD: Rect = Rect {
        x: 0,
        y: 0,
        width: 1920,
        height: 1040,
    };

    #[test]
    fn coin_bas_droit_avec_marge() {
        assert_eq!(bottom_right(FHD, (410, 280), 1.0), (1494, 744));
        // Écran à 150 % : marge de 24 px physiques.
        assert_eq!(bottom_right(FHD, (615, 420), 1.5), (1281, 596));
    }

    #[test]
    fn coin_bas_droit_ecran_secondaire_a_gauche() {
        let left = Rect {
            x: -1280,
            y: 0,
            width: 1280,
            height: 984,
        };
        assert_eq!(bottom_right(left, (410, 280), 1.0), (-426, 688));
    }

    #[test]
    fn fenetre_plus_grande_que_l_ecran() {
        let tiny = Rect {
            x: 0,
            y: 0,
            width: 300,
            height: 200,
        };
        assert_eq!(bottom_right(tiny, (410, 280), 1.0), (0, 0));
    }

    #[test]
    fn taille_declaree_si_la_fenetre_cachee_annonce_zero() {
        // Constaté sous Linux (GTK) : 0 × 0 avant le premier affichage.
        assert_eq!(physical_size((0, 0), (410.0, 280.0), 1.0), (410, 280));
        assert_eq!(physical_size((0, 0), (410.0, 280.0), 1.25), (513, 350));
        assert_eq!(physical_size((615, 420), (410.0, 280.0), 1.5), (615, 420));
        // Le coin bas droit en tient compte : plus de fenêtre poussée hors de l'écran.
        let size = physical_size((0, 0), (410.0, 280.0), 1.0);
        assert_eq!(bottom_right(FHD, size, 1.0), (1494, 744));
    }

    #[test]
    fn position_gardee_si_l_ecran_existe() {
        let saved = Some(Saved { x: 100, y: 200 });
        assert_eq!(restore(saved, (410, 280), &[FHD]), Some((100, 200)));
    }

    #[test]
    fn position_oubliee_si_l_ecran_a_disparu() {
        let saved = Some(Saved { x: 2500, y: 200 });
        assert_eq!(restore(saved, (410, 280), &[FHD]), None);
        assert_eq!(restore(None, (410, 280), &[FHD]), None);
    }

    #[test]
    fn aller_retour_sur_le_disque() {
        let dir = std::env::temp_dir().join(format!("kobi-placement-{}", std::process::id()));
        let path = state_file(&dir);
        let tx = spawn_saver(path.clone());
        tx.send(Saved { x: 1, y: 2 }).unwrap();
        tx.send(Saved { x: 30, y: 40 }).unwrap();
        drop(tx);
        // Le fil écrit la dernière position dès que l'émetteur disparaît.
        let mut found = None;
        for _ in 0..50 {
            found = load(&path);
            if found.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(found, Some(Saved { x: 30, y: 40 }));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fenetre_secondaire_jamais_sous_kobi() {
        let ecran = Rect {
            x: 0,
            y: 0,
            width: 1440,
            height: 900,
        };
        let kobi = Rect {
            x: 1014,
            y: 604,
            width: 410,
            height: 280,
        };
        // Centrée, elle passerait sous Kobi : rangée à sa gauche.
        let (x, y) = a_cote_de_kobi(ecran, (900, 640), kobi, 1.0);
        assert_eq!((x, y), (98, 130));
        let posee = Rect {
            x,
            y,
            width: 900,
            height: 640,
        };
        assert!(!posee.croise(&kobi));
        // Grand écran : centrée, Kobi est loin.
        let grand = Rect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1040,
        };
        let kobi = Rect {
            x: 1494,
            y: 744,
            width: 410,
            height: 280,
        };
        assert_eq!(a_cote_de_kobi(grand, (900, 640), kobi, 1.0), (510, 200));
        // Écran trop étroit pour la gauche : au-dessus.
        let etroit = Rect {
            x: 0,
            y: 0,
            width: 1000,
            height: 1400,
        };
        let kobi = Rect {
            x: 574,
            y: 900,
            width: 410,
            height: 280,
        };
        let (x, y) = a_cote_de_kobi(etroit, (900, 640), kobi, 1.0);
        assert!(!Rect {
            x,
            y,
            width: 900,
            height: 640
        }
        .croise(&kobi));
        assert_eq!(y, 900 - 16 - 640);
    }
}

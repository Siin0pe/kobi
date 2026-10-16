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
}

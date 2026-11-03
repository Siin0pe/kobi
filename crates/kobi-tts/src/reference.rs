//! Ce que le haut-parleur joue, et quand (KOB-248) : la référence de
//! l'annulation d'écho de l'écoute.
//!
//! Le fil audio y dépose ce qu'il envoie à la carte son, silences compris,
//! avec l'instant où il le fait. L'écoute reprend, pour chaque bloc du micro,
//! ce qui a été joué au même instant : le retard entre les deux reste
//! constant (celui des tampons audio et de la pièce, que l'annulation d'écho
//! estime), même quand la carte son ou le micro livrent par à-coups.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Gardé au plus : le micro arrive avec du retard, jusqu'à plusieurs
/// secondes quand l'écoute est prise par la transcription (veille d'une
/// réunion en retard, KOB-248). Borné : 10 s à 48 kHz, moins de 2 Mo.
const MAX_S: f64 = 10.0;
/// Écart à la chronologie attendue au-delà duquel le flux repart de zéro
/// (flux rouvert, fil audio suspendu) : en deçà, c'est la gigue des rappels.
const RECALAGE: Duration = Duration::from_millis(100);
/// Part de l'écart rattrapée à chaque rappel : la chronologie suit les
/// rappels réels (trous de lecture, horloges) sans à-coup, la gigue
/// s'efface.
const SUIVI: f64 = 0.05;

struct Tampon {
    frequence: u32,
    /// Instant où le premier échantillon de `file` a été joué.
    debut: Option<Instant>,
    file: VecDeque<f32>,
}

static TAMPON: Mutex<Tampon> = Mutex::new(Tampon {
    frequence: 0,
    debut: None,
    file: VecDeque::new(),
});

fn duree(n: usize, frequence: u32) -> Duration {
    Duration::from_secs_f64(n as f64 / frequence as f64)
}

/// Fil audio : échantillons mono joués à `frequence` Hz à partir de
/// `maintenant`. L'écoute ne tient le verrou que le temps d'une copie : rien
/// n'est perdu, sinon la chronologie se décalerait.
pub fn deposer(echantillons: &[f32], frequence: u32, maintenant: Instant) {
    let mut t = TAMPON.lock().unwrap_or_else(|e| e.into_inner());
    let attendu = t
        .debut
        .map(|d| d + duree(t.file.len(), t.frequence))
        .filter(|_| t.frequence == frequence);
    let suite =
        attendu.is_some_and(|a| maintenant.max(a).duration_since(maintenant.min(a)) < RECALAGE);
    if !suite {
        t.file.clear();
        t.frequence = frequence;
        t.debut = Some(maintenant);
    } else if let (Some(debut), Some(a)) = (t.debut, attendu) {
        let ecart = if maintenant >= a {
            maintenant.duration_since(a).as_secs_f64()
        } else {
            -a.duration_since(maintenant).as_secs_f64()
        } * SUIVI;
        t.debut = Some(if ecart >= 0.0 {
            debut + Duration::from_secs_f64(ecart)
        } else {
            debut - Duration::from_secs_f64(-ecart)
        });
    }
    t.file.extend(echantillons);
    let trop = t
        .file
        .len()
        .saturating_sub((MAX_S * frequence as f64) as usize);
    if trop > 0 {
        t.file.drain(..trop);
        t.debut = t.debut.map(|d| d + duree(trop, frequence));
    }
}

/// Écoute : `n` échantillons à `vers` Hz, ceux joués à partir de `instant`.
/// Silence pour ce qui n'a pas été joué (flux fermé, ou trop ancien). Tout
/// ce qui précède est oublié.
pub fn prendre(instant: Instant, n: usize, vers: u32) -> Vec<f32> {
    let (brut, frequence, avance) = {
        let mut t = TAMPON.lock().unwrap_or_else(|e| e.into_inner());
        let (Some(debut), f) = (t.debut, t.frequence) else {
            return vec![0.0; n];
        };
        let m = (n as u64 * f as u64).div_ceil(vers as u64) as usize;
        // Position de `instant` dans la file, en échantillons (négative :
        // avant le premier gardé).
        let pos = if instant >= debut {
            (instant.duration_since(debut).as_secs_f64() * f as f64) as i64
        } else {
            -((debut.duration_since(instant).as_secs_f64() * f as f64) as i64)
        };
        let de = pos.max(0) as usize;
        let a = (pos + m as i64).max(0) as usize;
        let len = t.file.len();
        let pris: Vec<f32> = t.file.range(de.min(len)..a.min(len)).copied().collect();
        let oublie = a.min(len);
        t.file.drain(..oublie);
        t.debut = Some(debut + duree(oublie, f));
        // Silence devant ce qui précède le premier échantillon gardé.
        let avance = (-pos).clamp(0, m as i64) as usize;
        (pris, f, avance)
    };
    let mut joue = vec![0.0; avance];
    joue.extend(brut);
    let mut x = crate::sortie::reechantillonner(&joue, frequence, vers);
    x.resize(n, 0.0);
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un seul test touche le tampon global : pas de course entre tests.
    #[test]
    fn repris_a_l_instant_ou_il_a_ete_joue() {
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // 30 ms de silence, puis 20 ms de signal, à 48 kHz, en deux rappels.
        deposer(&[0.0; 1440], 48_000, ms(0));
        deposer(&[0.5; 960], 48_000, ms(30));
        // Le bloc de 10 ms qui commence à 25 ms : moitié silence, moitié signal.
        let x = prendre(ms(25), 160, 16_000);
        assert_eq!(x.len(), 160);
        assert!(x[..70].iter().all(|v| v.abs() < 1e-6), "{:?}", &x[..80]);
        assert!(x[90..].iter().all(|v| (v - 0.5).abs() < 1e-6));
        // Ce qui a été joué entre 35 et 45 ms : le signal, sans rien sauter.
        assert!(prendre(ms(35), 160, 16_000)
            .iter()
            .all(|v| (v - 0.5).abs() < 1e-6));
        // Après la fin de ce qui a été joué : du silence.
        assert!(prendre(ms(60), 160, 16_000).iter().all(|v| *v == 0.0));
        // Un trou dans la chronologie (flux rouvert) : on repart de son instant.
        deposer(&[0.25; 480], 48_000, ms(1000));
        let y = prendre(ms(1000), 160, 16_000);
        assert!(y.iter().all(|v| (v - 0.25).abs() < 1e-6));
    }
}

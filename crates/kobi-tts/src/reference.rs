//! Ce que le haut-parleur joue, au moment où il le joue (KOB-248) : la
//! référence de l'annulation d'écho de l'écoute.
//!
//! Le fil audio y dépose ce qu'il envoie à la carte son, silences compris :
//! la chronologie reste continue. L'écoute le reprend en 16 kHz, au rythme du
//! micro. Le tampon est borné : sans lecteur, il ne grossit pas, et une dérive
//! entre les horloges de la carte son et du micro reste petite (l'annulation
//! d'écho estime elle-même le retard).

use std::collections::VecDeque;
use std::sync::Mutex;

/// Au-delà, le plus ancien part.
const MAX_S: f64 = 0.3;

struct Tampon {
    frequence: u32,
    file: VecDeque<f32>,
}

static TAMPON: Mutex<Tampon> = Mutex::new(Tampon {
    frequence: 0,
    file: VecDeque::new(),
});

/// Fil audio : échantillons mono joués à `frequence` Hz. Jamais bloquant : si
/// l'écoute tient le verrou à cet instant, ce bloc est perdu.
pub fn deposer(echantillons: &[f32], frequence: u32) {
    let Ok(mut t) = TAMPON.try_lock() else {
        return;
    };
    if t.frequence != frequence {
        t.file.clear();
        t.frequence = frequence;
    }
    t.file.extend(echantillons);
    let max = (MAX_S * frequence as f64) as usize;
    let trop = t.file.len().saturating_sub(max);
    t.file.drain(..trop);
}

/// Écoute : `n` échantillons à `vers` Hz, complétés de silence si le
/// haut-parleur n'en a pas joué autant (flux fermé, ou en avance du micro).
pub fn prendre(n: usize, vers: u32) -> Vec<f32> {
    let (brut, frequence) = {
        let mut t = TAMPON.lock().unwrap_or_else(|e| e.into_inner());
        if t.frequence == 0 {
            return vec![0.0; n];
        }
        let m = (n as u64 * t.frequence as u64).div_ceil(vers as u64) as usize;
        let k = m.min(t.file.len());
        (t.file.drain(..k).collect::<Vec<_>>(), t.frequence)
    };
    let mut x = crate::sortie::reechantillonner(&brut, frequence, vers);
    x.resize(n, 0.0);
    x
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un seul test touche le tampon global : pas de course entre tests.
    #[test]
    fn depose_puis_repris_en_16_khz_borne_et_complete() {
        // 30 ms à 48 kHz : rien, puis un signal.
        deposer(&[0.0; 480], 48_000);
        deposer(&[0.5; 960], 48_000);
        let x = prendre(480, 16_000);
        assert_eq!(x.len(), 480);
        assert!(x[..150].iter().all(|v| v.abs() < 1e-6), "silence d'abord");
        assert!(
            x[170..320].iter().all(|v| (v - 0.5).abs() < 1e-6),
            "puis le signal"
        );
        // Plus rien de joué : du silence.
        assert!(prendre(160, 16_000).iter().all(|v| *v == 0.0));
        // Sans lecteur, le tampon reste borné à MAX_S.
        for _ in 0..100 {
            deposer(&[0.25; 4800], 48_000);
        }
        let tout = prendre(16_000, 16_000);
        let pleins = tout.iter().filter(|v| **v != 0.0).count();
        assert!(pleins <= (MAX_S * 16_000.0) as usize + 1, "{pleins}");
    }
}

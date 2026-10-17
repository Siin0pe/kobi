//! Porte d'énergie de l'écoute continue (KOB-132) : Nemotron ne reçoit que
//! la parole, pour que le silence ne coûte presque rien.
//!
//! Le plancher de bruit suit le niveau le plus bas récent ; une trame
//! nettement au-dessus ouvre la porte. À l'ouverture, les dernières
//! [`PREROULAGE`] secondes sont rendues pour ne pas couper le début du mot
//! (« Ko… ») ; la porte reste ouverte [`TRAINE`] secondes après la dernière
//! trame forte, puis [`VIDANGE`] secondes de silence numérique sont ajoutées
//! pour que Nemotron (blocs de 560 ms et contexte droit) conclue la phrase.

use std::collections::VecDeque;

/// Audio rendu à l'ouverture, en secondes.
pub const PREROULAGE: f64 = 0.5;
/// Silence transmis après la parole, en secondes.
pub const TRAINE: f64 = 1.5;
/// Silence numérique ajouté à la fermeture, en secondes.
pub const VIDANGE: f64 = 1.0;
/// Écart au plancher de bruit qui ouvre la porte (≈ +10 dB).
const RAPPORT: f32 = 3.0;
/// En dessous de ce niveau (−50 dBFS), rien n'ouvre la porte.
const NIVEAU_MIN: f32 = 0.003;
/// Remontée du plancher par seconde quand le bruit augmente (×2 en ~7 s).
const REMONTEE: f32 = 1.1;

/// Niveau RMS d'une trame.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|x| x * x).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Voir le module.
#[derive(Debug)]
pub struct Porte {
    plancher: f32,
    /// Fin de la dernière trame forte, en secondes.
    derniere_voix: Option<f64>,
    ouverte: bool,
    /// Trames gardées pour le pré-roulage : (début, échantillons).
    tampon: VecDeque<(f64, Vec<f32>)>,
    taux: f64,
}

impl Porte {
    pub fn new(taux: u32) -> Self {
        Self {
            plancher: NIVEAU_MIN,
            derniere_voix: None,
            ouverte: false,
            tampon: VecDeque::new(),
            taux: f64::from(taux),
        }
    }

    pub fn ouverte(&self) -> bool {
        self.ouverte
    }

    /// Une trame commençant à `t` ; renvoie les trames à transcrire.
    pub fn trame(&mut self, t: f64, samples: Vec<f32>) -> Vec<(f64, Vec<f32>)> {
        let duree = samples.len() as f64 / self.taux;
        let fin = t + duree;
        let niveau = rms(&samples);
        // Plancher : suit le minimum, remonte lentement si le bruit monte.
        if niveau < self.plancher {
            self.plancher = niveau.max(NIVEAU_MIN / 4.0);
        } else {
            self.plancher *= REMONTEE.powf(duree as f32);
        }
        let forte = niveau > NIVEAU_MIN && niveau > self.plancher * RAPPORT;
        if forte {
            self.derniere_voix = Some(fin);
        }
        let voix_recente = self.derniere_voix.is_some_and(|v| fin - v < TRAINE);

        if self.ouverte {
            self.ouverte = voix_recente;
            let mut sortie = vec![(t, samples)];
            if !self.ouverte {
                sortie.push((fin, vec![0.0; (VIDANGE * self.taux) as usize]));
            }
            return sortie;
        }
        if forte {
            self.ouverte = true;
            let mut sortie: Vec<_> = self.tampon.drain(..).collect();
            sortie.push((t, samples));
            return sortie;
        }
        self.tampon.push_back((t, samples));
        while self
            .tampon
            .front()
            .is_some_and(|(debut, _)| fin - debut > PREROULAGE)
        {
            self.tampon.pop_front();
        }
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAUX: u32 = 16_000;

    /// Trame de 10 ms d'amplitude `a` (sinusoïde, RMS ≈ a / √2).
    fn trame(a: f32) -> Vec<f32> {
        (0..160).map(|i| a * (i as f32 * 0.3).sin()).collect()
    }

    fn rejouer(porte: &mut Porte, niveaux: &[(f32, usize)]) -> Vec<f64> {
        let mut t = 0.0;
        let mut transmises = Vec::new();
        for &(a, n) in niveaux {
            for _ in 0..n {
                transmises.extend(porte.trame(t, trame(a)).into_iter().map(|(d, _)| d));
                t += 0.01;
            }
        }
        transmises
    }

    #[test]
    fn le_silence_ne_passe_pas() {
        let mut p = Porte::new(TAUX);
        assert!(rejouer(&mut p, &[(0.0, 500), (0.001, 500)]).is_empty());
        assert!(!p.ouverte());
    }

    #[test]
    fn parole_avec_preroulage_et_traine() {
        let mut p = Porte::new(TAUX);
        // 2 s de bruit faible, 1 s de parole, 3 s de silence.
        let t = rejouer(&mut p, &[(0.002, 200), (0.2, 100), (0.002, 300)]);
        let premiere = t[0];
        // La dernière sortie est la vidange, qui suit la dernière trame.
        let (vidange, t) = t.split_last().unwrap();
        let derniere = *t.last().unwrap();
        assert!((premiere - (2.0 - PREROULAGE)).abs() < 0.02, "{premiere}");
        assert!((derniere - (3.0 + TRAINE)).abs() < 0.03, "{derniere}");
        assert!((vidange - (derniere + 0.01)).abs() < 1e-6);
        assert!(!p.ouverte());
        // Sans trou entre le début et la fin.
        assert_eq!(t.len(), ((derniere - premiere) / 0.01).round() as usize + 1);
    }

    #[test]
    fn bruit_de_fond_constant_ignore() {
        let mut p = Porte::new(TAUX);
        // Ventilateur à −30 dBFS : la porte s'ouvre au début, puis le
        // plancher s'y adapte et elle se referme.
        let t = rejouer(&mut p, &[(0.03, 6000)]);
        assert!(!p.ouverte());
        assert!(t.len() < 3000, "{} trames transmises", t.len());
        // Une voix au-dessus du ventilateur rouvre la porte.
        assert!(!rejouer(&mut p, &[(0.5, 20)]).is_empty());
    }
}

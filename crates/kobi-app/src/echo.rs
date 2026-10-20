//! Annulation de l'écho de la voix de Kobi dans le micro (KOB-248).
//!
//! Sur des haut-parleurs, le micro entend Kobi. Transcrite, sa voix le
//! coupait, se mêlait aux questions posées pendant qu'il parle et faisait
//! perdre « stop ». L'AEC3 de WebRTC (crate `aec3`) la retire, avec pour
//! référence ce que joue le haut-parleur ([`kobi_tts::reference`]) ; il estime
//! lui-même le retard entre les deux.
//!
//! Il ne tourne que pendant la voix de Kobi et sa traîne : au repos, le micro
//! passe tel quel, sans calcul. Le réglage `audio.annulation_echo` est relu à
//! chaque nouvelle voix.

use std::time::{Duration, Instant};

use aec3::nodes::audio::AudioFormat;
use aec3::pipelines::linear::{self, LinearPipeline};

const FREQUENCE: u32 = kobi_stt::SAMPLE_RATE;
/// Blocs de 10 ms, ceux de l'AEC3.
const BLOC: usize = (FREQUENCE / 100) as usize;
/// L'écho continue un peu après la dernière syllabe (pièce, tampons audio).
const TRAINE: Duration = Duration::from_secs(2);
/// Amplitude au-dessus de laquelle la référence porte la voix de Kobi.
const SEUIL: f32 = 1e-4;

pub struct AnnulationEcho {
    aec: Option<LinearPipeline>,
    /// Micro reçu, pas encore traité (moins d'un bloc).
    reste: Vec<f32>,
    /// Dernier bloc de référence où Kobi parlait.
    voix: Option<Instant>,
    /// Réglage lu au début de la voix en cours.
    active: bool,
    /// AEC3 impossible à construire : le micro passe tel quel.
    hors_service: bool,
    /// Temps passé dans l'AEC3, pour le bilan de charge.
    pub calcul: Duration,
}

impl Default for AnnulationEcho {
    fn default() -> Self {
        Self {
            aec: None,
            reste: Vec::new(),
            voix: None,
            active: false,
            hors_service: false,
            calcul: Duration::ZERO,
        }
    }
}

impl AnnulationEcho {
    /// Micro 16 kHz mono : le même nombre d'échantillons, sans la voix de
    /// Kobi (au plus un bloc de 10 ms passe à la trame suivante).
    /// `reglage` n'est lu qu'au début d'une voix.
    pub fn traiter(
        &mut self,
        micro: &[f32],
        maintenant: Instant,
        reglage: impl Fn() -> bool,
    ) -> Vec<f32> {
        let mut reste = std::mem::take(&mut self.reste);
        reste.extend_from_slice(micro);
        let blocs = reste.len() / BLOC;
        let mut sortie = Vec::with_capacity(blocs * BLOC);
        for i in 0..blocs {
            let bloc = &reste[i * BLOC..(i + 1) * BLOC];
            // La référence est toujours reprise, même au repos : elle reste
            // alignée sur le micro.
            let reference = kobi_tts::reference::prendre(BLOC, FREQUENCE);
            if reference.iter().any(|x| x.abs() > SEUIL) {
                if self.voix.is_none() {
                    self.active = reglage();
                }
                self.voix = Some(maintenant);
            }
            let en_voix = self
                .voix
                .is_some_and(|v| maintenant.duration_since(v) < TRAINE);
            if !en_voix {
                self.voix = None;
            }
            if !(en_voix && self.active && !self.hors_service) {
                sortie.extend_from_slice(bloc);
                continue;
            }
            let debut = Instant::now();
            let propre = self.annuler(&reference, bloc);
            self.calcul += debut.elapsed();
            sortie.extend_from_slice(propre.as_deref().unwrap_or(bloc));
        }
        reste.drain(..blocs * BLOC);
        self.reste = reste;
        sortie
    }

    fn annuler(&mut self, reference: &[f32], micro: &[f32]) -> Option<Vec<f32>> {
        if self.aec.is_none() {
            let format = AudioFormat::ten_ms(FREQUENCE, 1);
            // L'écho seul : ni réduction de bruit ni gain automatique, la
            // porte et la transcription gardent le niveau du micro.
            match linear::builder(format, format)
                .enable_noise_suppression(false)
                .enable_gain_controller2(false)
                .enable_post_filter(false)
                .build()
            {
                Ok(aec) => self.aec = Some(aec),
                Err(e) => {
                    tracing::warn!(erreur = %e, "annulation d'écho indisponible");
                    self.hors_service = true;
                    return None;
                }
            }
        }
        let aec = self.aec.as_mut()?;
        let mut propre = vec![0.0; BLOC];
        let fait = aec
            .handle_render_frame(reference)
            .and_then(|()| aec.process_capture_frame(micro, &mut propre));
        match fait {
            Ok(true) => Some(propre),
            Ok(false) => None,
            Err(e) => {
                tracing::warn!(erreur = %e, "annulation d'écho");
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sans voix de Kobi (référence muette), le micro passe tel quel, par
    /// blocs entiers ; le reste attend la trame suivante.
    #[test]
    fn au_repos_le_micro_passe_tel_quel() {
        let mut e = AnnulationEcho::default();
        let micro: Vec<f32> = (0..250).map(|i| (i as f32 * 0.01).sin()).collect();
        let lu = std::cell::Cell::new(0);
        let reglage = || {
            lu.set(lu.get() + 1);
            true
        };
        let a = e.traiter(&micro, Instant::now(), reglage);
        assert_eq!(a, micro[..160]);
        let b = e.traiter(&micro[..70], Instant::now(), reglage);
        assert_eq!(b[..90], micro[160..]);
        assert_eq!(b[90..], micro[..70]);
        assert!(e.aec.is_none(), "aucun calcul au repos");
        assert_eq!(lu.get(), 0, "réglage lu seulement quand Kobi parle");
    }

    /// Bruit pseudo-aléatoire, de spectre large comme une voix.
    fn bruit(n: usize) -> Vec<f32> {
        let mut x = 12345u32;
        (0..n)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
                ((x >> 16) as f32 / 32768.0 - 1.0) * 0.5
            })
            .collect()
    }

    fn energie(x: &[f32]) -> f64 {
        x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum()
    }

    /// Le micro entend la voix de Kobi atténuée et retardée de 80 ms : après
    /// convergence, l'écho perd plus de 10 dB. Réglage coupé : rien n'est
    /// retiré. Un seul test touche la référence globale.
    #[test]
    fn la_voix_de_kobi_est_retiree_du_micro() {
        let n = 6 * FREQUENCE as usize;
        let voix = bruit(n);
        let retard = 80 * FREQUENCE as usize / 1000;
        let micro: Vec<f32> = (0..n)
            .map(|i| {
                if i >= retard {
                    0.3 * voix[i - retard]
                } else {
                    0.0
                }
            })
            .collect();
        let jouer = |active: bool| {
            let mut e = AnnulationEcho::default();
            let mut sortie = Vec::new();
            for k in 0..n / BLOC {
                let r = k * BLOC..(k + 1) * BLOC;
                kobi_tts::reference::deposer(&voix[r.clone()], FREQUENCE);
                sortie.extend(e.traiter(&micro[r], Instant::now(), || active));
            }
            sortie
        };
        let fin = 4 * FREQUENCE as usize..n;
        let avant = energie(&micro[fin.clone()]);
        let apres = energie(&jouer(true)[fin.clone()]);
        let gain = 10.0 * (avant / apres.max(1e-12)).log10();
        assert!(gain > 10.0, "écho atténué de {gain:.1} dB seulement");
        let coupe = jouer(false);
        assert_eq!(coupe[fin.clone()], micro[fin]);
    }
}

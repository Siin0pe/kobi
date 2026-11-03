//! Annulation de l'écho de la voix de Kobi dans le micro (KOB-248).
//!
//! Sur des haut-parleurs, le micro entend Kobi. Transcrite, sa voix le
//! coupait, se mêlait aux questions posées pendant qu'il parle et faisait
//! perdre « stop ». L'AEC3 de WebRTC (crate `aec3`) la retire, avec pour
//! référence ce que joue le haut-parleur ([`kobi_tts::reference`]) ; il estime
//! lui-même le retard entre les deux.
//!
//! Il ne tourne que pendant la voix de Kobi et sa traîne : au repos, ou
//! réglage coupé, le micro passe tel quel, trame pour trame, sans recoupe ni
//! calcul. Le réglage `audio.annulation_echo` est relu toutes les
//! [`RELECTURE`].

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
/// Le réglage est lu sur le disque : au plus une fois par intervalle.
const RELECTURE: Duration = Duration::from_secs(2);

pub struct AnnulationEcho {
    aec: Option<LinearPipeline>,
    /// Micro reçu, pas encore traité (moins d'un bloc).
    reste: Vec<f32>,
    /// Dernier bloc de référence où Kobi parlait.
    voix: Option<Instant>,
    /// Réglage, et quand il a été lu.
    active: bool,
    lu: Option<Instant>,
    /// AEC3 impossible à construire : le micro passe tel quel.
    hors_service: bool,
    /// Temps passé dans l'AEC3, pour le bilan de charge.
    pub calcul: Duration,
    /// Diagnostic (`KOBI_ECHO_BRUT=<dossier>`) : référence, micro et sortie
    /// de chaque bloc, en f32 bruts, pour mesurer l'alignement hors ligne.
    brut: Option<Vec<std::io::BufWriter<std::fs::File>>>,
}

fn ouvrir_brut() -> Option<Vec<std::io::BufWriter<std::fs::File>>> {
    let dossier = std::path::PathBuf::from(std::env::var_os("KOBI_ECHO_BRUT")?);
    let _ = std::fs::create_dir_all(&dossier);
    ["reference.f32", "micro.f32", "sortie.f32"]
        .iter()
        .map(|nom| {
            std::fs::File::create(dossier.join(nom))
                .ok()
                .map(std::io::BufWriter::new)
        })
        .collect()
}

impl Default for AnnulationEcho {
    fn default() -> Self {
        Self {
            aec: None,
            reste: Vec::new(),
            voix: None,
            active: false,
            lu: None,
            hors_service: false,
            calcul: Duration::ZERO,
            brut: ouvrir_brut(),
        }
    }
}

impl AnnulationEcho {
    /// Micro 16 kHz mono capté à partir de `capte`, sans la voix de Kobi.
    /// Réglage coupé, ou au repos (référence muette, hors traîne) : la trame
    /// telle quelle, jamais recoupée ni vide. Pendant la voix : par blocs de
    /// 10 ms, le reste (moins d'un bloc) passe à la trame suivante, la
    /// sortie peut alors être vide. `reglage` est relu au plus toutes les
    /// [`RELECTURE`].
    pub fn traiter(
        &mut self,
        micro: &[f32],
        capte: Instant,
        reglage: impl Fn() -> bool,
    ) -> Vec<f32> {
        let maintenant = Instant::now();
        if self
            .lu
            .is_none_or(|lu| maintenant.duration_since(lu) >= RELECTURE)
        {
            self.active = reglage();
            self.lu = Some(maintenant);
        }
        if !self.active || self.hors_service {
            // Un reste d'avant la coupure passe devant, rien ne se perd.
            self.voix = None;
            let mut sortie = std::mem::take(&mut self.reste);
            sortie.extend_from_slice(micro);
            return sortie;
        }
        // Le reste de la trame précédente a été capté juste avant celle-ci.
        let debut = capte
            .checked_sub(Duration::from_secs_f64(
                self.reste.len() as f64 / FREQUENCE as f64,
            ))
            .unwrap_or(capte);
        let blocs = (self.reste.len() + micro.len()) / BLOC;
        // La référence ne se reprend qu'une fois (`prendre` oublie ce qu'il
        // rend) : celle du reste et de la trame au repos, celle des blocs
        // pendant la voix.
        let au_repos = !self.en_voix(maintenant);
        let n = if au_repos {
            self.reste.len() + micro.len()
        } else {
            blocs * BLOC
        };
        let reference = kobi_tts::reference::prendre(debut, n, FREQUENCE);
        if au_repos && reference.iter().all(|x| x.abs() <= SEUIL) {
            // Le reste laissé par la fin d'une voix passe devant : le flux
            // revient trame pour trame.
            self.voix = None;
            let mut sortie = std::mem::take(&mut self.reste);
            sortie.extend_from_slice(micro);
            return sortie;
        }
        let mut reste = std::mem::take(&mut self.reste);
        reste.extend_from_slice(micro);
        let mut sortie = Vec::with_capacity(blocs * BLOC);
        for i in 0..blocs {
            let bloc = &reste[i * BLOC..(i + 1) * BLOC];
            let reference = &reference[i * BLOC..(i + 1) * BLOC];
            if reference.iter().any(|x| x.abs() > SEUIL) {
                self.voix = Some(maintenant);
            }
            if !self.en_voix(maintenant) {
                self.voix = None;
            }
            let propre = if self.voix.is_some() {
                let chrono = Instant::now();
                let propre = self.annuler(reference, bloc);
                self.calcul += chrono.elapsed();
                propre
            } else {
                None
            };
            let sorti = propre.as_deref().unwrap_or(bloc);
            if let Some(f) = &mut self.brut {
                use std::io::Write;
                for (f, x) in f.iter_mut().zip([reference, bloc, sorti]) {
                    let octets: Vec<u8> = x.iter().flat_map(|v| v.to_le_bytes()).collect();
                    let _ = f.write_all(&octets);
                }
            }
            sortie.extend_from_slice(sorti);
        }
        reste.drain(..blocs * BLOC);
        self.reste = reste;
        sortie
    }

    fn en_voix(&self, maintenant: Instant) -> bool {
        self.voix
            .is_some_and(|v| maintenant.duration_since(v) < TRAINE)
    }

    /// Micro ouvert, fermé ou perdu : le reste en attente et la voix en cours
    /// ne valent plus pour le flux suivant (KOB-248).
    pub fn reinitialiser(&mut self) {
        self.reste.clear();
        self.voix = None;
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

    /// Réglage coupé, ou au repos (référence muette) : chaque trame passe
    /// telle quelle, même de moins de 10 ms, jamais vide ni recoupée ; le
    /// réglage est relu au plus toutes les 2 s.
    #[test]
    fn coupe_ou_au_repos_le_micro_passe_tel_quel() {
        // Une heure plus tard : rien n'y a été joué, la référence globale y
        // est muette, quoi que joue l'autre test en même temps.
        let plus_tard = Instant::now() + Duration::from_secs(3600);
        for active in [false, true] {
            let mut e = AnnulationEcho::default();
            let lu = std::cell::Cell::new(0);
            let reglage = || {
                lu.set(lu.get() + 1);
                active
            };
            for (k, n) in [85usize, 85, 250, 70].into_iter().enumerate() {
                let micro: Vec<f32> = (0..n).map(|i| ((i + k) as f32 * 0.01).sin()).collect();
                let sorti = e.traiter(
                    &micro,
                    plus_tard + Duration::from_millis(20 * k as u64),
                    reglage,
                );
                assert_eq!(sorti, micro, "réglage {active}, trame {k}");
            }
            assert!(e.aec.is_none(), "aucun calcul");
            assert_eq!(lu.get(), 1, "réglage relu au plus toutes les 2 s");
        }
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
    /// retiré. Seul test qui joue dans la référence globale.
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
        // Le haut-parleur joue par blocs de 30 ms, le micro livre par 20 ms :
        // des à-coups différents, sur une même horloge.
        let t0 = Instant::now();
        let a = |s: usize| t0 + Duration::from_secs_f64(s as f64 / FREQUENCE as f64);
        let jouer = |active: bool| {
            let mut e = AnnulationEcho::default();
            let mut sortie = Vec::new();
            let (mut joue, mut capte) = (0, 0);
            while capte + 320 <= n {
                while joue < (capte + 320 + 480).min(n) {
                    let fin = (joue + 480).min(n);
                    kobi_tts::reference::deposer(&voix[joue..fin], FREQUENCE, a(joue));
                    joue = fin;
                }
                sortie.extend(e.traiter(&micro[capte..capte + 320], a(capte), || active));
                capte += 320;
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

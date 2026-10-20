//! Où va la voix : le haut-parleur, ou un fichier WAV (essais sans carte son).

use std::path::Path;
use std::time::{Duration, Instant};

use crate::{Arret, Erreur, Resultat};

/// Joue des échantillons mono et rend la main quand ils sont joués, ou dès
/// que `arret` est levé.
pub trait Sortie {
    fn jouer(&mut self, echantillons: &[f32], frequence: u32, arret: &Arret) -> Resultat<()>;
}

/// Pas de temps pour suivre la lecture et l'arrêt.
const PAS: Duration = Duration::from_millis(20);
/// Marge au-delà de la durée du son : un flux qui ne consomme plus (casque
/// débranché) rend la main au lieu de bloquer la voix (KOB-235).
#[cfg(feature = "lecture")]
const MARGE: Duration = Duration::from_secs(2);

/// Attend que la carte son ait tout joué (`file` vide), l'arrêt, une panne
/// du flux ou l'échéance. Arrêt : la file est vidée, sans erreur. Panne ou
/// échéance : la file est vidée et la voix est en erreur.
#[cfg(any(feature = "lecture", test))]
pub(crate) fn attendre_lecture(
    file: &std::sync::Mutex<std::collections::VecDeque<f32>>,
    arret: &Arret,
    panne: &std::sync::atomic::AtomicBool,
    echeance: Instant,
) -> Resultat<()> {
    use std::sync::atomic::Ordering;
    loop {
        let vider = || file.lock().unwrap_or_else(|e| e.into_inner()).clear();
        if arret.arrete() {
            vider();
            return Ok(());
        }
        if file.lock().unwrap_or_else(|e| e.into_inner()).is_empty() {
            return Ok(());
        }
        if panne.load(Ordering::SeqCst) {
            vider();
            return Err(Erreur::Sortie("la sortie audio s'est arrêtée".into()));
        }
        if Instant::now() >= echeance {
            vider();
            return Err(Erreur::Sortie(
                "la sortie audio ne joue plus (débranchée ?)".into(),
            ));
        }
        std::thread::sleep(PAS);
    }
}

/// Écrit la voix dans un WAV mono 16 bits, au rythme où elle serait jouée
/// (l'interruption coupe le fichier au même endroit qu'un haut-parleur).
pub struct Fichier {
    wav: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    chemin: std::path::PathBuf,
    /// Temps réel (`true`) ou d'un coup (tests).
    rythme: bool,
}

impl Fichier {
    pub fn new(chemin: &Path, rythme: bool) -> Self {
        Self {
            wav: None,
            chemin: chemin.to_path_buf(),
            rythme,
        }
    }
}

impl Sortie for Fichier {
    fn jouer(&mut self, echantillons: &[f32], frequence: u32, arret: &Arret) -> Resultat<()> {
        if self.wav.is_none() {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: frequence,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let wav = hound::WavWriter::create(&self.chemin, spec)
                .map_err(|e| Erreur::Sortie(format!("{} : {e}", self.chemin.display())))?;
            self.wav = Some(wav);
        }
        let wav = self.wav.as_mut().expect("ouvert ci-dessus");
        let pas = (frequence as f64 * PAS.as_secs_f64()) as usize;
        let debut = Instant::now();
        for (k, bloc) in echantillons.chunks(pas.max(1)).enumerate() {
            if arret.arrete() {
                break;
            }
            for &x in bloc {
                let v = (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
                wav.write_sample(v)
                    .map_err(|e| Erreur::Sortie(e.to_string()))?;
            }
            if self.rythme {
                if let Some(attente) = (PAS * (k as u32 + 1)).checked_sub(debut.elapsed()) {
                    std::thread::sleep(attente);
                }
            }
        }
        wav.flush().map_err(|e| Erreur::Sortie(e.to_string()))
    }
}

impl Drop for Fichier {
    fn drop(&mut self) {
        if let Some(wav) = self.wav.take() {
            let _ = wav.finalize();
        }
    }
}

#[cfg(feature = "lecture")]
pub use haut_parleur::HautParleur;

#[cfg(feature = "lecture")]
mod haut_parleur {
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    use super::*;

    /// Sortie audio par défaut (ou `nom`, exact ou extrait), par cpal. Le flux reste ouvert ;
    /// les échantillons sont rééchantillonnés et copiés sur chaque canal.
    pub struct HautParleur {
        _flux: cpal::Stream,
        file: Arc<Mutex<VecDeque<f32>>>,
        /// Levé par le flux en erreur (sortie débranchée) : plus rien ne se
        /// joue, la voix rend la main tout de suite (KOB-235).
        panne: Arc<std::sync::atomic::AtomicBool>,
        frequence: u32,
        canaux: usize,
        pub description: String,
    }

    impl HautParleur {
        pub fn ouvrir(nom: Option<&str>) -> Resultat<Self> {
            let host = cpal::default_host();
            let device = match nom {
                Some(nom) => {
                    let devices: Vec<cpal::Device> = host
                        .output_devices()
                        .map_err(|e| Erreur::Sortie(e.to_string()))?
                        .collect();
                    let noms: Vec<String> = devices.iter().map(nom_de).collect();
                    // Nom exact, sinon un extrait (« PulseAudio »).
                    let extrait = nom.to_lowercase();
                    let i = noms
                        .iter()
                        .position(|n| n == nom)
                        .or_else(|| {
                            noms.iter()
                                .position(|n| n.to_lowercase().contains(&extrait))
                        })
                        .ok_or_else(|| {
                            Erreur::Sortie(format!(
                                "sortie « {nom} » introuvable ; disponibles : {}",
                                noms.join(", ")
                            ))
                        })?;
                    devices.into_iter().nth(i).expect("indice trouvé ci-dessus")
                }
                None => host
                    .default_output_device()
                    .ok_or_else(|| Erreur::Sortie("aucune sortie audio".into()))?,
            };
            let config = device
                .default_output_config()
                .map_err(|e| Erreur::Sortie(e.to_string()))?;
            let frequence = config.sample_rate();
            let canaux = config.channels() as usize;
            let description = format!("{} ({} Hz, {} canaux)", nom_de(&device), frequence, canaux);
            let file = Arc::new(Mutex::new(VecDeque::<f32>::new()));
            let panne = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let flux = match config.sample_format() {
                cpal::SampleFormat::F32 => construire::<f32>(&device, &config, &file, &panne),
                cpal::SampleFormat::I16 => construire::<i16>(&device, &config, &file, &panne),
                cpal::SampleFormat::U16 => construire::<u16>(&device, &config, &file, &panne),
                cpal::SampleFormat::I32 => construire::<i32>(&device, &config, &file, &panne),
                autre => Err(Erreur::Sortie(format!("format {autre:?} non géré"))),
            }?;
            flux.play().map_err(|e| Erreur::Sortie(e.to_string()))?;
            Ok(Self {
                _flux: flux,
                file,
                panne,
                frequence,
                canaux,
                description,
            })
        }
    }

    /// Nom d'une sortie, tel que `KOBI_VOIX_SORTIE` l'attend.
    fn nom_de(device: &cpal::Device) -> String {
        device
            .description()
            .map(|d| d.name().to_string())
            .unwrap_or_else(|_| "?".into())
    }

    fn construire<T>(
        device: &cpal::Device,
        config: &cpal::SupportedStreamConfig,
        file: &Arc<Mutex<VecDeque<f32>>>,
        panne: &Arc<std::sync::atomic::AtomicBool>,
    ) -> Resultat<cpal::Stream>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let lue = Arc::clone(file);
        let panne = Arc::clone(panne);
        device
            .build_output_stream::<T, _, _>(
                config.config(),
                move |sortie: &mut [T], _| {
                    let mut file = lue.lock().unwrap_or_else(|e| e.into_inner());
                    for x in sortie.iter_mut() {
                        *x = T::from_sample(file.pop_front().unwrap_or(0.0));
                    }
                },
                move |e| {
                    tracing::warn!(erreur = %e, "sortie audio");
                    // Sortie disparue (casque débranché) : plus rien ne se joue.
                    if e.kind() == cpal::ErrorKind::DeviceNotAvailable {
                        panne.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                },
                None,
            )
            .map_err(|e| Erreur::Sortie(e.to_string()))
    }

    impl Sortie for HautParleur {
        fn jouer(&mut self, echantillons: &[f32], frequence: u32, arret: &Arret) -> Resultat<()> {
            if self.panne.load(std::sync::atomic::Ordering::SeqCst) {
                return Err(Erreur::Sortie("la sortie audio s'est arrêtée".into()));
            }
            let son = super::reechantillonner(echantillons, frequence, self.frequence);
            let echeance = {
                let mut file = self.file.lock().unwrap_or_else(|e| e.into_inner());
                for x in son {
                    for _ in 0..self.canaux {
                        file.push_back(x);
                    }
                }
                // Tout ce qui est en file, plus une marge (KOB-235).
                let en_file = file.len() / self.canaux.max(1);
                Instant::now()
                    + Duration::from_secs_f64(en_file as f64 / self.frequence as f64)
                    + MARGE
            };
            attendre_lecture(&self.file, arret, &self.panne, echeance)
        }
    }
}

/// Rééchantillonnage linéaire, suffisant pour la voix.
pub fn reechantillonner(x: &[f32], de: u32, vers: u32) -> Vec<f32> {
    if de == vers || x.is_empty() {
        return x.to_vec();
    }
    let n = (x.len() as u64 * vers as u64 / de as u64) as usize;
    let pas = de as f64 / vers as f64;
    (0..n)
        .map(|i| {
            let p = i as f64 * pas;
            let k = p as usize;
            let f = (p - k as f64) as f32;
            let a = x[k.min(x.len() - 1)];
            let b = x[(k + 1).min(x.len() - 1)];
            a + (b - a) * f
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KOB-235 : une sortie qui ne consomme plus (casque débranché) rend la
    /// main en erreur à l'échéance, la file vidée ; une panne signalée, tout
    /// de suite ; l'arrêt, sans erreur.
    #[test]
    fn la_voix_ne_reste_pas_bloquee_sur_une_sortie_qui_ne_joue_plus() {
        use std::collections::VecDeque;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Mutex;
        let file = Mutex::new(VecDeque::from(vec![0.1f32; 48_000]));
        let panne = AtomicBool::new(false);
        let arret = Arret::default();
        let debut = Instant::now();
        let r = attendre_lecture(&file, &arret, &panne, debut + Duration::from_millis(300));
        assert!(matches!(r, Err(Erreur::Sortie(_))), "{r:?}");
        assert!(debut.elapsed() < Duration::from_secs(2));
        assert!(file.lock().unwrap().is_empty());
        // Panne levée par le flux : en erreur sans attendre l'échéance.
        file.lock().unwrap().extend([0.1; 480]);
        panne.store(true, Ordering::SeqCst);
        let debut = Instant::now();
        let loin = debut + Duration::from_secs(60);
        assert!(attendre_lecture(&file, &arret, &panne, loin).is_err());
        assert!(debut.elapsed() < Duration::from_millis(200));
        // Arrêt demandé : la file est vidée, sans erreur.
        panne.store(false, Ordering::SeqCst);
        file.lock().unwrap().extend([0.1; 480]);
        arret.arreter();
        assert!(attendre_lecture(&file, &arret, &panne, loin).is_ok());
        assert!(file.lock().unwrap().is_empty());
    }

    #[test]
    fn reechantillonnage() {
        let x: Vec<f32> = (0..220).map(|i| i as f32 / 220.0).collect();
        let y = reechantillonner(&x, 22_050, 48_000);
        assert_eq!(y.len(), 478);
        assert!(y.windows(2).all(|w| w[1] >= w[0]), "rampe conservée");
        assert_eq!(reechantillonner(&x, 16_000, 16_000), x);
    }

    #[test]
    fn le_fichier_s_arrete_avec_la_voix() {
        let dir = tempfile::tempdir().unwrap();
        let chemin = dir.path().join("voix.wav");
        let arret = Arret::default();
        {
            let mut f = Fichier::new(&chemin, false);
            f.jouer(&[0.5; 1000], 16_000, &arret).unwrap();
            arret.arreter();
            f.jouer(&[0.5; 1000], 16_000, &arret).unwrap();
        }
        let lu = hound::WavReader::open(&chemin).unwrap();
        assert_eq!(lu.spec().sample_rate, 16_000);
        assert_eq!(lu.len(), 1000, "rien n'est écrit après l'arrêt");
    }
}

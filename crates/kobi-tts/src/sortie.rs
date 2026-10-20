//! Où va la voix : le haut-parleur, ou un fichier WAV (essais sans carte son).

use std::path::Path;
use std::time::{Duration, Instant};

use crate::{Arret, Erreur, Resultat};

/// Joue des échantillons mono et rend la main quand ils sont joués, ou dès
/// que `arret` est levé.
pub trait Sortie {
    fn jouer(&mut self, echantillons: &[f32], frequence: u32, arret: &Arret) -> Resultat<()>;

    /// Appelé régulièrement par le fil de lecture, entre deux sons : une
    /// sortie en panne y lâche son flux mort (KOB-279).
    fn entretenir(&mut self) {}
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

/// Une panne, ce sont des erreurs qui durent : au moins [`ERREURS_PANNE`]
/// sur au moins [`DUREE_PANNE`] d'affilée. Un flux mort en rend des milliers
/// par seconde, sans fin. Un flux vivant en rend une isolée de temps en
/// temps, ou une rafale de quelques dizaines en une milliseconde à son
/// ouverture quand le serveur audio vient de démarrer (PulseAudio,
/// `snd_pcm_avail_delay`) : ni l'une ni l'autre n'est une panne.
#[cfg(any(feature = "lecture", test))]
const ERREURS_PANNE: u32 = 20;
#[cfg(any(feature = "lecture", test))]
const DUREE_PANNE: Duration = Duration::from_millis(250);
/// Sans erreur depuis ce temps, la rafale suivante repart de zéro. Court :
/// une rafale ne dure que si ses erreurs sont serrées (un flux mort en rend
/// une toutes les 0,1 ms). Une rafale d'ouverture suivie d'une erreur isolée,
/// ou des erreurs isolées espacées, ne s'additionnent pas (KOB-296).
#[cfg(any(feature = "lecture", test))]
const FIN_RAFALE: Duration = Duration::from_millis(100);
/// Une erreur isolée se journalise au plus une fois par minute.
#[cfg(any(feature = "lecture", test))]
const JOURNAL_ERREURS: Duration = Duration::from_secs(60);

/// Ce que fait le rappel d'erreur du flux de sortie.
#[cfg(any(feature = "lecture", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Rien à faire (xrun, ou erreur déjà journalisée il y a peu).
    Rien,
    /// Erreur isolée, à journaliser.
    Journaliser,
    /// Panne : la sortie est rouverte à la phrase suivante (KOB-263).
    Panne,
}

/// Suivi des erreurs d'un flux de sortie (KOB-279, KOB-283) : une sortie
/// disparue, ou des erreurs en rafale, font une panne ; une erreur isolée
/// est seulement journalisée, sans couper la voix.
#[cfg(any(feature = "lecture", test))]
#[derive(Debug)]
pub(crate) struct SuiviErreurs {
    /// Début de la rafale en cours et dernière erreur.
    rafale: Option<(Instant, Instant)>,
    nombre: u32,
    journal: Option<Instant>,
}

#[cfg(any(feature = "lecture", test))]
impl SuiviErreurs {
    pub(crate) fn new() -> Self {
        Self {
            rafale: None,
            nombre: 0,
            journal: None,
        }
    }

    pub(crate) fn noter(&mut self, maintenant: Instant, xrun: bool, disparue: bool) -> Verdict {
        if xrun {
            return Verdict::Rien;
        }
        let debut = match self.rafale {
            Some((debut, derniere)) if maintenant.duration_since(derniere) < FIN_RAFALE => debut,
            _ => {
                self.nombre = 0;
                maintenant
            }
        };
        self.rafale = Some((debut, maintenant));
        self.nombre += 1;
        if disparue
            || (self.nombre >= ERREURS_PANNE && maintenant.duration_since(debut) >= DUREE_PANNE)
        {
            return Verdict::Panne;
        }
        if self
            .journal
            .is_some_and(|t| maintenant.duration_since(t) < JOURNAL_ERREURS)
        {
            return Verdict::Rien;
        }
        self.journal = Some(maintenant);
        Verdict::Journaliser
    }
}

/// Rouvre une sortie tombée en panne (casque débranché) : celle choisie si
/// elle est encore là, sinon celle par défaut à ce moment-là (KOB-263).
#[cfg(any(feature = "lecture", test))]
pub(crate) fn rouvrir<T>(
    nom: Option<&str>,
    ouvrir: impl Fn(Option<&str>) -> Resultat<T>,
) -> Resultat<T> {
    match ouvrir(nom) {
        Err(e) if nom.is_some() => {
            tracing::warn!(erreur = %e, "sortie choisie disparue : sortie par défaut");
            ouvrir(None)
        }
        r => r,
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
        /// `None` : flux en panne, lâché (KOB-279) ; rouvert à la phrase
        /// suivante.
        flux: Option<cpal::Stream>,
        file: Arc<Mutex<VecDeque<f32>>>,
        /// Levé par le flux en erreur (sortie débranchée) : plus rien ne se
        /// joue, la voix rend la main tout de suite (KOB-235).
        panne: Arc<std::sync::atomic::AtomicBool>,
        frequence: u32,
        canaux: usize,
        pub description: String,
        /// Sortie demandée, pour la rouvrir après une panne (KOB-263).
        nom: Option<String>,
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
                flux: Some(flux),
                file,
                panne,
                frequence,
                canaux,
                description,
                nom: nom.map(String::from),
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
        let suivi = Mutex::new(super::SuiviErreurs::new());
        let (frequence, canaux) = (config.sample_rate(), config.channels().max(1) as usize);
        let mut joue = Vec::new();
        device
            .build_output_stream::<T, _, _>(
                config.config(),
                move |sortie: &mut [T], _| {
                    let mut file = lue.lock().unwrap_or_else(|e| e.into_inner());
                    joue.clear();
                    for (i, x) in sortie.iter_mut().enumerate() {
                        let v = file.pop_front().unwrap_or(0.0);
                        if i % canaux == 0 {
                            joue.push(v);
                        }
                        *x = T::from_sample(v);
                    }
                    drop(file);
                    // Ce qui part vers la carte son, silences compris, et quand
                    // (KOB-248).
                    crate::reference::deposer(&joue, frequence, std::time::Instant::now());
                },
                move |e| {
                    use std::sync::atomic::Ordering;
                    let verdict = suivi.lock().unwrap_or_else(|e| e.into_inner()).noter(
                        Instant::now(),
                        e.kind() == cpal::ErrorKind::Xrun,
                        e.kind() == cpal::ErrorKind::DeviceNotAvailable,
                    );
                    match verdict {
                        // Sortie disparue (casque débranché, serveur audio
                        // arrêté) : plus rien ne se joue. Journalisé une fois.
                        super::Verdict::Panne => {
                            if !panne.swap(true, Ordering::SeqCst) {
                                tracing::warn!(erreur = %e, "sortie audio en panne");
                            }
                        }
                        super::Verdict::Journaliser => tracing::warn!(erreur = %e, "sortie audio"),
                        super::Verdict::Rien => {}
                    }
                },
                None,
            )
            .map_err(|e| Erreur::Sortie(e.to_string()))
    }

    impl Sortie for HautParleur {
        fn jouer(&mut self, echantillons: &[f32], frequence: u32, arret: &Arret) -> Resultat<()> {
            // Flux mort (casque débranché) : la sortie est rouverte, la voix
            // revient par celle qui reste (KOB-263).
            if self.panne.load(std::sync::atomic::Ordering::SeqCst) {
                let neuve = super::rouvrir(self.nom.as_deref(), HautParleur::ouvrir)
                    .map_err(|e| Erreur::Sortie(format!("la sortie audio s'est arrêtée ({e})")))?;
                tracing::info!(sortie = %neuve.description, "sortie audio rouverte après une panne");
                // La sortie demandée reste celle à rouvrir à la prochaine panne.
                let nom = self.nom.take();
                *self = HautParleur { nom, ..neuve };
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

        fn entretenir(&mut self) {
            // Un flux mort rappelle son erreur en boucle et tient un cœur :
            // il est lâché tout de suite, pas à la phrase suivante (KOB-279).
            if self.flux.is_some() && self.panne.load(std::sync::atomic::Ordering::SeqCst) {
                self.flux = None;
                tracing::info!(sortie = %self.description, "sortie audio en panne lâchée");
            }
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

    /// KOB-263 : la sortie choisie est rouverte si elle est encore là, sinon
    /// celle par défaut ; sans rien, l'erreur reste.
    #[test]
    fn sortie_rouverte_apres_une_panne() {
        let casque_parti = |nom: Option<&str>| match nom {
            Some("Casque") => Err(Erreur::Sortie("introuvable".into())),
            Some(n) => Ok(n.to_owned()),
            None => Ok("Haut-parleurs".to_owned()),
        };
        assert_eq!(
            rouvrir(Some("Casque"), casque_parti).unwrap(),
            "Haut-parleurs"
        );
        assert_eq!(rouvrir(Some("Écran"), casque_parti).unwrap(), "Écran");
        assert_eq!(rouvrir(None, casque_parti).unwrap(), "Haut-parleurs");
        let rien = |_: Option<&str>| -> Resultat<String> { Err(Erreur::Sortie("aucune".into())) };
        assert!(rouvrir(Some("Casque"), rien).is_err());
    }

    /// KOB-279, KOB-283 : un flux mort qui rappelle son erreur 10 000 fois
    /// fait une panne ; une erreur isolée d'un flux vivant (une par phrase
    /// sous PulseAudio) est journalisée au plus une fois par minute, sans
    /// panne ; une sortie disparue est une panne tout de suite ; un xrun,
    /// rien.
    #[test]
    fn erreurs_du_flux() {
        let t0 = Instant::now();
        let mut s = SuiviErreurs::new();
        assert_eq!(s.noter(t0, true, false), Verdict::Rien, "xrun");
        // Erreurs isolées, une toutes les 10 s : une ligne, pas de panne.
        let isolees: Vec<_> = (0..6)
            .map(|i| s.noter(t0 + Duration::from_secs(10 * i), false, false))
            .collect();
        assert_eq!(isolees[0], Verdict::Journaliser);
        assert!(
            isolees[1..].iter().all(|v| *v == Verdict::Rien),
            "{isolees:?}"
        );
        assert_eq!(
            s.noter(t0 + Duration::from_secs(61), false, false),
            Verdict::Journaliser
        );
        // Rafale à l'ouverture : 50 erreurs en une milliseconde, puis plus
        // rien ; pas de panne.
        let t1 = t0 + Duration::from_secs(120);
        let mut s = SuiviErreurs::new();
        let rafale: Vec<_> = (0..50)
            .map(|i| s.noter(t1 + Duration::from_micros(20 * i), false, false))
            .collect();
        assert!(!rafale.contains(&Verdict::Panne), "{rafale:?}");
        // KOB-296 : puis une erreur isolée 300 ms après, et des erreurs
        // isolées toutes les 0,9 s pendant 30 s : toujours pas de panne.
        let apres = s.noter(t1 + Duration::from_millis(300), false, false);
        assert_ne!(apres, Verdict::Panne);
        let isolees: Vec<_> = (1..=33)
            .map(|i| s.noter(t1 + Duration::from_millis(300 + 900 * i), false, false))
            .collect();
        assert!(!isolees.contains(&Verdict::Panne), "{isolees:?}");
        // Flux mort : 10 000 erreurs par seconde, sans fin ; panne passé
        // 250 ms, une seule ligne avant.
        let t2 = t1 + Duration::from_secs(10);
        let mut s = SuiviErreurs::new();
        let verdicts: Vec<_> = (0..10_000)
            .map(|i| s.noter(t2 + Duration::from_micros(100 * i), false, false))
            .collect();
        assert_eq!(
            verdicts
                .iter()
                .filter(|v| **v == Verdict::Journaliser)
                .count(),
            1
        );
        let premiere = verdicts.iter().position(|v| *v == Verdict::Panne).unwrap();
        assert_eq!(premiere, 2_500, "panne à 250 ms");
        // Sortie disparue : panne tout de suite.
        let mut s = SuiviErreurs::new();
        assert_eq!(s.noter(t2, false, true), Verdict::Panne);
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

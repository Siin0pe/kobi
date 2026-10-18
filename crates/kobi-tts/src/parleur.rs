//! Le parleur : deux fils, l'un synthétise, l'autre joue. Pendant qu'une
//! phrase se joue, la suivante se synthétise ; [`Parleur::interrompre`] coupe
//! la lecture en cours et oublie ce qui attendait.
//!
//! Chaque prise de parole (« énoncé ») a un numéro ; tout ce qui porte un
//! numéro périmé est jeté sans bruit.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::{Arret, Resultat, Sortie, Synthese};

/// Ce que le parleur signale à l'application.
#[derive(Debug, Clone, PartialEq)]
pub enum Evenement {
    /// Premier son d'un énoncé, `premier_son` après la première phrase reçue.
    Debut { premier_son: Duration },
    /// Fin d'un énoncé, jusqu'au bout ou interrompu.
    Fin { interrompu: bool },
    /// La voix ou la sortie audio a échoué (l'énoncé est abandonné).
    Erreur(String),
}

pub type Fabrique<T> = Box<dyn FnOnce() -> Resultat<T> + Send>;
type Rappel = Arc<dyn Fn(Evenement) + Send + Sync>;

enum VersSynthese {
    Phrase { enonce: u64, texte: String },
    Fin { enonce: u64 },
}

enum VersLecture {
    Audio {
        enonce: u64,
        echantillons: Vec<f32>,
        frequence: u32,
    },
    Fin {
        enonce: u64,
    },
    Erreur {
        enonce: u64,
        message: String,
    },
}

struct Partage {
    /// Énoncé en cours ; les autres sont périmés.
    enonce: AtomicU64,
    arret: Mutex<Arret>,
    parle: AtomicBool,
    /// Ce qui a été confié au parleur pour l'énoncé en cours (repérage de
    /// l'écho de la voix dans le micro).
    texte: Mutex<String>,
    /// Arrivée de la première phrase de l'énoncé en cours.
    t0: Mutex<Option<Instant>>,
    rappel: Rappel,
}

impl Partage {
    fn courant(&self, enonce: u64) -> bool {
        self.enonce.load(Ordering::SeqCst) == enonce
    }
}

pub struct Parleur {
    vers_synthese: Mutex<Sender<VersSynthese>>,
    partage: Arc<Partage>,
}

impl Parleur {
    /// Lance les fils. La voix et la sortie sont créées dans leur fil
    /// (chargement du modèle hors du fil de l'appelant ; un flux audio cpal
    /// ne change pas de fil).
    pub fn new(
        voix: Fabrique<Box<dyn Synthese>>,
        sortie: Fabrique<Box<dyn Sortie>>,
        rappel: impl Fn(Evenement) + Send + Sync + 'static,
    ) -> Self {
        let partage = Arc::new(Partage {
            enonce: AtomicU64::new(0),
            arret: Mutex::new(Arret::default()),
            parle: AtomicBool::new(false),
            texte: Mutex::new(String::new()),
            t0: Mutex::new(None),
            rappel: Arc::new(rappel),
        });
        let (tx_s, rx_s) = mpsc::channel();
        let (tx_l, rx_l) = mpsc::channel();
        let p = Arc::clone(&partage);
        std::thread::Builder::new()
            .name("voix-synthese".into())
            .spawn(move || synthese(voix, rx_s, tx_l, p))
            .expect("fil de synthèse");
        let p = Arc::clone(&partage);
        std::thread::Builder::new()
            .name("voix-lecture".into())
            .spawn(move || lecture(sortie, rx_l, p))
            .expect("fil de lecture");
        Self {
            vers_synthese: Mutex::new(tx_s),
            partage,
        }
    }

    /// Nouvel énoncé : coupe celui en cours et renvoie son numéro.
    pub fn nouvel_enonce(&self) -> u64 {
        self.interrompre();
        self.partage.enonce.load(Ordering::SeqCst)
    }

    /// Ajoute une phrase à l'énoncé `enonce` (ignorée s'il est périmé).
    pub fn dire(&self, enonce: u64, phrase: &str) {
        if !self.partage.courant(enonce) || phrase.trim().is_empty() {
            return;
        }
        {
            let mut t0 = self.partage.t0.lock().unwrap_or_else(|e| e.into_inner());
            t0.get_or_insert_with(Instant::now);
        }
        {
            let mut texte = self.partage.texte.lock().unwrap_or_else(|e| e.into_inner());
            texte.push_str(phrase);
            texte.push(' ');
        }
        self.envoyer(VersSynthese::Phrase {
            enonce,
            texte: phrase.to_string(),
        });
    }

    /// Plus rien à dire pour cet énoncé : `Fin` suivra la dernière phrase.
    pub fn finir(&self, enonce: u64) {
        self.envoyer(VersSynthese::Fin { enonce });
    }

    /// Coupe la voix tout de suite et oublie ce qui attendait.
    pub fn interrompre(&self) {
        let mut arret = self.partage.arret.lock().unwrap_or_else(|e| e.into_inner());
        arret.arreter();
        *arret = Arret::default();
        self.partage.enonce.fetch_add(1, Ordering::SeqCst);
        self.partage
            .texte
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        *self.partage.t0.lock().unwrap_or_else(|e| e.into_inner()) = None;
    }

    /// La voix est-elle en train de jouer ?
    pub fn parle(&self) -> bool {
        self.partage.parle.load(Ordering::SeqCst)
    }

    /// Texte confié pour l'énoncé en cours.
    pub fn texte_en_cours(&self) -> String {
        self.partage
            .texte
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn envoyer(&self, m: VersSynthese) {
        if let Ok(tx) = self.vers_synthese.lock() {
            let _ = tx.send(m);
        }
    }
}

fn synthese(
    voix: Fabrique<Box<dyn Synthese>>,
    rx: Receiver<VersSynthese>,
    tx: Sender<VersLecture>,
    p: Arc<Partage>,
) {
    let debut = Instant::now();
    let mut voix = match voix() {
        Ok(v) => {
            tracing::info!(ms = debut.elapsed().as_millis() as u64, "voix chargée");
            Ok(v)
        }
        Err(e) => {
            tracing::warn!(erreur = %e, "voix indisponible");
            Err(e.to_string())
        }
    };
    while let Ok(m) = rx.recv() {
        match m {
            VersSynthese::Phrase { enonce, texte } => {
                if !p.courant(enonce) {
                    continue;
                }
                let v = match voix.as_mut() {
                    Ok(v) => v,
                    Err(e) => {
                        let _ = tx.send(VersLecture::Erreur {
                            enonce,
                            message: e.clone(),
                        });
                        continue;
                    }
                };
                let t = Instant::now();
                match v.synthetiser(&texte) {
                    Ok(echantillons) => {
                        tracing::debug!(
                            ms = t.elapsed().as_millis() as u64,
                            duree_ms =
                                (echantillons.len() as u64 * 1000) / v.frequence().max(1) as u64,
                            "phrase synthétisée"
                        );
                        let _ = tx.send(VersLecture::Audio {
                            enonce,
                            echantillons,
                            frequence: v.frequence(),
                        });
                    }
                    Err(e) => {
                        let _ = tx.send(VersLecture::Erreur {
                            enonce,
                            message: e.to_string(),
                        });
                    }
                }
            }
            VersSynthese::Fin { enonce } => {
                let _ = tx.send(VersLecture::Fin { enonce });
            }
        }
    }
}

fn lecture(sortie: Fabrique<Box<dyn Sortie>>, rx: Receiver<VersLecture>, p: Arc<Partage>) {
    let mut sortie = sortie().map_err(|e| e.to_string());
    if let Err(e) = &sortie {
        tracing::warn!(erreur = %e, "sortie audio indisponible");
    }
    // Énoncé dont le premier son est parti.
    let mut commence: Option<u64> = None;
    let fin = |p: &Partage, interrompu: bool| {
        p.parle.store(false, Ordering::SeqCst);
        (p.rappel)(Evenement::Fin { interrompu });
    };
    loop {
        // Une interruption pendant l'attente : l'énoncé commencé est fini.
        let m = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(m) => Some(m),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if let Some(e) = commence {
            if !p.courant(e) {
                commence = None;
                fin(&p, true);
            }
        }
        let Some(m) = m else { continue };
        match m {
            VersLecture::Audio {
                enonce,
                echantillons,
                frequence,
            } => {
                if !p.courant(enonce) {
                    continue;
                }
                let s = match sortie.as_mut() {
                    Ok(s) => s,
                    Err(e) => {
                        (p.rappel)(Evenement::Erreur(e.clone()));
                        continue;
                    }
                };
                if commence != Some(enonce) {
                    commence = Some(enonce);
                    p.parle.store(true, Ordering::SeqCst);
                    let t0 = *p.t0.lock().unwrap_or_else(|e| e.into_inner());
                    let premier_son = t0.map(|t| t.elapsed()).unwrap_or_default();
                    (p.rappel)(Evenement::Debut { premier_son });
                }
                let arret = p.arret.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if let Err(e) = s.jouer(&echantillons, frequence, &arret) {
                    (p.rappel)(Evenement::Erreur(e.to_string()));
                }
            }
            VersLecture::Fin { enonce } => {
                if commence == Some(enonce) {
                    commence = None;
                    fin(&p, !p.courant(enonce));
                }
            }
            VersLecture::Erreur { enonce, message } => {
                if p.courant(enonce) {
                    (p.rappel)(Evenement::Erreur(message));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Erreur;

    /// Voix factice : 100 ms de son par mot, synthèse en 30 ms.
    struct Factice;
    impl Synthese for Factice {
        fn frequence(&self) -> u32 {
            1_000
        }
        fn synthetiser(&mut self, phrase: &str) -> Resultat<Vec<f32>> {
            std::thread::sleep(Duration::from_millis(30));
            Ok(vec![0.1; 100 * phrase.split_whitespace().count()])
        }
    }

    /// Sortie factice : joue en temps réel par pas de 10 ms, note ce qui est joué.
    struct Ecoute(Arc<Mutex<Vec<usize>>>);
    impl Sortie for Ecoute {
        fn jouer(&mut self, e: &[f32], f: u32, arret: &Arret) -> Resultat<()> {
            let mut joue = 0;
            for bloc in e.chunks((f / 100) as usize) {
                if arret.arrete() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
                joue += bloc.len();
            }
            self.0.lock().unwrap().push(joue);
            Ok(())
        }
    }

    fn parleur() -> (Parleur, Arc<Mutex<Vec<usize>>>, Receiver<Evenement>) {
        let joue = Arc::new(Mutex::new(Vec::new()));
        let j = Arc::clone(&joue);
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let p = Parleur::new(
            Box::new(|| Ok(Box::new(Factice) as Box<dyn Synthese>)),
            Box::new(move || Ok(Box::new(Ecoute(j)) as Box<dyn Sortie>)),
            move |e| {
                let _ = tx.lock().unwrap().send(e);
            },
        );
        (p, joue, rx)
    }

    #[test]
    fn un_enonce_se_dit_en_entier() {
        let (p, joue, rx) = parleur();
        let e = p.nouvel_enonce();
        p.dire(e, "une deux trois");
        p.dire(e, "quatre cinq");
        p.finir(e);
        let debut = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let Evenement::Debut { premier_son } = debut else {
            panic!("{debut:?}")
        };
        assert!(premier_son < Duration::from_millis(200), "{premier_son:?}");
        assert!(p.parle());
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Evenement::Fin { interrompu: false }
        );
        assert!(!p.parle());
        assert_eq!(*joue.lock().unwrap(), [300, 200]);
        assert_eq!(p.texte_en_cours(), "une deux trois quatre cinq ");
    }

    #[test]
    fn interrompre_coupe_net_et_oublie_la_suite() {
        let (p, joue, rx) = parleur();
        let e = p.nouvel_enonce();
        p.dire(e, &"mot ".repeat(20)); // 2 s
        p.dire(e, "la suite qui ne sera jamais dite");
        p.finir(e);
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Evenement::Debut { .. }
        ));
        std::thread::sleep(Duration::from_millis(150));
        let t = Instant::now();
        p.interrompre();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            Evenement::Fin { interrompu: true }
        );
        assert!(
            t.elapsed() < Duration::from_millis(150),
            "{:?}",
            t.elapsed()
        );
        std::thread::sleep(Duration::from_millis(300));
        let joue = joue.lock().unwrap();
        assert_eq!(joue.len(), 1, "la phrase suivante n'est pas jouée");
        assert!(joue[0] < 600, "coupé net : {joue:?}");
        // Une phrase pour l'énoncé périmé est ignorée.
        p.dire(e, "trop tard");
        assert!(rx.recv_timeout(Duration::from_millis(200)).is_err());
    }

    #[test]
    fn un_nouvel_enonce_remplace_le_precedent() {
        let (p, _joue, rx) = parleur();
        let a = p.nouvel_enonce();
        p.dire(a, &"mot ".repeat(20));
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            Evenement::Debut { .. }
        ));
        let b = p.nouvel_enonce();
        p.dire(b, "nouvelle réponse");
        p.finir(b);
        let suite: Vec<_> = (0..3)
            .map_while(|_| rx.recv_timeout(Duration::from_secs(2)).ok())
            .collect();
        assert!(matches!(suite[0], Evenement::Fin { interrompu: true }));
        assert!(matches!(suite[1], Evenement::Debut { .. }));
        assert_eq!(suite[2], Evenement::Fin { interrompu: false });
    }

    #[test]
    fn voix_absente_signalee() {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let p = Parleur::new(
            Box::new(|| Err(Erreur::VoixAbsente("pas de paquet".into()))),
            Box::new(|| Ok(Box::new(Ecoute(Default::default())) as Box<dyn Sortie>)),
            move |e| {
                let _ = tx.lock().unwrap().send(e);
            },
        );
        let e = p.nouvel_enonce();
        p.dire(e, "bonjour à tous");
        let Evenement::Erreur(m) = rx.recv_timeout(Duration::from_secs(2)).unwrap() else {
            panic!()
        };
        assert!(m.contains("pas de paquet"), "{m}");
    }
}

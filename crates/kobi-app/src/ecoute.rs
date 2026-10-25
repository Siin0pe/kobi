//! Écoute continue du micro (KOB-132) : Nemotron transcrit en local, en
//! permanence tant que Kobi n'est pas en pause, et [`Ecouteur`] repère
//! « Kobi » dans la transcription. Rien ne sort de la machine : le fil
//! publie segments et questions au chef d'orchestre ([`crate::chef`]), qui
//! seul écrit la réunion (quand on l'a demandé) et interroge Claude. Le fil ne
//! s'arrête jamais pour attendre une réponse.
//!
//! Le fil garde le micro ouvert et le modèle chargé. Le raccourci clavier
//! reste un déclencheur de secours : il lui envoie [`Commande::Raccourci`].
//!
//! `KOBI_DIAG=1` journalise le niveau du micro et les segments à part
//! ([`crate::diag`]) ; `KOBI_ECOUTE_WAV=<fichier.wav>` remplace le micro par
//! ce fichier, joué en temps réel puis suivi de silence (essais sans voix).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kobi_audio::{AudioFrame, Capture, CaptureOptions};
use kobi_stt::nemotron::NemotronEngine;
use kobi_stt::SttEngine;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::chef::{publier, Message};
use crate::diag::Diag;
use crate::intentions::Intention;
use crate::interpellation::{charger_stt, erreur};
use crate::mot_cle::{Action, Ecouteur};
use crate::porte::Porte;
use crate::session::etat;
use crate::voix::CommandeVoix;

/// Après un échec d'ouverture du micro, nouvel essai après ce délai.
const REESSAI_MICRO: Duration = Duration::from_secs(5);
/// Micro ouvert sans aucune trame depuis ce temps : il est perdu, même sans
/// erreur signalée (serveur audio arrêté, KOB-280). Il en arrive une toutes
/// les 100 ms tant qu'il vit.
const MICRO_MUET: Duration = Duration::from_secs(3);
/// Attente d'une trame quand la file est vide.
const ATTENTE_TRAME: Duration = Duration::from_millis(200);
/// Attente de l'ouverture du micro, faite dans un fil à part : au-delà,
/// l'écoute ne reste pas bloquée (serveur audio mort ou figé, KOB-285) et
/// la reprend au tour suivant.
const ATTENTE_OUVERTURE: Duration = Duration::from_secs(5);
/// Ouverture du micro toujours en cours après ce temps : abandonnée, une
/// nouvelle est lancée (le serveur audio a pu revenir entre-temps).
const ABANDON_OUVERTURE: Duration = Duration::from_secs(30);
/// Attente de la fermeture du micro par son fil porteur.
const ATTENTE_FERMETURE: Duration = Duration::from_secs(2);
/// Réunion mise en pause sous charge : l'audio dit avant la pause mais pas
/// encore transcrit l'est avant de fermer le micro (KOB-261), tant que la
/// réunion reste en pause ou se termine, au plus pendant ce temps (KOB-277).
const VIDANGE_FILE: Duration = Duration::from_secs(600);
/// Période du bilan de charge dans le journal.
const BILAN: Duration = Duration::from_secs(300);
/// Événement du retard de la transcription sur la parole (KOB-250) :
/// [`Retard`], ou `null` quand elle a rattrapé.
pub const EVENEMENT_RETARD: &str = "transcription_retard";
/// Au-delà de ce retard (s), Kobi le signale : un ordre dit maintenant ne
/// sera entendu qu'après.
const SEUIL_RETARD: f64 = 15.0;
/// Retard (s) à partir duquel la transcription colle les phrases, souvent sans
/// ponctuation : un ordre de réunion collé au nom compte alors comme un appel
/// (KOB-289, KOB-301).
const RETARD_PHRASES_COLLEES: f64 = 3.0;
/// Le retard est signalé de nouveau à chaque palier de ce nombre de secondes.
const PALIER_RETARD: f64 = 30.0;

/// Début de phrase transcrit à l'avance quand la transcription est en
/// retard, en secondes : assez pour « Kobi, arrête l'enregistrement »
/// (KOB-250).
const EXTRAIT_ANTICIPE: f64 = 2.5;
/// Une phrase arrivée depuis plus longtemps (s) n'est pas transcrite à
/// l'avance : la veille reste sur la parole fraîche au lieu de prendre du
/// retard à son tour ; la transcription ordinaire s'en chargera.
const FRAICHEUR_ANTICIPEE: f64 = 6.0;

/// Un ordre de fin de réunion dans une phrase qui appelle Kobi (« Kobi,
/// arrête l'enregistrement », « Kobi, la réunion est terminée »).
fn ordre_de_fin(texte: &str) -> bool {
    crate::mot_cle::reperer(texte).is_some_and(|q| {
        crate::intentions::fin_de_reunion_dite(q)
            || crate::intentions::intention(q) == Some(Intention::TerminerReunion)
    })
}

/// Écoute anticipée (KOB-250) : quand la transcription est en retard, le
/// début de chaque phrase qui vient d'être dite est transcrit à part, pour
/// qu'un ordre de fin de réunion n'attende pas son tour dans la file.
#[derive(Default)]
struct Veille {
    porte: Option<Porte>,
    /// Début de la phrase en cours, à transcrire.
    extrait: Vec<f32>,
    debut: Option<f64>,
    /// Le début de cette phrase a déjà été transcrit.
    faite: bool,
    /// Instant (audio) des ordres déjà exécutés à l'avance : la transcription
    /// en retard ne les rejoue pas en arrivant à leur phrase.
    ordres: Vec<f64>,
}

/// Événement du micro perdu (KOB-236) : `true` tant que Kobi n'a plus de
/// micro (débranché, aucun autre), `false` quand il en a retrouvé un.
pub const EVENEMENT_MICRO: &str = "micro";

/// Charge de [`EVENEMENT_RETARD`].
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Retard {
    /// Retard de la transcription sur la parole, en secondes.
    pub secondes: u64,
    /// Raccourci de l'enregistrement (réglage `hotkey_record`) : il arrête la
    /// réunion tout de suite, retard ou non.
    pub raccourci: String,
}

/// Faut-il signaler le retard ? `Some(Some(s))` : signaler `s` secondes ;
/// `Some(None)` : il est rattrapé ; `None` : rien de nouveau. `signale` est
/// le dernier retard signalé.
fn signal_retard(retard: f64, signale: Option<u64>) -> Option<Option<u64>> {
    match signale {
        _ if retard >= SEUIL_RETARD
            && signale.is_none_or(|s| retard >= s as f64 + PALIER_RETARD) =>
        {
            Some(Some(retard as u64))
        }
        Some(_) if retard < SEUIL_RETARD / 3.0 => Some(None),
        _ => None,
    }
}
/// Événement du niveau du micro (0 à 1), pendant l'écoute.
pub const EVENEMENT_NIVEAU: &str = "niveau";
/// Période du niveau envoyé à l'interface (20 par seconde).
const PERIODE_NIVEAU: f64 = 0.05;

/// Ce que l'application demande au fil d'écoute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Commande {
    /// Raccourci clavier : la phrase suivante est une question.
    Raccourci,
    /// Met l'écoute en pause, ou la reprend.
    BasculerPause,
    /// Met l'écoute en pause (`true`) ou la reprend (`false`), interface de
    /// contrôle (KOB-161).
    Pause(bool),
    /// La réunion est mise en pause à cet instant : l'audio dit après ne
    /// sera pas transcrit (KOB-277).
    PauseReunion(Instant),
    /// La réunion reprend : la pause d'avant ne compte plus (KOB-297).
    RepriseReunion,
    /// « Annuler » est fait ou clos : la fenêtre ne se rouvre plus à la
    /// prochaine ouverture du micro (KOB-299).
    FinAnnulation,
    /// Kobi vient de répondre : la conversation attend la suite.
    Suite,
    /// La dernière carte de validation s'est fermée : sa fenêtre aussi, la
    /// conversation attend la suite (KOB-273).
    CartesFermees,
    /// La réunion vient de finir : « reprends » ou « annule » s'entend sans
    /// redire « Kobi » tant que la fin s'annule (KOB-165, KOB-245). Porte
    /// l'instant réel de l'arrêt : sous retard, ce qui a été dit avant ne
    /// l'annule pas, et le délai court depuis lui (KOB-265).
    Annulable(Instant),
    /// Kobi pose une question et attend la réponse ce temps-là (carte de
    /// validation, KOB-197).
    ReponseAttendue(Duration),
}

/// Canal vers le fil d'écoute.
pub struct Ecoute(Mutex<Sender<Commande>>);

/// L'écoute est en pause (micro coupé), tenu à jour par le fil d'écoute.
#[derive(Default)]
pub struct EnPause(pub AtomicBool);

/// Lance le fil d'écoute.
pub fn installer<R: Runtime>(app: &AppHandle<R>) {
    let (tx, rx) = mpsc::channel();
    app.manage(Ecoute(Mutex::new(tx)));
    app.manage(EnPause::default());
    let app = app.clone();
    std::thread::Builder::new()
        .name("ecoute".into())
        .spawn(move || Fil::new(app).boucle(rx))
        .expect("fil d'écoute");
}

/// Envoie une commande au fil d'écoute.
pub fn commander<R: Runtime>(app: &AppHandle<R>, commande: Commande) -> Result<(), String> {
    app.try_state::<Ecoute>()
        .ok_or("écoute non lancée")?
        .0
        .lock()
        .map_err(|e| e.to_string())?
        .send(commande)
        .map_err(|e| e.to_string())
}

/// Raccourci de l'enregistrement tel que réglé (pris à chaud, KOB-214).
fn raccourci_enregistrement() -> String {
    kobi_session::config::default_path()
        .filter(|p| p.exists())
        .map(|p| {
            kobi_session::config::load_or_create(&p)
                .config
                .hotkey_record
        })
        .unwrap_or_else(|| kobi_session::config::Config::default().hotkey_record)
}

/// D'où vient l'audio.
enum Entree {
    Micro(Porteur),
    /// Fichier injecté (`KOBI_ECOUTE_WAV`), joué par un fil à part.
    Fichier(Receiver<AudioFrame>),
}

impl Entree {
    fn frames(&self) -> &Receiver<AudioFrame> {
        match self {
            Entree::Micro(porteur) => &porteur.trames,
            Entree::Fichier(rx) => rx,
        }
    }

    /// Ferme l'entrée ; renvoie l'audio reçu mais pas encore lu.
    fn fermer(self) -> Vec<AudioFrame> {
        match self {
            Entree::Micro(porteur) => porteur.fermer(),
            Entree::Fichier(rx) => rx.try_iter().collect(),
        }
    }

    /// Panne du périphérique signalée par le flux (micro débranché), xruns
    /// mis à part : le canal des trames, lui, ne se ferme pas (KOB-236).
    fn panne(&self) -> Option<String> {
        match self {
            Entree::Micro(porteur) => panne_peripherique(porteur.erreurs.try_iter()),
            Entree::Fichier(_) => None,
        }
    }

    /// Ouvre le fichier injecté, ou le micro choisi ; s'il a disparu, celui
    /// par défaut (KOB-236). Renvoie aussi sa description pour le journal.
    /// Appelé dans le fil qui gardera la capture ([`Ouverture::lancer`]).
    fn ouvrir_ici() -> Result<(Ouvert, String), String> {
        if let Some(wav) = std::env::var_os("KOBI_ECOUTE_WAV") {
            let wav = PathBuf::from(wav);
            let samples = kobi_stt::wav::read_wav_16k_mono(&wav)
                .map_err(|e| format!("{} : {e}", wav.display()))?;
            let description = format!(
                "fichier {} ({:.1} s, 16 kHz mono)",
                wav.display(),
                samples.len() as f64 / kobi_stt::SAMPLE_RATE as f64
            );
            return Ok((Ouvert::Fichier(jouer(samples)), description));
        }
        let demarrer = |mic: Option<String>| {
            Capture::start(&CaptureOptions {
                sources: vec![kobi_audio::Source::Moi],
                mic,
                output: None,
            })
        };
        // Micro choisi au premier lancement (KOB-144), sinon celui du système.
        let capture = match crate::bienvenue::micro_choisi() {
            Some(choisi) => demarrer(Some(choisi.clone())).or_else(|e| {
                tracing::warn!(%choisi, erreur = %e, "micro choisi indisponible : micro par défaut");
                demarrer(None)
            }),
            None => demarrer(None),
        }
        .map_err(|e| e.to_string())?;
        let description = capture
            .sources()
            .iter()
            .map(|s| {
                format!(
                    "micro « {} » ({} Hz, {} canaux, {:?}), rééchantillonné en 16 kHz mono",
                    s.device, s.sample_rate, s.channels, s.sample_format
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        Ok((Ouvert::Micro(capture), description))
    }
}

/// Ce que [`Entree::ouvrir_ici`] a ouvert.
enum Ouvert {
    Micro(Capture),
    Fichier(Receiver<AudioFrame>),
}

/// Micro tenu par un fil porteur (KOB-285) : la capture ne quitte jamais
/// le fil qui l'a ouverte (un flux cpal n'est pas `Send` partout), et
/// l'écoute ne reste jamais bloquée sur son ouverture ni sa fermeture. Les
/// trames et les erreurs du flux sont relayées.
struct Porteur {
    trames: Receiver<AudioFrame>,
    erreurs: Receiver<kobi_audio::StreamError>,
    /// Demande de fermeture ; la réponse rend l'audio reçu mais pas encore
    /// relayé.
    fermer: Sender<Sender<Vec<AudioFrame>>>,
}

impl Porteur {
    /// Ferme la capture ; renvoie l'audio reçu mais pas encore lu, dans
    /// l'ordre. Le porteur bloqué (serveur audio mort) n'est pas attendu
    /// au-delà de [`ATTENTE_FERMETURE`].
    fn fermer(self) -> Vec<AudioFrame> {
        let (tx, rx) = mpsc::channel();
        let fin = if self.fermer.send(tx).is_ok() {
            rx.recv_timeout(ATTENTE_FERMETURE).unwrap_or_default()
        } else {
            Vec::new()
        };
        // La réponse reçue, le porteur ne relaie plus rien : tout ce qu'il a
        // relayé est déjà là, et passe avant le reste de la capture. Relevé
        // avant la réponse, une trame relayée entre-temps passait après
        // (KOB-297).
        let mut reste: Vec<AudioFrame> = self.trames.try_iter().collect();
        reste.extend(fin);
        reste
    }
}

/// Flux d'un micro ouvert, vu par son porteur.
trait Flux {
    fn trames(&self) -> &Receiver<AudioFrame>;
    fn erreurs(&self) -> &Receiver<kobi_audio::StreamError>;
    /// Arrête le flux ; renvoie l'audio reçu mais pas encore lu.
    fn arreter(self) -> Vec<AudioFrame>;
}

impl Flux for Capture {
    fn trames(&self) -> &Receiver<AudioFrame> {
        self.frames()
    }
    fn erreurs(&self) -> &Receiver<kobi_audio::StreamError> {
        self.errors()
    }
    fn arreter(self) -> Vec<AudioFrame> {
        self.stop().0
    }
}

/// Fil porteur : relaie les trames et les erreurs de `capture` jusqu'à la
/// demande de fermeture, ou jusqu'à ce que l'écoute lâche le micro.
fn porter(
    capture: impl Flux,
    trames: Sender<AudioFrame>,
    erreurs: Sender<kobi_audio::StreamError>,
    fermer: Receiver<Sender<Vec<AudioFrame>>>,
) {
    loop {
        match capture.trames().recv_timeout(Duration::from_millis(50)) {
            Ok(trame) => {
                if trames.send(trame).is_err() {
                    return;
                }
                for trame in capture.trames().try_iter() {
                    if trames.send(trame).is_err() {
                        return;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            // Flux fini : l'écoute le voit au canal relayé, fermé à son tour.
            Err(RecvTimeoutError::Disconnected) => return,
        }
        for erreur in capture.erreurs().try_iter() {
            let _ = erreurs.send(erreur);
        }
        match fermer.try_recv() {
            Ok(reponse) => {
                let _ = reponse.send(capture.arreter());
                return;
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => return,
        }
    }
}

/// Ouverture du micro en cours dans un fil à part (KOB-285).
struct Ouverture {
    /// L'entrée, sa description, et l'instant où son flux a démarré : la
    /// base de l'horloge de ses trames (KOB-294).
    resultat: Receiver<Result<(Entree, String, Instant), String>>,
    depuis: Instant,
}

impl Ouverture {
    /// Ouvre l'entrée dans un fil qui, pour un micro, reste ensuite son
    /// porteur.
    fn lancer() -> Self {
        Self::lancer_avec(Entree::ouvrir_ici)
    }

    fn lancer_avec(
        ouvrir: impl FnOnce() -> Result<(Ouvert, String), String> + Send + 'static,
    ) -> Self {
        let (tx, resultat) = mpsc::channel();
        let lance = std::thread::Builder::new()
            .name("ouverture-micro".into())
            .spawn(move || match ouvrir().map(|o| (o, Instant::now())) {
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
                Ok(((Ouvert::Fichier(rx), description), demarre)) => {
                    let _ = tx.send(Ok((Entree::Fichier(rx), description, demarre)));
                }
                Ok(((Ouvert::Micro(capture), description), demarre)) => {
                    let (trames_tx, trames) = mpsc::channel();
                    let (erreurs_tx, erreurs) = mpsc::channel();
                    let (fermer, fermer_rx) = mpsc::channel();
                    let porteur = Porteur {
                        trames,
                        erreurs,
                        fermer,
                    };
                    // L'écoute a renoncé entre-temps : la capture se ferme.
                    if tx
                        .send(Ok((Entree::Micro(porteur), description, demarre)))
                        .is_ok()
                    {
                        porter(capture, trames_tx, erreurs_tx, fermer_rx);
                    }
                }
            });
        if let Err(e) = lance {
            let (tx, rx) = mpsc::channel();
            let _ = tx.send(Err(e.to_string()));
            return Ouverture {
                resultat: rx,
                depuis: Instant::now(),
            };
        }
        Ouverture {
            resultat,
            depuis: Instant::now(),
        }
    }

    /// Le résultat, s'il arrive avant `delai` ; `None` : toujours en cours.
    fn attendre(&self, delai: Duration) -> Option<Result<(Entree, String, Instant), String>> {
        match self.resultat.recv_timeout(delai) {
            Ok(r) => Some(r),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => {
                Some(Err("ouverture du micro interrompue".into()))
            }
        }
    }
}

/// Passe les trames de `file` à `traiter`, dans l'ordre, jusqu'à `limite`
/// ou jusqu'à ce que `reprise` soit vraie ; renvoie le nombre de trames
/// traitées (KOB-261).
fn vidanger<T, C>(
    file: &mut std::collections::VecDeque<T>,
    limite: Instant,
    reprise: impl Fn(&mut C) -> bool,
    mut traiter: impl FnMut(&mut C, T),
    contexte: &mut C,
) -> usize {
    let mut traitees = 0;
    while !file.is_empty() && Instant::now() < limite && !reprise(contexte) {
        if let Some(trame) = file.pop_front() {
            traiter(contexte, trame);
            traitees += 1;
        }
    }
    traitees
}

/// Ce que le micro a envoyé depuis le dernier tour.
enum Arrivee {
    Trames(Vec<AudioFrame>),
    /// Rien pour l'instant.
    Rien,
    /// Rien depuis [`MICRO_MUET`] : le flux s'est arrêté sans le dire.
    Muet,
    /// Le canal des trames est fermé.
    Ferme,
}

/// Trames arrivées du micro : attend la première au plus [`ATTENTE_TRAME`]
/// si `attendre` (file vide), puis prend tout ce qui est arrivé.
/// `derniere`, l'instant de la dernière trame reçue, est tenu à jour ; sans
/// trame depuis `muet`, le micro est muet (KOB-280).
fn recevoir(
    trames: &Receiver<AudioFrame>,
    attendre: bool,
    derniere: &mut Instant,
    muet: Duration,
) -> Arrivee {
    let mut arrivees = Vec::new();
    if attendre {
        match trames.recv_timeout(ATTENTE_TRAME) {
            Ok(trame) => arrivees.push(trame),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Arrivee::Ferme,
        }
    }
    arrivees.extend(trames.try_iter());
    if !arrivees.is_empty() {
        *derniere = Instant::now();
        Arrivee::Trames(arrivees)
    } else if derniere.elapsed() >= muet {
        Arrivee::Muet
    } else {
        Arrivee::Rien
    }
}

/// L'écoute de Kobi sera-t-elle coupée une fois `commandes` appliquées ?
/// Le raccourci la rouvre, comme la fin de la coupure (KOB-295).
fn ecoute_en_pause_apres(en_pause: bool, commandes: &[Commande]) -> bool {
    commandes.iter().fold(en_pause, |pause, c| match c {
        Commande::Pause(p) => *p,
        Commande::BasculerPause => !pause,
        Commande::Raccourci => false,
        _ => pause,
    })
}

/// La vidange de la pause continue-t-elle ? Tant que la réunion reste en
/// pause ou se termine (le compte rendu attend la fin de la transcription),
/// ou que seule l'écoute de Kobi est coupée ; pas à la reprise, ni une fois
/// la session revenue en veille (KOB-277).
fn vidange_continue(session: &str, ecoute_en_pause: bool) -> bool {
    match session {
        "pause" | "fin" => true,
        "reunion" => ecoute_en_pause,
        _ => false,
    }
}

/// Écarte de `file` les trames dites après `pause` (sur l'horloge du micro
/// ouvert à `origine`, décalée de `decalage`) ; renvoie leur nombre.
fn garder_avant(
    file: &mut std::collections::VecDeque<AudioFrame>,
    origine: Instant,
    decalage: f64,
    pause: Instant,
) -> usize {
    let avant = file.len();
    file.retain(|f| origine + Duration::from_secs_f64(decalage) + f.t <= pause);
    avant - file.len()
}

/// Durée (s) de l'audio en file, à 16 kHz.
fn duree_audio(file: &std::collections::VecDeque<AudioFrame>) -> f64 {
    file.iter().map(|f| f.samples.len()).sum::<usize>() as f64 / kobi_stt::SAMPLE_RATE as f64
}

/// Première erreur de périphérique parmi celles d'un flux ; un xrun (trames
/// perdues) n'arrête pas le micro (KOB-236).
fn panne_peripherique(erreurs: impl Iterator<Item = kobi_audio::StreamError>) -> Option<String> {
    erreurs.filter(|e| !e.xrun).map(|e| e.message).next()
}

/// Joue des échantillons 16 kHz en temps réel, par trames de 100 ms, puis du
/// silence tant que le récepteur existe.
fn jouer(samples: Vec<f32>) -> Receiver<AudioFrame> {
    const TRAME: usize = 1_600;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let silence = vec![0.0; TRAME];
        let mut morceaux = samples.chunks(TRAME);
        let debut = Instant::now();
        for i in 0u64.. {
            let morceau = morceaux.next().unwrap_or(&silence);
            let t = Duration::from_millis(100 * i);
            if let Some(attente) = t.checked_sub(debut.elapsed()) {
                std::thread::sleep(attente);
            }
            let frame = AudioFrame {
                source: kobi_audio::Source::Moi,
                samples: morceau.to_vec(),
                t,
            };
            if tx.send(frame).is_err() {
                return;
            }
        }
    });
    rx
}

/// Transcrit une trame et en déduit les actions de l'écoute. La porte ne
/// laisse passer vers Nemotron que la parole ; l'écoute voit passer le temps
/// à chaque trame. `t` est la fin de la trame en temps du micro (celui du
/// moteur repart de zéro à chaque question).
/// `entendu` reçoit chaque segment (partiel ou final) et dit s'il faut
/// l'ignorer : c'est la voix de Kobi qui revient dans le micro (KOB-142).
/// Prise de parole en cours, pour rattraper un mot court resté sans texte
/// (KOB-197) : l'audio transmis depuis l'ouverture de la porte, et si le
/// moteur a déjà sorti du texte.
#[derive(Default)]
struct Rattrapage {
    audio: Vec<f32>,
    texte: bool,
    /// Dernier partiel resté sans final (KOB-253), et son début.
    partiel: Option<(String, f64)>,
}

/// Au plus 3 s gardées : au-delà, ce n'est plus un mot court.
const RATTRAPAGE_MAX: usize = 3 * kobi_stt::SAMPLE_RATE as usize;
/// Partie voisée d'un mot court, en secondes.
const MOT_COURT: (f64, f64) = (0.1, 1.5);

/// Partie voisée d'un audio (au-dessus d'un seuil d'amplitude), si elle a
/// la durée d'un mot court.
fn mot_court(audio: &[f32]) -> Option<&[f32]> {
    let fort = |x: &f32| x.abs() > 0.02;
    let debut = audio.iter().position(fort)?;
    let fin = audio.iter().rposition(fort)? + 1;
    let duree = (fin - debut) as f64 / kobi_stt::SAMPLE_RATE as f64;
    (MOT_COURT.0..=MOT_COURT.1)
        .contains(&duree)
        .then(|| &audio[debut..fin])
}

/// « Oui, oui » → « Oui » : le mot redoublé pour le moteur revient seul.
fn sans_repetition(texte: &str) -> String {
    let mut mots: Vec<&str> = Vec::new();
    for mot in texte.split_whitespace() {
        let nu = |m: &str| {
            m.trim_matches(|c: char| !c.is_alphanumeric())
                .to_lowercase()
        };
        if mots.last().is_none_or(|d| nu(d) != nu(mot)) {
            mots.push(mot);
        }
    }
    mots.join(" ")
        .trim_end_matches(|c: char| !c.is_alphanumeric())
        .to_string()
}

/// Le mot répété pour le moteur, lu comme « Oui » ou « Non » quand ses
/// graphies s'en approchent (« Uui, uii, ui », « No, no ») : seulement pour
/// un mot court rattrapé pendant que Kobi attend une réponse.
fn oui_ou_non(texte: &str) -> Option<&'static str> {
    const OUI: &[&str] = &[
        "oui", "ui", "uui", "uii", "ouii", "wi", "ouais", "ouai", "wee", "we",
    ];
    const NON: &[&str] = &["non", "no", "nom", "nan", "noon", "nono"];
    let mots: Vec<String> = texte
        .split(|c: char| !c.is_alphanumeric())
        .filter(|m| !m.is_empty())
        .map(str::to_lowercase)
        .collect();
    let compte = |liste: &[&str]| mots.iter().filter(|m| liste.contains(&m.as_str())).count();
    let (oui, non) = (compte(OUI), compte(NON));
    // La majorité des mots, et pas un mélange des deux.
    if oui * 2 > mots.len() && non == 0 {
        Some("Oui")
    } else if non * 2 > mots.len() && oui == 0 {
        Some("Non")
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn transcrire(
    stt: &mut impl SttEngine,
    porte: &mut Porte,
    ecouteur: &mut Ecouteur,
    diag: &mut Option<Diag>,
    rattrapage: &mut Rattrapage,
    frame: kobi_stt::AudioFrame,
    t: f64,
    entendu: &dyn Fn(&str) -> bool,
    kobi_parle: bool,
) -> (Vec<Action>, Vec<kobi_stt::Segment>) {
    if let Some(d) = diag {
        d.trame(frame.t, &frame.samples);
    }
    let mut actions = ecouteur.tick(t);
    let mut finals = Vec::new();
    let etait_ouverte = porte.ouverte();
    let sorties = porte.trame(frame.t, frame.samples);
    // Une parole commence après un silence non transmis : flux neuf si la
    // phrase précédente est conclue (KOB-190).
    if porte.ouverte() && !etait_ouverte {
        stt.nouvel_enonce(kobi_stt::Source::Moi);
        *rattrapage = Rattrapage::default();
    }
    if let (Some(d), true) = (diag.as_mut(), porte.ouverte() != etait_ouverte) {
        d.note(if porte.ouverte() {
            "porte ouverte : parole"
        } else {
            "porte fermée : silence"
        });
    }
    for (debut, samples) in sorties {
        if rattrapage.audio.len() < RATTRAPAGE_MAX {
            rattrapage.audio.extend_from_slice(&samples);
        }
        let frame = kobi_stt::AudioFrame {
            source: kobi_stt::Source::Moi,
            samples,
            t: debut,
        };
        match stt.push(&frame) {
            Ok(segments) => {
                for s in segments {
                    rattrapage.texte |= !s.text.trim().is_empty();
                    if entendu(&s.text) {
                        if let Some(d) = diag {
                            d.note(&format!("écho de Kobi ignoré : « {} »", s.text));
                        }
                        continue;
                    }
                    let suite = ecouteur.segment(&s.text, s.is_final, t);
                    if let Some(d) = diag {
                        d.segment(&s.text, s.is_final, &suite);
                    }
                    actions.extend(suite);
                    if s.is_final {
                        rattrapage.partiel = None;
                        finals.push(s);
                    } else if !s.text.trim().is_empty() {
                        rattrapage.partiel = Some((s.text, s.start));
                    }
                }
            }
            Err(e) => tracing::warn!(erreur = %e, "écoute continue : transcription"),
        }
    }
    // La porte se ferme sur un partiel que le moteur ne conclut pas (plus
    // d'audio jusqu'à la parole suivante), alors que Kobi attend une
    // réponse : il vaut final, tout de suite (KOB-253). Hors carte, une pause
    // au milieu d'une question ne la coupe pas.
    if etait_ouverte && !porte.ouverte() && ecouteur.attend_carte(t) {
        if let Some((texte, debut)) = rattrapage.partiel.take() {
            stt.conclure(kobi_stt::Source::Moi);
            if let Some(d) = diag {
                d.note(&format!("partiel conclu à la fermeture : « {texte} »"));
            }
            let suite = ecouteur.segment(&texte, true, t);
            if let Some(d) = diag {
                d.segment(&texte, true, &suite);
            }
            actions.extend(suite);
            finals.push(kobi_stt::Segment {
                start: debut,
                end: t,
                source: kobi_stt::Source::Moi,
                text: texte,
                is_final: true,
            });
        }
    }
    // La porte se ferme sur une prise de parole restée sans texte, alors que
    // Kobi attend une réponse : un « oui » ou un « non » seul (KOB-197). Ou
    // pendant qu'il parle : un « stop » seul (KOB-248).
    let attend = ecouteur.attend_reponse();
    if etait_ouverte && !porte.ouverte() && !rattrapage.texte && (attend || kobi_parle) {
        let audio = std::mem::take(&mut rattrapage.audio);
        if let Some(texte) = mot_court(&audio).and_then(|m| stt.transcrire_court(m)) {
            let texte = oui_ou_non(&texte)
                .map(str::to_owned)
                .unwrap_or_else(|| sans_repetition(&texte));
            // Pendant la voix, seul « stop » compte : un reste d'écho ne
            // coupe pas Kobi. `entendu` le fait taire.
            let stop = crate::voix::commande_vocale(&texte) == Some(CommandeVoix::Stop);
            if !texte.is_empty() && (attend || stop) && !entendu(&texte) {
                if let Some(d) = diag {
                    d.note(&format!("mot court rattrapé : « {texte} »"));
                }
                if attend {
                    let suite = ecouteur.segment(&texte, true, t);
                    if let Some(d) = diag {
                        d.segment(&texte, true, &suite);
                    }
                    actions.extend(suite);
                }
            }
        }
    }
    (actions, finals)
}

struct Fil<R: Runtime> {
    app: AppHandle<R>,
    stt: Result<NemotronEngine, String>,
    entree: Option<Entree>,
    diag: Option<Diag>,
    porte: Porte,
    ecouteur: Ecouteur,
    rattrapage: Rattrapage,
    en_pause: bool,
    /// Fin de l'audio reçu, en secondes depuis l'ouverture du micro.
    t: f64,
    /// Ouverture du micro, sur l'horloge du système : `t` et la fin des
    /// segments se comptent depuis cet instant (KOB-198).
    origine: Instant,
    /// Charge : audio transcrit et temps passé à le transcrire.
    audio: f64,
    calcul: Duration,
    bilan: Instant,
    /// Dernier retard de la transcription signalé (KOB-250).
    retard: Option<u64>,
    /// Audio reçu, pas encore transcrit : la veille voit le plus récent.
    file: std::collections::VecDeque<AudioFrame>,
    veille: Veille,
    /// Une réunion est ouverte : pas de mode conversation, et la transcription
    /// n'est jamais remise à zéro.
    en_reunion: bool,
    /// Niveau du micro en cours d'accumulation : (somme des carrés, échantillons).
    niveau: (f64, usize),
    /// Dernier partiel envoyé au panneau de mise au point (KOB-50).
    partiel_publie: Option<String>,
    /// Le micro a été perdu et n'est pas encore rouvert (KOB-236).
    micro_perdu: bool,
    /// Commandes reçues pendant une vidange, à appliquer ensuite (KOB-295).
    differees: Vec<Commande>,
    /// Instant réel de la dernière fin de réunion annulable (KOB-287).
    arret_annulable: Option<Instant>,
    /// Ouverture du micro toujours en cours au tour précédent (KOB-285).
    ouverture: Option<Ouverture>,
    /// Instant de la pause de la réunion, tant que le micro reste ouvert
    /// (KOB-277).
    pause_reunion: Option<Instant>,
    /// Décalage (s) des instants du micro ouvert sur l'horloge de l'audio :
    /// un micro rouvert après une panne repart de 0, la session non
    /// (KOB-278).
    decalage: f64,
    /// Dernière trame reçue du micro ouvert (KOB-280).
    derniere_trame: Instant,
}

impl<R: Runtime> Fil<R> {
    fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            stt: charger_stt(),
            entree: None,
            diag: Diag::ouvrir(),
            porte: Porte::new(kobi_stt::SAMPLE_RATE),
            ecouteur: Ecouteur::default(),
            rattrapage: Rattrapage::default(),
            en_pause: false,
            t: 0.0,
            origine: Instant::now(),
            audio: 0.0,
            calcul: Duration::ZERO,
            bilan: Instant::now(),
            retard: None,
            file: Default::default(),
            veille: Veille::default(),
            en_reunion: false,
            niveau: (0.0, 0),
            partiel_publie: None,
            micro_perdu: false,
            differees: Vec::new(),
            arret_annulable: None,
            ouverture: None,
            pause_reunion: None,
            decalage: 0.0,
            derniere_trame: Instant::now(),
        }
    }

    fn note(&mut self, texte: &str) {
        if let Some(d) = &mut self.diag {
            d.note(texte);
        }
    }

    fn boucle(mut self, rx: Receiver<Commande>) {
        if let Err(e) = &self.stt {
            tracing::warn!(erreur = %e, "écoute continue impossible : Nemotron absent");
            let e = e.clone();
            self.note(&format!("Nemotron absent : {e}"));
        }
        loop {
            // Reçues pendant une vidange : appliquées dans l'ordre (KOB-295).
            for commande in std::mem::take(&mut self.differees) {
                self.commande(commande);
            }
            while let Ok(commande) = rx.try_recv() {
                self.commande(commande);
            }
            let session = etat(&self.app).etat;
            let en_reunion = matches!(session, "reunion" | "pause" | "fin");
            if en_reunion != self.en_reunion {
                self.en_reunion = en_reunion;
                self.ecouteur.conversation = !en_reunion;
                if en_reunion {
                    self.ecouteur.oublier();
                }
            }
            // En pause (Kobi ou la réunion), ni micro ni transcription.
            if self.en_pause || session == "pause" || self.stt.is_err() {
                self.fermer_micro(&rx);
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(commande) => self.commande(commande),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            // Micro perdu (KOB-236) : la session continue avec le suivant.
            let rouvert = |fil: &mut Self| {
                if fil.micro_perdu {
                    fil.rouvrir_apres_panne()
                } else {
                    fil.ouvrir_micro()
                }
            };
            if self.entree.is_none() && !rouvert(&mut self) {
                match rx.recv_timeout(REESSAI_MICRO) {
                    Ok(commande) => self.commande(commande),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            self.trame();
        }
    }

    fn ouvrir_micro(&mut self) -> bool {
        // Hors du fil d'écoute et avec une limite de temps : une ouverture
        // bloquée (serveur audio mort ou figé) ne fige plus l'écoute ; elle
        // est reprise au tour suivant, puis relancée (KOB-285).
        let ouverture = match self.ouverture.take() {
            Some(o) if o.depuis.elapsed() < ABANDON_OUVERTURE => o,
            Some(_) => {
                tracing::warn!("écoute continue : ouverture du micro bloquée, relancée");
                self.note("ouverture du micro bloquée : relancée");
                Ouverture::lancer()
            }
            None => Ouverture::lancer(),
        };
        let Some(resultat) = ouverture.attendre(ATTENTE_OUVERTURE) else {
            tracing::warn!(
                attente_s = ouverture.depuis.elapsed().as_secs(),
                "écoute continue : ouverture du micro en attente"
            );
            self.note("ouverture du micro en attente");
            self.ouverture = Some(ouverture);
            return false;
        };
        match resultat {
            // Pause demandée pendant l'attente de l'ouverture : le micro, ouvert
            // trop tard, est refermé aussitôt, et ce qu'il a capté est jeté
            // (KOB-293).
            Ok((entree, _, _)) if self.en_pause || etat(&self.app).etat == "pause" => {
                let jetees = entree.fermer().len();
                tracing::info!(
                    trames = jetees,
                    "écoute continue : micro ouvert pendant la pause, refermé aussitôt"
                );
                self.note("micro ouvert pendant la pause : refermé aussitôt");
                false
            }
            Ok((entree, description, demarre)) => {
                if let Ok(stt) = self.stt.as_mut() {
                    stt.reset();
                }
                self.ecouteur = Ecouteur::new(!self.en_reunion);
                self.porte = Porte::new(kobi_stt::SAMPLE_RATE);
                self.t = 0.0;
                // L'horloge des trames part du démarrage du flux, pas de
                // l'instant où l'écoute relève l'ouverture, jusqu'à 5 s plus
                // tard (KOB-294).
                self.origine = demarre;
                // Réunion terminée micro fermé (pendant la pause) : la
                // fenêtre « Annuler » vaut encore pour le micro rouvert
                // (KOB-287).
                if let Some((depuis, duree)) = self.arret_annulable.and_then(|arret| {
                    fenetre_annulation(arret, self.origine, crate::chef::ANNULATION)
                }) {
                    self.ecouteur.annulable(depuis, duree);
                }
                self.decalage = 0.0;
                self.derniere_trame = Instant::now();
                self.pause_reunion = None;
                self.entree = Some(entree);
                tracing::info!(entree = %description, "écoute continue : micro ouvert");
                if std::mem::take(&mut self.micro_perdu) {
                    let texte =
                        kobi_i18n::tr("coque.ecoute.microRetrouve", &[("micro", &description)]);
                    crate::activite::noter(&self.app, crate::activite::Genre::Erreur, &texte, None);
                    let _ = self.app.emit(EVENEMENT_MICRO, false);
                }
                if let Some(d) = &mut self.diag {
                    d.entree(&description);
                }
                true
            }
            Err(e) => {
                tracing::warn!(erreur = %e, "écoute continue : micro indisponible");
                self.note(&format!("micro indisponible : {e}"));
                false
            }
        }
    }

    /// Rouvre un micro après une panne (KOB-236) : la session continue.
    /// L'écouteur garde son état (carte qui attend « oui », fenêtre
    /// « Annuler », conversation) et l'horloge de l'audio continue celle du
    /// micro perdu (KOB-278).
    fn rouvrir_apres_panne(&mut self) -> bool {
        let ecouteur = std::mem::take(&mut self.ecouteur);
        let origine = self.origine;
        let rouvert = self.ouvrir_micro();
        self.ecouteur = ecouteur;
        if rouvert {
            // `ouvrir_micro` a posé l'origine au démarrage du nouveau flux.
            let demarre = self.origine;
            self.origine = origine;
            self.decalage = reprise_de_l_horloge(origine, demarre);
            self.t = self.decalage;
        }
        rouvert
    }

    fn fermer_micro(&mut self, commandes: &Receiver<Commande>) {
        // Une ouverture encore en attente est lâchée : aboutie pendant la
        // pause, sa capture se ferme d'elle-même au lieu d'enregistrer
        // (KOB-293).
        if self.ouverture.take().is_some() {
            tracing::info!("écoute continue : ouverture du micro en attente abandonnée");
        }
        if let Some(entree) = self.entree.take() {
            let reste = entree.fermer();
            tracing::info!("écoute continue : micro fermé");
            self.note("micro fermé");
            // En réunion, ce qui a été dit avant la pause est transcrit
            // (KOB-261) ; hors réunion, rien n'attend cet audio.
            if self.en_reunion {
                self.file.extend(reste);
                // Sous charge, le micro se ferme après la pause : ce qui a
                // été dit depuis n'est pas transcrit (KOB-277).
                if let Some(pause) = self.pause_reunion.take() {
                    let apres = garder_avant(&mut self.file, self.origine, self.decalage, pause);
                    if apres > 0 {
                        tracing::info!(
                            trames = apres,
                            "écoute continue : audio dit après la pause écarté"
                        );
                    }
                }
                self.vider(commandes);
            }
        }
        crate::chef::avancer(&self.app, kobi_stt::Source::Moi, None);
        self.signaler_retard(0.0);
        self.file.clear();
        self.veille = Veille::default();
    }

    /// Transcrit l'audio resté en file à la fermeture du micro, au plus
    /// pendant [`VIDANGE_FILE`], puis Nemotron conclut la dernière phrase.
    /// Sans effet sur l'écoute : un ordre dit avant la pause n'est plus
    /// exécuté. S'arrête si la réunion reprend entre-temps (KOB-261).
    fn vider(&mut self, commandes: &Receiver<Commande>) {
        if self.stt.is_err() {
            return;
        }
        let debut = Instant::now();
        let a_vider = self.file.len();
        let mut file = std::mem::take(&mut self.file);
        // Les commandes arrivées entre deux trames sont lues tout de suite :
        // l'écoute rouverte (fin de la coupure, raccourci) arrête la vidange,
        // pour rouvrir le micro ; toutes sont appliquées après (KOB-295).
        let transcrites = vidanger(
            &mut file,
            debut + VIDANGE_FILE,
            |fil: &mut Self| {
                fil.differees.extend(commandes.try_iter());
                let en_pause = ecoute_en_pause_apres(fil.en_pause, &fil.differees);
                !vidange_continue(etat(&fil.app).etat, en_pause)
            },
            |fil: &mut Self, frame| fil.traiter(frame, false),
            self,
        );
        // Reprise (ou session close) avant la fin : ce qui reste ne sera pas
        // transcrit, et l'utilisateur le saura (KOB-277).
        let perdues = duree_audio(&file);
        let mut conclue = false;
        if let Ok(stt) = self.stt.as_mut() {
            match stt.finish() {
                Ok(segments) => {
                    for s in segments.into_iter().filter(|s| s.is_final) {
                        if crate::voix::est_ma_voix(&self.app, &s.text) {
                            continue;
                        }
                        conclue = true;
                        crate::transcription_brute::publier(
                            &self.app,
                            kobi_stt::Source::Moi,
                            &s.text,
                            true,
                        );
                        let dit_a = self.origine + Duration::from_secs_f64(s.end.max(0.0));
                        publier(&self.app, Message::Segment(s, dit_a));
                    }
                }
                Err(e) => tracing::warn!(erreur = %e, "écoute continue : fin de la transcription"),
            }
            stt.reset();
        }
        self.rattrapage = Rattrapage::default();
        // Le partiel affiché n'aura pas de final (KOB-274).
        if let Some(ancien) = self.partiel_publie.take().filter(|_| !conclue) {
            crate::transcription_brute::abandonner(&self.app, kobi_stt::Source::Moi, &ancien);
        }
        if a_vider > 0 {
            tracing::info!(
                trames = transcrites,
                ms = debut.elapsed().as_millis() as u64,
                "écoute continue : audio en retard transcrit à la pause"
            );
        }
        if transcrites < a_vider {
            tracing::warn!(
                trames = a_vider - transcrites,
                secondes = perdues as u64,
                "écoute continue : fin de l'audio non transcrite (délai dépassé ou reprise)"
            );
            publier(&self.app, Message::TranscriptionIncomplete(perdues));
        }
    }

    /// Le micro s'est arrêté (débranché, casque coupé) : Kobi le dit, une
    /// fois, et rouvre celui par défaut s'il y en a un ; sinon il réessaie à
    /// intervalle et REC le montre (KOB-236).
    fn micro_coupe(&mut self, cause: &str) {
        tracing::warn!(%cause, "écoute continue : le micro s'est arrêté");
        self.note(&format!("le micro s'est arrêté : {cause}"));
        self.entree = None;
        // L'audio en retard n'est pas transcrit ici, à la différence de la
        // pause (KOB-261) : le micro par défaut se rouvre tout de suite.
        if !self.file.is_empty() {
            tracing::warn!(
                trames = self.file.len(),
                "écoute continue : audio en retard perdu avec le micro"
            );
        }
        self.file.clear();
        crate::chef::avancer(&self.app, kobi_stt::Source::Moi, None);
        if self.micro_perdu {
            return;
        }
        // Dit tout de suite, avant de chercher un autre micro : la
        // réouverture peut attendre le serveur audio (KOB-285). Le micro
        // retrouvé est noté à son ouverture.
        self.micro_perdu = true;
        let _ = self.app.emit(EVENEMENT_MICRO, true);
        let titre = kobi_i18n::tr("coque.ecoute.microPerdu", &[]);
        let message = kobi_i18n::tr("coque.ecoute.microPerduRecherche", &[]);
        // L'erreur affichée entre seule dans l'activité ; la noter en plus
        // la doublait.
        let app = self.app.clone();
        std::thread::spawn(move || erreur(&app, &titre, &message));
        self.rouvrir_apres_panne();
    }

    fn commande(&mut self, commande: Commande) {
        let avant = self.en_pause;
        self.executer_commande(commande);
        // Mis en pause hors réunion : la conversation ne peut plus finir
        // d'elle-même (micro coupé), elle se termine ici (KOB-238).
        if self.en_pause && !avant && !self.en_reunion {
            self.ecouteur = Ecouteur::new(true);
            publier(&self.app, Message::EcouteCoupee);
        }
        if let Some(p) = self.app.try_state::<EnPause>() {
            p.0.store(self.en_pause, Ordering::Relaxed);
        }
    }

    fn executer_commande(&mut self, commande: Commande) {
        match commande {
            Commande::Suite => self.ecouteur.suite(self.t),
            Commande::CartesFermees => self.ecouteur.cartes_fermees(self.t),
            Commande::ReponseAttendue(duree) => {
                self.ecouteur.reponse_attendue(self.t, duree.as_secs_f64())
            }
            Commande::Annulable(arret) => {
                self.arret_annulable = Some(arret);
                self.ecouteur.annulable(
                    instant_audio(self.origine, arret),
                    crate::chef::ANNULATION.as_secs_f64(),
                );
            }
            Commande::BasculerPause => {
                self.en_pause = !self.en_pause;
                tracing::info!(pause = self.en_pause, "écoute continue");
            }
            Commande::Pause(pause) => {
                self.en_pause = pause;
                tracing::info!(pause, "écoute continue (contrôle)");
            }
            Commande::PauseReunion(quand) => self.pause_reunion = Some(quand),
            Commande::RepriseReunion => self.pause_reunion = None,
            Commande::FinAnnulation => self.arret_annulable = None,
            Commande::Raccourci => {
                self.note("raccourci clavier");
                if self.en_pause {
                    self.en_pause = false;
                    tracing::info!(pause = false, "écoute continue : reprise par le raccourci");
                    crate::menu::ecoute_reprise(&self.app);
                }
                if self.stt.is_err() {
                    self.stt = charger_stt();
                }
                let probleme = match &self.stt {
                    Err(e) => Some(e.clone()),
                    Ok(_) => None,
                }
                .or_else(|| {
                    (self.entree.is_none() && !self.ouvrir_micro())
                        .then(|| kobi_i18n::tr("coque.ecoute.microIndisponible", &[]))
                });
                if let Some(message) = probleme {
                    let app = self.app.clone();
                    let titre = kobi_i18n::tr("coque.ecoute.erreurMicro", &[]);
                    std::thread::spawn(move || erreur(&app, &titre, &message));
                    return;
                }
                // Hors réunion, la phrase en cours est oubliée : la suivante est
                // la question. En réunion, rien n'est perdu de la transcription.
                if !self.en_reunion {
                    if let Ok(stt) = self.stt.as_mut() {
                        stt.reset();
                    }
                }
                let actions = self.ecouteur.raccourci(self.t);
                self.executer(actions);
            }
        }
    }

    fn trame(&mut self) {
        let Some(entree) = &self.entree else { return };
        if let Some(panne) = entree.panne() {
            self.micro_coupe(&panne);
            return;
        }
        // Tout ce qui est arrivé entre-temps : la veille voit la parole la
        // plus récente, même quand la transcription est en retard (KOB-250).
        let arrivees = match recevoir(
            entree.frames(),
            self.file.is_empty(),
            &mut self.derniere_trame,
            MICRO_MUET,
        ) {
            Arrivee::Trames(trames) => trames,
            Arrivee::Rien => Vec::new(),
            Arrivee::Ferme => {
                self.micro_coupe("le flux s'est fermé");
                return;
            }
            // Flux arrêté sans erreur (serveur audio arrêté) : même réaction
            // qu'un micro débranché (KOB-280).
            Arrivee::Muet => {
                self.micro_coupe(&format!("aucune trame depuis {} s", MICRO_MUET.as_secs()));
                return;
            }
        };
        for frame in arrivees {
            self.veiller(&frame);
            self.file.push_back(frame);
        }
        // Réunion mise en pause pendant ce tour : rien de plus en direct, la
        // boucle ferme le micro et la vidange transcrit ce qui a été dit
        // avant (KOB-277).
        if self.en_reunion && etat(&self.app).etat == "pause" {
            return;
        }
        let Some(frame) = self.file.pop_front() else {
            return;
        };
        self.traiter(frame, true);
    }

    /// Transcrit une trame et publie ses segments finis ; `en_direct` : en
    /// déduit aussi les actions de l'écoute (faux pendant la vidange de la
    /// pause, KOB-261).
    fn traiter(&mut self, frame: AudioFrame, en_direct: bool) {
        if en_direct {
            self.niveau(&frame.samples);
        }
        let Ok(stt) = self.stt.as_mut() else { return };
        let frame = kobi_stt::AudioFrame {
            source: kobi_stt::Source::Moi,
            samples: frame.samples,
            t: frame.t.as_secs_f64() + self.decalage,
        };
        self.t = frame.end();
        self.audio += frame.samples.len() as f64 / kobi_stt::SAMPLE_RATE as f64;
        let debut = Instant::now();
        let t = self.t;
        // Sa propre voix est ignorée ; une autre voix pendant qu'il parle le
        // fait taire (KOB-142).
        let app = &self.app;
        let annulation = self.ecouteur.en_annulation();
        let kobi_parle = crate::voix::parle(app);
        let entendu = |texte: &str| {
            if crate::voix::est_ma_voix(app, texte)
                && !ordre_d_annulation(texte, annulation, crate::voix::parle(app))
            {
                return true;
            }
            crate::voix::entendu(app, texte);
            false
        };
        let ouverte_avant = self.porte.ouverte();
        let (actions, finals) = transcrire(
            stt,
            &mut self.porte,
            &mut self.ecouteur,
            &mut self.diag,
            &mut self.rattrapage,
            frame,
            t,
            &entendu,
            kobi_parle,
        );
        self.calcul += debut.elapsed();
        // Panneau de mise au point (KOB-50) : le partiel en cours, puis les finaux.
        let partiel = self
            .rattrapage
            .partiel
            .as_ref()
            .map(|(texte, _)| texte.clone());
        // Partiel affiché que le moteur a laissé tomber (phrase reprise après
        // un silence, KOB-190) : il n'aura pas de final, la ligne se fige
        // (KOB-274).
        let nouvel_enonce = !ouverte_avant && self.porte.ouverte();
        if partiel_abandonne(partiel.is_some(), !finals.is_empty(), nouvel_enonce) {
            if let Some(ancien) = self.partiel_publie.take() {
                crate::transcription_brute::abandonner(&self.app, kobi_stt::Source::Moi, &ancien);
            }
        }
        if partiel.is_some() && partiel != self.partiel_publie {
            crate::transcription_brute::publier(
                &self.app,
                kobi_stt::Source::Moi,
                partiel.as_deref().unwrap_or_default(),
                false,
            );
        }
        self.partiel_publie = partiel;
        for s in &finals {
            crate::transcription_brute::publier(&self.app, kobi_stt::Source::Moi, &s.text, true);
        }
        for s in finals {
            let dit_a = self.origine + Duration::from_secs_f64(s.end.max(0.0));
            publier(&self.app, Message::Segment(s, dit_a));
        }
        // Tout l'audio reçu jusqu'ici est transcrit.
        let jusqu_a = self.origine + Duration::from_secs_f64(self.t.max(0.0));
        crate::chef::avancer(&self.app, kobi_stt::Source::Moi, Some(jusqu_a));
        if en_direct {
            self.executer(actions);
            // Sous charge, la transcription peut aller moins vite que la
            // parole : un ordre dit maintenant ne serait entendu qu'après
            // (KOB-250).
            let retard = self.origine.elapsed().as_secs_f64() - self.t;
            self.signaler_retard(retard);
            // Nemotron colle les phrases dès quelques secondes de retard,
            // bien avant le signal de KOB-250 (15 s) (KOB-289, KOB-301).
            self.ecouteur.sous_charge = retard >= RETARD_PHRASES_COLLEES;
        }

        if self.bilan.elapsed() >= BILAN {
            tracing::info!(
                audio_s = self.audio as u64,
                calcul_s = self.calcul.as_secs_f64() as u64,
                rtf = format!("{:.3}", self.calcul.as_secs_f64() / self.audio.max(1.0)),
                "écoute continue : charge"
            );
            self.audio = 0.0;
            self.calcul = Duration::ZERO;
            self.bilan = Instant::now();
        }
    }

    /// Écoute anticipée (KOB-250) : en réunion, quand la transcription est en
    /// retard, le début de chaque phrase qui vient d'arriver est transcrit à
    /// part ; un ordre de fin de réunion part tout de suite.
    fn veiller(&mut self, frame: &AudioFrame) {
        if !self.en_reunion || self.retard.is_none() {
            if self.veille.porte.is_some() {
                self.veille = Veille {
                    ordres: std::mem::take(&mut self.veille.ordres),
                    ..Veille::default()
                };
            }
            return;
        }
        let v = &mut self.veille;
        let porte = v
            .porte
            .get_or_insert_with(|| Porte::new(kobi_stt::SAMPLE_RATE));
        for (debut, morceau) in
            porte.trame(frame.t.as_secs_f64() + self.decalage, frame.samples.clone())
        {
            if !v.faite {
                v.debut.get_or_insert(debut);
                v.extrait.extend(morceau);
            }
        }
        let ouverte = porte.ouverte();
        let pret = v.extrait.len() as f64 >= EXTRAIT_ANTICIPE * kobi_stt::SAMPLE_RATE as f64;
        let rassise = v.debut.is_some_and(|d| {
            self.origine.elapsed().as_secs_f64() - d > FRAICHEUR_ANTICIPEE + EXTRAIT_ANTICIPE
        });
        if !v.faite && rassise {
            // Trop ancienne : la transcription ordinaire s'en chargera.
            v.faite = true;
        }
        // Réunion mise en pause : plus d'extrait anticipé, qui coûte cher
        // sous charge et retarderait la fermeture du micro (KOB-277).
        let en_cours = || etat(&self.app).etat == "reunion";
        if !v.faite && (pret || (!ouverte && !v.extrait.is_empty())) && en_cours() {
            let v = &mut self.veille;
            v.faite = true;
            let debut = v.debut.unwrap_or_default();
            let texte = self
                .stt
                .as_mut()
                .ok()
                .and_then(|stt| stt.transcrire_extrait(&self.veille.extrait));
            if texte.as_deref().is_some_and(ordre_de_fin) {
                tracing::info!(
                    retard_s = self.retard.unwrap_or_default(),
                    "écoute anticipée : fin de réunion entendue avant son tour"
                );
                self.veille.ordres.push(debut);
                publier(&self.app, Message::Commande(Intention::TerminerReunion));
            }
        }
        let v = &mut self.veille;
        if !v.porte.as_ref().is_some_and(Porte::ouverte) {
            // Phrase finie : la suivante sera écoutée à son tour.
            v.extrait.clear();
            v.debut = None;
            v.faite = false;
        }
    }

    /// La transcription arrive à un ordre de fin déjà exécuté à l'avance
    /// (même phrase, à quelques secondes près) : il ne repart pas.
    fn deja_execute(&mut self, question: &str) -> bool {
        let t = self.t;
        let Some(i) = self
            .veille
            .ordres
            .iter()
            .position(|&d| (d - 2.0..=d + 20.0).contains(&t))
        else {
            return false;
        };
        if !ordre_de_fin(&format!("Kobi, {question}")) && !ordre_de_fin(question) {
            return false;
        }
        self.veille.ordres.remove(i);
        tracing::info!("écoute anticipée : ordre de fin déjà exécuté, pas rejoué");
        true
    }

    /// Signale le retard de la transcription quand il franchit un palier, et
    /// son rattrapage.
    fn signaler_retard(&mut self, retard: f64) {
        let Some(nouveau) = signal_retard(retard, self.retard) else {
            return;
        };
        self.retard = nouveau;
        let charge = nouveau.map(|secondes| {
            tracing::warn!(
                secondes,
                "écoute continue : la transcription est en retard sur la parole"
            );
            Retard {
                secondes,
                raccourci: raccourci_enregistrement(),
            }
        });
        if charge.is_none() {
            tracing::info!("écoute continue : la transcription a rattrapé son retard");
        }
        let _ = self.app.emit(EVENEMENT_RETARD, charge);
    }

    /// Niveau du micro, 20 fois par seconde, seulement quand Kobi écoute.
    fn niveau(&mut self, samples: &[f32]) {
        self.niveau.0 += samples
            .iter()
            .map(|&x| f64::from(x) * f64::from(x))
            .sum::<f64>();
        self.niveau.1 += samples.len();
        if (self.niveau.1 as f64) < PERIODE_NIVEAU * kobi_stt::SAMPLE_RATE as f64 {
            return;
        }
        let rms = (self.niveau.0 / self.niveau.1 as f64).sqrt();
        self.niveau = (0.0, 0);
        if etat(&self.app).mascotte == "listen" {
            let _ = self.app.emit(EVENEMENT_NIVEAU, niveau_normalise(rms));
        }
    }

    fn executer(&mut self, actions: Vec<Action>) {
        let actions = sans_ordres_deja_executes(actions, |q| self.deja_execute(q));
        for action in actions {
            let message = match action {
                Action::Interpeller => {
                    // On appelle Kobi : il se tait pour écouter (KOB-142).
                    crate::voix::taire(&self.app, "interpellation");
                    tracing::info!("Kobi interpellé");
                    Message::Interpelle
                }
                Action::Question(q) => Message::QuestionPartielle(q),
                Action::Envoyer(q) => {
                    tracing::info!(longueur = q.len(), "question transcrite");
                    Message::Question {
                        texte: q,
                        voix: true,
                    }
                }
                Action::EnvoyerPeutEtre(q) => {
                    tracing::info!(
                        longueur = q.len(),
                        "phrase qui cite Kobi, peut-être pour lui"
                    );
                    Message::QuestionPeutEtre(q)
                }
                Action::Abandonner => {
                    tracing::info!("interpellation sans question");
                    Message::SansQuestion
                }
            };
            publier(&self.app, message);
        }
    }
}

/// Le partiel déjà affiché est-il abandonné (KOB-274) ? Sans final dans ce
/// tour, il a disparu, ou la phrase a repris après un silence (nouvel
/// énoncé : le partiel actuel est celui de la nouvelle phrase).
fn partiel_abandonne(partiel: bool, final_publie: bool, nouvel_enonce: bool) -> bool {
    !final_publie && (!partiel || nouvel_enonce)
}

/// Horloge de l'audio d'un micro rouvert en pleine session (KOB-278) : elle
/// reprend au temps écoulé depuis l'ouverture du premier, pour que les
/// délais de l'écouteur (carte, « Annuler », conversation) gardent leur sens.
fn reprise_de_l_horloge(origine: Instant, maintenant: Instant) -> f64 {
    maintenant.saturating_duration_since(origine).as_secs_f64()
}

/// Fenêtre « Annuler » d'une fin à l'instant `arret`, sur l'horloge d'un
/// micro ouvert à `origine` (KOB-287) : `(depuis, duree)` en secondes
/// d'audio, ou `None` si elle est déjà close à l'ouverture.
fn fenetre_annulation(
    arret: Instant,
    origine: Instant,
    annulation: Duration,
) -> Option<(f64, f64)> {
    let fin = arret + annulation;
    let reste = fin.saturating_duration_since(arret.max(origine));
    (!reste.is_zero()).then(|| (instant_audio(origine, arret), reste.as_secs_f64()))
}

/// Instant `quand` dans le temps de l'audio du micro ouvert à `origine`.
fn instant_audio(origine: Instant, quand: Instant) -> f64 {
    quand.saturating_duration_since(origine).as_secs_f64()
}

/// Retire les ordres de fin déjà exécutés par l'écoute anticipée (KOB-250),
/// et avec eux l'interpellation qui les annonce dans le même lot : Kobi ne
/// passe pas en écoute pour rien (KOB-265). Annoncé plus tôt, l'appel est
/// clos sans question.
fn sans_ordres_deja_executes(
    actions: Vec<Action>,
    mut deja_execute: impl FnMut(&str) -> bool,
) -> Vec<Action> {
    let mut gardees = Vec::with_capacity(actions.len());
    for action in actions {
        if let Action::Envoyer(q) = &action {
            if deja_execute(q) {
                match gardees.iter().rposition(|a| *a == Action::Interpeller) {
                    Some(i) => gardees.truncate(i),
                    None => gardees.push(Action::Abandonner),
                }
                continue;
            }
        }
        gardees.push(action);
    }
    gardees
}

/// « Reprends » juste après un arrêt (KOB-165) : Kobi vient de dire « Dis
/// « reprends » si c'était une erreur », et le filtre d'écho prendrait la
/// réponse pour sa propre voix. Une fois qu'il s'est tu, un ordre bref qui
/// annule l'arrêt passe ; l'écho de la phrase entière, plus longue, reste
/// ignoré.
fn ordre_d_annulation(texte: &str, annulation: bool, kobi_parle: bool) -> bool {
    annulation
        && !kobi_parle
        && texte.split_whitespace().count() <= 2
        && crate::intentions::annule_la_fin(texte)
}

/// Niveau RMS ramené entre 0 et 1 : -60 dBFS (silence) à -10 dBFS (voix forte).
pub fn niveau_normalise(rms: f64) -> f32 {
    let db = 20.0 * rms.max(1e-10).log10();
    ((db + 60.0) / 50.0).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpellation::modele_nemotron;
    use std::path::Path;

    /// KOB-265 : l'ordre de fin déjà exécuté à l'avance ne repart pas, ni
    /// l'interpellation qui l'annonce dans le même lot ; annoncée plus tôt,
    /// elle est close sans question. Les autres questions passent.
    #[test]
    fn ordre_deja_execute_sans_interpellation() {
        let deja = |q: &str| q == "termine la réunion";
        let fin = || Action::Envoyer("termine la réunion".into());
        assert_eq!(
            sans_ordres_deja_executes(
                vec![
                    Action::Interpeller,
                    Action::Question("termine".into()),
                    fin()
                ],
                deja
            ),
            []
        );
        assert_eq!(
            sans_ordres_deja_executes(vec![fin()], deja),
            [Action::Abandonner]
        );
        let autre = vec![
            Action::Interpeller,
            Action::Envoyer("quelle heure ?".into()),
        ];
        assert_eq!(sans_ordres_deja_executes(autre.clone(), deja), autre);
    }

    /// KOB-274 : un partiel affiché sans final se fige quand il disparaît ou
    /// qu'une nouvelle phrase commence ; pas quand il s'affine, ni quand un
    /// final le conclut.
    #[test]
    fn partiel_affiche_abandonne() {
        assert!(partiel_abandonne(false, false, false), "disparu sans final");
        assert!(partiel_abandonne(true, false, true), "nouvelle phrase");
        assert!(!partiel_abandonne(true, false, false), "il s'affine");
        assert!(
            !partiel_abandonne(false, true, false),
            "conclu par un final"
        );
        assert!(
            !partiel_abandonne(true, true, true),
            "conclu, puis nouvelle phrase"
        );
    }

    /// KOB-278 : la carte attend « oui » depuis 10 s quand le micro tombe à
    /// 12 s ; le suivant s'ouvre à 13 s et ses trames repartent de 0. Avec
    /// l'écouteur gardé et l'horloge reprise, le « oui » sans le nom répond
    /// encore à la carte ; un écouteur neuf (avant) l'ignorait.
    #[test]
    fn carte_entendue_apres_micro_rouvert() {
        let origine = Instant::now();
        let decalage = reprise_de_l_horloge(origine, origine + Duration::from_secs(13));
        assert_eq!(decalage, 13.0);
        assert_eq!(
            reprise_de_l_horloge(origine + Duration::from_secs(1), origine),
            0.0
        );
        let mut e = Ecouteur::default();
        e.reponse_attendue(10.0, 60.0);
        let t = 0.8 + decalage;
        assert!(e.tick(t).is_empty());
        assert!(e.attend_carte(t));
        let actions = e.segment("Oui", true, t);
        assert!(
            actions.contains(&Action::Envoyer("Oui".into())),
            "{actions:?}"
        );
        let mut neuf = Ecouteur::new(true);
        assert!(!neuf
            .segment("Oui", true, 0.8)
            .contains(&Action::Envoyer("Oui".into())));
    }

    /// KOB-287 : réunion terminée pendant la pause, micro fermé ; à sa
    /// réouverture, la fenêtre « Annuler » court encore jusqu'à la même
    /// échéance ; ouverte après l'échéance, plus rien.
    #[test]
    fn fenetre_annuler_au_micro_rouvert() {
        let arret = Instant::now();
        let annulation = Duration::from_secs(10);
        // Micro rouvert 4 s après l'arrêt : 6 s restent, depuis le début.
        let (depuis, duree) =
            fenetre_annulation(arret, arret + Duration::from_secs(4), annulation).unwrap();
        assert_eq!(depuis, 0.0);
        assert!((duree - 6.0).abs() < 1e-9);
        // Micro ouvert avant l'arrêt (cas ordinaire) : 10 s, depuis l'arrêt.
        let (depuis, duree) =
            fenetre_annulation(arret, arret - Duration::from_secs(30), annulation).unwrap();
        assert!((depuis - 30.0).abs() < 1e-9 && (duree - 10.0).abs() < 1e-9);
        assert_eq!(
            fenetre_annulation(arret, arret + Duration::from_secs(12), annulation),
            None
        );
        // « reprends » sans le nom, dit 2 s après la réouverture, part.
        let mut e = Ecouteur::new(false);
        let (depuis, duree) =
            fenetre_annulation(arret, arret + Duration::from_secs(4), annulation).unwrap();
        e.annulable(depuis, duree);
        assert_eq!(
            e.segment("reprends", true, 2.0),
            [Action::Interpeller, Action::Envoyer("reprends".into())]
        );
    }

    #[test]
    fn instant_audio_de_l_arret() {
        let origine = Instant::now();
        assert_eq!(
            instant_audio(origine, origine + Duration::from_secs(42)),
            42.0
        );
        // Arrêt avant l'ouverture du micro : début de l'audio.
        assert_eq!(
            instant_audio(origine + Duration::from_secs(5), origine),
            0.0
        );
    }

    /// KOB-250 : seuls les ordres de fin de réunion adressés à Kobi partent
    /// en avance ; une mention ou une autre question attend son tour.
    #[test]
    fn ordres_de_fin_anticipes() {
        for oui in [
            "Kobi, arrête l'enregistrement.",
            "Kobi, la réunion est terminée",
            "Koby, fin de la réunion",
        ] {
            assert!(ordre_de_fin(oui), "{oui}");
        }
        for non in [
            "on arrête l'enregistrement du client demain",
            "Kobi, quelle heure est-il ?",
            "Kobi, mets en pause",
            "la réunion est terminée pour eux",
        ] {
            assert!(!ordre_de_fin(non), "{non}");
        }
    }

    /// KOB-250 : le retard est signalé au-delà du seuil, puis à chaque
    /// palier, et son rattrapage une seule fois.
    #[test]
    fn retard_signale_par_paliers() {
        assert_eq!(signal_retard(3.0, None), None);
        assert_eq!(signal_retard(16.0, None), Some(Some(16)));
        assert_eq!(signal_retard(30.0, Some(16)), None);
        assert_eq!(signal_retard(47.0, Some(16)), Some(Some(47)));
        assert_eq!(signal_retard(10.0, Some(47)), None, "pas encore rattrapé");
        assert_eq!(signal_retard(2.0, Some(47)), Some(None));
        assert_eq!(signal_retard(2.0, None), None);
    }

    /// « Reprends » après un arrêt n'est pas pris pour l'écho de « Dis
    /// « reprends » si c'était une erreur » (essai audio injecté, KOB-165).
    #[test]
    fn reprends_n_est_pas_un_echo() {
        assert!(ordre_d_annulation("Reprends.", true, false));
        assert!(ordre_d_annulation("non, continue", true, false));
        // Kobi parle encore, ou rien à annuler : filtre d'écho ordinaire.
        assert!(!ordre_d_annulation("Reprends.", true, true));
        assert!(!ordre_d_annulation("Reprends.", false, false));
        // L'écho de sa phrase, plus longue, reste un écho.
        assert!(!ordre_d_annulation(
            "Dis reprends si c'était une erreur",
            true,
            false
        ));
        assert!(!ordre_d_annulation("une erreur", true, false));
    }

    #[test]
    fn niveau_entre_zero_et_un() {
        assert_eq!(niveau_normalise(0.0), 0.0);
        assert_eq!(niveau_normalise(1.0), 1.0);
        let parole = niveau_normalise(0.03);
        assert!(parole > 0.4 && parole < 0.6, "{parole}");
    }

    /// Rejoue un WAV dans Nemotron et l'écoute comme le fil le fait avec le
    /// micro (trames de 10 ms, puis 3 s de silence). Renvoie les actions et
    /// les segments finals.
    fn rejouer(
        stt: &mut NemotronEngine,
        diag: &mut Option<Diag>,
        wav: &Path,
    ) -> (Vec<Action>, Vec<String>) {
        let samples = kobi_stt::wav::read_wav_16k_mono(wav).unwrap();
        stt.reset();
        let mut ecouteur = Ecouteur::default();
        let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
        let mut rattrapage = Rattrapage::default();
        let (mut actions, mut finals) = (Vec::new(), Vec::new());
        let silence = vec![0.0; 48_000];
        // Trames de 10 ms ; `KOBI_TRAME` (échantillons) pour celles du micro.
        let n = std::env::var("KOBI_TRAME")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(160);
        for (i, morceau) in samples.chunks(n).chain(silence.chunks(n)).enumerate() {
            let frame = kobi_stt::AudioFrame {
                source: kobi_stt::Source::Moi,
                samples: morceau.to_vec(),
                t: (i * n) as f64 / kobi_stt::SAMPLE_RATE as f64,
            };
            let t = frame.end();
            let (suite, _) = transcrire(
                stt,
                &mut porte,
                &mut ecouteur,
                diag,
                &mut rattrapage,
                frame,
                t,
                &|_| false,
                false,
            );
            if let Some(d) = diag {
                finals.extend(d.finals());
            }
            actions.extend(suite);
        }
        (actions, finals)
    }

    /// KOB-197 : « oui » ou « non » seul ne sort pas du flux ; quand Kobi
    /// attend une réponse, la prise de parole courte est rattrapée à part.
    #[test]
    fn oui_seul_rattrape_quand_kobi_attend() {
        #[derive(Default)]
        struct Muet {
            appels: usize,
        }
        impl SttEngine for Muet {
            fn name(&self) -> &str {
                "muet"
            }
            fn push(
                &mut self,
                _frame: &kobi_stt::AudioFrame,
            ) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                Ok(Vec::new())
            }
            fn finish(&mut self) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                Ok(Vec::new())
            }
            fn transcrire_court(&mut self, _samples: &[f32]) -> Option<String> {
                self.appels += 1;
                Some("Oui, oui.".into())
            }
        }
        // 1 s de silence, `parole` trames de 100 ms de voix, 4 s de silence.
        let jouer = |ecouteur: &mut Ecouteur, parole: usize| {
            let mut stt = Muet::default();
            let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
            let mut rattrapage = Rattrapage::default();
            let voix: Vec<f32> = (0..1600).map(|i| 0.2 * (i as f32 * 0.3).sin()).collect();
            let (mut t, mut actions) = (0.0, Vec::new());
            for (n, fort) in [(10, false), (parole, true), (40, false)] {
                for _ in 0..n {
                    let frame = kobi_stt::AudioFrame {
                        source: kobi_stt::Source::Moi,
                        samples: if fort { voix.clone() } else { vec![0.0; 1600] },
                        t,
                    };
                    t += 0.1;
                    let (a, _) = transcrire(
                        &mut stt,
                        &mut porte,
                        ecouteur,
                        &mut None,
                        &mut rattrapage,
                        frame,
                        t,
                        &|_| false,
                        false,
                    );
                    actions.extend(a);
                }
            }
            (stt.appels, actions)
        };
        // Carte ouverte : le « oui » rattrapé part comme réponse, une fois.
        let mut e = Ecouteur::default();
        e.reponse_attendue(0.0, 60.0);
        let (appels, actions) = jouer(&mut e, 4);
        assert_eq!(appels, 1);
        assert!(
            actions.contains(&Action::Envoyer("Oui".into())),
            "{actions:?}"
        );
        // Personne n'attend de réponse : rien n'est rattrapé.
        let (appels, actions) = jouer(&mut Ecouteur::default(), 4);
        assert_eq!((appels, actions.len()), (0, 0), "{actions:?}");
        // Une longue prise de parole n'est pas un « oui ».
        let mut e = Ecouteur::default();
        e.reponse_attendue(0.0, 60.0);
        assert_eq!(jouer(&mut e, 25).0, 0);
    }

    /// KOB-253 : le moteur ne sort qu'un partiel « Non », jamais conclu
    /// (plus d'audio après la fermeture de la porte). Carte ouverte, il part
    /// à la fermeture, une fois, et le flux repart à neuf ; sans carte, il
    /// attend la suite.
    #[test]
    fn partiel_conclu_quand_la_porte_se_ferme_sur_une_carte() {
        #[derive(Default)]
        struct Partiel {
            voix: usize,
            dit: bool,
            conclus: usize,
        }
        impl SttEngine for Partiel {
            fn name(&self) -> &str {
                "partiel"
            }
            fn push(
                &mut self,
                frame: &kobi_stt::AudioFrame,
            ) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                if frame.samples.iter().any(|x| x.abs() > 0.01) {
                    self.voix += 1;
                }
                if self.voix < 3 || self.dit {
                    return Ok(Vec::new());
                }
                self.dit = true;
                Ok(vec![kobi_stt::Segment {
                    start: frame.t,
                    end: frame.end(),
                    source: kobi_stt::Source::Moi,
                    text: "Non".into(),
                    is_final: false,
                }])
            }
            fn finish(&mut self) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                Ok(Vec::new())
            }
            fn conclure(&mut self, _source: kobi_stt::Source) {
                self.conclus += 1;
            }
        }
        // 1 s de silence, 0,5 s de voix, 4 s de silence.
        let jouer = |ecouteur: &mut Ecouteur| {
            let mut stt = Partiel::default();
            let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
            let mut rattrapage = Rattrapage::default();
            let voix: Vec<f32> = (0..1600).map(|i| 0.2 * (i as f32 * 0.3).sin()).collect();
            let (mut t, mut actions, mut finals) = (0.0, Vec::new(), Vec::new());
            for (n, fort) in [(10, false), (5, true), (40, false)] {
                for _ in 0..n {
                    let frame = kobi_stt::AudioFrame {
                        source: kobi_stt::Source::Moi,
                        samples: if fort { voix.clone() } else { vec![0.0; 1600] },
                        t,
                    };
                    t += 0.1;
                    let (a, f) = transcrire(
                        &mut stt,
                        &mut porte,
                        ecouteur,
                        &mut None,
                        &mut rattrapage,
                        frame,
                        t,
                        &|_| false,
                        false,
                    );
                    actions.extend(a);
                    finals.extend(f);
                }
            }
            (stt.conclus, actions, finals)
        };
        let mut e = Ecouteur::default();
        e.reponse_attendue(0.0, 60.0);
        let (conclus, actions, finals) = jouer(&mut e);
        assert_eq!(conclus, 1);
        let envois: Vec<_> = actions
            .iter()
            .filter(|a| matches!(a, Action::Envoyer(_)))
            .collect();
        assert_eq!(envois, [&Action::Envoyer("Non".into())], "{actions:?}");
        assert_eq!(finals.len(), 1);
        assert_eq!(finals[0].text, "Non");
        // Sans carte : le partiel n'est pas tranché à la fermeture.
        let (conclus, actions, finals) = jouer(&mut Ecouteur::default());
        assert_eq!(conclus, 0);
        assert!(
            !actions.iter().any(|a| matches!(a, Action::Envoyer(_))),
            "{actions:?}"
        );
        assert!(finals.is_empty());
        // Fenêtre de suite d'une conversation, sans carte : une question de
        // suite coupée par une pause n'est pas tranchée non plus (revue
        // KOB-225).
        let mut e = Ecouteur::default();
        e.suite(0.0);
        let (conclus, actions, _) = jouer(&mut e);
        assert_eq!(conclus, 0);
        assert!(
            !actions.iter().any(|a| matches!(a, Action::Envoyer(_))),
            "{actions:?}"
        );
    }

    /// KOB-248 : pendant la voix de Kobi, un « stop » dit seul, que le moteur
    /// ne sort pas, est rattrapé et le fait taire (`entendu`) ; un autre mot
    /// court (reste d'écho) ne compte pas.
    #[test]
    fn stop_seul_rattrape_pendant_la_voix() {
        struct Court(&'static str);
        impl SttEngine for Court {
            fn name(&self) -> &str {
                "court"
            }
            fn push(
                &mut self,
                _frame: &kobi_stt::AudioFrame,
            ) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                Ok(Vec::new())
            }
            fn finish(&mut self) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                Ok(Vec::new())
            }
            fn transcrire_court(&mut self, _samples: &[f32]) -> Option<String> {
                Some(self.0.into())
            }
        }
        // 1 s de silence, 0,4 s de voix, 4 s de silence, Kobi parlant.
        let jouer = |mot: &'static str| {
            let mut stt = Court(mot);
            let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
            let mut rattrapage = Rattrapage::default();
            let mut ecouteur = Ecouteur::default();
            let entendus = std::cell::RefCell::new(Vec::new());
            let entendu = |texte: &str| {
                entendus.borrow_mut().push(texte.to_owned());
                false
            };
            let voix: Vec<f32> = (0..1600).map(|i| 0.2 * (i as f32 * 0.3).sin()).collect();
            let (mut t, mut actions) = (0.0, Vec::new());
            for (n, fort) in [(10, false), (4, true), (40, false)] {
                for _ in 0..n {
                    let frame = kobi_stt::AudioFrame {
                        source: kobi_stt::Source::Moi,
                        samples: if fort { voix.clone() } else { vec![0.0; 1600] },
                        t,
                    };
                    t += 0.1;
                    let (a, _) = transcrire(
                        &mut stt,
                        &mut porte,
                        &mut ecouteur,
                        &mut None,
                        &mut rattrapage,
                        frame,
                        t,
                        &entendu,
                        true,
                    );
                    actions.extend(a);
                }
            }
            (entendus.into_inner(), actions)
        };
        let (entendus, actions) = jouer("Stop, stop.");
        assert_eq!(entendus, ["Stop"]);
        assert!(actions.is_empty(), "{actions:?}");
        let (entendus, actions) = jouer("Le, le.");
        assert!(entendus.is_empty(), "{entendus:?}");
        assert!(actions.is_empty(), "{actions:?}");
    }

    #[test]
    fn mot_redouble_ramene_a_un_seul() {
        assert_eq!(sans_repetition("Oui, oui."), "Oui");
        assert_eq!(sans_repetition("Non, non, non"), "Non");
        assert_eq!(sans_repetition("Oui, vas-y"), "Oui, vas-y");
        // Graphies proches relevées sur les essais de KOB-197.
        assert_eq!(oui_ou_non("Uui, uii, ui"), Some("Oui"));
        assert_eq!(oui_ou_non("No, no"), Some("Non"));
        assert_eq!(oui_ou_non("Oui, non"), None);
        assert_eq!(oui_ou_non("D'accord"), None);
    }

    /// KOB-190 : chaque parole qui suit un silence (porte rouverte) repart
    /// d'un énoncé neuf du moteur ; jamais au milieu d'une parole.
    #[test]
    fn nouvel_enonce_a_chaque_parole() {
        #[derive(Default)]
        struct Faux {
            enonces: Vec<f64>,
            pos: f64,
        }
        impl SttEngine for Faux {
            fn name(&self) -> &str {
                "faux"
            }
            fn push(
                &mut self,
                frame: &kobi_stt::AudioFrame,
            ) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                self.pos = frame.end();
                Ok(Vec::new())
            }
            fn finish(&mut self) -> kobi_stt::Result<Vec<kobi_stt::Segment>> {
                Ok(Vec::new())
            }
            fn nouvel_enonce(&mut self, _source: kobi_stt::Source) {
                self.enonces.push(self.pos);
            }
        }
        let mut stt = Faux::default();
        let mut porte = Porte::new(kobi_stt::SAMPLE_RATE);
        let mut ecouteur = Ecouteur::default();
        // 1 s de silence, 0,6 s de parole, 4 s de silence, 0,6 s de parole,
        // 3 s de silence ; trames de 100 ms, comme le micro.
        let parole: Vec<f32> = (0..1600).map(|i| 0.2 * (i as f32 * 0.3).sin()).collect();
        let silence = vec![0.0f32; 1600];
        let plan = [(10, false), (6, true), (40, false), (6, true), (30, false)];
        let mut t = 0.0;
        for (n, fort) in plan {
            for _ in 0..n {
                let frame = kobi_stt::AudioFrame {
                    source: kobi_stt::Source::Moi,
                    samples: if fort {
                        parole.clone()
                    } else {
                        silence.clone()
                    },
                    t,
                };
                t += 0.1;
                transcrire(
                    &mut stt,
                    &mut porte,
                    &mut ecouteur,
                    &mut None,
                    &mut Rattrapage::default(),
                    frame,
                    t,
                    &|_| false,
                    false,
                );
            }
        }
        assert_eq!(stt.enonces.len(), 2, "{:?}", stt.enonces);
    }

    /// Un WAV : `KOBI_ECOUTE_WAV=<fichier.wav> cargo test -p kobi-app
    /// --release -- --ignored wav_injecte --nocapture` ;
    /// `KOBI_ECOUTE_ATTENDU=rien` pour un fichier qui ne doit rien déclencher.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_WAV et le modèle Nemotron"]
    fn wav_injecte() {
        assert!(modele_nemotron().is_some(), "modèle Nemotron absent");
        let wav = PathBuf::from(std::env::var_os("KOBI_ECOUTE_WAV").expect("KOBI_ECOUTE_WAV"));
        let mut stt = charger_stt().unwrap();
        let (actions, finals) = rejouer(&mut stt, &mut Diag::ouvrir(), &wav);
        println!("{finals:#?}\n{actions:#?}");
        let envoyees = actions
            .iter()
            .filter(|a| matches!(a, Action::Envoyer(_)))
            .count();
        match std::env::var("KOBI_ECOUTE_ATTENDU").as_deref() {
            Ok("rien") => assert_eq!(envoyees, 0),
            _ => assert!(envoyees > 0, "aucune question envoyée"),
        }
    }

    /// Les transcriptions déjà faites par [`corpus`] (`*.txt`), rejouées
    /// comme phrases finales : mesure d'une règle de repérage sans Nemotron
    /// (KOB-179). `KOBI_ECOUTE_CORPUS=<dossier> cargo test -p kobi-app --
    /// --ignored corpus_texte --nocapture`.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_CORPUS (transcriptions de corpus)"]
    fn corpus_texte() {
        let dir =
            PathBuf::from(std::env::var_os("KOBI_ECOUTE_CORPUS").expect("KOBI_ECOUTE_CORPUS"));
        let mut fichiers: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "txt"))
            .collect();
        fichiers.sort();
        let (mut pos, mut pos_ok, mut fausses) = (0, 0, 0);
        for f in &fichiers {
            let nom = f.file_name().unwrap().to_string_lossy().into_owned();
            let mut ecouteur = Ecouteur::default();
            let actions: Vec<Action> = std::fs::read_to_string(f)
                .unwrap()
                .lines()
                .enumerate()
                .flat_map(|(i, l)| ecouteur.segment(l, true, i as f64 * 5.0))
                .collect();
            let envoyee = actions.iter().any(|a| matches!(a, Action::Envoyer(_)));
            if nom.starts_with("pos") {
                pos += 1;
                pos_ok += usize::from(envoyee);
                if !envoyee {
                    println!("MANQUÉ {nom}");
                }
            } else if actions.contains(&Action::Interpeller) {
                fausses += 1;
                println!("FAUSSE {nom}");
            }
        }
        println!("BILAN texte : positifs {pos_ok}/{pos}, fausses interpellations {fausses}");
    }

    /// Un corpus : `pos*.wav` doivent envoyer une question, `neg*.wav` ne
    /// rien déclencher. `KOBI_ECOUTE_CORPUS=<dossier> cargo test -p kobi-app
    /// --release -- --ignored corpus --nocapture`.
    #[test]
    #[ignore = "demande KOBI_ECOUTE_CORPUS et le modèle Nemotron"]
    fn corpus() {
        let dir =
            PathBuf::from(std::env::var_os("KOBI_ECOUTE_CORPUS").expect("KOBI_ECOUTE_CORPUS"));
        let mut stt = charger_stt().unwrap();
        let mut fichiers: Vec<PathBuf> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "wav"))
            .collect();
        fichiers.sort();
        let (mut pos, mut pos_ok, mut fausses, mut envois) = (0, 0, 0, 0);
        // Temps de calcul et durée d'audio : le coût d'un réglage (KOB-179).
        let (mut calcul, mut audio) = (std::time::Duration::ZERO, 0.0);
        let mut manques = Vec::new();
        for wav in &fichiers {
            let nom = wav.file_name().unwrap().to_string_lossy().into_owned();
            audio += kobi_stt::wav::read_wav_16k_mono(wav).unwrap().len() as f64
                / kobi_stt::SAMPLE_RATE as f64;
            let debut = std::time::Instant::now();
            let (actions, finals) = rejouer(&mut stt, &mut Some(Diag::memoire()), wav);
            calcul += debut.elapsed();
            // Transcription complète, pour juger de sa qualité.
            std::fs::write(wav.with_extension("txt"), finals.join("\n")).unwrap();
            let envoyees: Vec<_> = actions
                .iter()
                .filter_map(|a| match a {
                    Action::Envoyer(q) => Some(q.as_str()),
                    _ => None,
                })
                .collect();
            let interpellations = actions
                .iter()
                .filter(|a| **a == Action::Interpeller)
                .count();
            if nom.starts_with("pos") {
                pos += 1;
                pos_ok += usize::from(!envoyees.is_empty());
                if envoyees.is_empty() {
                    manques.push(format!("{nom}: {finals:?}"));
                }
                println!("{nom}: {finals:?} -> {envoyees:?}");
            } else {
                fausses += interpellations;
                // Ce qui part vers Claude (KOB-237) : questions, et phrases
                // « peut-être pas pour toi ». Longueur seulement : le corpus
                // réel n'est jamais recopié.
                for a in &actions {
                    let (genre, q) = match a {
                        Action::Envoyer(q) => ("question", q),
                        Action::EnvoyerPeutEtre(q) => ("peut-être pas pour toi", q),
                        _ => continue,
                    };
                    envois += 1;
                    println!("{nom}: {genre} envoyée, {} caractères", q.chars().count());
                }
                for f in finals
                    .iter()
                    .filter(|f| crate::mot_cle::reperer(f).is_some())
                {
                    println!("{nom}: fausse détection « {f} »");
                }
                println!(
                    "{nom}: {} segments, {interpellations} interpellations",
                    finals.len()
                );
            }
        }
        for m in &manques {
            println!("MANQUÉ {m}");
        }
        println!(
            "BILAN positifs {pos_ok}/{pos}, fausses interpellations {fausses}, envois à Claude sans appel {envois}, \
             calcul {:.1} s pour {audio:.1} s d'audio (×{:.2} temps réel)",
            calcul.as_secs_f64(),
            calcul.as_secs_f64() / audio
        );
    }

    /// KOB-295 : écoute coupée pendant une réunion en cours, puis rouverte
    /// (fin de la coupure, bascule, raccourci) pendant la vidange : elle
    /// s'arrête pour rouvrir le micro ; les autres commandes n'y changent rien.
    #[test]
    fn ecoute_rouverte_arrete_la_vidange() {
        use Commande::*;
        assert!(ecoute_en_pause_apres(true, &[]));
        assert!(ecoute_en_pause_apres(
            true,
            &[Suite, ReponseAttendue(Duration::from_secs(60))]
        ));
        for rouvre in [vec![Pause(false)], vec![BasculerPause], vec![Raccourci]] {
            let pause = ecoute_en_pause_apres(true, &rouvre);
            assert!(!pause, "{rouvre:?}");
            assert!(!vidange_continue("reunion", pause), "{rouvre:?}");
        }
        // Coupée, rouverte, recoupée : la vidange continue.
        assert!(ecoute_en_pause_apres(true, &[Pause(false), BasculerPause]));
        // La réunion en pause, elle, continue de se vider.
        assert!(vidange_continue(
            "pause",
            ecoute_en_pause_apres(true, &[Raccourci])
        ));
    }

    /// KOB-277 : la vidange continue tant que la réunion reste en pause ou se
    /// termine (plus de borne de 45 s), ou que seule l'écoute est coupée ;
    /// elle s'arrête à la reprise ou en veille.
    #[test]
    fn vidange_tant_que_la_pause_dure() {
        assert!(vidange_continue("pause", false));
        assert!(vidange_continue("fin", false));
        assert!(
            vidange_continue("reunion", true),
            "écoute coupée, réunion en cours"
        );
        assert!(!vidange_continue("reunion", false), "reprise");
        assert!(!vidange_continue("veille", false));
        assert!(!vidange_continue("assistant", true));
        assert!(VIDANGE_FILE >= Duration::from_secs(300));
    }

    /// KOB-277 : sous charge, le micro se ferme 24 s après la pause ; ce qui
    /// a été dit depuis est écarté, le reste est gardé et chiffré.
    #[test]
    fn audio_dit_apres_la_pause_ecarte() {
        let origine = Instant::now();
        let mut file: std::collections::VecDeque<AudioFrame> = (0..400)
            .map(|i| AudioFrame {
                source: kobi_audio::Source::Moi,
                samples: vec![0.0; 1_600],
                t: Duration::from_millis(100 * i),
            })
            .collect();
        // Pause à 16 s : le micro, rouvert après une panne, a son horloge
        // décalée de 10 s (KOB-278) ; la pause tombe donc à 6 s de ses
        // trames.
        let pause = origine + Duration::from_secs(16);
        assert_eq!(garder_avant(&mut file, origine, 10.0, pause), 339);
        assert_eq!(file.len(), 61);
        assert!(file.back().is_some_and(|f| f.t <= Duration::from_secs(6)));
        assert!((duree_audio(&file) - 6.1).abs() < 1e-9);
    }

    /// KOB-261 : 30 s d'audio en file à la pause sont toutes transcrites,
    /// dans l'ordre ; la vidange s'arrête si la réunion reprend, ou au bout
    /// du délai.
    #[test]
    fn pause_avec_30_s_en_file_rien_de_perdu() {
        let trente_s = || -> std::collections::VecDeque<AudioFrame> {
            (0..300)
                .map(|i| AudioFrame {
                    source: kobi_audio::Source::Moi,
                    samples: vec![0.0; 1_600],
                    t: Duration::from_millis(100 * i),
                })
                .collect()
        };
        let limite = Instant::now() + VIDANGE_FILE;
        let traiter = |vues: &mut Vec<Duration>, f: AudioFrame| vues.push(f.t);

        let mut file = trente_s();
        let mut vues = Vec::new();
        assert_eq!(
            vidanger(&mut file, limite, |_| false, traiter, &mut vues),
            300
        );
        assert!(file.is_empty());
        let attendu: Vec<_> = (0..300).map(|i| Duration::from_millis(100 * i)).collect();
        assert_eq!(vues, attendu);

        // Reprise après 10 s d'audio vidées.
        let mut file = trente_s();
        let mut vues = Vec::new();
        let reprise = |v: &mut Vec<Duration>| v.len() >= 100;
        assert_eq!(
            vidanger(&mut file, limite, reprise, traiter, &mut vues),
            100
        );
        assert_eq!(file.len(), 200);

        // Délai dépassé : rien n'est plus transcrit.
        let mut file = trente_s();
        let mut vues = Vec::new();
        assert_eq!(
            vidanger(&mut file, Instant::now(), |_| false, traiter, &mut vues),
            0
        );
    }

    /// Doublure de micro pour le porteur.
    struct FauxFlux {
        trames: Receiver<AudioFrame>,
        erreurs: Receiver<kobi_audio::StreamError>,
    }

    impl Flux for FauxFlux {
        fn trames(&self) -> &Receiver<AudioFrame> {
            &self.trames
        }
        fn erreurs(&self) -> &Receiver<kobi_audio::StreamError> {
            &self.erreurs
        }
        fn arreter(self) -> Vec<AudioFrame> {
            self.trames.try_iter().collect()
        }
    }

    fn trame_a(ms: u64) -> AudioFrame {
        AudioFrame {
            source: kobi_audio::Source::Moi,
            samples: vec![0.0; 1_600],
            t: Duration::from_millis(ms),
        }
    }

    /// KOB-285 : une ouverture bloquée (serveur audio mort) ne bloque pas
    /// l'écoute : `attendre` rend la main au délai ; l'ouverture reprise
    /// plus tard rend son résultat ; une erreur remonte telle quelle.
    #[test]
    fn ouverture_bloquee_ne_bloque_pas_l_ecoute() {
        let (debloquer, bloque) = mpsc::channel::<()>();
        let ouverture = Ouverture::lancer_avec(move || {
            let _ = bloque.recv();
            let (_tx, rx) = mpsc::channel();
            Ok((Ouvert::Fichier(rx), "fichier de test".into()))
        });
        let debut = Instant::now();
        assert!(ouverture.attendre(Duration::from_millis(200)).is_none());
        assert!(debut.elapsed() < Duration::from_secs(2));
        debloquer.send(()).unwrap();
        let Some(Ok((Entree::Fichier(_), description, _))) =
            ouverture.attendre(Duration::from_secs(5))
        else {
            panic!("l'ouverture reprise doit aboutir");
        };
        assert_eq!(description, "fichier de test");
        let echec = Ouverture::lancer_avec(|| Err("aucun micro".into()));
        assert!(matches!(
            echec.attendre(Duration::from_secs(5)),
            Some(Err(e)) if e == "aucun micro"
        ));
    }

    /// KOB-294 : l'instant de démarrage du flux est pris quand l'ouverture
    /// aboutit, dans son fil, pas quand l'écoute relève le résultat.
    #[test]
    fn horloge_au_demarrage_du_flux() {
        let ouverture = Ouverture::lancer_avec(|| {
            let (_tx, rx) = mpsc::channel();
            Ok((Ouvert::Fichier(rx), "fichier".into()))
        });
        let avant = Instant::now();
        // L'écoute relève le résultat 300 ms après (elle attendait ailleurs).
        std::thread::sleep(Duration::from_millis(300));
        let releve = Instant::now();
        let Some(Ok((_, _, demarre))) = ouverture.attendre(Duration::from_secs(5)) else {
            panic!("ouverture");
        };
        assert!(
            demarre <= avant + Duration::from_millis(100),
            "pris à l'ouverture"
        );
        assert!(releve.duration_since(demarre) >= Duration::from_millis(250));
    }

    /// KOB-297 : le porteur relaie encore une trame (B) entre la demande de
    /// fermeture et sa réponse (C, le reste de la capture) : l'audio est
    /// rendu dans l'ordre A, B, C.
    #[test]
    fn porteur_rend_l_audio_dans_l_ordre() {
        let (trames_tx, trames) = mpsc::channel();
        let (_erreurs_tx, erreurs) = mpsc::channel();
        let (fermer, fermer_rx) = mpsc::channel::<Sender<Vec<AudioFrame>>>();
        trames_tx.send(trame_a(0)).unwrap();
        let faux_porteur = std::thread::spawn(move || {
            let reponse = fermer_rx.recv().unwrap();
            trames_tx.send(trame_a(100)).unwrap();
            reponse.send(vec![trame_a(200)]).unwrap();
        });
        let reste: Vec<_> = Porteur {
            trames,
            erreurs,
            fermer,
        }
        .fermer()
        .into_iter()
        .map(|f| f.t)
        .collect();
        faux_porteur.join().unwrap();
        assert_eq!(reste, [0, 100, 200].map(Duration::from_millis));
    }

    /// KOB-293 : une ouverture lente, lâchée (pause) avant d'aboutir, ferme
    /// sa capture dès qu'elle aboutit : rien n'est enregistré ni relayé.
    #[test]
    fn ouverture_lachee_ferme_sa_capture() {
        let (debloquer, bloque) = mpsc::channel::<()>();
        let (micro, trames_micro) = mpsc::channel::<AudioFrame>();
        let (fini_tx, fini) = mpsc::channel::<()>();
        let ouverture = Ouverture::lancer_avec(move || {
            let _ = bloque.recv();
            let _ = fini_tx.send(());
            Ok((Ouvert::Fichier(trames_micro), "micro lent".into()))
        });
        assert!(ouverture.attendre(Duration::from_millis(100)).is_none());
        // Pause : l'écoute lâche l'ouverture en attente.
        drop(ouverture);
        debloquer.send(()).unwrap();
        fini.recv_timeout(Duration::from_secs(5)).unwrap();
        // L'entrée ouverte n'a trouvé personne : elle est fermée, plus rien
        // ne se capte.
        let limite = Instant::now() + Duration::from_secs(5);
        while micro.send(trame_a(0)).is_ok() {
            assert!(Instant::now() < limite, "la capture reste ouverte");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// KOB-285 : le porteur relaie les trames et les erreurs du flux, rend à
    /// la fermeture l'audio pas encore lu, dans l'ordre, et s'arrête quand
    /// l'écoute lâche le micro.
    #[test]
    fn porteur_relaie_et_ferme() {
        let (flux_trames, trames_flux) = mpsc::channel();
        let (flux_erreurs, erreurs_flux) = mpsc::channel();
        let flux = FauxFlux {
            trames: trames_flux,
            erreurs: erreurs_flux,
        };
        let (trames_tx, trames) = mpsc::channel();
        let (erreurs_tx, erreurs) = mpsc::channel();
        let (fermer, fermer_rx) = mpsc::channel();
        let fil = std::thread::spawn(move || porter(flux, trames_tx, erreurs_tx, fermer_rx));
        let porteur = Porteur {
            trames,
            erreurs,
            fermer,
        };
        flux_trames.send(trame_a(0)).unwrap();
        flux_erreurs
            .send(kobi_audio::StreamError {
                source: kobi_audio::Source::Moi,
                xrun: false,
                message: "débranché".into(),
            })
            .unwrap();
        let premiere = porteur.trames.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(premiere.t, Duration::ZERO);
        let erreur = porteur
            .erreurs
            .recv_timeout(Duration::from_secs(2))
            .unwrap();
        assert_eq!(erreur.message, "débranché");
        // Deux trames relayées mais pas lues, une restée dans le flux.
        flux_trames.send(trame_a(100)).unwrap();
        flux_trames.send(trame_a(200)).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        flux_trames.send(trame_a(300)).unwrap();
        let reste: Vec<_> = porteur.fermer().into_iter().map(|f| f.t).collect();
        assert_eq!(
            reste,
            [100, 200, 300].map(Duration::from_millis),
            "rien de perdu, dans l'ordre"
        );
        fil.join().unwrap();

        // L'écoute lâche le micro (micro coupé) : le porteur s'arrête seul.
        let (_flux_trames, trames_flux) = mpsc::channel();
        let (_flux_erreurs, erreurs_flux) = mpsc::channel();
        let flux = FauxFlux {
            trames: trames_flux,
            erreurs: erreurs_flux,
        };
        let (trames_tx, trames) = mpsc::channel();
        let (erreurs_tx, erreurs) = mpsc::channel();
        let (fermer, fermer_rx) = mpsc::channel();
        let fil = std::thread::spawn(move || porter(flux, trames_tx, erreurs_tx, fermer_rx));
        drop(Porteur {
            trames,
            erreurs,
            fermer,
        });
        fil.join().unwrap();
    }

    /// KOB-280 : une doublure d'entrée qui cesse d'envoyer des trames, sans
    /// erreur ni fermeture, est vue muette au bout du délai ; une trame qui
    /// revient la ranime ; le canal fermé reste « fermé ».
    #[test]
    fn micro_muet_sans_erreur() {
        let trame = || AudioFrame {
            source: kobi_audio::Source::Moi,
            samples: vec![0.0; 1_600],
            t: Duration::ZERO,
        };
        let muet = Duration::from_millis(300);
        let (tx, rx) = mpsc::channel();
        let mut derniere = Instant::now();
        tx.send(trame()).unwrap();
        tx.send(trame()).unwrap();
        assert!(matches!(
            recevoir(&rx, true, &mut derniere, muet),
            Arrivee::Trames(t) if t.len() == 2
        ));
        assert!(matches!(
            recevoir(&rx, false, &mut derniere, muet),
            Arrivee::Rien
        ));
        std::thread::sleep(muet);
        assert!(matches!(
            recevoir(&rx, true, &mut derniere, muet),
            Arrivee::Muet
        ));
        tx.send(trame()).unwrap();
        assert!(matches!(
            recevoir(&rx, true, &mut derniere, muet),
            Arrivee::Trames(_)
        ));
        assert!(matches!(
            recevoir(&rx, false, &mut derniere, muet),
            Arrivee::Rien
        ));
        drop(tx);
        assert!(matches!(
            recevoir(&rx, true, &mut derniere, muet),
            Arrivee::Ferme
        ));
    }

    /// KOB-236 : une erreur de périphérique (micro débranché) arrête le
    /// micro ; un xrun, non.
    #[test]
    fn micro_debranche_vu_dans_les_erreurs_du_flux() {
        let erreur = |xrun: bool, message: &str| kobi_audio::StreamError {
            source: kobi_audio::Source::Moi,
            xrun,
            message: message.into(),
        };
        assert_eq!(panne_peripherique(std::iter::empty()), None);
        assert_eq!(
            panne_peripherique([erreur(true, "xrun"), erreur(true, "xrun")].into_iter()),
            None
        );
        assert_eq!(
            panne_peripherique(
                [
                    erreur(true, "xrun"),
                    erreur(false, "device no longer available")
                ]
                .into_iter()
            )
            .as_deref(),
            Some("device no longer available")
        );
        // Par le canal, comme `Capture::errors()`.
        let (tx, rx) = mpsc::channel();
        tx.send(erreur(false, "débranché")).unwrap();
        assert_eq!(
            panne_peripherique(rx.try_iter()).as_deref(),
            Some("débranché")
        );
        assert_eq!(panne_peripherique(rx.try_iter()), None, "signalée une fois");
    }
}

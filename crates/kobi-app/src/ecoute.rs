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
/// Période du bilan de charge dans le journal.
const BILAN: Duration = Duration::from_secs(300);
/// Événement du retard de la transcription sur la parole (KOB-250) :
/// [`Retard`], ou `null` quand elle a rattrapé.
pub const EVENEMENT_RETARD: &str = "transcription_retard";
/// Au-delà de ce retard (s), Kobi le signale : un ordre dit maintenant ne
/// sera entendu qu'après.
const SEUIL_RETARD: f64 = 15.0;
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
    /// Kobi vient de répondre : la conversation attend la suite.
    Suite,
    /// La réunion vient de finir : « reprends » ou « annule » s'entend sans
    /// redire « Kobi » tant que la fin s'annule (KOB-165, KOB-245).
    Annulable,
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
    Micro(Capture),
    /// Fichier injecté (`KOBI_ECOUTE_WAV`), joué par un fil à part.
    Fichier(Receiver<AudioFrame>),
}

impl Entree {
    fn frames(&self) -> &Receiver<AudioFrame> {
        match self {
            Entree::Micro(capture) => capture.frames(),
            Entree::Fichier(rx) => rx,
        }
    }

    /// Ouvre le fichier injecté, ou le micro par défaut ; renvoie aussi sa
    /// description pour le journal.
    fn ouvrir() -> Result<(Entree, String), String> {
        if let Some(wav) = std::env::var_os("KOBI_ECOUTE_WAV") {
            let wav = PathBuf::from(wav);
            let samples = kobi_stt::wav::read_wav_16k_mono(&wav)
                .map_err(|e| format!("{} : {e}", wav.display()))?;
            let description = format!(
                "fichier {} ({:.1} s, 16 kHz mono)",
                wav.display(),
                samples.len() as f64 / kobi_stt::SAMPLE_RATE as f64
            );
            return Ok((Entree::Fichier(jouer(samples)), description));
        }
        let capture = Capture::start(&CaptureOptions {
            sources: vec![kobi_audio::Source::Moi],
            // Micro choisi au premier lancement (KOB-144), sinon celui du système.
            mic: crate::bienvenue::micro_choisi(),
            output: None,
        })
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
        Ok((Entree::Micro(capture), description))
    }
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
    if etait_ouverte && !porte.ouverte() && ecouteur.attend_reponse() {
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
                self.fermer_micro();
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(commande) => self.commande(commande),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
                continue;
            }
            if self.entree.is_none() && !self.ouvrir_micro() {
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
        match Entree::ouvrir() {
            Ok((entree, description)) => {
                if let Ok(stt) = self.stt.as_mut() {
                    stt.reset();
                }
                self.ecouteur = Ecouteur::new(!self.en_reunion);
                self.porte = Porte::new(kobi_stt::SAMPLE_RATE);
                self.t = 0.0;
                self.origine = Instant::now();
                self.entree = Some(entree);
                tracing::info!(entree = %description, "écoute continue : micro ouvert");
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

    fn fermer_micro(&mut self) {
        crate::chef::avancer(&self.app, kobi_stt::Source::Moi, None);
        self.signaler_retard(0.0);
        self.file.clear();
        self.veille = Veille::default();
        if self.entree.take().is_some() {
            tracing::info!("écoute continue : micro fermé");
            self.note("micro fermé");
        }
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
            Commande::ReponseAttendue(duree) => {
                self.ecouteur.reponse_attendue(self.t, duree.as_secs_f64())
            }
            Commande::Annulable => self
                .ecouteur
                .annulable(self.t, crate::chef::ANNULATION.as_secs_f64()),
            Commande::BasculerPause => {
                self.en_pause = !self.en_pause;
                tracing::info!(pause = self.en_pause, "écoute continue");
            }
            Commande::Pause(pause) => {
                self.en_pause = pause;
                tracing::info!(pause, "écoute continue (contrôle)");
            }
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
        let mut arrivees = Vec::new();
        if self.file.is_empty() {
            match entree.frames().recv_timeout(Duration::from_millis(200)) {
                Ok(frame) => arrivees.push(frame),
                Err(RecvTimeoutError::Timeout) => return,
                Err(RecvTimeoutError::Disconnected) => {
                    tracing::warn!("écoute continue : le micro s'est arrêté");
                    self.note("le micro s'est arrêté");
                    self.entree = None;
                    crate::chef::avancer(&self.app, kobi_stt::Source::Moi, None);
                    return;
                }
            }
        }
        // Tout ce qui est arrivé entre-temps : la veille voit la parole la
        // plus récente, même quand la transcription est en retard (KOB-250).
        arrivees.extend(entree.frames().try_iter());
        for frame in arrivees {
            self.veiller(&frame);
            self.file.push_back(frame);
        }
        let Some(frame) = self.file.pop_front() else {
            return;
        };
        self.niveau(&frame.samples);
        let Ok(stt) = self.stt.as_mut() else { return };
        let frame = kobi_stt::AudioFrame {
            source: kobi_stt::Source::Moi,
            samples: frame.samples,
            t: frame.t.as_secs_f64(),
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
        for s in finals {
            let dit_a = self.origine + Duration::from_secs_f64(s.end.max(0.0));
            publier(&self.app, Message::Segment(s, dit_a));
        }
        // Tout l'audio reçu jusqu'ici est transcrit.
        let jusqu_a = self.origine + Duration::from_secs_f64(self.t.max(0.0));
        crate::chef::avancer(&self.app, kobi_stt::Source::Moi, Some(jusqu_a));
        self.executer(actions);
        // Sous charge, la transcription peut aller moins vite que la parole :
        // un ordre dit maintenant ne serait entendu qu'après (KOB-250).
        self.signaler_retard(self.origine.elapsed().as_secs_f64() - self.t);

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
        for (debut, morceau) in porte.trame(frame.t.as_secs_f64(), frame.samples.clone()) {
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
        if !v.faite && (pret || (!ouverte && !v.extrait.is_empty())) {
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
        for action in actions {
            let message = match action {
                Action::Interpeller => {
                    // On appelle Kobi : il se tait pour écouter (KOB-142).
                    crate::voix::taire(&self.app, "interpellation");
                    tracing::info!("Kobi interpellé");
                    Message::Interpelle
                }
                Action::Question(q) => Message::QuestionPartielle(q),
                Action::Envoyer(q) if self.deja_execute(&q) => continue,
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
}

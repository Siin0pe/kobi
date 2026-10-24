//! Chef d'orchestre (KOB-134) : le seul à toucher à l'état de la session,
//! aux fichiers de la réunion et à l'interface.
//!
//! Les autres composants tournent en parallèle et lui parlent par messages
//! ([`Message`]) : le fil d'écoute (segments, interpellations, questions),
//! l'acteur Claude ([`crate::cerveau`] : réponses, cartes, notes) et
//! l'interface (texte tapé, clics, menu). Aucun ne bloque l'autre : la
//! transcription s'écrit pendant qu'une réponse arrive, une question tapée
//! passe pendant qu'une carte attend, les notes avancent sur leur session.
//!
//! « Kobi, assiste-nous pour la réunion » est reconnu sur place
//! ([`crate::intentions`]) : la réunion démarre avant tout appel à Claude.

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kobi_session::etat::Evenement;
use kobi_session::stockage::{maintenant_ms, CompteRendu, Origine, Qui};
use kobi_stt::Segment;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::cerveau::{self, Cerveau, Demande, Fabrique, Retour};
use crate::ecrivain::{Ecrivain, Nouvelle};
use crate::intentions::{intention, Intention};
use crate::interpellation::{EVENEMENT_ERREUR, EVENEMENT_QUESTION, EVENEMENT_REPONSE};
use crate::session::{appliquer, etat};

/// État de l'enregistrement (contrat avec l'interface, KOB-137).
pub const EVENEMENT_ENREGISTREMENT: &str = "enregistrement";
/// La réunion en cours a reçu un titre (KOB-223) ; son dossier a peut-être
/// changé de nom : l'app suit.
pub const EVENEMENT_REUNION_NOMMEE: &str = "reunion-nommee";

/// Charge de [`EVENEMENT_REUNION_NOMMEE`].
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ReunionNommee {
    /// Ancien nom du dossier, s'il a changé.
    pub ancien: Option<String>,
    /// Nom actuel du dossier (celui du Carnet).
    pub reunion: String,
    pub titre: Option<String>,
}

/// Titre de réunion (KOB-223) : demandé après ce délai, avec au moins
/// [`TITRE_MOTS`] mots dits.
const TITRE_APRES: Duration = Duration::from_secs(120);
const TITRE_MOTS: usize = 60;
/// Après deux fois [`TITRE_APRES`], ce peu de mots suffit.
const TITRE_MOTS_MIN: usize = 20;
/// Parole gardée pour le titre, au plus (octets).
const TITRE_PAROLE_MAX: usize = 8_000;
/// Demandes de titre au plus, quand Claude ne trouve pas.
const TITRE_ESSAIS: u8 = 3;

/// Notes en mode fin de réunion (`notes.mode = end`, KOB-71) : rien n'est
/// demandé à Claude pendant la réunion, tout passe par le compte rendu. Lu
/// au début de chaque réunion.
fn notes_a_la_fin() -> bool {
    kobi_session::config::default_path().is_some_and(|p| {
        kobi_session::config::load_or_create(&p).config.notes.mode
            == kobi_session::config::NotesMode::End
    })
}

/// Titre automatique réglé (`meetings.titre_auto`). Lu au début de chaque
/// réunion, pas à chaque tick sous le verrou du chef (KOB-265).
fn titre_auto_actif() -> bool {
    kobi_session::config::default_path().is_none_or(|p| {
        kobi_session::config::load_or_create(&p)
            .config
            .meetings
            .titre_auto
    })
}
/// Historique de la conversation (dernières bulles, KOB-133).
pub const EVENEMENT_CONVERSATION: &str = "conversation";
/// La conversation du quotidien est close : la vue agrandie pourra de
/// nouveau s'ouvrir seule (KOB-238).
pub const EVENEMENT_FIN_CONVERSATION: &str = "fin_conversation";
/// Carte de confirmation (bulle d'action).
pub const EVENEMENT_CARTE: &str = "carte";
/// `notes.md` vient d'être écrit (panneau des notes en direct, KOB-73).
pub const EVENEMENT_NOTES: &str = "notes";
/// Compte rendu de fin de réunion, prêt à ouvrir dans le Carnet (KOB-74).
pub const EVENEMENT_COMPTE_RENDU: &str = "compte_rendu";
/// Étape d'un tour : outil appelé par Claude (fil d'activité, KOB-152).
pub const EVENEMENT_ACTIVITE: &str = "activite";

/// Premier bloc de notes après ce délai, puis toutes les `interval_min`.
const PREMIER_BLOC: Duration = Duration::from_secs(60);
/// Temps de lecture minimal d'une réponse avant le retour au calme.
const LECTURE_MIN: Duration = Duration::from_secs(5);
/// Entre le passage « au travail » et un résultat de tâche montré : la
/// transition se voit (KOB-238).
const TRANSITION_RESULTAT: Duration = Duration::from_millis(600);
/// Temps d'affichage d'une erreur.
const ERREUR_AFFICHEE: Duration = Duration::from_secs(6);
/// Bulles gardées dans l'historique.
const HISTORIQUE: usize = 12;
/// Étapes gardées sous chaque bulle de l'historique.
const ETAPES: usize = 24;
/// Une conversation du quotidien est close après ce temps en veille sans
/// nouveau tour : lecture de la réponse (25 s au plus) et suite possible
/// sans redire « Kobi » (20 s, KOB-133).
const FIN_CONVERSATION: Duration = Duration::from_secs(45);
/// La fin d'une réunion s'annule pendant ce temps (« Annuler », Ctrl+Z,
/// « reprends », KOB-245) : la fin est immédiate, le compte rendu part tout de
/// suite, mais n'est écrit qu'à la fin de ce délai.
pub const ANNULATION: Duration = Duration::from_secs(10);
/// À la fin d'une réunion, le compte rendu attend au plus ce temps que les
/// pistes aient transcrit ce qui a été dit avant l'arrêt (KOB-198). Sous
/// forte charge, la transcription va deux fois moins vite que la parole :
/// une demi-minute de retard demande une minute. Kobi montre déjà qu'il
/// rédige pendant ce temps (KOB-245).
pub const VIDANGE: Duration = Duration::from_secs(60);

/// Ce qui arrive au chef.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// « Kobi » entendu, ou raccourci : Kobi écoute.
    Interpelle,
    /// Question en cours de transcription.
    QuestionPartielle(String),
    /// Question finie (dite, ou tapée si `voix` est faux).
    Question {
        texte: String,
        voix: bool,
    },
    /// Bouton « Publier » du Carnet (KOB-75) : la bulle montre `affiche`,
    /// Claude reçoit `consigne` (le document et où le publier). L'écriture
    /// par le MCP passe par la carte de confirmation.
    Publier {
        affiche: String,
        consigne: String,
    },
    /// Phrase dite qui cite Kobi sans qu'on sache si elle lui parle
    /// (KOB-168) : Claude peut ne rien répondre, Kobi reste alors silencieux.
    QuestionPeutEtre(String),
    /// L'interpellation n'a pas été suivie d'une question.
    SansQuestion,
    /// L'écoute est mise en pause hors réunion : la conversation se termine
    /// et Kobi revient en veille (KOB-238).
    EcouteCoupee,
    /// Segment final de la transcription continue, et l'instant (horloge du
    /// système) où sa fin a été dite : sous charge, il arrive bien après
    /// (KOB-198).
    Segment(Segment, Instant),
    /// La piste a transcrit son audio jusqu'à cet instant (`None` : piste
    /// fermée). Par le même canal que les segments : le chef ne voit une
    /// piste à jour qu'après avoir reçu tout ce qu'elle a publié avant
    /// (KOB-261).
    Avancement(kobi_stt::Source, Option<Instant>),
    /// Menu, interface ou commande vocale.
    Commande(Intention),
    /// Clic sur une carte.
    Decision {
        carte: u64,
        accepte: bool,
    },
    /// Échap ou « stop » : la réponse en cours s'arrête net.
    Interrompre,
    /// Dossiers de code changés (KOB-153) : les sessions Claude sont
    /// rouvertes avec les nouveaux.
    CodebasesChangees,
    /// Une carte de validation attend la réponse de l'utilisateur (KOB-156).
    ValidationOuverte {
        id: String,
        resume: String,
    },
    /// La carte est tranchée (ou le délai est passé).
    ValidationFermee {
        id: String,
        reponse: crate::validation::Reponse,
    },
    Cerveau(Retour),
    /// Nouvelle de l'écrivain (fichiers créés, compte rendu rangé, erreur).
    Disque(Nouvelle),
    /// Rappel arrivé à l'heure dite (KOB-145).
    Rappel(crate::rappels::RappelUi),
}

/// État de l'enregistrement envoyé à l'interface (événement
/// [`EVENEMENT_ENREGISTREMENT`], contrat avec KOB-137).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Enregistrement {
    pub actif: bool,
    pub pause: bool,
    /// Début de la réunion, en ms depuis l'époque Unix.
    pub debut_ms: u64,
    /// Lignes de notes prises.
    pub notes: u32,
    pub titre: Option<String>,
    /// Dernière note ajoutée, pour la fiche de prise de notes.
    pub derniere_note: Option<String>,
    /// Réunion terminée, fin encore annulable (KOB-245) : REC éteint, le
    /// bandeau « Réunion terminée · Annuler » s'affiche.
    pub annulable: bool,
}

/// Charge de [`EVENEMENT_NOTES`] : de quoi relire `notes.md` sur le disque.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NotesEcrites {
    /// Nom du dossier de la réunion (celui du Carnet).
    pub reunion: String,
    pub dossier: String,
    /// Lignes de notes, comme dans `enregistrement`.
    pub notes: u32,
}

/// Bulle de l'historique.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bulle {
    pub tour: u64,
    pub question: String,
    pub reponse: String,
    /// `attente`, `reponse`, `tache`, `fini`, `erreur`.
    pub statut: &'static str,
    /// Question tapée au clavier.
    pub clavier: bool,
    /// Étapes du tour (outils appelés), repliées sous la réponse (KOB-152).
    pub etapes: Vec<ActiviteUi>,
}

/// Une étape du fil d'activité (événement [`EVENEMENT_ACTIVITE`], contrat
/// commun des permissions, section 5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ActiviteUi {
    pub tour: u64,
    pub id: String,
    /// Serveur pour un outil MCP, sinon la famille.
    pub outil: String,
    /// `lecture`, `recherche`, `commande`, `mcp`, `ecriture`, `reglage`.
    #[serde(rename = "type")]
    pub famille: kobi_brain::Famille,
    pub resume: String,
    pub chemin_relatif: Option<String>,
    /// `lecture`, `ecriture`, `refus`.
    pub classe: &'static str,
    /// `accepte`, `refuse`, `delai` : réponse à une carte, s'il y en a eu une.
    pub decision: Option<&'static str>,
    /// Appel reçu de Claude, en ms depuis l'époque Unix.
    pub at_ms: u64,
}

impl ActiviteUi {
    pub fn nouvelle(
        demande: &kobi_brain::DemandeOutil,
        at_ms: u64,
        ctx: &kobi_perms::Contexte,
    ) -> Self {
        let etape = demande.action.etape(&ctx.dossiers);
        let outil = match &demande.action {
            kobi_brain::Action::Mcp { serveur, .. } => serveur.clone(),
            _ => serde_json::to_value(etape.famille)
                .ok()
                .and_then(|v| v.as_str().map(String::from))
                .unwrap_or_default(),
        };
        Self {
            tour: demande.tour,
            id: demande.id.clone(),
            outil,
            famille: etape.famille,
            resume: etape.resume,
            chemin_relatif: etape.chemin_relatif,
            classe: crate::lecture::classe(&demande.action, ctx),
            decision: None,
            at_ms,
        }
    }
}

/// Compte rendu de fin de réunion, résumé pour la carte de Kobi : un clic
/// l'ouvre dans le Carnet (`ouvrir_carnet { reunion }`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompteRenduUi {
    /// Nom de la réunion dans le stockage (son dossier).
    pub reunion: String,
    pub resume: String,
    pub decisions: usize,
    pub actions: usize,
    pub questions: usize,
}

/// Résume `compte-rendu.md` pour l'interface : le résumé et le nombre de
/// puces de chaque section. Sans section « Résumé » (compte rendu de secours,
/// Claude indisponible), le premier paragraphe en tient lieu : il dit pourquoi.
pub fn compte_rendu_ui(reunion: &str, markdown: &str) -> CompteRenduUi {
    let cr = CompteRendu::depuis_markdown(markdown);
    let resume = if cr.resume.trim().is_empty() {
        markdown
            .split("\n\n")
            .map(str::trim)
            .find(|p| !p.is_empty() && !p.starts_with('#'))
            .unwrap_or("")
            .to_owned()
    } else {
        cr.resume.trim().to_owned()
    };
    let puces = |section: &str| {
        section
            .lines()
            .map(str::trim_start)
            .filter(|l| l.starts_with("- ") || l.starts_with("* "))
            .count()
    };
    CompteRenduUi {
        reunion: reunion.to_owned(),
        resume,
        decisions: puces(&cr.decisions),
        actions: puces(&cr.actions),
        questions: puces(&cr.questions),
    }
}

/// Carte de confirmation, au format `ActionCard` de l'interface.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CarteUi {
    pub id: String,
    pub kicker: String,
    pub title: String,
    pub meta: Option<String>,
    /// `pending`, `confirmed`, `dismissed`.
    pub status: &'static str,
}

struct EnCours {
    /// Dossier de la réunion, connu quand l'écrivain l'a créé.
    dossier: Option<PathBuf>,
    debut: Instant,
    debut_ms: u64,
    pause: bool,
    /// Début de la pause : ce qui a été dit avant est gardé, même transcrit
    /// après (KOB-261).
    pause_a: Option<Instant>,
    notes: String,
    derniere_note: Option<String>,
    /// Transcription pas encore passée aux notes.
    a_noter: String,
    /// Bloc envoyé, en attente de réponse (remis en tête s'il échoue).
    envoye: Option<String>,
    prochain_bloc: Instant,
    fin_demandee: bool,
    /// Fin encore annulable jusqu'à cet instant (KOB-245).
    annulable_jusqu: Option<Instant>,
    /// Transcription confiée au compte rendu : rendue à la réunion si la fin
    /// est annulée.
    pour_compte_rendu: String,
    /// Compte rendu revenu pendant « Annuler » : écrit à la fin du délai.
    compte_rendu_garde: Option<Result<String, String>>,
    /// Instant de la fin : ce qui a été dit avant est gardé, même transcrit
    /// après (KOB-198).
    arret: Option<Instant>,
    /// Le compte rendu attend que les pistes aient rattrapé l'arrêt, au plus
    /// jusqu'à cet instant.
    vidange: Option<Instant>,
    /// Compte rendu demandé : plus rien n'entre dans la transcription.
    compte_rendu_demande: bool,
    /// Titre de la réunion (KOB-223), affiché dans le badge REC.
    titre: Option<String>,
    /// Titre donné à la main : Kobi ne le remplace plus.
    titre_manuel: bool,
    /// Premières minutes de parole, d'où Kobi tire le titre.
    debut_parole: String,
    /// Demande de titre envoyée (ou titre déjà donné).
    titre_demande: bool,
    /// Demandes sans titre exploitable (« — ») : Kobi réessaie avec plus de
    /// parole, au plus [`TITRE_ESSAIS`] fois.
    titre_essais: u8,
    /// `notes.mode = end` (KOB-71) : ni bloc de notes ni titre pendant la
    /// réunion ; le titre est demandé avec le compte rendu.
    a_la_fin: bool,
    /// `meetings.titre_auto`, lu au début de la réunion (KOB-265).
    titre_auto: bool,
    /// Latence de chaque interpellation, résumée à la fin du compte rendu
    /// (KOB-85).
    latences: crate::latence::Latences,
}

/// Le chef, partagé entre son fil et les commandes de l'interface.
pub struct Chef<R: Runtime> {
    app: AppHandle<R>,
    cerveau: Cerveau,
    ecrivain: Ecrivain,
    reunion: Option<EnCours>,
    intervalle: Duration,
    tour: u64,
    /// Dernière question posée : c'est elle que montre la bulle principale.
    dernier: u64,
    historique: Vec<Bulle>,
    cartes: BTreeMap<u64, CarteUi>,
    /// Transcription que la conversation n'a pas encore vue.
    pour_conversation: String,
    /// Retour au calme prévu (fin de lecture d'une réponse, d'une erreur).
    calme_a: Option<(Instant, Evenement)>,
    /// Résultats de tâches à montrer, un à la fois (KOB-238) : tour, texte,
    /// question.
    a_livrer: std::collections::VecDeque<(u64, String, Option<String>)>,
    /// Le prochain résultat attend ce moment (Kobi vient de passer « au
    /// travail » : la transition se voit avant la réponse).
    livrable_a: Option<Instant>,
    /// Dernier tour de la conversation du quotidien ouverte chez l'écrivain
    /// (KOB-141), s'il y en a une.
    conversation: Option<Instant>,
    /// Réponse dite à voix haute, pour le tour indiqué (KOB-142).
    voix: Option<(u64, crate::voix::Reponse)>,
    /// La conversation attend la suite à la fin de la voix, pas avant
    /// (KOB-133) : la fenêtre de 20 s repart quand Kobi se tait.
    suite_apres_voix: bool,
    /// Dernière question répondue et sa réponse (« répète », « plus court »…).
    derniere: Option<(String, String)>,
    /// Tâches longues en cours : début et question (« où en est… ? ») ;
    /// tant qu'il en reste, Kobi revient à « je m'en occupe » (KOB-152).
    taches: BTreeMap<u64, (Instant, String)>,
    /// Tâches dont la lenteur a déjà été annoncée (une annonce au plus pour
    /// des tâches simultanées, KOB-238).
    annoncees: std::collections::BTreeSet<u64>,
    /// Tours abandonnés : ce qui en revient encore est ignoré.
    interrompus: HashSet<u64>,
    /// Cartes de validation ouvertes (KOB-156), dans l'ordre de l'écran :
    /// « oui » ou « non » répond à celle affichée (KOB-218).
    validations: crate::validation::FileValidations,
    /// La question en cours de traitement ne s'adresse peut-être pas à Kobi.
    peut_etre: bool,
    /// Tour d'une telle question : sa réponse n'est montrée qu'une fois
    /// finie, et une réponse « rien » le laisse silencieux (KOB-168).
    tour_peut_etre: Option<u64>,
    /// Compte rendu remis à l'écrivain, pas encore sur le disque : quitter
    /// l'attend encore (KOB-226).
    compte_rendu_a_ecrire: bool,
    /// Comptes rendus demandés puis abandonnés par « Annuler » (KOB-245) :
    /// leurs réponses sont ignorées.
    comptes_rendus_abandonnes: u32,
    /// Jusqu'où les pistes ont transcrit leur audio (KOB-198).
    avancement: Avancement,
}

/// Consigne jointe à une phrase qui cite Kobi sans lui parler à coup sûr
/// (KOB-168).
const CONSIGNE_PEUT_ETRE: &str = "Cette phrase, entendue au micro, contient ton nom mais ne s'adresse peut-être pas à toi (on peut parler de toi à quelqu'un d'autre). Si elle ne t'est pas adressée, réponds seulement « — » et rien d'autre ; sinon réponds normalement.\n\nPhrase : ";

/// Réponse « rien » à une phrase qui ne s'adressait pas à Kobi.
fn est_silence(reponse: &str) -> bool {
    matches!(reponse.trim(), "" | "—" | "–" | "-" | "…" | "(silence)")
}

/// Chef partagé (état géré par Tauri).
pub struct ChefPartage<R: Runtime>(pub Mutex<Chef<R>>);
/// Canal vers le fil du chef.
pub struct Bus(pub Mutex<Sender<Message>>);

/// Jusqu'où chaque piste a transcrit son audio, sur l'horloge du système
/// (KOB-198) ; `None` : piste fermée. Sous charge, la transcription prend du
/// retard : à la fin, le compte rendu attend qu'elle ait rattrapé l'arrêt.
/// Tenu par le chef, d'après [`Message::Avancement`] (KOB-261).
#[derive(Debug, Default)]
struct Avancement {
    moi: Option<Instant>,
    eux: Option<Instant>,
}

impl Avancement {
    fn avancer(&mut self, source: kobi_stt::Source, jusqu_a: Option<Instant>) {
        match source {
            kobi_stt::Source::Moi => self.moi = jusqu_a,
            kobi_stt::Source::Eux => self.eux = jusqu_a,
        }
    }

    /// La piste `source` a transcrit son audio jusqu'à `instant` (ou elle est
    /// fermée).
    fn piste_a_jour(&self, source: kobi_stt::Source, instant: Instant) -> bool {
        self.piste(source)
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none_or(|t| t >= instant)
    }

    /// Les pistes ouvertes ont toutes transcrit leur audio jusqu'à `instant`.
    fn a_jour(&self, instant: Instant) -> bool {
        [self.moi, self.eux]
            .iter()
            .all(|p| p.is_none_or(|t| t >= instant))
    }
}

/// La piste `source` a transcrit son audio jusqu'à `jusqu_a` (`None` : piste
/// fermée, plus rien à attendre d'elle). Passe derrière les segments déjà
/// publiés par la piste (KOB-261).
pub fn avancer<R: Runtime>(app: &AppHandle<R>, source: kobi_stt::Source, jusqu_a: Option<Instant>) {
    publier(app, Message::Avancement(source, jusqu_a));
}

/// Transcription confiée au compte rendu : le bloc parti à la prise de notes
/// (sans réponse encore) précède ce qui reste à noter ; sans lui, sa
/// transcription manquerait au compte rendu (KOB-228).
fn transcript_du_compte_rendu(envoye: Option<&str>, reste: &str) -> String {
    let mut transcript = envoye.unwrap_or_default().to_owned();
    transcript.push_str(reste);
    transcript
}

/// Envoie un message au chef.
pub fn publier<R: Runtime>(app: &AppHandle<R>, message: Message) {
    if let Some(bus) = app.try_state::<Bus>() {
        let _ = bus.0.lock().map(|tx| tx.send(message));
    }
}

/// Lance le chef et l'acteur Claude ; renvoie le canal du chef.
pub fn installer<R: Runtime>(
    app: &AppHandle<R>,
    fabrique: Fabrique,
    intervalle_min: u8,
    delais: cerveau::Delais,
) {
    let (tx, rx) = mpsc::channel();
    app.manage(Bus(Mutex::new(tx.clone())));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("cerveau")
        .build()
        .expect("runtime tokio du cerveau");
    let tx_disque = Mutex::new(tx.clone());
    let sortie_tx = Mutex::new(tx);
    let sortie: cerveau::Sortie = Arc::new(move |retour| {
        let _ = sortie_tx.lock().map(|tx| tx.send(Message::Cerveau(retour)));
    });
    let cerveau = {
        let _guard = rt.enter();
        let app = app.clone();
        Cerveau::lancer_avec(
            fabrique,
            sortie,
            Arc::new(move || crate::validation::attend(&app)),
            delais,
        )
    };
    let ecrivain = Ecrivain::lancer(move |compte| {
        let _ = tx_disque.lock().map(|tx| tx.send(Message::Disque(compte)));
    });
    app.manage(ChefPartage(Mutex::new(Chef::new(
        app.clone(),
        cerveau,
        ecrivain,
        Duration::from_secs(60 * u64::from(intervalle_min.max(1))),
    ))));
    let app = app.clone();
    std::thread::Builder::new()
        .name("chef".into())
        .spawn(move || {
            // Le runtime vit aussi longtemps que le fil.
            let _rt = rt;
            boucle(app, rx)
        })
        .expect("fil du chef");
}

fn boucle<R: Runtime>(app: AppHandle<R>, rx: Receiver<Message>) {
    loop {
        let message = match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let chef = app.state::<ChefPartage<R>>();
        let mut chef = chef.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = message {
            chef.recevoir(m);
        }
        chef.tick();
    }
}

/// Réponse à « répète » ou « plus court » avant toute réponse.
fn sans_reponse() -> String {
    kobi_i18n::tr("coque.chef.sansReponse", &[])
}

/// Consigne envoyée à Claude pour reprendre la dernière réponse. Elle la
/// rappelle en entier : la session de conversation a pu changer depuis
/// (tâche longue détachée, interruption).
pub fn reprise(intention: Intention, (question, reponse): &(String, String)) -> String {
    let demande = match intention {
        Intention::PlusCourt => "Redis-la en plus court, en une phrase.",
        Intention::Developpe => "Développe-la, avec plus de détails, en cinq phrases au plus.",
        _ => "Redis-la en anglais.",
    };
    format!("Ta dernière réponse, à la question « {question} », était : « {reponse} ». {demande}")
}

/// Lignes de notes (puces et cases).
fn puces(notes: &str) -> impl Iterator<Item = &str> {
    notes.lines().filter_map(|l| {
        let l = l.trim_start();
        ["- ", "* ", "+ "]
            .iter()
            .find_map(|p| l.strip_prefix(p))
            .map(str::trim)
            .filter(|l| !l.is_empty())
    })
}

/// Lignes de notes : le compteur affiché sur la mascotte.
pub fn compter_notes(notes: &str) -> u32 {
    puces(notes).count() as u32
}

/// Dernière note ajoutée : la dernière puce nouvelle par rapport à
/// `avant`, sinon la dernière puce.
pub fn derniere_note(avant: &str, apres: &str) -> Option<String> {
    let anciennes: std::collections::HashSet<&str> = puces(avant).collect();
    puces(apres)
        .filter(|p| !anciennes.contains(p))
        .last()
        .or_else(|| puces(apres).last())
        .map(|p| p.replace("**", "").replace('`', ""))
}

/// Carte lisible pour un appel d'outil : « Linear · save_issue ».
pub fn carte_ui(carte: u64, outil: &str, entree: &serde_json::Value) -> CarteUi {
    let mut morceaux = outil.trim_start_matches("mcp__").splitn(2, "__");
    let serveur = morceaux.next().unwrap_or(outil);
    let action = morceaux.next().unwrap_or("");
    let titre = [
        "title", "titre", "name", "summary", "subject", "body", "content",
    ]
    .iter()
    .find_map(|cle| entree.get(cle).and_then(|v| v.as_str()))
    .map(String::from)
    .unwrap_or_else(|| action.replace('_', " "));
    let meta = entree.as_object().map(|o| {
        o.iter()
            .filter(|(_, v)| v.is_string() || v.is_number())
            .take(3)
            .map(|(k, v)| {
                format!(
                    "{k} : {}",
                    v.as_str().map_or_else(|| v.to_string(), String::from)
                )
            })
            .collect::<Vec<_>>()
            .join(" · ")
    });
    let mut serveur_maj = serveur.to_string();
    if let Some(c) = serveur_maj.get_mut(..1) {
        c.make_ascii_uppercase();
    }
    CarteUi {
        id: carte.to_string(),
        kicker: format!("{serveur_maj} · {}", action.replace('_', " ")),
        title: titre.chars().take(120).collect(),
        meta: meta.filter(|m| !m.is_empty()),
        status: "pending",
    }
}

impl<R: Runtime> Chef<R> {
    fn new(app: AppHandle<R>, cerveau: Cerveau, ecrivain: Ecrivain, intervalle: Duration) -> Self {
        Self {
            app,
            cerveau,
            ecrivain,
            reunion: None,
            intervalle,
            tour: 0,
            dernier: 0,
            historique: Vec::new(),
            cartes: BTreeMap::new(),
            pour_conversation: String::new(),
            calme_a: None,
            a_livrer: Default::default(),
            livrable_a: None,
            conversation: None,
            voix: None,
            suite_apres_voix: false,
            derniere: None,
            taches: BTreeMap::new(),
            annoncees: Default::default(),
            interrompus: HashSet::new(),
            validations: Default::default(),
            peut_etre: false,
            tour_peut_etre: None,
            compte_rendu_a_ecrire: false,
            comptes_rendus_abandonnes: 0,
            avancement: Avancement::default(),
        }
    }

    /// État de l'enregistrement.
    /// Dès la fin demandée, l'enregistrement est annoncé arrêté (REC
    /// s'éteint tout de suite, KOB-166), même si la réunion reste ouverte le
    /// temps d'écrire le compte rendu.
    pub fn enregistrement(&self) -> Enregistrement {
        match &self.reunion {
            None => Enregistrement::default(),
            // REC éteint dès la fin, « Annuler » encore possible (KOB-245).
            Some(r) if r.fin_demandee => Enregistrement {
                annulable: r.annulable_jusqu.is_some(),
                ..Enregistrement::default()
            },
            Some(r) => Enregistrement {
                actif: true,
                pause: r.pause,
                debut_ms: r.debut_ms,
                notes: compter_notes(&r.notes),
                titre: r.titre.clone(),
                derniere_note: r.derniere_note.clone(),
                annulable: false,
            },
        }
    }

    pub fn historique(&self) -> Vec<Bulle> {
        self.historique.clone()
    }

    fn emettre_enregistrement(&self) {
        let e = self.enregistrement();
        tracing::info!(
            actif = e.actif,
            pause = e.pause,
            notes = e.notes,
            "enregistrement"
        );
        let _ = self.app.emit(EVENEMENT_ENREGISTREMENT, e);
    }

    fn emettre_historique(&self) {
        let _ = self.app.emit(EVENEMENT_CONVERSATION, &self.historique);
    }

    fn bulle(&mut self, tour: u64) -> Option<&mut Bulle> {
        self.historique.iter_mut().find(|b| b.tour == tour)
    }

    fn transition(&self, evenements: &[Evenement]) -> bool {
        appliquer(&self.app, evenements).is_ok()
    }

    /// Activité affichée (`calme`, `ecoute`…), ou `None` hors activité.
    fn activite(&self) -> Option<&'static str> {
        etat(&self.app).activite
    }

    pub fn recevoir(&mut self, message: Message) {
        match message {
            Message::Interpelle => {
                self.calme_a = None;
                if !self.transition(&[Evenement::Interpellation]) {
                    tracing::info!("interpellation ignorée par la session");
                }
            }
            Message::QuestionPartielle(q) => {
                let _ = self.app.emit(EVENEMENT_QUESTION, &q);
            }
            Message::SansQuestion => {
                self.transition(&[Evenement::PasDeQuestion]);
            }
            Message::EcouteCoupee => {
                // Micro coupé : plus rien ne fera passer le délai de la
                // conversation, elle se termine tout de suite (KOB-238).
                tracing::info!("écoute en pause : fin de la conversation");
                self.suite_apres_voix = false;
                self.transition(&[Evenement::PasDeQuestion]);
                self.fermer_conversation();
            }
            Message::Question { texte, voix } => self.question(texte, voix),
            Message::Publier { affiche, consigne } => {
                tracing::info!("publication demandée depuis le Carnet");
                self.transition(&[Evenement::Interpellation]);
                let _ = self.app.emit(EVENEMENT_QUESTION, &affiche);
                self.envoyer(affiche, Some(consigne), false);
            }
            Message::QuestionPeutEtre(texte) => {
                self.peut_etre = true;
                self.question(texte, true);
                self.peut_etre = false;
            }
            Message::Segment(s, dit_a) => self.segment(s, dit_a),
            Message::Avancement(source, jusqu_a) => self.avancement.avancer(source, jusqu_a),
            Message::Commande(i) => {
                self.commande(i);
            }
            Message::Decision { carte, accepte } => {
                self.cerveau.demander(Demande::Decision { carte, accepte })
            }
            Message::Interrompre => self.interrompre(),
            Message::CodebasesChangees => self.cerveau.demander(Demande::Renouveler),
            Message::ValidationOuverte { id, resume } => self.validation_ouverte(id, &resume),
            Message::ValidationFermee { id, reponse } => self.validation_fermee(&id, reponse),
            Message::Cerveau(r) => self.retour(r),
            Message::Disque(c) => self.disque(c),
            Message::Rappel(r) => {
                tracing::info!(id = %r.id, "rappel livré à l'interface");
                let _ = self.app.emit(crate::rappels::EVENEMENT_RAPPEL, &r);
                // À voix haute aussi, si la voix est active (KOB-142).
                crate::voix::dire(
                    &self.app,
                    &kobi_i18n::tr("kobi.taches.rappelAria", &[("texte", &r.texte)]),
                );
            }
        }
    }

    /// Question dite ou tapée : ordre local, ou tour de Claude.
    pub fn question(&mut self, texte: String, voix: bool) {
        let texte = texte.trim().to_string();
        if texte.is_empty() {
            return;
        }
        // Tapée au clavier : Kobi « écoute » le temps de l'envoyer.
        if !voix {
            self.transition(&[Evenement::Interpellation]);
        }
        let _ = self.app.emit(EVENEMENT_QUESTION, &texte);
        // Une carte de validation attend : « oui » ou « non » lui répondent.
        if let Some(id) = self.validations.cible().map(str::to_owned) {
            if let Some(accepte) = crate::intentions::reponse_validation(&texte) {
                tracing::info!(%id, accepte, "réponse à la carte, à la voix ou au clavier");
                crate::validation::decider(&self.app, &id, accepte);
                return;
            }
        }
        // « Kobi, parle », « en silence », « stop » (KOB-142).
        match crate::voix::commande_vocale(&texte) {
            Some(crate::voix::CommandeVoix::Stop) => {
                self.interrompre();
                return;
            }
            Some(c) => {
                tracing::info!(commande = ?c, "commande de la voix");
                let confirmation = crate::voix::executer(&self.app, c).unwrap_or_default();
                self.transition(&[Evenement::PasDeQuestion]);
                let _ = self.app.emit(EVENEMENT_REPONSE, &confirmation);
                crate::voix::dire(&self.app, &confirmation);
                self.tour += 1;
                self.ajouter(Bulle {
                    tour: self.tour,
                    question: texte,
                    reponse: confirmation,
                    statut: "fini",
                    clavier: !voix,
                    etapes: Vec::new(),
                });
                return;
            }
            None => {}
        }
        // « Reprends », « c'était une erreur » juste après un arrêt (KOB-165).
        if self.fin_annulable() && crate::intentions::annule_la_fin(&texte) {
            self.transition(&[Evenement::PasDeQuestion]);
            let reponse = self.piloter(Intention::Reprendre, true);
            self.tour += 1;
            self.ajouter(Bulle {
                tour: self.tour,
                question: texte,
                reponse,
                statut: "fini",
                clavier: !voix,
                etapes: Vec::new(),
            });
            return;
        }
        let mut consigne = None;
        match intention(&texte) {
            Some(Intention::Interrompre) => {
                self.interrompre();
                return;
            }
            // « Montre en grand » (KOB-169) : la conversation s'ouvre en grand,
            // sans nouvel échange qui la remplacerait.
            Some(Intention::MontrerEnGrand) => {
                self.transition(&[Evenement::PasDeQuestion]);
                crate::agrandir::vue(&self.app, "ouvrir");
                return;
            }
            Some(Intention::Repeter) => {
                let reponse = match &self.derniere {
                    Some((_, r)) => r.clone(),
                    None => self.pas_encore_de_reponse(),
                };
                self.repondre_localement(texte, reponse, voix);
                return;
            }
            Some(Intention::OuEnEst) => {
                let reponse = self.avancement();
                self.repondre_localement(texte, reponse, voix);
                return;
            }
            Some(Intention::Annuler) => {
                let reponse = self.annuler();
                self.repondre_localement(texte, reponse, voix);
                return;
            }
            Some(i @ (Intention::PlusCourt | Intention::Developpe | Intention::EnAnglais)) => {
                let Some(c) = self.derniere.as_ref().map(|d| reprise(i, d)) else {
                    let reponse = self.pas_encore_de_reponse();
                    self.repondre_localement(texte, reponse, voix);
                    return;
                };
                consigne = Some(c);
            }
            _ => {}
        }
        // « Rappelle-moi … », « ajoute … à ma liste » (KOB-145) : rangé sur place.
        if let Some(reponse) = self.rappel(&texte) {
            // Affichée comme une réponse : on lit la confirmation, puis la
            // conversation attend la suite.
            self.calme_a = None;
            self.transition(&[Evenement::QuestionEnvoyee, Evenement::ReponseCommence]);
            let _ = self.app.emit(EVENEMENT_REPONSE, &reponse);
            crate::voix::dire(&self.app, &reponse);
            self.lire(&reponse);
            self.tour += 1;
            self.dernier = self.tour;
            self.ajouter(Bulle {
                tour: self.tour,
                question: texte,
                reponse,
                statut: "fini",
                clavier: !voix,
                etapes: Vec::new(),
            });
            return;
        }
        // « Kobi, appelle cette réunion … » (KOB-223) : titre donné à la main.
        if self.reunion.is_some() && consigne.is_none() {
            if let Some(titre) = crate::intentions::titre_de_reunion_dit(&texte) {
                let reponse = self.renommer_reunion(&titre).unwrap_or_else(|e| e);
                self.repondre_localement(texte, reponse, voix);
                return;
            }
        }
        // Pendant une réunion, « la réunion est terminée », « tu peux arrêter
        // de noter »… l'arrêtent sur place, sans dépendre de Claude (KOB-203).
        let fin_dite = self.reunion.is_some() && crate::intentions::fin_de_reunion_dite(&texte);
        let intention = if fin_dite {
            Some(Intention::TerminerReunion)
        } else {
            intention(&texte)
        };
        if let Some(i) = intention.filter(|_| consigne.is_none()) {
            tracing::info!(intention = ?i, "ordre vocal");
            self.transition(&[Evenement::PasDeQuestion]);
            let reponse = self.piloter(i, true);
            self.tour += 1;
            self.ajouter(Bulle {
                tour: self.tour,
                question: texte,
                reponse: reponse.clone(),
                statut: "fini",
                clavier: !voix,
                etapes: Vec::new(),
            });
            return;
        }
        self.envoyer(texte, consigne, voix);
    }

    /// Envoie une question à Claude : `texte` est montré et rangé, Claude
    /// reçoit `consigne` à la place s'il y en a une.
    fn envoyer(&mut self, texte: String, mut consigne: Option<String>, voix: bool) {
        self.tour += 1;
        self.dernier = self.tour;
        self.calme_a = None;
        self.transition(&[Evenement::QuestionEnvoyee]);
        self.ranger(Qui::Moi, &texte, voix);
        self.voix = Some((self.tour, crate::voix::Reponse::new(&self.app)));
        let notes = self
            .reunion
            .as_ref()
            .map(|r| r.notes.clone())
            .unwrap_or_default();
        if self.peut_etre && consigne.is_none() {
            consigne = Some(format!("{CONSIGNE_PEUT_ETRE}{texte}"));
            self.tour_peut_etre = Some(self.tour);
        }
        if let Some(r) = &mut self.reunion {
            r.latences.question(self.tour, Instant::now());
        }
        self.cerveau.demander(Demande::Question {
            tour: self.tour,
            question: self.avec_contexte(consigne.unwrap_or_else(|| texte.clone())),
            transcript: std::mem::take(&mut self.pour_conversation),
            notes,
        });
        tracing::info!(
            tour = self.tour,
            longueur = texte.len(),
            clavier = !voix,
            "question envoyée"
        );
        self.ajouter(Bulle {
            tour: self.tour,
            question: texte,
            reponse: String::new(),
            statut: "attente",
            clavier: !voix,
            etapes: Vec::new(),
        });
    }

    /// Demande de rappel, de tâche ou de liste : rangée, avec sa réponse.
    fn rappel(&mut self, texte: &str) -> Option<String> {
        let stockage = crate::stockage::actuel(&self.app)?;
        let source = self
            .reunion
            .as_ref()
            .and_then(|r| r.dossier.as_ref())
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        let reponse =
            crate::rappels::traiter(&stockage, texte, &jiff::Zoned::now(), source.as_deref())?;
        tracing::info!("rappel ou tâche rangé");
        crate::rappels::reveiller(&self.app);
        Some(reponse)
    }

    /// Range un tour de la conversation du quotidien. En réunion, la
    /// question et la réponse de Kobi vont aux notes et au compte rendu.
    fn ranger(&mut self, qui: Qui, texte: &str, voix: bool) {
        if let Some(r) = &mut self.reunion {
            let t = r.debut.elapsed().as_secs();
            let qui = match qui {
                Qui::Moi => "interpellation de Kobi",
                Qui::Kobi => "réponse de Kobi",
            };
            let texte = texte.replace('\n', " ");
            r.a_noter
                .push_str(&format!("[{:02}:{:02} {qui}] {texte}\n", t / 60, t % 60));
            return;
        }
        let Some(stockage) = crate::stockage::actuel(&self.app) else {
            return;
        };
        self.ecrivain
            .tour(stockage, maintenant_ms(), qui, texte.to_owned(), voix);
        self.conversation = Some(Instant::now());
    }

    fn ajouter(&mut self, bulle: Bulle) {
        self.historique.push(bulle);
        let trop = self.historique.len().saturating_sub(HISTORIQUE);
        self.historique.drain(..trop);
        self.emettre_historique();
    }

    /// Ordre (menu, raccourci, interface ou CLI) ; renvoie la phrase de
    /// confirmation, affichée et dite.
    pub fn commande(&mut self, intention: Intention) -> String {
        let Some(reponse) = self.executer(intention) else {
            return String::new();
        };
        self.emettre_enregistrement();
        let _ = self.app.emit(EVENEMENT_REPONSE, &reponse);
        crate::voix::dire(&self.app, &reponse);
        reponse
    }

    /// Ordre de réunion entendu (chemin rapide) ou décidé par Claude avec le
    /// MCP de Kobi (KOB-165) : pas de carte, Kobi agit tout de suite.
    /// `annoncer` : Kobi affiche et dit la confirmation (Claude, lui,
    /// l'annonce dans sa réponse).
    pub fn piloter(&mut self, intention: Intention, annoncer: bool) -> String {
        let Some(reponse) = self.executer(intention) else {
            return String::new();
        };
        self.emettre_enregistrement();
        if annoncer {
            let _ = self.app.emit(EVENEMENT_REPONSE, &reponse);
            crate::voix::dire(&self.app, &reponse);
        }
        reponse
    }

    /// Une réunion est-elle ouverte ? Reste vrai pendant que le compte rendu
    /// s'écrit, jusqu'à ce qu'il soit sur le disque, quand
    /// [`Chef::enregistrement`] annonce déjà REC éteint (KOB-166) : quitter
    /// attend alors le compte rendu (KOB-226).
    pub fn reunion_ouverte(&self) -> bool {
        self.reunion.is_some() || self.compte_rendu_a_ecrire
    }

    /// La fin de la réunion peut-elle encore s'annuler ?
    pub fn fin_annulable(&self) -> bool {
        self.reunion
            .as_ref()
            .is_some_and(|r| r.annulable_jusqu.is_some())
    }

    fn executer(&mut self, intention: Intention) -> Option<String> {
        Some(match intention {
            Intention::DemarrerReunion => self.demarrer(),
            Intention::Pause => match &mut self.reunion {
                Some(r) if !r.pause => {
                    r.pause = true;
                    r.pause_a = Some(Instant::now());
                    self.transition(&[Evenement::Pause]);
                    kobi_i18n::tr("coque.chef.reunionEnPause", &[])
                }
                Some(_) => kobi_i18n::tr("coque.chef.dejaEnPause", &[]),
                None => kobi_i18n::tr("coque.chef.aucuneReunion", &[]),
            },
            Intention::Reprendre if self.fin_annulable() => self.annuler_fin(),
            Intention::Reprendre => match &mut self.reunion {
                Some(r) if r.pause => {
                    r.pause = false;
                    r.pause_a = None;
                    self.transition(&[Evenement::Reprendre]);
                    kobi_i18n::tr("coque.chef.enregistreDeNouveau", &[])
                }
                Some(_) => kobi_i18n::tr("coque.chef.enregistreDeja", &[]),
                None => kobi_i18n::tr("coque.chef.aucuneReunion", &[]),
            },
            Intention::TerminerReunion => self.terminer(),
            Intention::Interrompre => {
                self.interrompre();
                return None;
            }
            Intention::OuvrirCarnet => match crate::carnet::ouvrir(&self.app) {
                Ok(()) => kobi_i18n::tr("coque.chef.ouvreCarnet", &[]),
                Err(e) => {
                    tracing::warn!(erreur = %e, "carnet");
                    kobi_i18n::tr("coque.chef.carnetImpossible", &[("erreur", &e.to_string())])
                }
            },
            // Reprises et suivi des tâches : seulement par la question.
            Intention::Repeter
            | Intention::PlusCourt
            | Intention::Developpe
            | Intention::EnAnglais
            | Intention::OuEnEst
            | Intention::Annuler
            | Intention::MontrerEnGrand => return None,
        })
    }

    fn demarrer(&mut self) -> String {
        if let Some(r) = &self.reunion {
            return if r.fin_demandee {
                kobi_i18n::tr("coque.chef.compteRenduEnCours", &[])
            } else {
                kobi_i18n::tr("coque.chef.reunionDejaEnregistree", &[])
            };
        }
        let Some(stockage) = crate::stockage::actuel(&self.app) else {
            return kobi_i18n::tr("coque.chef.dossierIntrouvable", &[]);
        };
        let debut = Instant::now();
        let debut_ms = maintenant_ms();
        self.ecrivain.ouvrir_reunion(stockage, debut_ms);
        // Ses notes et son compte rendu ne partent pas de la réunion
        // précédente (KOB-252).
        self.cerveau.demander(Demande::NouvelleReunion);
        // Depuis un échange hors réunion, on revient d'abord en veille.
        if etat(&self.app).etat == "assistant" {
            self.transition(&[Evenement::PasDeQuestion]);
            self.transition(&[Evenement::ReponseTerminee]);
        }
        self.transition(&[Evenement::DemarrerReunion]);
        // La conversation du quotidien s'arrête là ; la réunion prend le relais.
        self.fermer_conversation();
        tracing::info!(ms = debut.elapsed().as_millis() as u64, "réunion démarrée");
        self.reunion = Some(EnCours {
            dossier: None,
            debut,
            debut_ms,
            pause: false,
            notes: String::new(),
            derniere_note: None,
            a_noter: String::new(),
            envoye: None,
            prochain_bloc: debut + PREMIER_BLOC,
            fin_demandee: false,
            annulable_jusqu: None,
            pour_compte_rendu: String::new(),
            compte_rendu_garde: None,
            arret: None,
            vidange: None,
            compte_rendu_demande: false,
            pause_a: None,
            titre: None,
            titre_manuel: false,
            debut_parole: String::new(),
            titre_demande: false,
            titre_essais: 0,
            a_la_fin: notes_a_la_fin(),
            titre_auto: titre_auto_actif(),
            latences: Default::default(),
        });
        kobi_i18n::tr("coque.chef.reunionCommencee", &[])
    }

    /// Titre proposé par Claude (KOB-223) : retenu s'il est exploitable et
    /// qu'aucun titre n'a été donné à la main entre-temps.
    fn titre_recu(&mut self, resultat: Result<String, String>) {
        let Some(r) = &mut self.reunion else { return };
        let titre = match resultat {
            Ok(texte) => cerveau::titre_propose(&texte),
            Err(e) => {
                tracing::warn!(erreur = %e, "titre de la réunion");
                None
            }
        };
        let Some(titre) = titre else {
            // Rien d'exploitable : Kobi réessaiera avec plus de parole.
            r.titre_essais += 1;
            r.titre_demande = false;
            tracing::info!(
                essais = r.titre_essais,
                "pas encore de titre pour la réunion"
            );
            return;
        };
        if r.titre_manuel {
            return;
        }
        tracing::info!(%titre, "titre de la réunion");
        r.titre = Some(titre.clone());
        r.debut_parole.clear();
        self.ecrivain.nommer(titre, Origine::Auto);
        self.emettre_enregistrement();
    }

    /// Dossier de la réunion en cours, une fois créé.
    pub fn dossier_en_cours(&self) -> Option<PathBuf> {
        self.reunion.as_ref().and_then(|r| r.dossier.clone())
    }

    /// Titre donné à la main (app, voix, `kobi meeting rename`) : la réunion
    /// en cours le garde, Kobi ne le remplace plus.
    pub fn renommer_reunion(&mut self, titre: &str) -> Result<String, String> {
        let titre = titre.trim();
        if titre.is_empty() {
            return Err(kobi_i18n::tr("coque.chef.titreManquant", &[]));
        }
        let Some(r) = &mut self.reunion else {
            return Err(kobi_i18n::tr("coque.chef.aucuneReunion", &[]));
        };
        r.titre = Some(titre.to_owned());
        r.titre_manuel = true;
        r.titre_demande = true;
        tracing::info!(titre, "titre de la réunion donné à la main");
        self.ecrivain.nommer(titre.to_owned(), Origine::Manuel);
        self.emettre_enregistrement();
        Ok(kobi_i18n::tr(
            "coque.chef.reunionRenommee",
            &[("titre", titre)],
        ))
    }

    /// Fin de la réunion, la même par la voix, Claude, le raccourci, le menu
    /// ou `kobi meeting stop` (KOB-245) : REC s'éteint, Kobi passe en fin
    /// (rédaction) et le compte rendu part tout de suite. « Annuler »
    /// reprend la même réunion pendant [`ANNULATION`].
    fn terminer(&mut self) -> String {
        let Some(r) = &mut self.reunion else {
            return kobi_i18n::tr("coque.chef.aucuneReunion", &[]);
        };
        if r.fin_demandee {
            // Redemandée pendant « Annuler » : la fin devient définitive.
            if r.annulable_jusqu.is_some() {
                self.clore_annulation();
            }
            return kobi_i18n::tr("coque.chef.compteRenduDejaEnCours", &[]);
        }
        r.fin_demandee = true;
        let arret = Instant::now();
        r.arret = Some(arret);
        r.annulable_jusqu = Some(arret + ANNULATION);
        self.transition(&[Evenement::TerminerReunion]);
        self.emettre_enregistrement();
        self.vider_puis_compte_rendu();
        // « Reprends » s'entend sans redire « Kobi » pendant « Annuler ».
        // L'instant réel de l'arrêt : sous retard, seul ce qui est dit après
        // annule la fin (KOB-265).
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Annulable(arret));
        tracing::info!("réunion terminée, annulable");
        kobi_i18n::tr("coque.chef.reunionTerminee", &[])
    }

    /// Fin de « Annuler » (délai écoulé, fin redemandée, quitter) : la fin est
    /// définitive, le compte rendu déjà revenu est écrit.
    pub fn clore_annulation(&mut self) {
        let Some(r) = &mut self.reunion else { return };
        if r.annulable_jusqu.take().is_none() {
            return;
        }
        tracing::info!("fin de la réunion confirmée");
        match r.compte_rendu_garde.take() {
            Some(resultat) => self.compte_rendu(resultat),
            None => self.emettre_enregistrement(),
        }
    }

    /// « Annuler » : la même réunion reprend (même dossier, compteur
    /// continu, rien de perdu de ce qui restait à noter) ; le compte rendu
    /// demandé est abandonné.
    fn annuler_fin(&mut self) -> String {
        let Some(r) = &mut self.reunion else {
            return kobi_i18n::tr("coque.chef.aucuneReunion", &[]);
        };
        r.fin_demandee = false;
        r.annulable_jusqu = None;
        r.pause = false;
        r.pause_a = None;
        r.arret = None;
        r.vidange = None;
        let reste = std::mem::take(&mut r.a_noter);
        r.a_noter = std::mem::take(&mut r.pour_compte_rendu) + &reste;
        // Compte rendu déjà demandé et encore chez Claude : sa réponse sera
        // ignorée. Demandé seulement après la vidange (KOB-198), il peut ne
        // pas l'être encore.
        if std::mem::take(&mut r.compte_rendu_demande) && r.compte_rendu_garde.take().is_none() {
            self.comptes_rendus_abandonnes += 1;
        }
        self.transition(&[Evenement::Reprendre]);
        self.emettre_enregistrement();
        tracing::info!("fin de la réunion annulée");
        kobi_i18n::tr("coque.chef.reunionContinue", &[])
    }

    /// Le compte rendu part dès que les pistes ont transcrit ce qui a été dit
    /// avant l'arrêt (tout de suite si rien n'est en retard), au plus tard
    /// après [`VIDANGE`] (KOB-198).
    fn vider_puis_compte_rendu(&mut self) {
        let Some(r) = &mut self.reunion else { return };
        r.vidange = Some(Instant::now() + VIDANGE);
        self.vidange();
    }

    /// Compte rendu demandé si les pistes ont rattrapé l'arrêt ou si l'attente
    /// est finie.
    fn vidange(&mut self) {
        let Some(r) = &mut self.reunion else { return };
        let (Some(limite), Some(arret)) = (r.vidange, r.arret) else {
            return;
        };
        let a_jour = self.avancement.a_jour(arret);
        if !a_jour && Instant::now() < limite {
            return;
        }
        let attente_ms = arret.elapsed().as_millis() as u64;
        if a_jour {
            tracing::info!(
                attente_ms,
                "fin de réunion : transcription à jour, compte rendu demandé"
            );
        } else {
            tracing::warn!(
                attente_ms,
                "fin de réunion : transcription encore en retard, compte rendu demandé sans la fin"
            );
        }
        r.vidange = None;
        self.demander_compte_rendu();
    }

    /// Envoie à Claude la demande de compte rendu de la réunion.
    fn demander_compte_rendu(&mut self) {
        let Some(r) = &mut self.reunion else { return };
        r.compte_rendu_demande = true;
        // Mode fin de réunion (KOB-71) : le titre n'a pas été demandé pendant ;
        // il part avant le compte rendu, et revient avant lui.
        if r.a_la_fin && !r.titre_demande && !r.titre_manuel && r.titre_auto {
            r.titre_demande = true;
            tracing::info!("titre de la réunion demandé avec le compte rendu");
            self.cerveau.demander(Demande::Titre {
                transcript: r.debut_parole.clone(),
            });
        }
        let reste = std::mem::take(&mut r.a_noter);
        // Rendu à la réunion si la fin est annulée (KOB-245) ; le bloc parti à
        // la prise de notes y revient de lui-même s'il échoue.
        r.pour_compte_rendu = reste.clone();
        let transcript = transcript_du_compte_rendu(r.envoye.as_deref(), &reste);
        let notes = r.notes.clone();
        self.cerveau
            .demander(Demande::CompteRendu { transcript, notes });
    }

    /// Segment final, dont la fin a été dite à `dit_a` : écrit tout de suite
    /// dans `transcript.jsonl`.
    fn segment(&mut self, mut s: Segment, dit_a: Instant) {
        let Some(r) = &mut self.reunion else { return };
        if !s.is_final {
            return;
        }
        // En pause, seul ce qui a été dit avant la pause est gardé : sous
        // charge, il arrive après (KOB-261).
        if r.pause && r.pause_a.is_none_or(|pause| dit_a > pause) {
            return;
        }
        // Dit avant la fin mais transcrit après (charge, file d'attente) :
        // gardé jusqu'au compte rendu. Ce qui est dit après (le « annule »
        // d'« Annuler »…) ne l'est pas (KOB-198).
        if r.compte_rendu_demande || (r.fin_demandee && r.arret.is_none_or(|arret| dit_a > arret)) {
            return;
        }
        // Horodaté sur la réunion, au moment où il a été dit : l'horloge du
        // moteur repart à chaque question, et le segment arrive en retard
        // quand la machine est chargée.
        let duree = (s.end - s.start).max(0.0);
        s.end = dit_a
            .min(Instant::now())
            .saturating_duration_since(r.debut)
            .as_secs_f64();
        s.start = (s.end - duree).max(0.0);
        self.ecrivain.segment(s.clone());
        let t = s.start as u64;
        let ligne = format!(
            "[{:02}:{:02} {}] {}\n",
            t / 60,
            t % 60,
            match s.source {
                kobi_stt::Source::Moi => "moi",
                kobi_stt::Source::Eux => "eux",
            },
            s.text
        );
        r.a_noter.push_str(&ligne);
        if !r.titre_demande && r.debut_parole.len() < TITRE_PAROLE_MAX {
            r.debut_parole.push_str(&ligne);
        }
        self.pour_conversation.push_str(&ligne);
    }

    /// Réponse que Kobi donne sans Claude (« répète », « où en est… ? »,
    /// « annule ») : affichée et lue comme une réponse ordinaire.
    fn repondre_localement(&mut self, question: String, reponse: String, voix: bool) {
        self.tour += 1;
        self.dernier = self.tour;
        self.calme_a = None;
        self.transition(&[Evenement::QuestionEnvoyee]);
        self.transition(&[Evenement::ReponseCommence]);
        let _ = self.app.emit(EVENEMENT_REPONSE, &reponse);
        crate::voix::dire(&self.app, &reponse);
        tracing::info!(tour = self.tour, "réponse locale");
        self.ranger(Qui::Moi, &question, voix);
        self.ranger(Qui::Kobi, &reponse, false);
        self.ajouter(Bulle {
            tour: self.tour,
            question,
            reponse: reponse.clone(),
            statut: "fini",
            clavier: !voix,
            etapes: Vec::new(),
        });
        self.lire(&reponse);
    }

    /// « Répète » avant toute réponse : rien à répéter, ou une tâche qui
    /// tourne encore.
    fn pas_encore_de_reponse(&self) -> String {
        if self.taches.is_empty() {
            sans_reponse()
        } else {
            self.avancement()
        }
    }

    /// Avancement des tâches longues.
    fn avancement(&self) -> String {
        if self.taches.is_empty() {
            return kobi_i18n::tr("coque.chef.aucuneTache", &[]);
        }
        let taches: Vec<String> = self
            .taches
            .values()
            .map(|(debut, q)| {
                kobi_i18n::tr(
                    "coque.chef.tacheDepuis",
                    &[
                        ("secondes", &debut.elapsed().as_secs().to_string()),
                        ("question", q),
                    ],
                )
            })
            .collect();
        let taches = taches.join(&kobi_i18n::tr("coque.chef.separateurTaches", &[]));
        kobi_i18n::tr("coque.chef.travailleEncore", &[("taches", &taches)])
    }

    /// Annule la dernière tâche longue lancée.
    fn annuler(&mut self) -> String {
        let Some((&tour, (_, question))) = self.taches.iter().next_back() else {
            return kobi_i18n::tr("coque.chef.aucuneTacheAAnnuler", &[]);
        };
        let question = question.clone();
        self.taches.remove(&tour);
        self.interrompus.insert(tour);
        self.cerveau.demander(Demande::Annuler { tour });
        if let Some(b) = self.bulle(tour) {
            b.statut = "fini";
            b.reponse = kobi_i18n::tr("coque.chef.tacheAnnulee", &[]);
        }
        self.emettre_historique();
        kobi_i18n::tr("coque.chef.aiAnnule", &[("question", &question)])
    }

    /// Coupe la réponse en cours : la bulle garde le texte déjà reçu, la
    /// suite du tour est ignorée, et Kobi revient tout de suite au calme.
    pub fn interrompre(&mut self) {
        let tour = self.dernier;
        let en_cours = self
            .bulle(tour)
            .is_some_and(|b| matches!(b.statut, "attente" | "reponse"));
        if en_cours {
            if let Some(b) = self.bulle(tour) {
                b.statut = "fini";
            }
            self.interrompus.insert(tour);
            self.cerveau.demander(Demande::Interrompre { tour });
            self.emettre_historique();
        }
        tracing::info!(tour, en_cours, "réponse interrompue");
        crate::voix::taire(&self.app, "interruption");
        self.voix = None;
        self.suite_apres_voix = false;
        self.calme_a = None;
        match self.activite() {
            Some("reflechit") => {
                self.transition(&[Evenement::ReponseCommence]);
                self.transition(&[Evenement::ReponseTerminee]);
            }
            Some("repond") => {
                self.transition(&[Evenement::ReponseTerminee]);
            }
            Some("ecoute") => {
                self.transition(&[Evenement::PasDeQuestion]);
            }
            _ => {}
        }
    }

    /// Une action qui écrit attend un « oui » : Kobi passe en proposition et
    /// écoute la réponse sans qu'on redise « Kobi » (hors réunion). Seule la
    /// carte affichée est annoncée ; les suivantes le seront à leur tour
    /// (KOB-218).
    fn validation_ouverte(&mut self, id: String, resume: &str) {
        let annonce = self.validations.ouvrir(id, resume);
        self.calme_a = None;
        self.transition(&[Evenement::PermissionDemandee]);
        if let Some(resume) = annonce {
            self.annoncer_validation(&resume);
        }
        // « Oui » ou « non » sans le nom, tant que la carte attend (KOB-197).
        let _ = crate::ecoute::commander(
            &self.app,
            crate::ecoute::Commande::ReponseAttendue(crate::validation::ECHEANCE),
        );
    }

    fn validation_fermee(&mut self, id: &str, reponse: crate::validation::Reponse) {
        let suivante = self.validations.fermer(id);
        if self.validations.est_vide() {
            self.transition(&[if reponse.acceptee() {
                Evenement::PermissionAccordee
            } else {
                Evenement::PermissionRefusee
            }]);
        }
        // Plus de carte : la fenêtre de suite reprend sa durée habituelle (KOB-197).
        if self.validations.est_vide() {
            let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Suite);
        }
        // La carte montre déjà « Pas de réponse : refusé. » ; la voix ne le
        // redit que si elle est le seul canal (KOB-215).
        if reponse == crate::validation::Reponse::Delai && self.voix_seul_canal() {
            let message = kobi_i18n::tr("coque.chef.delaiCarte", &[]);
            let _ = self.app.emit(EVENEMENT_REPONSE, &message);
            crate::voix::dire(&self.app, &message);
        }
        if let Some(resume) = suivante {
            self.annoncer_validation(&resume);
            // La carte suivante attend à son tour sa réponse (KOB-197).
            let _ = crate::ecoute::commander(
                &self.app,
                crate::ecoute::Commande::ReponseAttendue(crate::validation::ECHEANCE),
            );
        }
    }

    /// La carte se lit à l'écran : la voix demande seulement « Je le fais ? ».
    /// Fenêtre de Kobi cachée, la voix est le seul canal : elle relit la
    /// demande (KOB-215).
    fn annoncer_validation(&self, resume: &str) {
        let phrase = if self.voix_seul_canal() {
            kobi_i18n::tr("coque.chef.resumeJeLeFais", &[("resume", resume)])
        } else {
            kobi_i18n::tr("coque.chef.jeLeFais", &[])
        };
        crate::voix::dire(&self.app, &phrase);
    }

    /// La fenêtre de Kobi n'est pas à l'écran : ce qui s'affiche ne se voit pas.
    fn voix_seul_canal(&self) -> bool {
        !self
            .app
            .get_webview_window("main")
            .is_some_and(|w| w.is_visible().unwrap_or(true))
    }

    fn retour(&mut self, retour: Retour) {
        let tour_du_retour = match &retour {
            Retour::Texte { tour, .. }
            | Retour::Fin { tour, .. }
            | Retour::Detache { tour, .. }
            | Retour::Resultat { tour, .. }
            | Retour::Erreur { tour, .. }
            | Retour::Relance { tour }
            | Retour::Patience { tour }
            | Retour::EnAttente { tour, .. } => Some(*tour),
            _ => None,
        };
        if tour_du_retour.is_some_and(|t| self.interrompus.contains(&t)) {
            return;
        }
        match retour {
            // Phrase peut-être pas pour Kobi : rien ne s'affiche avant la fin.
            Retour::Texte { tour, .. } if self.tour_peut_etre == Some(tour) => {}
            Retour::Fin { tour, texte }
                if self.tour_peut_etre == Some(tour) && est_silence(&texte) =>
            {
                tracing::info!(tour, "phrase qui ne s'adressait pas à Kobi : silence");
                self.tour_peut_etre = None;
                if let Some(r) = &mut self.reunion {
                    r.latences.abandon(tour);
                }
                self.historique.retain(|b| b.tour != tour);
                if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                    self.voix = None;
                }
                if tour == self.dernier {
                    // Retour au calme sans montrer de réponse.
                    self.transition(&[Evenement::ReponseCommence]);
                    self.transition(&[Evenement::ReponseTerminee]);
                }
                self.emettre_historique();
            }
            Retour::Texte { tour, texte } => {
                if let Some(r) = &mut self.reunion {
                    r.latences.premier_mot(tour, Instant::now());
                }
                if let Some(b) = self.bulle(tour) {
                    b.reponse.clone_from(&texte);
                    b.statut = "reponse";
                }
                if tour == self.dernier {
                    self.transition(&[Evenement::ReponseCommence]);
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                }
                if let Some((t, r)) = &mut self.voix {
                    if *t == tour {
                        r.morceau(&self.app, &texte);
                    }
                }
                self.emettre_historique();
            }
            Retour::Fin { tour, texte } => {
                tracing::info!(tour, "réponse terminée");
                if let Some(r) = &mut self.reunion {
                    r.latences.fin(tour, Instant::now());
                }
                if self.tour_peut_etre == Some(tour) {
                    self.tour_peut_etre = None;
                }
                self.retenir(tour, &texte);
                self.ranger(Qui::Kobi, &texte, false);
                if let Some(b) = self.bulle(tour) {
                    b.reponse.clone_from(&texte);
                    b.statut = "fini";
                }
                if let Some((t, mut r)) = self.voix.take() {
                    if t == tour {
                        r.fin(&self.app, &texte);
                    } else {
                        self.voix = Some((t, r));
                    }
                }
                if tour == self.dernier {
                    self.transition(&[Evenement::ReponseCommence]);
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                    self.lire(&texte);
                }
                self.emettre_historique();
            }
            Retour::Detache { tour, annonce } => {
                tracing::info!(tour, annonce, "tour détaché");
                if annonce {
                    let question = self
                        .bulle(tour)
                        .map(|b| b.question.clone())
                        .unwrap_or_default();
                    self.taches.insert(tour, (Instant::now(), question));
                    if let Some(b) = self.bulle(tour) {
                        b.statut = "tache";
                    }
                    // Rien n'est dit ni écrit ici : Kobi au travail se voit,
                    // et la plupart des tâches finissent vite. Seule une
                    // tâche qui tarde est annoncée, une fois (`Patience`,
                    // KOB-215).
                    if tour == self.dernier {
                        if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                            self.voix = None;
                        }
                        // Kobi reste « je m'en occupe » jusqu'au résultat
                        // (KOB-152), sans retour au calme après l'annonce.
                        self.calme_a = None;
                        if !self.transition(&[Evenement::TacheLongue]) {
                            self.transition(&[Evenement::ReponseCommence]);
                        }
                    }
                    self.emettre_historique();
                }
            }
            Retour::Resultat { tour, texte } => {
                tracing::info!(tour, "résultat livré");
                if let Some(r) = &mut self.reunion {
                    r.latences.fin(tour, Instant::now());
                }
                self.taches.remove(&tour);
                self.retenir(tour, &texte);
                self.ranger(Qui::Kobi, &texte, false);
                let question = self.bulle(tour).map(|b| {
                    b.reponse.clone_from(&texte);
                    b.statut = "fini";
                    b.question.clone()
                });
                self.emettre_historique();
                // Montré à son tour : plusieurs résultats arrivés ensemble
                // passent l'un après l'autre (KOB-238).
                self.a_livrer.push_back((tour, texte, question));
                self.livrer();
            }
            Retour::Erreur { tour, message } => {
                tracing::warn!(tour, %message, "tour en erreur");
                if let Some(r) = &mut self.reunion {
                    r.latences.abandon(tour);
                }
                if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                    self.voix = None;
                }
                self.taches.remove(&tour);
                // La cause en une phrase et la marche à suivre (KOB-68).
                let (titre, explication) = crate::pannes::pour_l_utilisateur(&message);
                if let Some(b) = self.bulle(tour) {
                    b.reponse.clone_from(&explication);
                    b.statut = "erreur";
                }
                self.emettre_historique();
                if tour == self.dernier {
                    self.erreur(titre, &explication);
                }
            }
            Retour::Relance { tour } => self.patienter(tour, &crate::pannes::relance(), false),
            Retour::Patience { tour } => self.patienter(tour, &crate::pannes::patience(), true),
            Retour::EnAttente { tour, nombre } => {
                let (titre, explication) =
                    crate::pannes::expliquer(&crate::pannes::Panne::HorsLigne, "");
                if let Some(b) = self.bulle(tour) {
                    b.reponse =
                        kobi_i18n::tr("coque.chef.enAttenteReseau", &[("n", &nombre.to_string())]);
                    // Pas « tache » : l'Oups dit déjà qu'elle attend, sans
                    // notification en double (sobriété).
                    b.statut = "attente";
                }
                self.emettre_historique();
                if tour == self.dernier {
                    self.erreur(titre, &explication);
                }
            }
            Retour::Carte {
                tour,
                carte,
                outil,
                entree,
            } => {
                tracing::info!(tour, carte, %outil, "carte de confirmation");
                let ui = carte_ui(carte, &outil, &entree);
                self.cartes.insert(carte, ui.clone());
                self.calme_a = None;
                self.transition(&[Evenement::PermissionDemandee]);
                let _ = self.app.emit(EVENEMENT_CARTE, &ui);
            }
            Retour::CarteFermee { carte, accepte } => {
                tracing::info!(carte, ?accepte, "carte fermée");
                if let Some(mut ui) = self.cartes.remove(&carte) {
                    ui.status = if accepte == Some(true) {
                        "confirmed"
                    } else {
                        "dismissed"
                    };
                    let _ = self.app.emit(EVENEMENT_CARTE, &ui);
                }
                self.transition(&[if accepte == Some(true) {
                    Evenement::PermissionAccordee
                } else {
                    Evenement::PermissionRefusee
                }]);
            }
            Retour::Activite { demande, at_ms } => self.activite_outil(&demande, at_ms),
            Retour::Notes(resultat) => self.notes(resultat),
            Retour::CompteRendu(resultat) => self.compte_rendu(resultat),
            Retour::Titre(resultat) => self.titre_recu(resultat),
        }
    }

    /// Où Kobi a le droit d'aller, pour classer les étapes du fil : les
    /// dossiers de code, la mémoire des réunions et les outils de lecture du
    /// MCP de Kobi.
    fn contexte(&self) -> kobi_perms::Contexte {
        let mut dossiers = dossiers_de_code();
        if let Some(st) = crate::stockage::actuel(&self.app) {
            dossiers.push(st.dossier_reunions());
            dossiers.push(st.dossier_conversations());
        }
        let mut ctx = kobi_perms::Contexte::new(dossiers);
        for outil in kobi_ctl::mcp::outils_de_lecture(crate::mcp_config::SERVEUR_KOBI) {
            ctx.mcp.add_read_only(outil);
        }
        ctx
    }

    /// Outil appelé : une ligne du fil sous Kobi, gardée sous la bulle.
    fn activite_outil(&mut self, demande: &kobi_brain::DemandeOutil, at_ms: u64) {
        let ui = ActiviteUi::nouvelle(demande, at_ms, &self.contexte());
        tracing::info!(
            tour = ui.tour,
            famille = ?ui.famille,
            classe = ui.classe,
            resume = %ui.resume,
            "activité"
        );
        let _ = self.app.emit(EVENEMENT_ACTIVITE, &ui);
        if let Some(b) = self.bulle(demande.tour) {
            b.etapes.push(ui);
            let trop = b.etapes.len().saturating_sub(ETAPES);
            b.etapes.drain(..trop);
            self.emettre_historique();
        }
    }

    fn notes(&mut self, resultat: Result<String, String>) {
        let Some(r) = &mut self.reunion else { return };
        let envoye = r.envoye.take().unwrap_or_default();
        match resultat {
            Ok(notes) if !notes.trim().is_empty() => {
                r.derniere_note = derniere_note(&r.notes, &notes).or(r.derniere_note.take());
                r.notes = notes;
                self.ecrivain.notes(r.notes.clone());
                tracing::info!(lignes = compter_notes(&r.notes), "notes mises à jour");
                self.emettre_enregistrement();
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(erreur = %e, "notes : bloc en échec, repris au prochain");
                r.a_noter.insert_str(0, &envoye);
            }
        }
    }

    fn compte_rendu(&mut self, resultat: Result<String, String>) {
        if self.comptes_rendus_abandonnes > 0 {
            // Demandé avant un « Annuler » : la réunion a repris.
            self.comptes_rendus_abandonnes -= 1;
            tracing::info!("compte rendu abandonné ignoré");
            return;
        }
        if let Some(r) = self
            .reunion
            .as_mut()
            .filter(|r| r.annulable_jusqu.is_some())
        {
            // « Annuler » encore possible : écrit à la fin du délai.
            r.compte_rendu_garde = Some(resultat);
            return;
        }
        let Some(r) = self.reunion.take() else {
            return;
        };
        let texte = match resultat {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(erreur = %e, "compte rendu en échec : notes gardées");
                // Une panne reconnue se dit dans la langue de l'interface ;
                // une autre garde son message (KOB-211).
                let cause = match crate::pannes::classer_message(&e) {
                    crate::pannes::Panne::Autre => e.clone(),
                    p => crate::pannes::expliquer(&p, &e).0.to_owned(),
                };
                kobi_i18n::tr(
                    "coque.chef.compteRenduSecours",
                    &[("erreur", &cause), ("notes", &r.notes)],
                )
            }
        };
        // Latence de Kobi pendant la réunion, en fin de compte rendu (KOB-85).
        let texte = match r.latences.resume() {
            Some(resume) => format!("{}\n\n{resume}", texte.trim_end()),
            None => texte,
        };
        self.compte_rendu_a_ecrire = true;
        self.ecrivain
            .terminer_reunion(texte, r.notes, maintenant_ms());
        self.transition(&[Evenement::CompteRenduEcrit]);
        self.emettre_enregistrement();
    }

    /// Garde la dernière réponse pour « répète », « plus court »…
    fn retenir(&mut self, tour: u64, texte: &str) {
        if let Some(question) = self.bulle(tour).map(|b| b.question.clone()) {
            self.derniere = Some((question, texte.to_owned()));
        }
    }

    /// Nouvelles de l'écrivain.
    fn disque(&mut self, compte: Nouvelle) {
        match compte {
            Nouvelle::ReunionOuverte(dossier) => {
                if let Some(r) = &mut self.reunion {
                    tracing::info!(
                        dossier = %dossier.display(),
                        ms = r.debut.elapsed().as_millis() as u64,
                        "fichiers de la réunion prêts"
                    );
                    r.dossier = Some(dossier);
                }
            }
            Nouvelle::ReunionNommee {
                dossier,
                ancien,
                titre,
            } => {
                let nouveau = dossier
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                if let Some(r) = &mut self.reunion {
                    r.dossier = Some(dossier.clone());
                    if titre.is_some() {
                        r.titre = titre.clone();
                    }
                }
                tracing::info!(?ancien, %nouveau, "réunion nommée");
                let _ = self.app.emit(
                    EVENEMENT_REUNION_NOMMEE,
                    ReunionNommee {
                        ancien,
                        reunion: nouveau,
                        titre,
                    },
                );
                self.emettre_enregistrement();
            }
            Nouvelle::NotesEcrites(dossier) => {
                let notes = self.reunion.as_ref().map_or(0, |r| compter_notes(&r.notes));
                let charge = NotesEcrites {
                    reunion: dossier
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    dossier: dossier.display().to_string(),
                    notes,
                };
                tracing::info!(notes, "notes écrites");
                let _ = self.app.emit(EVENEMENT_NOTES, charge);
            }
            Nouvelle::CompteRenduEcrit(chemin) => {
                self.compte_rendu_a_ecrire = false;
                tracing::info!(fichier = %chemin.display(), "compte rendu écrit");
                let reunion = chemin
                    .parent()
                    .and_then(|d| d.file_name())
                    .map(|n| n.to_string_lossy().into_owned());
                match (reunion, std::fs::read_to_string(&chemin)) {
                    (Some(reunion), Ok(texte)) => {
                        let _ = self
                            .app
                            .emit(EVENEMENT_COMPTE_RENDU, compte_rendu_ui(&reunion, &texte));
                        // Annoncé une fois à voix haute, si la voix est active (KOB-204).
                        crate::voix::dire(
                            &self.app,
                            &kobi_i18n::tr("coque.chef.compteRenduPret", &[]),
                        );
                    }
                    (_, Err(e)) => tracing::warn!(erreur = %e, "compte rendu illisible"),
                    _ => {}
                }
                self.tour += 1;
                self.ajouter(Bulle {
                    tour: self.tour,
                    question: kobi_i18n::tr("coque.chef.finDeReunion", &[]),
                    reponse: kobi_i18n::tr(
                        "coque.chef.compteRenduEcrit",
                        &[("chemin", &chemin.display().to_string())],
                    ),
                    statut: "fini",
                    clavier: false,
                    etapes: Vec::new(),
                });
            }
            Nouvelle::Erreur(message) => {
                // Dont un compte rendu qui n'a pas pu être rangé : plus rien
                // à attendre pour quitter.
                self.compte_rendu_a_ecrire = false;
                tracing::warn!(%message, "écriture sur le disque");
                self.erreur(&kobi_i18n::tr("coque.chef.erreurDisque", &[]), &message);
            }
        }
    }

    /// La question, précédée du moment présent et de l'état de la réunion :
    /// où en est l'enregistrement, que Claude pilote (KOB-165), ou qu'aucune
    /// réunion n'est en cours (KOB-181).
    fn avec_contexte(&self, question: String) -> String {
        let maintenant = moment(&jiff::Zoned::now());
        let etat = match &self.reunion {
            // Dit aussi hors réunion : sans cela, Claude en suppose une
            // (« comment se passe ta réunion ? », KOB-181).
            None => {
                return format!(
                    "{maintenant}\n(Aucune réunion en cours.){}\n{question}",
                    consigne_longueur(crate::voix::verbosite(&self.app))
                )
            }
            Some(r) if r.fin_demandee => "arrêté, compte rendu à venir",
            Some(r) if r.pause => "en pause",
            Some(_) => "en cours",
        };
        format!(
            "{maintenant}\n(Réunion : enregistrement {etat}.){}\n{question}",
            consigne_longueur(crate::voix::verbosite(&self.app))
        )
    }

    fn fermer_conversation(&mut self) {
        if self.conversation.take().is_some() {
            self.ecrivain.fermer_conversation();
            let _ = self.app.emit(EVENEMENT_FIN_CONVERSATION, ());
        }
    }

    /// Kobi a fini de répondre : temps de lecture, puis la conversation
    /// attend la suite sans qu'on redise « Kobi » (KOB-133).
    fn lire(&mut self, texte: &str) {
        let lecture = LECTURE_MIN + Duration::from_millis(60 * texte.chars().count() as u64);
        self.calme_a = Some((
            Instant::now() + lecture.min(Duration::from_secs(25)),
            Evenement::ReponseTerminee,
        ));
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Suite);
        self.suite_apres_voix = true;
    }

    /// Tour qui tarde (`longue`) ou relancé : la bulle le dit, et Kobi aussi
    /// si c'est la question en cours (KOB-68).
    fn patienter(&mut self, tour: u64, phrase: &str, longue: bool) {
        tracing::info!(tour, phrase, "tour : patience");
        // Discret : la tâche longue ne s'annonce pas, Kobi au travail se voit.
        if longue && crate::voix::verbosite(&self.app) == kobi_session::config::Verbosite::Discret {
            return;
        }
        if let Some(b) = self.bulle(tour) {
            if b.reponse.is_empty() || b.statut == "tache" {
                b.reponse = phrase.into();
            }
        }
        self.emettre_historique();
        // Une seule annonce tant que des tâches tournent : une autre déjà
        // annoncée vaut pour celle-ci (rafale, KOB-238).
        self.annoncees.retain(|t| self.taches.contains_key(t));
        let deja = self.annoncees.iter().any(|t| *t != tour);
        self.annoncees.insert(tour);
        if tour == self.dernier && !deja {
            let _ = self.app.emit(EVENEMENT_REPONSE, phrase);
            crate::voix::dire(&self.app, phrase);
        }
    }

    fn erreur(&mut self, titre: &str, message: &str) {
        self.transition(&[Evenement::Erreur]);
        #[derive(Clone, Serialize)]
        struct ErreurUi<'a> {
            title: &'a str,
            message: &'a str,
        }
        let _ = self.app.emit(
            EVENEMENT_ERREUR,
            ErreurUi {
                title: titre,
                message,
            },
        );
        self.calme_a = Some((Instant::now() + ERREUR_AFFICHEE, Evenement::Acquitter));
    }

    /// Montre le prochain résultat de tâche, s'il peut l'être : pas pendant
    /// que Kobi écoute, propose une carte, parle, ou qu'une réponse se lit
    /// encore. Kobi passe d'abord « au travail » s'il n'y est pas, puis la
    /// réponse vient : une vraie transition, pas écoute → réflexion →
    /// réponse dans la même milliseconde (KOB-238).
    fn livrer(&mut self) {
        if self.a_livrer.is_empty() {
            return;
        }
        let activite = self.activite();
        let lit_encore = self
            .calme_a
            .is_some_and(|(_, e)| matches!(e, Evenement::ReponseTerminee));
        if matches!(
            activite,
            Some("ecoute" | "reflechit" | "repond" | "propose")
        ) || lit_encore
            || crate::voix::parle(&self.app)
            || self.livrable_a.is_some_and(|t| Instant::now() < t)
        {
            return;
        }
        // Réunion en pause ou qui se termine : pas d'état « au travail »,
        // le résultat est montré tel quel, sans attendre la reprise.
        let sans_activite = matches!(etat(&self.app).etat, "pause" | "fin");
        if activite != Some("occupe") && !sans_activite {
            // Sans état « au travail » possible ici (oups…), on réessaie au
            // prochain tic.
            self.livrable_a = Some(Instant::now() + TRANSITION_RESULTAT);
            self.transition(&[Evenement::TacheLongue]);
            return;
        }
        self.livrable_a = None;
        let Some((tour, texte, question)) = self.a_livrer.pop_front() else {
            return;
        };
        // La question n'est rappelée que si une autre a été posée depuis :
        // sinon la bulle la répétait (KOB-215).
        if tour != self.dernier {
            if let Some(q) = question {
                let _ = self.app.emit(EVENEMENT_QUESTION, &q);
            }
        }
        self.dernier = tour;
        self.transition(&[Evenement::ReponseCommence]);
        let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
        crate::voix::dire(&self.app, &texte);
        self.lire(&texte);
    }

    /// Délais : retour au calme, blocs de notes.
    pub fn tick(&mut self) {
        let parle = crate::voix::parle(&self.app);
        if let Some((quand, evenement)) = self.calme_a {
            // Le retour au calme attend que Kobi ait fini de parler.
            if Instant::now() >= quand && !parle {
                self.calme_a = None;
                self.transition(&[evenement]);
            }
        }
        // Résultats de tâches en attente : le suivant, s'il peut passer.
        self.livrer();
        // Kobi s'est tu : la conversation attend la suite à partir de maintenant.
        if self.suite_apres_voix && !parle {
            self.suite_apres_voix = false;
            let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Suite);
        }
        // Revenu au calme alors qu'une tâche tourne encore : Kobi s'en occupe.
        if !self.taches.is_empty()
            && self.calme_a.is_none()
            && matches!(self.activite(), Some("calme") | None)
            && matches!(etat(&self.app).etat, "veille" | "reunion")
        {
            self.transition(&[Evenement::TacheLongue]);
        }
        // Longtemps en veille : la prochaine question ouvre une autre conversation.
        if self
            .conversation
            .is_some_and(|dernier| dernier.elapsed() >= FIN_CONVERSATION)
            && etat(&self.app).etat == "veille"
        {
            self.fermer_conversation();
        }
        // Fin : le compte rendu attend la transcription en retard (KOB-198).
        if self.reunion.as_ref().is_some_and(|r| r.vidange.is_some()) {
            self.vidange();
        }
        let avancement = self.app.try_state::<Avancement>();
        let Some(r) = &mut self.reunion else { return };
        // « Annuler » n'a pas servi : la fin est définitive, une fois
        // transcrit ce qui a été dit pendant le délai (micro en retard,
        // KOB-265), au plus [`VIDANGE`] de plus.
        if r.annulable_jusqu.is_some_and(|quand| {
            let maintenant = Instant::now();
            maintenant >= quand
                && (maintenant >= quand + VIDANGE
                    || avancement
                        .as_ref()
                        .is_none_or(|a| a.piste_a_jour(kobi_stt::Source::Moi, quand)))
        }) {
            self.clore_annulation();
            return;
        }
        // Titre de la réunion (KOB-223) : demandé d'après les premières
        // minutes, plus tôt si la parole abonde, plus tard s'il y en a peu.
        if !r.titre_demande && !r.titre_manuel && !r.pause && !r.fin_demandee && !r.a_la_fin {
            let mots = r.debut_parole.split_whitespace().count();
            let seuil = TITRE_MOTS * (usize::from(r.titre_essais) + 1);
            let depuis = r.debut.elapsed();
            let pret = (depuis >= TITRE_APRES && mots >= seuil)
                || mots >= 3 * seuil
                || (depuis >= 2 * TITRE_APRES && mots >= TITRE_MOTS_MIN);
            if pret && r.titre_essais < TITRE_ESSAIS && r.titre_auto {
                r.titre_demande = true;
                tracing::info!(mots, s = depuis.as_secs(), "titre de la réunion demandé");
                self.cerveau.demander(Demande::Titre {
                    transcript: r.debut_parole.clone(),
                });
            }
        }
        if r.pause
            || r.fin_demandee
            || r.a_la_fin
            || r.envoye.is_some()
            || Instant::now() < r.prochain_bloc
        {
            return;
        }
        r.prochain_bloc = Instant::now() + self.intervalle;
        if r.a_noter.trim().is_empty() {
            return;
        }
        let bloc = std::mem::take(&mut r.a_noter);
        r.envoye = Some(bloc.clone());
        tracing::info!(octets = bloc.len(), "bloc de notes envoyé");
        self.cerveau.demander(Demande::Notes {
            transcript: bloc,
            notes: r.notes.clone(),
        });
    }
}

/// Où sont les réunions passées, lu à l'ouverture de chaque session : le
/// dossier du stockage se règle à chaud (KOB-141).
pub type SourceMemoire = Arc<dyn Fn() -> Option<crate::lecture::Memoire> + Send + Sync>;
/// L'interface a dessiné une étape du fil d'activité : le délai depuis
/// l'appel d'outil reçu de Claude part au journal (critère de KOB-152,
/// moins de 300 ms).
#[tauri::command]
pub fn activite_affichee(id: String, at_ms: u64) {
    let latence_ms = maintenant_ms().saturating_sub(at_ms);
    tracing::info!(%id, latence_ms, "activité affichée");
}

/// Dossiers de code déclarés (`codebases`), pour raccourcir les chemins du
/// fil d'activité ; relus à chaque étape, ils suivent les Réglages.
fn dossiers_de_code() -> Vec<PathBuf> {
    kobi_session::config::default_path()
        .map(|p| kobi_session::config::load_or_create(&p).config.codebases)
        .unwrap_or_default()
}

/// Sessions Claude Code réelles : Read, Grep et Glob sur les dossiers de code
/// et la mémoire des réunions (KOB-138, KOB-76), le MCP de Kobi en lecture
/// (KOB-161), les serveurs MCP de l'utilisateur derrière le serveur de
/// permission de Kobi (KOB-155), modèle rapide, prompt système de Kobi avec
/// la carte des dépôts (KOB-153). Dossiers et serveurs sont relus à chaque
/// ouverture : un changement vaut pour la session suivante.
pub fn fabrique_claude(
    modele: &'static str,
    dossiers: crate::lecture::Dossiers,
    memoire: SourceMemoire,
) -> Fabrique {
    // Commandes en lecture seule (KOB-154) : le hook qui les valide.
    let reglages = kobi_session::config::kobi_dir()
        .map(|d| d.join("mcp").join("reglages-claude.json"))
        .and_then(|f| {
            crate::commandes::reglages(&f)
                .map_err(|e| tracing::warn!(erreur = %e, "réglages des commandes : Kobi ne lancera pas de commande"))
                .ok()
        });
    let programme: Arc<tokio::sync::OnceCell<Result<PathBuf, String>>> = Arc::default();
    Arc::new(move |role| {
        let codebases = Arc::new(dossiers.read().unwrap_or_else(|e| e.into_inner()).clone());
        let (programme, memoire) = (programme.clone(), memoire());
        let reglages = reglages.clone();
        Box::pin(async move {
            let programme = programme
                .get_or_init(|| async {
                    match kobi_brain::claude_code::detect::detect(Duration::from_secs(30)).await {
                        kobi_brain::Detection::Ready { install, .. } => Ok(install.program),
                        kobi_brain::Detection::NotLoggedIn(_) => Err(
                            "Claude Code n'est pas connecté : lancer « claude auth login »".into(),
                        ),
                        autre => Err(format!("Claude Code indisponible : {autre:?}")),
                    }
                })
                .await
                .clone()
                .map_err(kobi_brain::BrainError::Unavailable)?;
            let cwd = crate::lecture::dossier_de_travail(&codebases);
            if let Some(cwd) = &cwd {
                let _ = std::fs::create_dir_all(cwd);
            }
            // Serveurs MCP relus à chaque session : un changement dans les
            // réglages vaut pour la conversation suivante (KOB-155).
            let mcp =
                crate::mcp::session_claude(role, modele, codebases.clone(), memoire, reglages);
            let config = kobi_brain::MeetingConfig {
                program: programme,
                cwd,
                args_with_perms: Some(mcp.args),
                permission_rules: mcp.regles,
                env: mcp.env,
                ..Default::default()
            };
            let debut = Instant::now();
            let session = kobi_brain::ClaudeCodeBrain::start(config).await?;
            tracing::info!(
                ?role,
                codebases = ?codebases,
                ms = debut.elapsed().as_millis() as u64,
                "session Claude ouverte"
            );
            Ok(Box::new(session) as Box<dyn kobi_brain::Brain + Send>)
        })
    })
}

/// Date et heure locales, données à Claude avec chaque question (KOB-182) :
/// sans dossier de code, il ne peut lancer aucune commande, pas même `date`,
/// et ne savait plus dire l'heure.
/// Longueur voulue, ajoutée à la question (KOB-215) ; rien en normal : le
/// prompt système demande déjà une ou deux phrases.
fn consigne_longueur(v: kobi_session::config::Verbosite) -> &'static str {
    use kobi_session::config::Verbosite::*;
    match v {
        Bavard => "\n(Tu peux répondre en 2 à 4 phrases.)",
        Normal => "",
        Discret => "\n(Réponds en une phrase.)",
    }
}

fn moment(m: &jiff::Zoned) -> String {
    const JOURS: [&str; 7] = [
        "lundi", "mardi", "mercredi", "jeudi", "vendredi", "samedi", "dimanche",
    ];
    const MOIS: [&str; 12] = [
        "janvier",
        "février",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "août",
        "septembre",
        "octobre",
        "novembre",
        "décembre",
    ];
    format!(
        "(Maintenant : {} {} {} {}, {} h {:02}.)",
        JOURS[m.weekday().to_monday_zero_offset() as usize],
        m.day(),
        MOIS[m.month() as usize - 1],
        m.year(),
        m.hour(),
        m.minute()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KOB-210 : les annonces de Kobi en anglais, mêmes variables.
    #[test]
    fn annonces_en_anglais() {
        use kobi_i18n::{t, Langue};
        assert_eq!(
            t(Langue::En, "coque.chef.reunionTerminee", &[]),
            "Meeting over, I'm preparing the summary."
        );
        assert_eq!(
            t(
                Langue::En,
                "coque.chef.reunionRenommee",
                &[("titre", "Devis")]
            ),
            "Got it: the meeting is called “Devis”."
        );
        assert_eq!(
            t(Langue::En, "coque.chef.enAttenteReseau", &[("n", "1")]),
            "Waiting for the network (1 question)."
        );
        assert_eq!(
            t(Langue::En, "coque.chef.enAttenteReseau", &[("n", "3")]),
            "Waiting for the network (3 questions)."
        );
        assert_eq!(
            t(Langue::Fr, "coque.chef.enAttenteReseau", &[("n", "1")]),
            "En attente du réseau (1 question)."
        );
        assert_eq!(
            t(
                Langue::En,
                "coque.rappels.rappelNote",
                &[("quand", "tomorrow at 9"), ("texte", "call Paul")]
            ),
            "Got it: I'll remind you tomorrow at 9 to call Paul."
        );
    }

    /// KOB-215 : la longueur voulue part avec la question, sauf en normal.
    #[test]
    fn consigne_de_longueur_selon_la_verbosite() {
        use kobi_session::config::Verbosite::*;
        assert_eq!(consigne_longueur(Normal), "");
        assert!(consigne_longueur(Discret).contains("une phrase"));
        assert!(consigne_longueur(Bavard).contains("2 à 4 phrases"));
    }

    /// KOB-182 : Claude reçoit la date et l'heure locales avec chaque
    /// question, même sans dossier de code (pas de commande `date`).
    #[test]
    fn moment_en_francais() {
        let tz = jiff::tz::TimeZone::fixed(jiff::tz::Offset::constant(2));
        let m = jiff::civil::date(2026, 10, 19)
            .at(9, 5, 0, 0)
            .to_zoned(tz.clone())
            .unwrap();
        assert_eq!(moment(&m), "(Maintenant : lundi 19 octobre 2026, 9 h 05.)");
        let m = jiff::civil::date(2026, 2, 1)
            .at(23, 59, 0, 0)
            .to_zoned(tz)
            .unwrap();
        assert_eq!(
            moment(&m),
            "(Maintenant : dimanche 1 février 2026, 23 h 59.)"
        );
    }

    /// Phrase qui ne s'adressait peut-être pas à Kobi (KOB-168) : « — » ou
    /// rien en réponse le laisse silencieux ; une vraie réponse s'affiche.
    #[test]
    fn reponse_silence_a_une_phrase_qui_ne_lui_parlait_pas() {
        for silence in ["", "  ", "—", " — ", "–", "-", "…", "(silence)"] {
            assert!(est_silence(silence), "{silence:?}");
        }
        assert!(!est_silence("Il est 14 h 05."));
        assert!(!est_silence("— Il est 14 h 05."));
        assert!(CONSIGNE_PEUT_ETRE.contains("réponds seulement « — »"));
        assert!(CONSIGNE_PEUT_ETRE.ends_with("Phrase : "));
    }

    #[test]
    fn compte_rendu_resume_pour_la_carte() {
        let md = "# Compte rendu\n\n## Résumé\nBudget du séminaire validé.\nLieu : Annecy.\n\n## Décisions\n- Budget validé\n- Annecy\n\n## Actions\n- [ ] Sophie : devis\n\n## Questions ouvertes\n- Date exacte ?\n\n## Interpellations\n- Kobi, le budget ? : 12 k€\n";
        let ui = compte_rendu_ui("2026-10-18-point-hebdo", md);
        assert_eq!(ui.reunion, "2026-10-18-point-hebdo");
        assert_eq!(ui.resume, "Budget du séminaire validé.\nLieu : Annecy.");
        assert_eq!((ui.decisions, ui.actions, ui.questions), (2, 1, 1));
        // Compte rendu de secours (Claude en échec) : pas de sections, rien à compter.
        let secours = compte_rendu_ui(
            "r",
            "# Compte rendu\n\nLe compte rendu n'a pas pu être écrit (Claude absent).\n\n## Notes\n\n- budget\n",
        );
        assert_eq!(
            secours.resume,
            "Le compte rendu n'a pas pu être écrit (Claude absent)."
        );
        assert_eq!(
            (secours.decisions, secours.actions, secours.questions),
            (0, 0, 0)
        );
        // En anglais (KOB-211) : la cause d'une panne reconnue est traduite.
        let cause = crate::pannes::expliquer(&crate::pannes::Panne::Deconnecte, "").0;
        let md = kobi_i18n::t(
            kobi_i18n::Langue::En,
            "coque.chef.compteRenduSecours",
            &[("erreur", cause), ("notes", "- budget\n")],
        );
        let en = compte_rendu_ui("r", &md);
        assert!(en.resume.starts_with("The summary"), "{}", en.resume);
        assert_eq!((en.decisions, en.actions, en.questions), (0, 0, 0));
    }

    #[test]
    fn compteur_de_notes() {
        let notes = "## Décisions\n- budget validé\n- séminaire à Annecy\n\n## Actions\n* Sophie : devis\ntexte libre\n";
        assert_eq!(compter_notes(notes), 3);
        assert_eq!(compter_notes(""), 0);
    }

    #[test]
    fn derniere_note_ajoutee() {
        let avant = "## Décisions\n- budget validé\n";
        let apres =
            "## Décisions\n- budget validé\n- séminaire à Annecy\n## Actions\n- **Sophie** : devis\n";
        assert_eq!(
            derniere_note(avant, apres).as_deref(),
            Some("Sophie : devis")
        );
        assert_eq!(
            derniere_note(apres, apres).as_deref(),
            Some("Sophie : devis")
        );
        assert_eq!(derniere_note("", "texte libre"), None);
        assert_eq!(
            derniere_note("", "- `cargo test` à lancer").as_deref(),
            Some("cargo test à lancer")
        );
    }

    #[test]
    fn reprise_rappelle_la_reponse() {
        let d = ("quelle heure ?".to_string(), "Il est midi.".to_string());
        let c = reprise(Intention::EnAnglais, &d);
        assert!(c.contains("« quelle heure ? »") && c.contains("« Il est midi. »"));
        assert!(c.ends_with("en anglais."));
        assert!(reprise(Intention::PlusCourt, &d).contains("plus court"));
        assert!(reprise(Intention::Developpe, &d).contains("Développe"));
    }

    #[test]
    fn carte_lisible() {
        let c = carte_ui(
            7,
            "mcp__linear__save_issue",
            &serde_json::json!({"title": "Relancer le devis", "team": "Kobi", "priority": 2}),
        );
        assert_eq!(c.id, "7");
        assert_eq!(c.kicker, "Linear · save issue");
        assert_eq!(c.title, "Relancer le devis");
        assert_eq!(c.status, "pending");
        assert!(c.meta.unwrap().contains("team : Kobi"));
    }

    /// Réunion menée par le vrai chef, avec un Claude qui ne répond jamais :
    /// les comptes rendus sont injectés à la main.
    fn chef_en_essai() -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir) {
        use tauri::test::{mock_builder, mock_context, noop_assets};
        let dir = tempfile::tempdir().unwrap();
        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let mut cfg = kobi_session::config::Config::default();
        cfg.stockage.dossier = Some(dir.path().join("donnees"));
        crate::stockage::installer(app.handle(), &cfg, None);
        let jamais: Fabrique = Arc::new(|_| Box::pin(std::future::pending()));
        installer(app.handle(), jamais, 5, cerveau::Delais::default());
        (app, dir)
    }

    fn avec_chef<T>(
        app: &tauri::App<tauri::test::MockRuntime>,
        f: impl FnOnce(&mut Chef<tauri::test::MockRuntime>) -> T,
    ) -> T {
        let chef = app.state::<ChefPartage<tauri::test::MockRuntime>>();
        let mut chef = chef.0.lock().unwrap();
        f(&mut chef)
    }

    /// KOB-245 : la fin est immédiate (REC éteint, état « fin », compte rendu
    /// demandé) et « Annuler » dans les 10 s reprend la même réunion.
    #[test]
    fn fin_immediate_puis_annuler() {
        let (app, _dir) = chef_en_essai();
        let h = app.handle();
        avec_chef(&app, |c| c.commande(Intention::DemarrerReunion));
        assert!(avec_chef(&app, |c| c.enregistrement().actif));
        let debut_ms = avec_chef(&app, |c| c.enregistrement().debut_ms);
        // Fin par Claude (pilotage), comme par la phrase ou le raccourci.
        let depuis = Instant::now();
        let dit = avec_chef(&app, |c| c.piloter(Intention::TerminerReunion, false));
        assert!(depuis.elapsed() < Duration::from_millis(300));
        assert_eq!(dit, "Réunion terminée, je prépare le compte rendu.");
        let e = avec_chef(&app, |c| c.enregistrement());
        assert!(!e.actif && e.annulable, "{e:?}");
        assert_eq!(etat(h).etat, "fin");
        // Le compte rendu revient pendant « Annuler » : gardé, pas écrit.
        avec_chef(&app, |c| {
            c.recevoir(Message::Cerveau(Retour::CompteRendu(Ok("# CR".into()))))
        });
        assert!(avec_chef(&app, |c| c.fin_annulable()));
        // « Annuler » : la même réunion reprend.
        avec_chef(&app, |c| c.commande(Intention::Reprendre));
        let e = avec_chef(&app, |c| c.enregistrement());
        assert!(e.actif && !e.pause && !e.annulable, "{e:?}");
        assert_eq!(e.debut_ms, debut_ms, "même réunion, compteur continu");
        assert_eq!(etat(h).etat, "reunion");
        // Fin par le raccourci, annulée avant le retour du compte rendu : sa
        // réponse tardive est ignorée, la réunion continue.
        avec_chef(&app, |c| c.commande(Intention::TerminerReunion));
        avec_chef(&app, |c| c.commande(Intention::Reprendre));
        avec_chef(&app, |c| {
            c.recevoir(Message::Cerveau(Retour::CompteRendu(Ok("# tard".into()))))
        });
        assert!(avec_chef(&app, |c| c.enregistrement().actif));
        assert_eq!(etat(h).etat, "reunion");
    }

    /// KOB-245 : sans « Annuler », la fin devient définitive au bout du
    /// délai et le compte rendu gardé s'écrit ; pause et fin restent
    /// distinctes.
    #[test]
    fn fin_definitive_apres_le_delai() {
        let (app, _dir) = chef_en_essai();
        let h = app.handle();
        avec_chef(&app, |c| c.commande(Intention::DemarrerReunion));
        avec_chef(&app, |c| c.commande(Intention::Pause));
        let e = avec_chef(&app, |c| c.enregistrement());
        assert!(e.actif && e.pause && !e.annulable, "pause, pas fin : {e:?}");
        assert_eq!(etat(h).etat, "pause");
        avec_chef(&app, |c| c.commande(Intention::TerminerReunion));
        avec_chef(&app, |c| {
            c.recevoir(Message::Cerveau(Retour::CompteRendu(Ok("# CR".into()))));
            // Délai écoulé.
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now());
            c.tick();
        });
        assert!(!avec_chef(&app, |c| c.fin_annulable()));
        assert!(
            avec_chef(&app, |c| c.reunion.is_none()),
            "compte rendu écrit"
        );
        assert_eq!(
            avec_chef(&app, |c| c.commande(Intention::Reprendre)),
            "Aucune réunion en cours."
        );
    }

    /// KOB-85 : une interpellation pendant la réunion est mesurée, et le
    /// compte rendu écrit se termine par la latence de Kobi.
    #[test]
    fn latence_en_fin_de_compte_rendu() {
        let (app, dir) = chef_en_essai();
        avec_chef(&app, |c| c.commande(Intention::DemarrerReunion));
        avec_chef(&app, |c| {
            let r = c.reunion.as_mut().unwrap();
            r.latences
                .question(9, Instant::now() - Duration::from_secs(3));
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour: 9,
                texte: "Il est".into(),
            }));
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: 9,
                texte: "Il est 10 h.".into(),
            }));
            assert_eq!(c.reunion.as_ref().unwrap().latences.mesures().len(), 1);
            c.commande(Intention::TerminerReunion);
            c.recevoir(Message::Cerveau(Retour::CompteRendu(Ok(
                "# Compte rendu\n\n## Résumé\nPoint budget.\n".into(),
            ))));
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now());
            c.tick();
        });
        // L'écrivain range le fichier sur son propre fil.
        let lu = (0..100).find_map(|_| {
            std::thread::sleep(Duration::from_millis(50));
            let reunions = dir.path().join("donnees").join("reunions");
            std::fs::read_dir(reunions)
                .ok()?
                .flatten()
                .find_map(|d| std::fs::read_to_string(d.path().join("compte-rendu.md")).ok())
        });
        let cr = lu.expect("compte rendu écrit");
        assert!(
            cr.starts_with("# Compte rendu\n\n## Résumé\nPoint budget.\n\n## Latence de Kobi\n"),
            "{cr}"
        );
        assert!(cr.contains("1 interpellation mesurée"), "{cr}");
        assert!(cr.contains("- Premier mot : médiane 3,0 s"), "{cr}");
    }

    /// KOB-75 : « Publier » du Carnet : la bulle montre la phrase courte, la
    /// demande part à Claude comme une question tapée.
    #[test]
    fn publier_depuis_le_carnet() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Publier {
                affiche: "Publie le compte rendu « Point budget » dans linear.".into(),
                consigne: "Publie le compte rendu ci-dessous…".into(),
            })
        });
        let h = avec_chef(&app, |c| c.historique());
        let b = h.last().expect("une bulle");
        assert_eq!(
            b.question,
            "Publie le compte rendu « Point budget » dans linear."
        );
        assert_eq!(b.statut, "attente");
        assert!(b.clavier);
    }

    fn dit(texte: &str) -> Segment {
        Segment {
            start: 0.0,
            end: 1.0,
            source: kobi_stt::Source::Eux,
            text: texte.into(),
            is_final: true,
        }
    }

    /// KOB-198 : ce qui a été dit avant la fin mais transcrit après (charge)
    /// est gardé ; le compte rendu attend que la piste ait rattrapé l'arrêt ;
    /// ce qui est dit après la fin ne compte pas.
    #[test]
    fn fin_attend_la_transcription_en_retard() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| c.commande(Intention::DemarrerReunion));
        // La piste « eux » n'a transcrit que jusqu'à il y a 5 s.
        let avant = Instant::now() - Duration::from_secs(5);
        avec_chef(&app, |c| {
            c.recevoir(Message::Avancement(kobi_stt::Source::Eux, Some(avant)));
            c.commande(Intention::TerminerReunion)
        });
        let attend = avec_chef(&app, |c| {
            let r = c.reunion.as_ref().unwrap();
            (r.vidange.is_some(), r.compte_rendu_demande)
        });
        assert_eq!(attend, (true, false), "le compte rendu attend la piste");
        // Arrive en retard, dit avant la fin : gardé. Dit après : ignoré.
        avec_chef(&app, |c| {
            c.recevoir(Message::Segment(dit("Le budget est validé."), avant));
            c.recevoir(Message::Segment(
                dit("Annule."),
                Instant::now() + Duration::from_secs(1),
            ));
        });
        // La piste rattrape l'arrêt : le compte rendu part avec la fin.
        avec_chef(&app, |c| {
            c.recevoir(Message::Avancement(
                kobi_stt::Source::Eux,
                Some(Instant::now()),
            ));
            c.tick()
        });
        let (demande, transcription) = avec_chef(&app, |c| {
            let r = c.reunion.as_ref().unwrap();
            (r.compte_rendu_demande, r.pour_compte_rendu.clone())
        });
        assert!(demande);
        assert!(
            transcription.contains("Le budget est validé."),
            "{transcription}"
        );
        assert!(!transcription.contains("Annule"), "{transcription}");
    }

    /// KOB-265 : micro en retard, « Annuler » reste ouvert tant que ce qui a
    /// été dit pendant le délai n'est pas transcrit, au plus la vidange.
    #[test]
    fn annuler_attend_le_micro_en_retard() {
        type Essai = Chef<tauri::test::MockRuntime>;
        let expire = |c: &mut Essai, depuis: Duration| {
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now() - depuis);
            c.tick();
            c.fin_annulable()
        };
        for rattrape in [true, false] {
            let (app, _dir) = chef_en_essai();
            let h = app.handle();
            avec_chef(&app, |c| c.commande(Intention::DemarrerReunion));
            avancer(
                h,
                kobi_stt::Source::Moi,
                Some(Instant::now() - Duration::from_secs(15)),
            );
            avec_chef(&app, |c| c.commande(Intention::TerminerReunion));
            assert!(avec_chef(&app, |c| expire(c, Duration::from_millis(1))));
            if rattrape {
                // Le micro rattrape le délai : la fin est définitive.
                avancer(h, kobi_stt::Source::Moi, Some(Instant::now()));
                assert!(!avec_chef(&app, |c| expire(c, Duration::from_millis(1))));
            } else {
                // Toujours en retard : au plus la vidange.
                assert!(!avec_chef(&app, |c| expire(c, VIDANGE)));
            }
        }
    }

    /// KOB-198 : la piste reste en retard ; au bout de la vidange, le compte
    /// rendu part quand même. « Annuler » pendant la vidange : aucun compte
    /// rendu n'était demandé, aucun n'est abandonné.
    #[test]
    fn vidange_bornee_et_annuler_pendant_la_vidange() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| c.commande(Intention::DemarrerReunion));
        avec_chef(&app, |c| {
            c.recevoir(Message::Avancement(
                kobi_stt::Source::Moi,
                Some(Instant::now() - Duration::from_secs(5)),
            ));
            c.commande(Intention::TerminerReunion)
        });
        avec_chef(&app, |c| {
            c.recevoir(Message::Segment(
                dit("Dernière décision."),
                Instant::now() - Duration::from_secs(6),
            ))
        });
        avec_chef(&app, |c| c.commande(Intention::Reprendre));
        let (abandonnes, a_noter) = avec_chef(&app, |c| {
            (
                c.comptes_rendus_abandonnes,
                c.reunion.as_ref().unwrap().a_noter.clone(),
            )
        });
        assert_eq!(abandonnes, 0);
        assert!(a_noter.contains("Dernière décision."), "{a_noter}");
        // Nouvelle fin, piste toujours en retard : la vidange expire.
        avec_chef(&app, |c| c.commande(Intention::TerminerReunion));
        avec_chef(&app, |c| {
            c.reunion.as_mut().unwrap().vidange = Some(Instant::now());
            c.tick();
        });
        assert!(avec_chef(&app, |c| c
            .reunion
            .as_ref()
            .unwrap()
            .compte_rendu_demande));
    }

    /// KOB-261 : une piste publie une rafale de segments puis se ferme (fin
    /// de réunion sous charge) ; son avancement passe par le même canal :
    /// le compte rendu ne part qu'après les avoir tous reçus.
    #[test]
    fn rafale_puis_avancement_rien_de_perdu() {
        let (app, _dir) = chef_en_essai();
        let h = app.handle().clone();
        let avant = Instant::now() - Duration::from_secs(30);
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Avancement(kobi_stt::Source::Eux, Some(avant)));
            c.commande(Intention::TerminerReunion);
        });
        // La piste « eux » vide sa file depuis son fil, comme
        // `son_ordi::fermer`.
        std::thread::spawn(move || {
            for i in 0..200 {
                publier(&h, Message::Segment(dit(&format!("Phrase {i}.")), avant));
            }
            avancer(&h, kobi_stt::Source::Eux, None);
        })
        .join()
        .unwrap();
        let limite = Instant::now() + Duration::from_secs(10);
        let transcription = loop {
            let fini = avec_chef(&app, |c| {
                let r = c.reunion.as_ref().unwrap();
                r.compte_rendu_demande.then(|| r.pour_compte_rendu.clone())
            });
            if let Some(t) = fini {
                break t;
            }
            assert!(Instant::now() < limite, "compte rendu jamais demandé");
            std::thread::sleep(Duration::from_millis(10));
        };
        for i in 0..200 {
            assert!(
                transcription.contains(&format!("Phrase {i}.")),
                "phrase {i} perdue"
            );
        }
    }

    /// KOB-261 : ce qui a été dit avant la pause mais transcrit après est
    /// gardé, y compris si la réunion se termine pendant la pause ; ce qui
    /// est dit après la pause ne l'est pas.
    #[test]
    fn pause_garde_ce_qui_a_ete_dit_avant() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let avant = Instant::now() - Duration::from_secs(30);
            c.commande(Intention::Pause);
            for i in 0..30 {
                c.recevoir(Message::Segment(
                    dit(&format!("Seconde {i}.")),
                    avant + Duration::from_secs(i),
                ));
            }
            c.recevoir(Message::Segment(
                dit("Pendant la pause."),
                Instant::now() + Duration::from_secs(1),
            ));
            let r = c.reunion.as_ref().unwrap();
            for i in 0..30 {
                assert!(
                    r.a_noter.contains(&format!("Seconde {i}.")),
                    "{}",
                    r.a_noter
                );
            }
            assert!(!r.a_noter.contains("Pendant la pause."), "{}", r.a_noter);
            // Fin pendant la pause : un retardataire dit avant la pause entre
            // encore au compte rendu, que le micro vide encore.
            c.recevoir(Message::Avancement(kobi_stt::Source::Moi, Some(avant)));
            c.commande(Intention::TerminerReunion);
            c.recevoir(Message::Segment(dit("Retardataire."), avant));
            c.recevoir(Message::Avancement(kobi_stt::Source::Moi, None));
            c.recevoir(Message::Avancement(kobi_stt::Source::Eux, None));
            c.tick();
            let r = c.reunion.as_ref().unwrap();
            assert!(r.compte_rendu_demande);
            assert!(r.pour_compte_rendu.contains("Seconde 29."));
            assert!(
                r.pour_compte_rendu.contains("Retardataire."),
                "{}",
                r.pour_compte_rendu
            );
        });
    }

    /// KOB-71 : en mode fin de réunion, aucun bloc de notes ni titre ne part
    /// pendant la réunion ; toute la transcription va au compte rendu, avec
    /// la demande de titre. En mode blocs, le bloc part à l'heure.
    /// KOB-265 : sans titre automatique (réglage lu au début de la réunion),
    /// aucun titre ne part, ni pendant ni avec le compte rendu.
    #[test]
    fn notes_en_mode_fin_de_reunion() {
        for (a_la_fin, titre_auto) in [(false, true), (true, true), (false, false), (true, false)] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                c.commande(Intention::DemarrerReunion);
                let r = c.reunion.as_mut().unwrap();
                r.a_la_fin = a_la_fin;
                r.titre_auto = titre_auto;
                for texte in ["Budget validé.", "Lieu : Annecy.", "Date à fixer."] {
                    c.recevoir(Message::Segment(dit(texte), Instant::now()));
                }
                // Le bloc est dû, puis le titre aussi (beaucoup de parole).
                let r = c.reunion.as_mut().unwrap();
                r.prochain_bloc = Instant::now();
                r.debut_parole = "mot ".repeat(4 * TITRE_MOTS);
                c.tick();
                let r = c.reunion.as_ref().unwrap();
                assert_eq!(r.envoye.is_some(), !a_la_fin, "bloc envoyé : {a_la_fin}");
                assert_eq!(
                    r.titre_demande,
                    !a_la_fin && titre_auto,
                    "titre : {a_la_fin} {titre_auto}"
                );
                if !a_la_fin {
                    return;
                }
                assert!(
                    r.a_noter.contains("Budget validé.") && r.a_noter.contains("Date à fixer.")
                );
                c.commande(Intention::TerminerReunion);
                let r = c.reunion.as_ref().unwrap();
                assert!(r.compte_rendu_demande);
                for texte in ["Budget validé.", "Lieu : Annecy.", "Date à fixer."] {
                    assert!(
                        r.pour_compte_rendu.contains(texte),
                        "{}",
                        r.pour_compte_rendu
                    );
                }
                assert_eq!(
                    r.titre_demande, titre_auto,
                    "titre demandé avec le compte rendu"
                );
            });
        }
    }

    /// KOB-228 : un compte rendu demandé pendant qu'un bloc est à la prise
    /// de notes reprend ce bloc, avant ce qui reste à noter.
    #[test]
    fn compte_rendu_avec_le_bloc_en_cours() {
        assert_eq!(
            transcript_du_compte_rendu(Some("[00:10 eux] Bloc envoyé.\n"), "[03:05 moi] Fin.\n"),
            "[00:10 eux] Bloc envoyé.\n[03:05 moi] Fin.\n"
        );
        assert_eq!(
            transcript_du_compte_rendu(None, "[00:01 moi] Seul.\n"),
            "[00:01 moi] Seul.\n"
        );
    }
}

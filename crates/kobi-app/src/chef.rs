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

use std::collections::{BTreeMap, HashMap, HashSet};
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
/// Attente au plus d'une annonce de service derrière une réponse (KOB-365).
const ANNONCE_ATTENTE: Duration = Duration::from_secs(30);

/// Mode de prise de notes (`notes.mode`), lu au début de chaque réunion. En
/// fin de réunion (`end`, KOB-71), rien n'est demandé à Claude pendant la
/// réunion, tout passe par le compte rendu. En continu (`continuous`,
/// KOB-72, expérimental), un bloc part toutes les [`PHRASES_CONTINU`]
/// phrases et Claude peut proposer une action, sur carte.
fn mode_de_notes() -> kobi_session::config::NotesMode {
    kobi_session::config::default_path()
        .map(|p| kobi_session::config::load_or_create(&p).config.notes.mode)
        .unwrap_or_default()
}

/// Phrases par bloc en notes continues (KOB-72) : assez peu pour réagir
/// vite, assez pour ne pas appeler Claude à chaque phrase.
const PHRASES_CONTINU: usize = 3;

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
/// Rien n'est encore transcrit quand [`VIDANGE`] expire, et les pistes sont
/// en retard : le compte rendu attend encore, au plus jusqu'à ce temps après
/// l'arrêt, plutôt que de partir vide (KOB-281).
const VIDANGE_SANS_PAROLE: Duration = Duration::from_secs(300);

/// Ce qui arrive au chef.
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    /// « Kobi » entendu, ou raccourci : Kobi écoute.
    Interpelle,
    /// Question en cours de transcription.
    QuestionPartielle(String),
    /// Question finie (dite, ou tapée si `voix` est faux). `dit_a` : quand
    /// elle a été dite (l'audio de l'écoute), pour une question dite.
    Question {
        texte: String,
        voix: bool,
        dit_a: Option<Instant>,
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
    QuestionPeutEtre(String, Instant),
    /// L'interpellation n'a pas été suivie d'une question.
    SansQuestion,
    /// Question dite avant l'arrêt, transcrite pendant « Annuler » : gardée
    /// jusqu'à la fin de la fenêtre (KOB-307). Avec l'instant où elle a été
    /// dite (KOB-337).
    QuestionDifferee(String, Instant),
    /// Ces secondes de parole dites avant la pause n'ont pas pu être
    /// transcrites (reprise pendant la vidange sous charge, KOB-277).
    TranscriptionIncomplete(f64),
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

/// Où va un échange avec Kobi, décidé à sa question (KOB-325, KOB-340).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Rangement {
    /// Les notes de la réunion commencée à cet instant.
    Notes(Instant),
    /// Le journal, dans la conversation de ce numéro.
    Journal(u64),
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
    /// Plages de fins annulées, de l'arrêt à l'annulation : ce qui y a été
    /// dit (« reprends », au revoir) n'entre pas dans les notes de la
    /// réunion reprise, même transcrit après (KOB-309).
    fins_annulees: FinsAnnulees,
    /// Segments dits après l'arrêt, arrivés pendant « Annuler » : rangés si
    /// la fin est annulée, jetés si elle tient. Une piste en avance sur
    /// l'autre (« eux » avant le « reprends » du micro) ne perd rien (KOB-339).
    apres_l_arret: Vec<(Segment, Instant)>,
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
    /// Compte rendu demandé avant que les pistes aient rattrapé l'arrêt : il
    /// le dit, et la suite arrive dans la transcription seule (KOB-281).
    incomplet: bool,
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
    /// Mots de [`debut_parole`](Self::debut_parole) à la dernière demande :
    /// la suivante attend [`TITRE_MOTS`] mots de plus, pas la même parole
    /// (KOB-359).
    titre_mots: usize,
    /// `notes.mode = end` (KOB-71) : ni bloc de notes ni titre pendant la
    /// réunion ; le titre est demandé avec le compte rendu.
    a_la_fin: bool,
    /// `notes.mode = continuous` (KOB-72) : blocs de [`PHRASES_CONTINU`]
    /// phrases, et propositions d'action sur carte.
    continu: bool,
    /// Le dernier bloc a échoué : il est repris à l'heure, même en continu.
    bloc_en_echec: bool,
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
    /// Dernière réunion close sur une transcription incomplète : son début
    /// et son arrêt, et ses fins annulées. Ce qui a été dit avant l'arrêt et
    /// arrive encore va dans sa transcription (KOB-281), sauf ce qui a été
    /// dit pendant une fin annulée (KOB-309).
    reunion_close: Option<(Instant, Instant, FinsAnnulees)>,
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
    /// Annonces de service qui attendent pour ne pas couper la réponse en
    /// cours (KOB-365, KOB-366), dans l'ordre : « compte rendu prêt »,
    /// rappel échu, carte expirée.
    annonces: std::collections::VecDeque<Annonce>,
    /// Carte de Kobi dont « Je le fais ? » attend que la voix se libère
    /// (KOB-375) : dit avant les annonces, abandonné si la carte n'est plus
    /// celle qui attend sa réponse ou si l'utilisateur a coupé la voix
    /// depuis (coupures de la voix au moment de l'ouverture).
    validation_a_dire: Option<(String, u64)>,
    /// Dernière question posée quand une carte de Kobi ou de la conversation
    /// s'est ouverte : une question posée depuis passe avant de reproposer
    /// la carte (KOB-384).
    question_a_la_carte: u64,
    /// « Je m'en occupe » venu pendant qu'une carte attendait la décision
    /// (KOB-385) : son tour, et depuis quand plus aucune carte n'attend. Dit
    /// seulement si la tâche tourne encore [`cerveau::PATIENCE`] après.
    patience_reportee: Option<(u64, Option<Instant>)>,
    /// Tours dont une carte MCP de la conversation a été acceptée : ils
    /// continuent, un refus d'une autre de leurs cartes n'oublie pas leur
    /// patience remise (KOB-396).
    tours_acceptes: HashSet<u64>,
    /// Question dont l'attente du réseau montre l'Oups : une carte ou sa
    /// réponse l'acquitte (KOB-386), pas une autre erreur.
    oups_du_reseau: Option<u64>,
    /// Tour d'une carte de la conversation acceptée pendant que d'autres
    /// attendaient : s'il est encore en cours, la dernière carte tranchée,
    /// même refusée, fait repartir Kobi sur lui (KOB-381).
    acceptee_en_attente: Option<u64>,
    /// Tour de chaque carte MCP de la conversation ouverte (KOB-381).
    tours_des_cartes: HashMap<u64, u64>,
    /// Carte MCP de la conversation dont « Je le fais ? » attend la voix
    /// libre (KOB-388), mêmes règles, après la carte de Kobi.
    carte_a_dire: Option<(u64, u64)>,
    /// Question en attente du réseau (KOB-377) : son énoncé, muet, est lâché
    /// pour que la voix reste libre (« Je le fais ? », annonces) ; sa réponse
    /// repart sur un énoncé neuf quand elle arrive et que la voix est libre,
    /// sauf si l'utilisateur a coupé la voix depuis (coupures au moment de
    /// l'attente), comme les annonces.
    voix_reportee: Option<(u64, u64)>,
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
    /// Où va chaque échange, décidé à la question : les notes de la réunion
    /// commencée à cet instant, ou le journal, dans une conversation
    /// ([`Rangement`]).
    /// La réponse suit sa question, même si elle arrive après « Annuler »,
    /// la reprise, la fin ou dans une autre réunion (KOB-325, KOB-327,
    /// KOB-330), ou de la même conversation (KOB-340).
    rangement: HashMap<u64, Rangement>,
    /// Numéro de la conversation du quotidien en cours, augmenté à chaque
    /// fermeture (KOB-340).
    conversation_n: u64,
    /// Cartes proposées par la prise de notes (tour 0, KOB-72), et si Kobi
    /// était au calme à leur ouverture : il y revient une fois décidées.
    cartes_des_notes: HashMap<u64, bool>,
    /// Cartes des notes acceptées au clic, dont la fermeture n'est pas encore
    /// revenue du cerveau (KOB-331).
    notes_acceptees: HashSet<u64>,
    /// Ouverture de chaque carte MCP : sans clic, kobi-perms la refuse seul
    /// à l'échéance sans que rien ne revienne ; le chef la ferme alors, pour
    /// ne pas rester en proposition (KOB-72).
    cartes_ouvertes: HashMap<u64, Instant>,
    /// Cartes de validation ouvertes (KOB-156), dans l'ordre de l'écran :
    /// « oui » ou « non » répond à celle affichée (KOB-218).
    validations: crate::validation::FileValidations,
    /// La question en cours de traitement ne s'adresse peut-être pas à Kobi.
    peut_etre: bool,
    /// Instant de l'audio de la question en cours de traitement, si elle a
    /// été dite (KOB-324).
    question_dite_a: Option<Instant>,
    /// La question en cours est différée (KOB-307) : elle part sur une
    /// session sans pilotage.
    differee: bool,
    /// Tour d'une telle question : sa réponse n'est montrée qu'une fois
    /// finie, et une réponse « rien » le laisse silencieux (KOB-168).
    tour_peut_etre: Option<u64>,
    /// Compte rendu remis à l'écrivain, pas encore sur le disque : quitter
    /// l'attend encore (KOB-226).
    compte_rendu_a_ecrire: bool,
    /// Comptes rendus demandés puis abandonnés par « Annuler » (KOB-245) :
    /// leurs réponses sont ignorées.
    comptes_rendus_abandonnes: u32,
    /// Questions dites avant l'arrêt, à poser une fois « Annuler » fermé
    /// (KOB-307).
    questions_differees: Vec<String>,
    /// Arrêt de la dernière réunion terminée pour de bon (« Annuler » clos) :
    /// une question dite avant, même transcrite bien plus tard, ne part
    /// jamais sur la conversation, qui pilote la réunion (KOB-326).
    arret_definitif: Option<Instant>,
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

/// Annonce de service à voix haute, en attente (KOB-365, KOB-366).
#[derive(Debug)]
struct Annonce {
    texte: String,
    depuis: Instant,
    /// Coupures de la voix au moment de l'annonce : si l'utilisateur a coupé
    /// la voix depuis, elle n'est plus de mise.
    coupures: u64,
}

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
        match source {
            kobi_stt::Source::Moi => self.moi,
            kobi_stt::Source::Eux => self.eux,
        }
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

/// Fins annulées d'une réunion : (arrêt, annulation, annulée à la voix)
/// (KOB-309, KOB-324).
type FinsAnnulees = Vec<(Instant, Instant, bool)>;

/// Silence compté dans la fin d'un segment Nemotron, au plus : fin
/// d'énoncé (0,8 s), bloc de décodage, et conclusion à la fermeture de la
/// porte (KOB-323).
const MARGE_FIN: Duration = Duration::from_millis(2400);

/// Segment de `commence_a` à `dit_a` dit après l'arrêt `arret`. Jugé sur la
/// fin (KOB-198) : le début d'un segment peut précéder la parole du silence
/// transmis, jusqu'à [`MARGE_FIN`]. Mais un énoncé commencé plus tôt est
/// de la parole d'avant l'arrêt, encore en cours au clic : il reste à la
/// réunion, en entier, sinon tout ce qu'il contient serait perdu (KOB-364).
fn dit_apres_l_arret(commence_a: Instant, dit_a: Instant, arret: Instant) -> bool {
    dit_a > arret && commence_a + MARGE_FIN > arret
}

/// Dit pendant l'une des fins annulées `fins` (arrêt, annulation), pour un
/// segment de `commence_a` à `dit_a` (KOB-309), d'après
/// [`dit_apres_l_arret`]. Une phrase dite juste avant le clic finit
/// après lui de tout le silence qui la suit (KOB-323) : jusqu'à
/// [`MARGE_FIN`] après l'annulation, un segment commencé avant est encore
/// de la fin. Au-delà, c'est de la parole qui a continué après la reprise :
/// gardée, comme avant.
///
/// Annulée à la voix, `annule` est la fin de la trame qui conclut
/// « reprends » : sur la piste du micro (`moi`), conclue dans l'ordre, tout
/// segment conclu après a été dit après lui, et la marge ferait perdre la
/// phrase d'après (KOB-324).
fn dit_pendant_une_fin_annulee(
    fins: &[(Instant, Instant, bool)],
    moi: bool,
    commence_a: Instant,
    dit_a: Instant,
) -> bool {
    fins.iter().any(|&(arret, annule, voix)| {
        let marge = !(voix && moi);
        dit_apres_l_arret(commence_a, dit_a, arret)
            && (dit_a <= annule || (marge && commence_a <= annule && dit_a <= annule + MARGE_FIN))
    })
}

/// Début d'un segment fini à `dit_a`, d'après sa durée.
fn commence_a(s: &Segment, dit_a: Instant) -> Instant {
    dit_a
        .checked_sub(Duration::from_secs_f64((s.end - s.start).max(0.0)))
        .unwrap_or(dit_a)
}

/// Horodate un segment sur la réunion commencée à `debut`, au moment où il a
/// été dit : l'horloge du moteur repart à chaque question, et le segment
/// arrive en retard quand la machine est chargée.
fn horodater(s: &mut Segment, debut: Instant, dit_a: Instant) {
    let duree = (s.end - s.start).max(0.0);
    s.end = dit_a
        .min(Instant::now())
        .saturating_duration_since(debut)
        .as_secs_f64();
    s.start = (s.end - duree).max(0.0);
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

/// La carte MCP dite à voix haute, quand la voix est le seul canal :
/// « Carnet, ecrire note : relire le budget » (KOB-388).
fn resume_carte(ui: &CarteUi) -> String {
    let kicker = ui.kicker.replace(" · ", ", ").replace('\n', " ");
    let titre = ui.title.replace('\n', " ");
    if titre.trim().is_empty() || kicker.ends_with(&titre) {
        kicker
    } else {
        format!("{kicker} : {titre}")
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
            annonces: Default::default(),
            validation_a_dire: None,
            question_a_la_carte: 0,
            patience_reportee: None,
            tours_acceptes: HashSet::new(),
            oups_du_reseau: None,
            acceptee_en_attente: None,
            tours_des_cartes: HashMap::new(),
            carte_a_dire: None,
            voix_reportee: None,
            derniere: None,
            taches: BTreeMap::new(),
            annoncees: Default::default(),
            interrompus: HashSet::new(),
            rangement: HashMap::new(),
            conversation_n: 0,
            cartes_des_notes: HashMap::new(),
            notes_acceptees: HashSet::new(),
            cartes_ouvertes: HashMap::new(),
            validations: Default::default(),
            peut_etre: false,
            question_dite_a: None,
            differee: false,
            tour_peut_etre: None,
            compte_rendu_a_ecrire: false,
            comptes_rendus_abandonnes: 0,
            questions_differees: Vec::new(),
            arret_definitif: None,
            avancement: Avancement::default(),
            reunion_close: None,
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

    /// `evenement` (carte proposée, réponse qui commence) pendant que la
    /// question en cours attend le réseau (KOB-386) : l'Oups de cette
    /// attente, et seulement lui, est acquitté. Revenu au calme, Kobi passe
    /// « au travail », seul chemin de la machine vers la proposition et la
    /// réponse, si `depuis_le_calme` : la réponse de la question en cours,
    /// ou une carte pendant qu'elle tourne encore (sans elle, rien ne le
    /// sortirait de la réflexion une fois la carte acceptée). Sinon (écoute,
    /// réponse en cours…), seul l'événement.
    fn depuis_le_repos(&mut self, evenement: Evenement, depuis_le_calme: bool) {
        if self.oups_du_reseau.take() == Some(self.dernier) && self.activite() == Some("oups") {
            self.transition(&[Evenement::Acquitter]);
        }
        if depuis_le_calme
            && matches!(self.activite(), Some("calme") | None)
            && matches!(etat(&self.app).etat, "veille" | "assistant" | "reunion")
        {
            self.transition(&[Evenement::TacheLongue]);
        }
        self.transition(&[evenement]);
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
            Message::TranscriptionIncomplete(secondes) => self.transcription_incomplete(secondes),
            Message::QuestionDifferee(q, dit_a) => {
                // Rien d'une réunion passée ne part dans la nouvelle (KOB-326) ;
                // une question de la réunion en cours, si (KOB-337).
                let passee = self.arret_definitif.is_some_and(|arret| dit_a <= arret);
                if passee && self.reunion_apres_l_arret() {
                    tracing::info!(
                        "question d'avant l'arrêt arrivée dans la réunion suivante : écartée"
                    );
                } else {
                    self.questions_differees.push(q);
                    self.poser_questions_differees();
                }
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
            Message::Question { texte, voix, dit_a } => {
                if !self.dit_avant_l_arret(dit_a, &texte) {
                    self.question_dite_a = dit_a;
                    self.question(texte, voix);
                    self.question_dite_a = None;
                }
            }
            Message::Publier { affiche, consigne } => {
                tracing::info!("publication demandée depuis le Carnet");
                self.transition(&[Evenement::Interpellation]);
                let _ = self.app.emit(EVENEMENT_QUESTION, &affiche);
                self.envoyer(affiche, Some(consigne), false);
            }
            Message::QuestionPeutEtre(texte, dit_a) => {
                if self.dit_avant_l_arret(Some(dit_a), &texte) {
                    return;
                }
                self.peut_etre = true;
                self.question_dite_a = Some(dit_a);
                self.question(texte, true);
                self.question_dite_a = None;
                self.peut_etre = false;
            }
            Message::Segment(s, dit_a) => self.segment(s, dit_a),
            Message::Avancement(source, jusqu_a) => self.avancement.avancer(source, jusqu_a),
            Message::Commande(i) => {
                self.commande(i);
            }
            Message::Decision { carte, accepte } => {
                // Acceptée au clic : l'action part ; sa fermeture peut encore
                // arriver après « Terminer » (KOB-331).
                if accepte && self.cartes_des_notes.contains_key(&carte) {
                    self.notes_acceptees.insert(carte);
                }
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
                // À voix haute aussi, si la voix est active (KOB-142), sans
                // couper la réponse en cours (KOB-366).
                self.annoncer(kobi_i18n::tr(
                    "kobi.taches.rappelAria",
                    &[("texte", &r.texte)],
                ));
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
        // Question dite avant l'arrêt, posée après (KOB-307) : rien ne se
        // fait sur place, ni ordre, ni carte, ni rappel, ni titre ; elle part
        // telle quelle sur la session en lecture seule (KOB-328).
        if self.differee {
            self.envoyer(texte, None, voix);
            return;
        }
        // Une carte de validation attend : « oui » ou « non » lui répondent.
        if let Some(id) = self.validations.cible().map(str::to_owned) {
            if let Some(accepte) = crate::intentions::reponse_validation(&texte) {
                tracing::info!(%id, accepte, "réponse à la carte, à la voix ou au clavier");
                crate::validation::decider(&self.app, &id, accepte);
                self.reproposer_si_une_autre_attend(Some(&id), None);
                self.reponse_a_la_carte(texte, voix, accepte);
                return;
            }
        }
        // Une carte MCP attend : « oui » ou « non » répondent à celle qui est
        // affichée, comme un clic (KOB-336). Seul le micro pose des
        // questions : la voix des autres (son de l'ordinateur) ne valide
        // rien ; en réunion, il faut le nom (« Kobi, oui »).
        if let Some(carte) = self.carte_affichee() {
            if let Some(accepte) = crate::intentions::reponse_carte_mcp(&texte) {
                tracing::info!(
                    carte,
                    accepte,
                    "réponse à la carte MCP, à la voix ou au clavier"
                );
                crate::validation::retirer(&self.app, &carte.to_string(), accepte);
                self.recevoir(Message::Decision { carte, accepte });
                // « Kobi, oui » sur une carte des notes : l'écoute ouverte par
                // le nom se referme (aucune réponse ne suit). Une autre carte
                // qui attend se remontre, Kobi y repropose. Celle de la
                // conversation, elle, fait repartir le tour (l'état passe de
                // l'écoute à la réflexion, KOB-336).
                if self.activite() == Some("ecoute") && self.cartes_des_notes.contains_key(&carte) {
                    self.transition(&[Evenement::PasDeQuestion]);
                    if let Some(ui) = self
                        .carte_affichee_sauf(Some(carte))
                        .and_then(|k| self.cartes.get(&k).cloned())
                    {
                        if self.activite() == Some("calme") {
                            self.transition(&[
                                Evenement::NotesCommencent,
                                Evenement::PermissionDemandee,
                            ]);
                            // Reproposées depuis le calme : la fermeture de la
                            // dernière y ramène Kobi, même venues d'ailleurs et
                            // dans n'importe quel ordre (KOB-419).
                            for (k, depuis_calme) in self.cartes_des_notes.iter_mut() {
                                if *k != carte && self.cartes.contains_key(k) {
                                    *depuis_calme = true;
                                }
                            }
                        }
                        let _ = self.app.emit(EVENEMENT_CARTE, &ui);
                    }
                } else {
                    self.reproposer_si_une_autre_attend(None, Some(carte));
                }
                self.reponse_a_la_carte(texte, voix, accepte);
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
        self.ranger(self.tour, Qui::Moi, &texte, voix);
        self.voix = Some((self.tour, crate::voix::Reponse::new(&self.app)));
        self.voix_reportee = None;
        self.oups_du_reseau = None;
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
            // Une session différée est jetable : la conversation garde ce
            // qu'elle n'a pas vu (KOB-307).
            transcript: if self.differee {
                self.pour_conversation.clone()
            } else {
                std::mem::take(&mut self.pour_conversation)
            },
            notes,
            differee: self.differee,
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
    /// question et la réponse de Kobi vont aux notes et au compte rendu ;
    /// posées pendant la fin ou la pause, elles vont au journal des
    /// conversations, comme hors réunion (KOB-325).
    fn ranger(&mut self, tour: u64, qui: Qui, texte: &str, voix: bool) {
        // Aux notes de la réunion en cours, sauf pendant sa fin ou sa pause ;
        // sinon à la conversation en cours.
        let rangement = match qui {
            Qui::Moi => {
                let rangement = self
                    .reunion
                    .as_ref()
                    .filter(|r| !r.fin_demandee && !r.pause)
                    .map_or(Rangement::Journal(self.conversation_n), |r| {
                        Rangement::Notes(r.debut)
                    });
                self.rangement.insert(tour, rangement);
                rangement
            }
            Qui::Kobi => self
                .rangement
                .remove(&tour)
                .unwrap_or(Rangement::Journal(self.conversation_n)),
        };
        let notes_de = match rangement {
            Rangement::Notes(debut) => Some(debut),
            Rangement::Journal(_) => None,
        };
        if let Some(r) = self.reunion.as_mut().filter(|r| Some(r.debut) == notes_de) {
            let t = r.debut.elapsed().as_secs();
            let qui_ = match qui {
                Qui::Moi => "interpellation de Kobi",
                Qui::Kobi => "réponse de Kobi",
            };
            let texte = texte.replace('\n', " ");
            let ligne = format!("[{:02}:{:02} {qui_}] {texte}\n", t / 60, t % 60);
            // Le compte rendu est déjà demandé : la réponse lui manque, mais
            // revient à la réunion si la fin est annulée, comme les segments
            // (KOB-281, KOB-330).
            if r.compte_rendu_demande {
                r.pour_compte_rendu.push_str(&ligne);
            } else {
                r.a_noter.push_str(&ligne);
            }
            return;
        }
        // Réponse à une question d'une réunion close : sa question est dans
        // les notes de celle-ci, pas dans une conversation. Seule au
        // journal, elle n'y aurait pas de sens, et rouvrirait une
        // conversation (KOB-334) : la bulle la montre, rien de plus.
        if notes_de.is_some() {
            tracing::info!(
                tour,
                "réponse arrivée après sa réunion : montrée, pas rangée"
            );
            return;
        }
        // Réponse d'une conversation fermée par le démarrage d'une réunion :
        // elle n'en ouvre pas une autre où elle serait seule pendant la
        // réunion (KOB-340) ; la bulle la montre. Hors réunion (micro coupé
        // pendant une tâche…), elle va au journal comme avant.
        if rangement != Rangement::Journal(self.conversation_n) && self.reunion.is_some() {
            tracing::info!(
                tour,
                "réponse arrivée après sa conversation : montrée, pas rangée"
            );
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
                // Fin demandée (« Annuler » encore possible, ou compte rendu
                // en cours) : pas de pause. L'automate la refuserait, et
                // l'écoute garderait une pause qui n'a pas eu lieu (KOB-305).
                Some(r) if r.fin_demandee => {
                    kobi_i18n::tr("coque.chef.compteRenduDejaEnCours", &[])
                }
                Some(r) if !r.pause => {
                    let maintenant = Instant::now();
                    r.pause = true;
                    r.pause_a = Some(maintenant);
                    // L'écoute ne transcrit pas ce qui est dit après, même si
                    // elle ne voit la pause qu'en retard (KOB-277).
                    let _ = crate::ecoute::commander(
                        &self.app,
                        crate::ecoute::Commande::PauseReunion(maintenant),
                    );
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
                    // La pause d'avant ne doit plus écarter de parole à une
                    // prochaine fermeture du micro (KOB-297).
                    let _ = crate::ecoute::commander(
                        &self.app,
                        crate::ecoute::Commande::RepriseReunion,
                    );
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
        if self.reunion.is_none() && !self.questions_differees.is_empty() {
            // Rien d'une réunion passée ne part dans la nouvelle.
            tracing::info!(
                nombre = self.questions_differees.len(),
                "questions dites avant l'arrêt abandonnées : nouvelle réunion"
            );
            self.questions_differees.clear();
        }
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
        self.reunion_close = None;
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
        let mode = mode_de_notes();
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
            incomplet: false,
            compte_rendu_demande: false,
            pause_a: None,
            fins_annulees: Vec::new(),
            apres_l_arret: Vec::new(),
            titre: None,
            titre_manuel: false,
            debut_parole: String::new(),
            titre_demande: false,
            titre_essais: 0,
            titre_mots: 0,
            a_la_fin: mode == kobi_session::config::NotesMode::End,
            continu: mode == kobi_session::config::NotesMode::Continuous,
            bloc_en_echec: false,
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
        self.refuser_cartes_des_notes();
        self.vider_puis_compte_rendu();
        // « Reprends » s'entend sans redire « Kobi » pendant « Annuler ».
        // L'instant réel de l'arrêt : sous retard, seul ce qui est dit après
        // annule la fin (KOB-265).
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Annulable(arret));
        tracing::info!("réunion terminée, annulable");
        kobi_i18n::tr("coque.chef.reunionTerminee", &[])
    }

    /// Une action proposée par les notes en continu (KOB-72) ne vaut que
    /// pendant la réunion : à sa fin, sa carte est refusée (KOB-331). Un oui
    /// tardif n'agit plus après la fin, et le tour de notes qui l'attendait
    /// finit sans retenir le compte rendu. Si la fin est ensuite annulée, la
    /// proposition ne revient pas : les notes suivantes pourront la refaire.
    fn refuser_cartes_des_notes(&mut self) {
        let cartes: Vec<u64> = self.cartes_des_notes.keys().copied().collect();
        for carte in cartes {
            // Déjà acceptée, sa fermeture pas encore revenue : l'action est
            // faite, sa trace va au compte rendu (KOB-331).
            if self.notes_acceptees.contains(&carte) {
                self.retour(Retour::CarteFermee {
                    carte,
                    accepte: Some(true),
                });
                continue;
            }
            tracing::info!(carte, "fin de la réunion : carte des notes refusée");
            self.cerveau.demander(Demande::Decision {
                carte,
                accepte: false,
            });
            self.retour(Retour::CarteFermee {
                carte,
                accepte: Some(false),
            });
        }
    }

    /// Fin de « Annuler » (délai écoulé, fin redemandée, quitter) : la fin est
    /// définitive, le compte rendu déjà revenu est écrit.
    pub fn clore_annulation(&mut self) {
        let Some(r) = &mut self.reunion else { return };
        if r.annulable_jusqu.take().is_none() {
            return;
        }
        // La fin tient : rien de ce qui a été dit après l'arrêt n'entre.
        r.apres_l_arret.clear();
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::FinConfirmee);
        tracing::info!("fin de la réunion confirmée");
        if r.arret.is_some() {
            self.arret_definitif = r.arret;
        }
        let Some(r) = &mut self.reunion else { return };
        match r.compte_rendu_garde.take() {
            Some(resultat) => self.compte_rendu(resultat),
            None => self.emettre_enregistrement(),
        }
    }

    /// Une réunion a démarré après la dernière fin confirmée (KOB-326) : une
    /// question d'avant cette fin n'y a pas sa place.
    fn reunion_apres_l_arret(&self) -> bool {
        self.reunion
            .as_ref()
            .zip(self.arret_definitif)
            .is_some_and(|(r, arret)| r.debut > arret)
    }

    /// Question dite avant l'arrêt d'une réunion terminée (KOB-326) : la
    /// fenêtre « Annuler » s'est fermée avant que la transcription la
    /// rattrape. Elle ne part que sur la session en lecture seule, comme
    /// pendant « Annuler » (KOB-307), si elle y est permise ; sinon elle ne
    /// part pas. Vrai si elle est ainsi prise en charge.
    fn dit_avant_l_arret(&mut self, dit_a: Option<Instant>, texte: &str) -> bool {
        let avant = dit_a
            .zip(self.arret_definitif)
            .is_some_and(|(dit, arret)| dit <= arret);
        if !avant {
            return false;
        }
        // L'écouteur a déjà fait écouter Kobi : rien ne part maintenant.
        self.transition(&[Evenement::PasDeQuestion]);
        if crate::mot_cle::differable(texte) && !self.reunion_apres_l_arret() {
            tracing::info!(
                longueur = texte.len(),
                "question dite avant l'arrêt, transcrite après « Annuler » : différée"
            );
            self.questions_differees.push(texte.to_owned());
            self.poser_questions_differees();
        } else {
            tracing::info!(
                longueur = texte.len(),
                "question dite avant l'arrêt, transcrite après « Annuler » : écartée"
            );
        }
        true
    }

    /// Questions dites avant l'arrêt (KOB-307) : posées une fois « Annuler »
    /// fermé, la fin annulée ou confirmée. Les ordres que le chef reconnaît
    /// n'arrivent pas jusqu'ici (`mot_cle::differable`) ; les autres partent
    /// sans rien faire sur place, sur la session en lecture seule, qui ne
    /// peut ni piloter la réunion ni écrire (KOB-328).
    fn poser_questions_differees(&mut self) {
        if self.questions_differees.is_empty() || self.fin_annulable() {
            return;
        }
        // Une à la fois, Kobi au calme : dans la réunion reprise, ou en
        // veille une fois le compte rendu écrit (pas pendant qu'il
        // s'écrit), ni pendant une autre réponse, sa lecture ou un résultat
        // à montrer. La réponse de l'une ne passe pas sous la suivante.
        let calme = match self.activite() {
            Some(activite) => activite == "calme",
            None => etat(&self.app).etat == "veille",
        };
        if !calme
            || self.calme_a.is_some()
            || !self.a_livrer.is_empty()
            || crate::voix::parle(&self.app)
        {
            return;
        }
        let q = self.questions_differees.remove(0);
        tracing::info!(longueur = q.len(), "question dite avant l'arrêt, posée");
        self.transition(&[Evenement::Interpellation]);
        self.differee = true;
        self.question(q, true);
        self.differee = false;
    }

    /// « Annuler » : la même réunion reprend (même dossier, compteur
    /// continu, rien de perdu de ce qui restait à noter) ; le compte rendu
    /// demandé est abandonné.
    fn annuler_fin(&mut self) -> String {
        let Some(r) = &mut self.reunion else {
            return kobi_i18n::tr("coque.chef.aucuneReunion", &[]);
        };
        // La fin est annulée : la fenêtre « Annuler » ne doit plus se
        // rouvrir avec le micro, dans une réunion en cours (KOB-299).
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::FinAnnulation);
        // La réunion reprend sans pause : l'écoute oublie toute pause notée
        // (KOB-297, KOB-305).
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::RepriseReunion);
        if let Some(arret) = r.arret {
            // « Reprends » dit à la voix : la fin s'arrête quand il a été dit,
            // pas quand sa transcription, en retard sous charge, arrive ;
            // ce qui est dit juste après reste dans les notes (KOB-324).
            let annule = self.question_dite_a.filter(|&dit| dit > arret);
            r.fins_annulees
                .push((arret, annule.unwrap_or_else(Instant::now), annule.is_some()));
        }
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
        let mut apres = std::mem::take(&mut r.apres_l_arret);
        // Dans l'ordre où ils ont été dits, « moi » et « eux » mêlés.
        apres.sort_by_key(|(_, dit_a)| *dit_a);
        self.transition(&[Evenement::Reprendre]);
        self.emettre_enregistrement();
        tracing::info!(segments = apres.len(), "fin de la réunion annulée");
        // Arrivé avant l'annulation, dit après l'arrêt : rangé maintenant,
        // selon la règle des fins annulées (KOB-339).
        for (s, dit_a) in apres {
            self.segment(s, dit_a);
        }
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
        // Rien encore transcrit, pistes en retard : un compte rendu vide ne
        // servirait à rien, il attend encore (KOB-281).
        let rien = r.envoye.is_none() && r.a_noter.trim().is_empty();
        if !a_jour && rien && arret.elapsed() < VIDANGE_SANS_PAROLE {
            return;
        }
        r.incomplet = !a_jour;
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
        // Rien de transcrit ni de noté : pas d'appel à Claude, dont la réponse
        // (« il n'y a rien à résumer ») deviendrait le compte rendu (KOB-281).
        if r.envoye.is_none() && r.a_noter.trim().is_empty() && r.notes.trim().is_empty() {
            tracing::warn!("fin de réunion : aucune parole transcrite, compte rendu sans Claude");
            self.compte_rendu(Ok(kobi_i18n::tr("coque.chef.compteRenduVide", &[])));
            return;
        }
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
        if !s.is_final {
            return;
        }
        let commence_a = commence_a(&s, dit_a);
        let moi = s.source == kobi_stt::Source::Moi;
        let Some(r) = &mut self.reunion else {
            // Réunion close avant que la transcription ait rattrapé l'arrêt :
            // ce qui a été dit avant va encore dans sa transcription (KOB-281),
            // y compris un énoncé commencé avant et fini après (KOB-364).
            // Sauf ce qui a été dit pendant une fin annulée (KOB-309).
            let debut = self
                .reunion_close
                .as_ref()
                .and_then(|(debut, arret, fins)| {
                    (!dit_apres_l_arret(commence_a, dit_a, *arret)
                        && !dit_pendant_une_fin_annulee(fins, moi, commence_a, dit_a))
                    .then_some(*debut)
                });
            if let Some(debut) = debut {
                horodater(&mut s, debut, dit_a);
                tracing::info!("segment arrivé après le compte rendu : ajouté à la transcription");
                self.ecrivain.segment(s);
            }
            return;
        };
        // En pause, seul ce qui a été dit avant la pause est gardé : sous
        // charge, il arrive après (KOB-261). Jugé sur le début du segment :
        // la dernière phrase, conclue à la fermeture du micro, finit après
        // la pause (marge de fin du moteur) et serait perdue (KOB-277).
        if r.pause && r.pause_a.is_none_or(|pause| commence_a > pause) {
            return;
        }
        // Dit avant la fin mais transcrit après (charge, file d'attente) :
        // gardé jusqu'au compte rendu, comme un énoncé encore en cours à
        // l'arrêt au clic (KOB-364). Ce qui est dit après (le « annule »
        // d'« Annuler »…) ne l'est pas (KOB-198).
        if r.fin_demandee
            && r.arret
                .is_none_or(|arret| dit_apres_l_arret(commence_a, dit_a, arret))
        {
            if r.annulable_jusqu.is_some() {
                r.apres_l_arret.push((s, dit_a));
            }
            return;
        }
        // Dit pendant une fin ensuite annulée, transcrit après l'annulation :
        // pas plus dans les notes que si la fin avait tenu (KOB-309).
        if dit_pendant_une_fin_annulee(&r.fins_annulees, moi, commence_a, dit_a) {
            return;
        }
        horodater(&mut s, r.debut, dit_a);
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
        // Arrivé après la demande de compte rendu : il manque à celui-ci, qui
        // le dit, mais la transcription le garde (KOB-281). Si la fin est
        // annulée, il revient à la réunion reprise avec le reste (KOB-288).
        if r.compte_rendu_demande {
            tracing::info!(
                "segment arrivé après la demande de compte rendu : ajouté à la transcription"
            );
            r.pour_compte_rendu.push_str(&ligne);
            return;
        }
        r.a_noter.push_str(&ligne);
        // Gardée jusqu'au titre, même pendant une demande : un nouvel essai
        // a ainsi la parole dite entre-temps (KOB-359).
        if r.titre.is_none() && r.debut_parole.len() < TITRE_PAROLE_MAX {
            r.debut_parole.push_str(&ligne);
        }
        self.pour_conversation.push_str(&ligne);
    }

    /// De la parole dite avant la pause n'a pas pu être transcrite (KOB-277) :
    /// le compte rendu le sait (mention dans la transcription qui lui
    /// parvient), et l'utilisateur aussi (activité).
    fn transcription_incomplete(&mut self, secondes: f64) {
        let secondes = (secondes.round() as u64).max(1).to_string();
        let texte = kobi_i18n::tr(
            "coque.chef.transcriptionIncomplete",
            &[("secondes", &secondes)],
        );
        tracing::warn!(%secondes, "transcription incomplète signalée");
        if let Some(r) = self.reunion.as_mut().filter(|r| !r.compte_rendu_demande) {
            r.a_noter.push_str(&format!("[{texte}]\n"));
        }
        crate::activite::noter(&self.app, crate::activite::Genre::Erreur, &texte, None);
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
        self.ranger(self.tour, Qui::Moi, &question, voix);
        self.ranger(self.tour, Qui::Kobi, &reponse, false);
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
        self.rangement.remove(&tour);
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
            self.rangement.remove(&tour);
            self.cerveau.demander(Demande::Interrompre { tour });
            self.emettre_historique();
        }
        tracing::info!(tour, en_cours, "réponse interrompue");
        crate::voix::taire(&self.app, "interruption");
        self.voix = None;
        self.voix_reportee = None;
        self.suite_apres_voix = false;
        // On vient de faire taire Kobi : pas d'annonce juste après.
        if !self.annonces.is_empty() {
            self.annonces.clear();
            tracing::info!("voix : annonce abandonnée après une interruption");
        }
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
        self.question_a_la_carte = self.dernier;
        self.calme_a = None;
        let en_cours = self.question_en_cours();
        self.depuis_le_repos(Evenement::PermissionDemandee, en_cours);
        if annonce.is_some() {
            self.annoncer_validation();
        }
        // « Oui » ou « non » sans le nom, tant que la carte attend (KOB-197).
        let _ = crate::ecoute::commander(
            &self.app,
            crate::ecoute::Commande::ReponseAttendue(crate::validation::ECHEANCE),
        );
    }

    fn validation_fermee(&mut self, id: &str, reponse: crate::validation::Reponse) {
        let suivante = self.validations.fermer(id);
        self.oublier_la_patience(reponse.acceptee(), None);
        // Une carte MCP de la conversation attend encore : c'est elle qui
        // décidera, Kobi reste en proposition ; refusée, une carte des notes
        // venue du calme l'y garde aussi (KOB-338, KOB-380).
        let derniere = self.validations.est_vide() && !self.carte_mcp_de_la_conversation();
        // Le tour d'une carte de Kobi n'est pas connu : acceptée, elle n'est
        // pas retenue pour la suite.
        let explicite = reponse != crate::validation::Reponse::Delai;
        self.decider_la_proposition(derniere, reponse.acceptee(), None, explicite);
        // Plus de carte : la fenêtre de suite reprend sa durée habituelle
        // (KOB-197), et celle de la carte se ferme (KOB-273).
        self.si_plus_aucune_carte();
        // La carte montre déjà « Pas de réponse : refusé. » ; la voix ne le
        // redit que si elle est le seul canal (KOB-215), et pas si la carte
        // suivante, de Kobi ou MCP (KOB-388), est annoncée aussitôt : dit
        // plus tard, il semblerait répondre à celle-ci (KOB-366).
        if reponse == crate::validation::Reponse::Delai && self.voix_seul_canal() {
            let message = kobi_i18n::tr("coque.chef.delaiCarte", &[]);
            let _ = self.app.emit(EVENEMENT_REPONSE, &message);
            if suivante.is_none() && self.carte_mcp_a_annoncer().is_none() {
                self.annoncer(message);
            }
        }
        if suivante.is_some() {
            self.annoncer_validation();
            // La carte suivante attend à son tour sa réponse (KOB-197).
            let _ = crate::ecoute::commander(
                &self.app,
                crate::ecoute::Commande::ReponseAttendue(crate::validation::ECHEANCE),
            );
        } else {
            self.annoncer_carte_mcp();
        }
    }

    /// « Oui » ou « non » dit à une carte : un échange fini dans la
    /// conversation, pas une question qui attendrait sa réponse (KOB-336).
    fn reponse_a_la_carte(&mut self, question: String, voix: bool, accepte: bool) {
        let reponse = kobi_i18n::tr(
            if accepte {
                "coque.chef.carteOui"
            } else {
                "coque.chef.carteNon"
            },
            &[],
        );
        self.tour += 1;
        self.ajouter(Bulle {
            tour: self.tour,
            question,
            reponse,
            statut: "fini",
            clavier: !voix,
            etapes: Vec::new(),
        });
    }

    /// « Kobi, oui » vient de trancher une carte (la validation `validation`
    /// ou la carte MCP `carte`) : si une autre attend encore, l'écoute ouverte
    /// par le nom se referme et Kobi y repropose, au lieu de rester figé en
    /// écoute jusqu'à sa décision (KOB-379). Seule, la carte tranchée fait
    /// passer de l'écoute à la réflexion à sa fermeture (KOB-336).
    fn reproposer_si_une_autre_attend(&mut self, validation: Option<&str>, carte: Option<u64>) {
        if self.activite() != Some("ecoute") {
            return;
        }
        let autre_validation = validation.map_or(!self.validations.est_vide(), |id| {
            self.validations.autre_que(id)
        });
        if autre_validation || self.carte_affichee_sauf(carte).is_some() {
            self.transition(&[Evenement::QuestionEnvoyee, Evenement::PermissionDemandee]);
        }
    }

    /// Une carte de Kobi ou de la conversation vient d'être tranchée
    /// (`explicite` : par l'utilisateur, pas à l'échéance). Si d'autres
    /// attendent, Kobi reste en proposition ; acceptée, la carte du tour
    /// `tour` est retenue. La dernière décide : acceptée, Kobi repart sur le
    /// tour ; refusée aussi si une carte retenue appartient à la question en
    /// cours, toujours en cours (KOB-381) ; sinon il revient au calme, sauf si
    /// une carte des notes venue du calme attend encore (KOB-338, KOB-380).
    fn decider_la_proposition(
        &mut self,
        derniere: bool,
        acceptee: bool,
        tour: Option<u64>,
        explicite: bool,
    ) {
        if !derniere {
            if acceptee {
                self.acceptee_en_attente = tour.or(self.acceptee_en_attente);
            }
            return;
        }
        let notes_du_calme = self.cartes_des_notes.values().any(|&calme| calme);
        // Pendant l'écoute ouverte par le nom, seule une réponse fait
        // repartir le tour : une carte échue laisse venir la question.
        let tour_continue = self.acceptee_en_attente.take().is_some_and(|t| {
            t == self.dernier
                && self
                    .historique
                    .iter()
                    .any(|b| b.tour == t && matches!(b.statut, "attente" | "reponse" | "tache"))
        }) && (explicite || self.activite() == Some("propose"));
        if acceptee || tour_continue {
            self.transition(&[Evenement::PermissionAccordee]);
        } else if !notes_du_calme {
            self.transition(&[Evenement::PermissionRefusee]);
        }
    }

    /// La dernière question posée attend-elle encore sa réponse (en cours,
    /// partie en tâche, ou en attente du réseau) ?
    fn question_en_cours(&self) -> bool {
        self.historique
            .iter()
            .any(|b| b.tour == self.dernier && matches!(b.statut, "attente" | "reponse" | "tache"))
    }

    /// Une carte MCP de la conversation (pas des notes) attend-elle ?
    fn carte_mcp_de_la_conversation(&self) -> bool {
        self.cartes
            .keys()
            .any(|k| !self.cartes_des_notes.contains_key(k))
    }

    /// Plus aucune carte, ni de Kobi ni MCP, n'attend : la fenêtre de la
    /// réponse se ferme (KOB-273, KOB-336).
    fn si_plus_aucune_carte(&self) {
        if self.validations.est_vide() && self.cartes.is_empty() {
            let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::CartesFermees);
        }
    }

    /// Carte MCP affichée, à qui « oui » / « non » répondent : la plus
    /// récente, comme à l'écran (KOB-343).
    fn carte_affichee(&self) -> Option<u64> {
        self.carte_affichee_sauf(None)
    }

    fn carte_affichee_sauf(&self, sauf: Option<u64>) -> Option<u64> {
        self.cartes_ouvertes
            .iter()
            // Échue chez kobi-perms (refusée), elle n'attend plus de réponse.
            .filter(|(carte, ouverte)| {
                self.cartes.contains_key(carte)
                    && Some(**carte) != sauf
                    && ouverte.elapsed() < kobi_perms::DEFAULT_TIMEOUT
            })
            .max_by_key(|(carte, ouverte)| (**ouverte, **carte))
            .map(|(carte, _)| *carte)
    }

    /// La carte affichée se lit à l'écran : la voix demande seulement « Je
    /// le fais ? ». Fenêtre de Kobi cachée, la voix est le seul canal : elle
    /// relit la demande (KOB-215). Dès que la voix est libre, sans couper la
    /// réponse d'un autre tour (KOB-375) : l'outil de Kobi qui demande la
    /// carte a détaché son tour, dont l'énoncé est déjà lâché (KOB-366) ;
    /// une réponse encore ouverte est donc celle d'une autre question.
    fn annoncer_validation(&mut self) {
        let coupures = crate::voix::interruptions(&self.app);
        self.validation_a_dire = self
            .validations
            .cible()
            .map(|id| (id.to_string(), coupures));
        self.annonce_en_attente();
    }

    /// « Je le fais ? » en attente, s'il est encore de mise : sa carte
    /// attend sa réponse et l'utilisateur n'a pas fait taire Kobi depuis
    /// (la carte reste à l'écran).
    fn dire_validation(&mut self) -> bool {
        let coupees = crate::voix::interruptions(&self.app);
        if let Some((id, coupures)) = self.validation_a_dire.take() {
            match self.validations.resume(&id) {
                None => tracing::info!(%id, "voix : carte tranchée avant « Je le fais ? »"),
                Some(_) if coupures != coupees => {
                    tracing::info!(%id, "voix : « Je le fais ? » abandonné, voix coupée");
                }
                Some(resume) => {
                    self.dire_je_le_fais(&resume);
                    return true;
                }
            }
        }
        // Puis la carte MCP de la conversation, encore affichée (KOB-388).
        let Some((carte, coupures)) = self.carte_a_dire.take() else {
            return false;
        };
        // Une carte de Kobi attend : « oui » irait à elle, pas à celle-ci,
        // reprise à la fermeture de la dernière (`annoncer_carte_mcp`).
        if !self.validations.est_vide() {
            tracing::info!(carte, "voix : carte MCP remise après la carte de Kobi");
            return false;
        }
        // Seule la carte affichée, à qui « oui » répond, s'annonce : pas une
        // carte tranchée, échue, ou cachée par une carte des notes plus
        // récente.
        if self.carte_affichee() != Some(carte) {
            tracing::info!(
                carte,
                "voix : carte MCP plus affichée, « Je le fais ? » abandonné"
            );
            return false;
        }
        let Some(ui) = self.cartes.get(&carte) else {
            return false;
        };
        if coupures != coupees {
            tracing::info!(carte, "voix : « Je le fais ? » abandonné, voix coupée");
            return false;
        }
        let resume = resume_carte(ui);
        self.dire_je_le_fais(&resume);
        true
    }

    /// Plus de carte de Kobi, ou la carte MCP affichée vient d'être
    /// tranchée : celle de la conversation qui attend encore (la plus
    /// récente) est annoncée à son tour (KOB-388).
    fn annoncer_carte_mcp(&mut self) {
        if let Some(carte) = self.carte_mcp_a_annoncer() {
            self.carte_a_dire = Some((carte, crate::voix::interruptions(&self.app)));
            self.annonce_en_attente();
        }
    }

    /// Carte MCP de la conversation à annoncer : celle qui est affichée (à
    /// qui « oui » répond), sans carte de Kobi qui attend ; une carte des
    /// notes affichée reste muette.
    fn carte_mcp_a_annoncer(&self) -> Option<u64> {
        if !self.validations.est_vide() {
            return None;
        }
        self.carte_affichee()
            .filter(|carte| !self.cartes_des_notes.contains_key(carte))
    }

    fn dire_je_le_fais(&self, resume: &str) {
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
            // Remise à zéro de la conversation du backend clé API (KOB-316) :
            // une ligne dans l'activité.
            Retour::Avis { texte } => {
                tracing::info!(%texte, "avis du cerveau");
                let texte = if texte == kobi_brain::api::AVIS_REPRISE {
                    kobi_i18n::tr("coque.activite.repriseConversation", &[])
                } else {
                    texte
                };
                crate::activite::noter(&self.app, crate::activite::Genre::Reunion, &texte, None);
            }
            // Phrase peut-être pas pour Kobi : rien ne s'affiche avant la fin.
            Retour::Texte { tour, .. } if self.tour_peut_etre == Some(tour) => {}
            Retour::Fin { tour, texte }
                if self.tour_peut_etre == Some(tour) && est_silence(&texte) =>
            {
                tracing::info!(tour, "phrase qui ne s'adressait pas à Kobi : silence");
                self.tour_peut_etre = None;
                self.rangement.remove(&tour);
                if let Some(r) = &mut self.reunion {
                    r.latences.abandon(tour);
                }
                self.historique.retain(|b| b.tour != tour);
                if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                    if let Some((_, r)) = self.voix.take() {
                        r.lacher(&self.app);
                    }
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
                    self.depuis_le_repos(Evenement::ReponseCommence, true);
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                }
                self.reprendre_voix();
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
                self.ranger(tour, Qui::Kobi, &texte, false);
                if let Some(b) = self.bulle(tour) {
                    b.reponse.clone_from(&texte);
                    b.statut = "fini";
                }
                let reportee = self.voix_reportee.is_some_and(|(t, _)| t == tour);
                // L'état d'abord : un Oups du réseau s'acquitte ici, et la
                // lecture armée par la reprise de la voix part de « répond ».
                // Avant, elle partait de l'Oups et gardait son acquittement,
                // refusé ensuite depuis « répond » (KOB-416).
                if tour == self.dernier {
                    self.depuis_le_repos(Evenement::ReponseCommence, true);
                }
                self.reprendre_voix();
                if let Some((t, mut r)) = self.voix.take() {
                    if t == tour {
                        r.fin(&self.app, &texte);
                    } else {
                        self.voix = Some((t, r));
                    }
                }
                if tour == self.dernier {
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                    // Réponse reportée (KOB-377) : la lecture commence quand
                    // elle est dite, ou abandonnée (`reprendre_voix`).
                    if !reportee {
                        self.lire(&texte);
                    }
                }
                self.emettre_historique();
            }
            Retour::Detache { tour, annonce } => {
                tracing::info!(tour, annonce, "tour détaché");
                // Le tour ne parle plus par sa réponse en cours (son résultat
                // viendra à part) : son énoncé se clôt, avec ou sans annonce
                // (carte, tâche longue), sinon la voix resterait occupée
                // (KOB-366).
                if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                    if let Some((_, r)) = self.voix.take() {
                        r.lacher(&self.app);
                    }
                }
                if self.voix_reportee.is_some_and(|(t, _)| t == tour) {
                    self.voix_reportee = None;
                }
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
                self.ranger(tour, Qui::Kobi, &texte, false);
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
                self.rangement.remove(&tour);
                if let Some(r) = &mut self.reunion {
                    r.latences.abandon(tour);
                }
                if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                    if let Some((_, r)) = self.voix.take() {
                        r.lacher(&self.app);
                    }
                }
                if self.voix_reportee.is_some_and(|(t, _)| t == tour) {
                    self.voix_reportee = None;
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
                // La question repart de zéro (le plus souvent sans rien avoir
                // montré, KOB-370) : la voix est libre pendant l'attente, ce
                // qui était confié se dit encore, et la réponse repartira sur
                // un énoncé neuf.
                if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                    if let Some((_, r)) = self.voix.take() {
                        r.lacher(&self.app);
                    }
                    self.voix_reportee = Some((tour, crate::voix::interruptions(&self.app)));
                    self.annonce_en_attente();
                }
                self.emettre_historique();
                if tour == self.dernier {
                    self.erreur(titre, &explication);
                    self.oups_du_reseau = Some(tour);
                }
            }
            Retour::Carte {
                tour,
                carte,
                outil,
                entree,
            } => {
                tracing::info!(tour, carte, %outil, "carte de confirmation");
                // Proposée par les notes après « Terminer » (tour de notes
                // déjà en cours) : refusée sans être montrée (KOB-331).
                if tour == 0 && self.reunion.as_ref().is_none_or(|r| r.fin_demandee) {
                    tracing::info!(carte, "carte des notes après la fin : refusée");
                    self.cerveau.demander(Demande::Decision {
                        carte,
                        accepte: false,
                    });
                    return;
                }
                let mut ui = carte_ui(carte, &outil, &entree);
                // Proposée par les notes, pas par la question en cours : elle
                // le dit en tête (KOB-336).
                if tour == 0 {
                    ui.kicker = format!(
                        "{} · {}",
                        kobi_i18n::tr("coque.chef.depuisLesNotes", &[]),
                        ui.kicker
                    );
                }
                self.cartes.insert(carte, ui.clone());
                self.cartes_ouvertes.insert(carte, Instant::now());
                if tour != 0 {
                    self.tours_des_cartes.insert(carte, tour);
                }
                if tour == 0 {
                    // Proposée par les notes en continu (KOB-72) : depuis le
                    // calme, Kobi passe des notes à la proposition. Occupé
                    // ailleurs (réponse, lecture, tâche), il n'en change pas,
                    // ni son retour au calme (KOB-338) : la carte s'affiche.
                    // Une autre carte des notes venue du calme attend déjà :
                    // celle-ci y ramène aussi.
                    let deja = self.activite() == Some("propose")
                        && self.cartes_des_notes.values().any(|&calme| calme)
                        && self
                            .cartes
                            .keys()
                            .all(|k| *k == carte || self.cartes_des_notes.contains_key(k));
                    let calme = self.activite() == Some("calme");
                    self.cartes_des_notes.insert(carte, calme || deja);
                    if calme {
                        self.calme_a = None;
                        self.transition(&[
                            Evenement::NotesCommencent,
                            Evenement::PermissionDemandee,
                        ]);
                    }
                } else {
                    self.question_a_la_carte = self.dernier;
                    self.calme_a = None;
                    let en_cours = self.question_en_cours();
                    self.depuis_le_repos(Evenement::PermissionDemandee, en_cours);
                }
                let _ = self.app.emit(EVENEMENT_CARTE, &ui);
                // Celle de la conversation s'annonce comme une carte de Kobi
                // (KOB-388) ; celle des notes reste muette en pleine réunion.
                if tour != 0 {
                    self.carte_a_dire = Some((carte, crate::voix::interruptions(&self.app)));
                    self.annonce_en_attente();
                }
                // « Oui » ou « non » sans le nom hors réunion, tant que la
                // carte attend (KOB-197, KOB-336).
                let _ = crate::ecoute::commander(
                    &self.app,
                    crate::ecoute::Commande::ReponseAttendue(kobi_perms::DEFAULT_TIMEOUT),
                );
            }
            Retour::CarteFermee { carte, accepte } => {
                tracing::info!(carte, ?accepte, "carte fermée");
                // Déjà fermée (écho du cerveau après un refus de Kobi lui-même,
                // KOB-331), ou jamais montrée (carte des notes après la fin) :
                // rien ne change une seconde fois (KOB-341).
                let Some(mut ui) = self.cartes.remove(&carte) else {
                    self.cartes_ouvertes.remove(&carte);
                    self.cartes_des_notes.remove(&carte);
                    self.tours_des_cartes.remove(&carte);
                    self.notes_acceptees.remove(&carte);
                    return;
                };
                let ui_fermee = Some(ui.clone());
                ui.status = if accepte == Some(true) {
                    "confirmed"
                } else {
                    "dismissed"
                };
                let _ = self.app.emit(EVENEMENT_CARTE, &ui);
                self.cartes_ouvertes.remove(&carte);
                self.si_plus_aucune_carte();
                // Une carte des notes n'a pas de tour retenu : acceptée, elle ne
                // fait continuer aucune tâche.
                let tour_de_la_carte = self.tours_des_cartes.get(&carte).copied();
                self.oublier_la_patience(accepte == Some(true), tour_de_la_carte);
                let des_notes = self.cartes_des_notes.remove(&carte);
                // La carte de la conversation de nouveau affichée s'annonce.
                self.annoncer_carte_mcp();
                let tour = self.tours_des_cartes.remove(&carte);
                self.notes_acceptees.remove(&carte);
                // Une action des notes acceptée laisse sa trace dans les notes
                // à venir et le compte rendu : Claude ne la repropose pas, même
                // sur une session neuve (KOB-331).
                if let (true, Some(true), Some(ui), Some(r)) = (
                    des_notes.is_some(),
                    accepte,
                    ui_fermee.as_ref(),
                    self.reunion.as_mut(),
                ) {
                    let t = r.debut.elapsed().as_secs();
                    r.a_noter.push_str(&format!(
                        "[{:02}:{:02} action acceptée] {} ({})\n",
                        t / 60,
                        t % 60,
                        ui.title.replace('\n', " "),
                        ui.kicker.replace('\n', " ")
                    ));
                }
                match des_notes {
                    // Carte des notes : aucune réponse ne la suit. Venue du
                    // calme, Kobi y revient quand plus aucune carte n'attend,
                    // s'il est encore en proposition (pas reparti sur une
                    // question entre-temps) ; venue d'ailleurs, elle n'a rien
                    // changé (KOB-338).
                    Some(depuis_calme) => {
                        // Une carte de Kobi qui attend tient aussi la
                        // proposition (KOB-380).
                        if self.cartes.is_empty()
                            && self.validations.est_vide()
                            && depuis_calme
                            && self.activite() == Some("propose")
                        {
                            self.transition(&[if accepte == Some(true) {
                                Evenement::PermissionAccordee
                            } else {
                                Evenement::PermissionRefusee
                            }]);
                            self.transition(&[Evenement::ReponseTerminee]);
                        }
                    }
                    // Carte de la conversation : c'est elle qui tient Kobi en
                    // proposition. Sans autre carte de la conversation, sa
                    // décision s'applique, même si une carte des notes attend ;
                    // refusée, Kobi reste en proposition si celle-ci est venue
                    // du calme (KOB-338).
                    None => {
                        // Une carte de Kobi qui attend encore tient aussi Kobi
                        // en proposition (KOB-380).
                        let derniere =
                            !self.carte_mcp_de_la_conversation() && self.validations.est_vide();
                        self.decider_la_proposition(
                            derniere,
                            accepte == Some(true),
                            tour,
                            accepte.is_some(),
                        );
                    }
                }
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

    /// Cartes MCP restées sans clic au-delà de l'échéance de kobi-perms (qui
    /// les a refusées) : fermées comme un refus (KOB-72).
    fn cartes_expirees(&mut self) {
        let echeance = kobi_perms::DEFAULT_TIMEOUT + Duration::from_secs(5);
        let expirees: Vec<u64> = self
            .cartes_ouvertes
            .iter()
            .filter(|(_, ouverte)| ouverte.elapsed() > echeance)
            .map(|(carte, _)| *carte)
            .collect();
        for carte in expirees {
            tracing::info!(carte, "carte sans réponse : refusée à l'échéance");
            self.retour(Retour::CarteFermee {
                carte,
                accepte: None,
            });
        }
    }

    fn notes(&mut self, resultat: Result<String, String>) {
        let Some(r) = &mut self.reunion else { return };
        let envoye = r.envoye.take().unwrap_or_default();
        match resultat {
            Ok(notes) if !notes.trim().is_empty() => {
                r.bloc_en_echec = false;
                r.derniere_note = derniere_note(&r.notes, &notes).or(r.derniere_note.take());
                r.notes = notes;
                self.ecrivain.notes(r.notes.clone());
                tracing::info!(lignes = compter_notes(&r.notes), "notes mises à jour");
                self.emettre_enregistrement();
            }
            Ok(_) => r.bloc_en_echec = false,
            Err(e) => {
                tracing::warn!(erreur = %e, "notes : bloc en échec, repris au prochain");
                r.a_noter.insert_str(0, &envoye);
                // Repris à l'heure, pas au tick suivant : en continu, le bloc
                // remis a déjà ses phrases (KOB-72).
                r.bloc_en_echec = true;
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
        // Transcription en retard au moment du compte rendu : il le dit, et la
        // suite va dans la transcription de cette réunion (KOB-281).
        let texte = if r.incomplet {
            self.reunion_close = r
                .arret
                .map(|arret| (r.debut, arret, r.fins_annulees.clone()));
            format!(
                "{}\n\n{}",
                texte.trim_end(),
                kobi_i18n::tr("coque.chef.compteRenduIncomplet", &[])
            )
        } else {
            texte
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
                        // Annoncé une fois à voix haute, si la voix est active (KOB-204),
                        // sans couper la réponse en cours (KOB-365).
                        self.annoncer(kobi_i18n::tr("coque.chef.compteRenduPret", &[]));
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
    /// réunion n'est en cours (KOB-181). Une question nettement anglaise est
    /// précédée d'un rappel en anglais : sans lui, ce contexte en français
    /// l'emporte et Claude répond en français (KOB-268).
    fn avec_contexte(&self, question: String) -> String {
        let maintenant = moment(&jiff::Zoned::now());
        let langue = kobi_brain::langue::rappel_de_langue(&question)
            .map(|r| format!("\n{r}"))
            .unwrap_or_default();
        let etat = match &self.reunion {
            // Dit aussi hors réunion : sans cela, Claude en suppose une
            // (« comment se passe ta réunion ? », KOB-181).
            None => {
                return format!(
                    "{maintenant}\n(Aucune réunion en cours.){}{langue}\n{question}",
                    consigne_longueur(crate::voix::verbosite(&self.app))
                )
            }
            Some(r) if r.fin_demandee => "arrêté, compte rendu à venir",
            Some(r) if r.pause => "en pause",
            Some(_) => "en cours",
        };
        format!(
            "{maintenant}\n(Réunion : enregistrement {etat}.){}{langue}\n{question}",
            consigne_longueur(crate::voix::verbosite(&self.app))
        )
    }

    fn fermer_conversation(&mut self) {
        if self.conversation.take().is_some() {
            self.conversation_n += 1;
            self.ecrivain.fermer_conversation();
            let _ = self.app.emit(EVENEMENT_FIN_CONVERSATION, ());
        }
    }

    /// Kobi a fini de répondre : temps de lecture, puis la conversation
    /// attend la suite sans qu'on redise « Kobi » (KOB-133).
    fn lire(&mut self, texte: &str) {
        let lecture = LECTURE_MIN + Duration::from_millis(60 * texte.chars().count() as u64);
        // Un Oups encore affiché garde son acquittement : le retour au calme
        // de la lecture n'est pas accepté depuis « oups », et Kobi y restait
        // pour de bon (KOB-392). Déjà acquitté (réponse arrivée après
        // l'attente du réseau, KOB-386), il laisse place à la lecture.
        let oups_affiche = self.activite() == Some("oups")
            && self
                .calme_a
                .is_some_and(|(_, e)| matches!(e, Evenement::Acquitter));
        if !oups_affiche {
            self.calme_a = Some((
                Instant::now() + lecture.min(Duration::from_secs(25)),
                Evenement::ReponseTerminee,
            ));
        }
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
        // Une carte attend la décision : Kobi ne s'occupe encore de rien
        // (KOB-385). Remis à plus tard, si la tâche tarde après.
        if longue && self.carte_attend() {
            tracing::info!(tour, "patience remise : une carte attend la décision");
            self.patience_reportee = Some((tour, None));
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

    /// Carte refusée : Kobi ne s'occupera de rien, la patience remise n'a
    /// plus de sens (KOB-385). Sauf si une carte MCP de son tour a déjà été
    /// acceptée : la tâche continue, sa patience sera dite si elle tarde, et
    /// oubliée si elle finit d'abord (KOB-396). Le tour d'une carte de Kobi
    /// n'est pas connu : acceptée, elle n'est pas retenue.
    fn oublier_la_patience(&mut self, acceptee: bool, tour: Option<u64>) {
        if acceptee {
            if let Some(tour) = tour {
                self.tours_acceptes.insert(tour);
            }
        } else if self
            .patience_reportee
            .is_some_and(|(t, _)| !self.tours_acceptes.contains(&t))
        {
            self.patience_reportee = None;
            tracing::info!("patience remise oubliée : carte refusée");
        }
        // Seuls comptent les tours qui tournent encore.
        let historique = &self.historique;
        self.tours_acceptes.retain(|t| {
            historique
                .iter()
                .any(|b| b.tour == *t && matches!(b.statut, "attente" | "reponse" | "tache"))
        });
    }

    /// Une carte attend-elle la décision de l'utilisateur : carte de Kobi,
    /// ou carte MCP de la conversation affichée ?
    fn carte_attend(&self) -> bool {
        !self.validations.est_vide()
            || self
                .carte_affichee()
                .is_some_and(|carte| !self.cartes_des_notes.contains_key(&carte))
    }

    /// Patience remise pendant une carte (KOB-385) : dite si le tour tourne
    /// encore [`cerveau::PATIENCE`] après la dernière décision ; oubliée si
    /// sa réponse est arrivée, s'il a été interrompu, ou si une carte a été
    /// refusée (`oublier_la_patience`).
    fn patience_en_attente(&mut self) {
        let Some((tour, libre_depuis)) = self.patience_reportee else {
            return;
        };
        // Le tour d'une carte MCP n'est pas une tâche annoncée (`taches`) :
        // c'est sa bulle qui dit s'il tourne encore.
        let en_cours = !self.interrompus.contains(&tour)
            && self
                .bulle(tour)
                .is_some_and(|b| matches!(b.statut, "attente" | "reponse" | "tache"));
        if !en_cours {
            self.patience_reportee = None;
        } else if self.carte_attend() {
            self.patience_reportee = Some((tour, None));
        } else {
            let depuis = libre_depuis.unwrap_or_else(Instant::now);
            if depuis.elapsed() < cerveau::PATIENCE {
                self.patience_reportee = Some((tour, Some(depuis)));
            } else {
                self.patience_reportee = None;
                self.patienter(tour, &crate::pannes::patience(), true);
            }
        }
    }

    fn erreur(&mut self, titre: &str, message: &str) {
        // Une nouvelle erreur : l'Oups n'est plus celui de l'attente du
        // réseau, qui le remet après coup s'il s'agit d'elle (KOB-386).
        self.oups_du_reseau = None;
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
        // La dernière question attend encore sa réponse (réseau coupé, ou
        // Kobi ramené au calme par le début d'une réunion…) : le résultat
        // d'une tâche plus ancienne passe après elle. Montré avant, il
        // devenait la dernière question, celle qui attendait n'était plus en
        // cours et sa réponse arrivait sans être montrée ni lue (KOB-422).
        let dernier = self.dernier;
        if self
            .historique
            .iter()
            .any(|b| b.tour == dernier && b.statut == "attente")
        {
            return;
        }
        let activite = self.activite();
        // Kobi réfléchit sans question en cours (carte acceptée après la
        // réponse à une autre question, son tour parti en tâche) : rien ne
        // l'en sortira, le résultat passe par « au travail » (KOB-384).
        let sans_question = activite == Some("reflechit") && !self.question_en_cours();
        // Kobi réfléchit à la question en cours, dont le tour, détaché par
        // une carte acceptée, livre sa réponse ici : elle passe tout de suite,
        // devant les résultats plus anciens, qui suivent ensuite (KOB-387).
        // Pareil s'il répondait déjà : le tour s'est détaché en cours de
        // réponse (permission refusée sans carte), rien d'autre ne le ferait
        // sortir de « répond » (KOB-415).
        let la_sienne = matches!(activite, Some("reflechit" | "repond"))
            && match self.a_livrer.iter().position(|(tour, ..)| *tour == dernier) {
                Some(i) => {
                    if let Some(sienne) = self.a_livrer.remove(i) {
                        self.a_livrer.push_front(sienne);
                    }
                    true
                }
                None => false,
            };
        let lit_encore = self
            .calme_a
            .is_some_and(|(_, e)| matches!(e, Evenement::ReponseTerminee));
        if (matches!(
            activite,
            Some("ecoute" | "reflechit" | "repond" | "propose")
        ) && !la_sienne
            && !sans_question)
            || lit_encore
            || crate::voix::parle(&self.app)
            // Depuis « répond », le début déjà confié à la voix se dit
            // jusqu'au bout, synthèse comprise (KOB-415).
            || (activite == Some("repond") && crate::voix::occupee(&self.app))
            || self.livrable_a.is_some_and(|t| Instant::now() < t)
        {
            return;
        }
        // Réunion en pause ou qui se termine : pas d'état « au travail »,
        // le résultat est montré tel quel, sans attendre la reprise.
        let sans_activite = matches!(etat(&self.app).etat, "pause" | "fin");
        if activite != Some("occupe") && !sans_activite && !la_sienne {
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
    /// Annonce de service à voix haute (KOB-365, KOB-366) : tout de suite si
    /// Kobi ne répond pas, sinon après la réponse en cours (écrite ou encore
    /// dite), sans jamais la couper.
    fn annoncer(&mut self, texte: String) {
        self.annonces.push_back(Annonce {
            texte,
            depuis: Instant::now(),
            coupures: crate::voix::interruptions(&self.app),
        });
        self.annonce_en_attente();
    }

    /// Annonces en attente, dans l'ordre : la première est dite quand la
    /// voix est libre ; abandonnée si l'utilisateur a coupé la voix depuis,
    /// ou au-delà de [`ANNONCE_ATTENTE`] (l'écran la montre déjà).
    fn annonce_en_attente(&mut self) {
        let coupures = crate::voix::interruptions(&self.app);
        let avant = self.annonces.len();
        self.annonces
            .retain(|a| a.coupures == coupures && a.depuis.elapsed() < ANNONCE_ATTENTE);
        if self.annonces.len() < avant {
            tracing::info!("voix : annonce abandonnée (voix coupée ou réponse trop longue)");
        }
        // Réponse reportée et voix coupée depuis : abandonnée tout de suite,
        // même si la voix joue encore (KOB-377).
        if self.voix_reportee.is_some_and(|(_, c)| c != coupures) {
            self.reprendre_voix();
        }
        if self.voix.is_none() && !crate::voix::occupee(&self.app) && !self.dire_validation() {
            self.reprendre_voix();
            if self.voix.is_none() && !crate::voix::occupee(&self.app) {
                if let Some(annonce) = self.annonces.pop_front() {
                    crate::voix::dire(&self.app, &annonce.texte);
                }
            }
        }
    }

    /// Réponse d'une question qui a attendu le réseau (KOB-377) : elle se dit
    /// sur un énoncé neuf, une fois arrivée et la voix libre, sans couper
    /// « Je le fais ? » ni une annonce dits pendant l'attente ; « Je le
    /// fais ? » en attente passe d'abord. Abandonnée si l'utilisateur a coupé
    /// la voix depuis (il parle, « Kobi »…) : la réponse reste à l'écran.
    fn reprendre_voix(&mut self) {
        let Some((tour, coupures)) = self.voix_reportee else {
            return;
        };
        if coupures != crate::voix::interruptions(&self.app) {
            tracing::info!(tour, "voix : réponse reportée abandonnée, voix coupée");
            self.voix_reportee = None;
            // Déjà arrivée : sa lecture à l'écran ramène Kobi au calme.
            let fini = self.bulle(tour).filter(|b| b.statut == "fini");
            if let Some(texte) = fini.map(|b| b.reponse.clone()) {
                if tour == self.dernier {
                    self.lire(&texte);
                }
            }
            return;
        }
        if self.voix.is_some()
            || self.validation_a_dire.is_some()
            || self.carte_a_dire.is_some()
            || crate::voix::occupee(&self.app)
        {
            return;
        }
        let Some((statut, texte)) = self.bulle(tour).map(|b| (b.statut, b.reponse.clone())) else {
            self.voix_reportee = None;
            return;
        };
        match statut {
            "attente" => return,
            "reponse" => {
                let mut r = crate::voix::Reponse::new(&self.app);
                r.morceau(&self.app, &texte);
                self.voix = Some((tour, r));
            }
            "fini" => {
                crate::voix::dire(&self.app, &texte);
                if tour == self.dernier {
                    self.lire(&texte);
                }
            }
            _ => {}
        }
        self.voix_reportee = None;
    }

    pub fn tick(&mut self) {
        self.annonce_en_attente();
        self.patience_en_attente();
        self.cartes_expirees();
        self.poser_questions_differees();
        let parle = crate::voix::parle(&self.app);
        // Un Oups sans acquittement armé (une carte ouverte ou un tour
        // détaché pendant qu'il s'affichait vident `calme_a`) s'efface quand
        // même au bout de son délai ; la carte ou la tâche reprend ensuite la
        // main (KOB-394).
        if self.calme_a.is_none() && self.activite() == Some("oups") {
            self.calme_a = Some((Instant::now() + ERREUR_AFFICHEE, Evenement::Acquitter));
        }
        if let Some((quand, evenement)) = self.calme_a {
            // Le retour au calme attend que Kobi ait fini de parler.
            if Instant::now() >= quand && !parle {
                self.calme_a = None;
                // Filet : un retour au calme refusé depuis un Oups l'acquitte,
                // plutôt que de l'y laisser pour de bon (KOB-392).
                if !self.transition(&[evenement]) && self.activite() == Some("oups") {
                    self.transition(&[Evenement::Acquitter]);
                }
            }
        }
        // Résultats de tâches en attente : le suivant, s'il peut passer.
        self.livrer();
        // Kobi s'est tu : la conversation attend la suite à partir de maintenant.
        if self.suite_apres_voix && !parle {
            self.suite_apres_voix = false;
            let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Suite);
        }
        // « Au travail » sans tâche, sans résultat à livrer ni question en
        // cours (la tâche d'une question plus ancienne a fini en erreur) :
        // rien ne l'en sortirait, retour au calme (KOB-417).
        if self.activite() == Some("occupe")
            && self.taches.is_empty()
            && self.a_livrer.is_empty()
            && !self.question_en_cours()
            && self.calme_a.is_none()
        {
            self.transition(&[Evenement::ReponseTerminee]);
        }
        // Revenu au calme alors qu'une tâche, ou la question en cours, tourne
        // encore (Oups acquitté pendant l'attente du réseau, reprise après la
        // pause…) : Kobi s'en occupe, au lieu de paraître au repos (KOB-418).
        if (!self.taches.is_empty() || self.question_en_cours())
            && self.calme_a.is_none()
            && matches!(self.activite(), Some("calme") | None)
            && matches!(etat(&self.app).etat, "veille" | "reunion")
        {
            self.transition(&[Evenement::TacheLongue]);
        }
        // Une carte de Kobi ou de la conversation attend encore et Kobi est
        // libre (réponse à une autre question finie) : il y repropose
        // (KOB-384). Une carte des notes venue d'ailleurs n'y ramène pas
        // (KOB-338), ni une carte MCP déjà refusée à l'échéance ; un
        // résultat à livrer passe d'abord, et la question en cours, partie en
        // tâche, livrera le sien sans attendre la carte (pas celle de la
        // carte elle-même, partie en tâche pour la demander).
        let carte_mcp = self.cartes.keys().any(|k| {
            !self.cartes_des_notes.contains_key(k)
                && self
                    .cartes_ouvertes
                    .get(k)
                    .is_some_and(|o| o.elapsed() < kobi_perms::DEFAULT_TIMEOUT)
        });
        if (!self.validations.est_vide() || carte_mcp)
            && !(self.question_en_cours() && self.dernier != self.question_a_la_carte)
            && self.calme_a.is_none()
            && self.a_livrer.is_empty()
            && matches!(self.activite(), Some("calme" | "occupe") | None)
            && matches!(etat(&self.app).etat, "veille" | "assistant" | "reunion")
        {
            if self.activite() != Some("occupe") {
                self.transition(&[Evenement::TacheLongue]);
            }
            self.transition(&[Evenement::PermissionDemandee]);
        }
        // Kobi réfléchit sans rien qui puisse répondre : aucune question qui
        // tourne encore (ni la dernière, ni une plus ancienne détachée par sa
        // carte), ni tâche, ni résultat à livrer (carte sans tour acceptée,
        // tâche finie en erreur) ; il revient au calme au lieu de rester figé
        // (KOB-391).
        let une_question_tourne = self
            .historique
            .iter()
            .any(|b| matches!(b.statut, "attente" | "reponse" | "tache"));
        if self.activite() == Some("reflechit")
            && !une_question_tourne
            && self.taches.is_empty()
            && self.a_livrer.is_empty()
            && self.calme_a.is_none()
        {
            tracing::info!("réflexion sans question ni tâche : retour au calme");
            self.transition(&[Evenement::ReponseTerminee]);
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
        let avancement = &self.avancement;
        let Some(r) = &mut self.reunion else { return };
        // « Annuler » n'a pas servi : la fin est définitive, une fois
        // transcrit ce qui a été dit pendant le délai (micro en retard,
        // KOB-265), au plus [`VIDANGE`] de plus.
        if r.annulable_jusqu.is_some_and(|quand| {
            let maintenant = Instant::now();
            maintenant >= quand
                && (maintenant >= quand + VIDANGE
                    || avancement.piste_a_jour(kobi_stt::Source::Moi, quand))
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
            // Après un refus, pas de nouvel essai sur la même parole (KOB-359).
            let nouvelle_parole = r.titre_essais == 0 || mots >= r.titre_mots + TITRE_MOTS;
            if pret && nouvelle_parole && r.titre_essais < TITRE_ESSAIS && r.titre_auto {
                r.titre_demande = true;
                r.titre_mots = mots;
                tracing::info!(mots, s = depuis.as_secs(), "titre de la réunion demandé");
                self.cerveau.demander(Demande::Titre {
                    transcript: r.debut_parole.clone(),
                });
            }
        }
        // En continu, le bloc part dès qu'il a ses phrases ; le reste part
        // à l'heure, comme en mode blocs.
        let du = Instant::now() >= r.prochain_bloc
            || (r.continu && !r.bloc_en_echec && r.a_noter.lines().count() >= PHRASES_CONTINU);
        if r.pause || r.fin_demandee || r.a_la_fin || r.envoye.is_some() || !du {
            return;
        }
        r.prochain_bloc = Instant::now() + self.intervalle;
        if r.a_noter.trim().is_empty() {
            return;
        }
        let bloc = std::mem::take(&mut r.a_noter);
        r.envoye = Some(bloc.clone());
        tracing::info!(
            octets = bloc.len(),
            continu = r.continu,
            "bloc de notes envoyé"
        );
        self.cerveau.demander(Demande::Notes {
            transcript: bloc,
            notes: r.notes.clone(),
            propositions: r.continu,
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
            // Réglage `brain` relu à chaque session (KOB-103) : Claude Code,
            // ou le backend clé API.
            let reglage = crate::cerveau_api::reglage();
            let programme = match reglage {
                kobi_session::config::Brain::Api => Err(String::new()),
                _ => programme
                    .get_or_init(|| async {
                        match kobi_brain::claude_code::detect::detect(Duration::from_secs(30)).await
                        {
                            kobi_brain::Detection::Ready { install, .. } => Ok(install.program),
                            kobi_brain::Detection::NotLoggedIn(_) => Err(
                                "Claude Code n'est pas connecté : lancer « claude auth login »"
                                    .into(),
                            ),
                            autre => Err(format!("Claude Code indisponible : {autre:?}")),
                        }
                    })
                    .await
                    .clone(),
            };
            // La clé n'est lue dans le trousseau que si elle peut servir.
            let cle = crate::cerveau_api::cle_utile(reglage, programme.is_ok());
            match crate::cerveau_api::choisir(reglage, programme.is_ok(), cle.is_some()) {
                crate::cerveau_api::Choix::Api => {
                    let cle = cle.unwrap_or_default();
                    let session = crate::cerveau_api::ouvrir(role, cle, codebases, memoire).await?;
                    tracing::info!(?role, "session clé API ouverte");
                    return Ok(Box::new(session) as Box<dyn kobi_brain::Brain + Send>);
                }
                crate::cerveau_api::Choix::SansCle => {
                    return Err(kobi_brain::BrainError::Unavailable(
                        "aucune clé API enregistrée : Réglages, panneau « Clé API »".into(),
                    ))
                }
                crate::cerveau_api::Choix::ClaudeCode => {}
            }
            let programme = programme.map_err(kobi_brain::BrainError::Unavailable)?;
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
        let jamais: Fabrique = Arc::new(|_| Box::pin(std::future::pending()));
        chef_en_essai_avec(jamais)
    }

    fn chef_en_essai_avec(
        fabrique: Fabrique,
    ) -> (tauri::App<tauri::test::MockRuntime>, tempfile::TempDir) {
        use tauri::test::{mock_builder, mock_context, noop_assets};
        let dir = tempfile::tempdir().unwrap();
        let app = crate::session::installer(mock_builder())
            .build(mock_context(noop_assets()))
            .unwrap();
        let mut cfg = kobi_session::config::Config::default();
        cfg.stockage.dossier = Some(dir.path().join("donnees"));
        crate::stockage::installer(app.handle(), &cfg, None);
        installer(app.handle(), fabrique, 5, cerveau::Delais::default());
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
        // De la parole : le compte rendu passe par Claude (KOB-281).
        avec_chef(&app, |c| {
            c.recevoir(Message::Segment(dit("On commence."), Instant::now()))
        });
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

    /// KOB-365 : une annonce de service attend la fin de la réponse en
    /// cours (écrite ou encore dite), puis part au tic ; une interruption
    /// l'abandonne ; trop longue, elle est sautée.
    #[test]
    fn annonce_apres_la_reponse_en_cours() {
        let (app, _dir) = chef_en_essai();
        // La voix (sans parleur) compte les coupures.
        crate::voix::installer_sans_parleur(app.handle(), &Default::default());
        avec_chef(&app, |c| {
            // Pas de réponse en cours : dite tout de suite.
            c.annoncer("Compte rendu prêt.".into());
            assert!(c.annonces.is_empty());
            // Une réponse s'écrit : l'annonce attend.
            c.voix = Some((7, crate::voix::Reponse::new(&c.app)));
            c.annoncer("Compte rendu prêt.".into());
            c.tick();
            assert_eq!(c.annonces.len(), 1, "pas pendant la réponse");
            // Réponse finie, voix libre : dite au tic.
            c.voix = None;
            c.tick();
            assert!(c.annonces.is_empty());
            // Interrompue : abandonnée.
            c.voix = Some((8, crate::voix::Reponse::new(&c.app)));
            c.annoncer("Compte rendu prêt.".into());
            c.interrompre();
            assert!(c.annonces.is_empty());
            // Toujours occupée au-delà du délai : sautée.
            c.voix = Some((9, crate::voix::Reponse::new(&c.app)));
            c.annonces.push_back(Annonce {
                texte: "Compte rendu prêt.".into(),
                depuis: Instant::now() - ANNONCE_ATTENTE,
                coupures: crate::voix::interruptions(&c.app),
            });
            c.tick();
            assert!(c.annonces.is_empty());
            // Voix coupée par l'utilisateur (il parle) sans passer par le
            // chef : abandonnée au tic.
            c.annoncer("Compte rendu prêt.".into());
            assert_eq!(c.annonces.len(), 1);
            crate::voix::taire(&c.app, "l'utilisateur parle");
            c.tick();
            assert!(c.annonces.is_empty());
        });
    }

    /// KOB-366 : une réponse lâchée sans fin (tour en erreur, ou parti en
    /// tâche) clôt son énoncé ; sinon la voix resterait « occupée » pour
    /// toujours et les annonces attendraient pour rien.
    #[test]
    fn reponse_lachee_libere_la_voix() {
        let (app, _dir) = chef_en_essai();
        crate::voix::installer_factice(app.handle());
        avec_chef(&app, |c| {
            let mut r = crate::voix::Reponse::new(&c.app);
            r.morceau(&c.app, "Je regarde le budget. Puis ");
            assert!(crate::voix::occupee(&c.app), "phrase confiée à la voix");
            c.voix = Some((5, r));
            c.recevoir(Message::Cerveau(Retour::Erreur {
                tour: 5,
                message: "erreur d'API : coupure".into(),
            }));
            assert!(c.voix.is_none());
            let limite = Instant::now() + Duration::from_secs(5);
            while crate::voix::occupee(&c.app) {
                assert!(Instant::now() < limite, "voix restée occupée");
                std::thread::sleep(Duration::from_millis(20));
            }
            // Détaché sans annonce (carte de permission) : pareil, et un
            // rappel échu ensuite n'attend pas pour rien.
            let mut r = crate::voix::Reponse::new(&c.app);
            r.morceau(&c.app, "Je crée le fichier. Puis ");
            c.voix = Some((6, r));
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour: 6,
                annonce: false,
            }));
            assert!(c.voix.is_none());
            let limite = Instant::now() + Duration::from_secs(5);
            while crate::voix::occupee(&c.app) {
                assert!(Instant::now() < limite, "voix restée occupée");
                std::thread::sleep(Duration::from_millis(20));
            }
            c.recevoir(Message::Rappel(crate::rappels::RappelUi {
                id: "r1".into(),
                texte: "Appeler Paul".into(),
                echeance_ms: None,
            }));
            assert!(c.annonces.is_empty(), "rappel dit tout de suite");
        });
    }

    /// KOB-366 : un rappel échu et « Pas de réponse : refusé » attendent la
    /// fin de la réponse en cours, comme « compte rendu prêt », puis sont
    /// dits dans l'ordre ; une interruption les abandonne.
    #[test]
    fn annonces_sans_couper_la_reponse() {
        let (app, _dir) = chef_en_essai();
        crate::voix::installer_sans_parleur(app.handle(), &Default::default());
        avec_chef(&app, |c| {
            c.voix = Some((7, crate::voix::Reponse::new(&c.app)));
            c.recevoir(Message::Rappel(crate::rappels::RappelUi {
                id: "r1".into(),
                texte: "Appeler Paul".into(),
                echeance_ms: None,
            }));
            // Carte expirée, fenêtre cachée : la voix est le seul canal.
            c.validation_ouverte("v1".into(), "Créer budget.md");
            c.annoncer(kobi_i18n::tr("coque.chef.delaiCarte", &[]));
            c.tick();
            assert_eq!(c.annonces.len(), 2, "rien pendant la réponse");
            assert!(c.annonces[0].texte.contains("Appeler Paul"));
            assert_eq!(
                c.validation_a_dire.as_ref().map(|v| v.0.as_str()),
                Some("v1")
            );
            // Réponse finie : « Je le fais ? » d'abord (la carte attend),
            // puis une annonce par tic, dans l'ordre.
            c.voix = None;
            c.tick();
            assert!(c.validation_a_dire.is_none());
            assert_eq!(c.annonces.len(), 2);
            c.tick();
            assert_eq!(c.annonces.len(), 1);
            c.tick();
            assert!(c.annonces.is_empty());
            // Interrompue : tout est abandonné.
            c.voix = Some((8, crate::voix::Reponse::new(&c.app)));
            c.recevoir(Message::Rappel(crate::rappels::RappelUi {
                id: "r2".into(),
                texte: "Relancer Marie".into(),
                echeance_ms: None,
            }));
            c.interrompre();
            assert!(c.annonces.is_empty());
        });
    }

    /// KOB-375 : la carte demandée par l'outil d'un tour détaché ne coupe
    /// pas la réponse d'une autre question : « Je le fais ? » attend la fin
    /// de cette réponse ; tranchée avant, la carte n'est plus annoncée.
    #[test]
    fn carte_d_un_tour_detache_sans_couper_la_reponse() {
        let (app, _dir) = chef_en_essai();
        crate::voix::installer_factice(app.handle());
        avec_chef(&app, |c| {
            let mut r = crate::voix::Reponse::new(&c.app);
            r.morceau(&c.app, "La réunion de demain commence à dix heures. Puis ");
            c.voix = Some((7, r));
            // Le tour 6 est parti en tâche ; son outil demande la carte.
            c.validation_ouverte("v1".into(), "Créer budget.md");
            c.tick();
            assert_eq!(
                c.validation_a_dire.as_ref().map(|v| v.0.as_str()),
                Some("v1")
            );
            assert!(!crate::voix::est_ma_voix(&c.app, "Je le fais ?"));
            let texte = "La réunion de demain commence à dix heures. Puis la revue.";
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: 7,
                texte: texte.into(),
            }));
            let limite = Instant::now() + Duration::from_secs(5);
            while c.validation_a_dire.is_some() {
                assert!(Instant::now() < limite, "« Je le fais ? » jamais dit");
                std::thread::sleep(Duration::from_millis(20));
                c.tick();
            }
            assert!(crate::voix::est_ma_voix(&c.app, "Je le fais ?"));
            c.validation_fermee("v1", crate::validation::Reponse::Accepte);
            while crate::voix::occupee(&c.app) {
                assert!(Instant::now() < limite, "voix restée occupée");
                std::thread::sleep(Duration::from_millis(20));
            }
            // Tranchée au clic pendant la réponse : rien n'est dit après.
            c.voix = Some((9, crate::voix::Reponse::new(&c.app)));
            c.validation_ouverte("v2".into(), "Créer budget.md");
            c.validation_fermee("v2", crate::validation::Reponse::Accepte);
            c.voix = None;
            c.tick();
            assert!(c.validation_a_dire.is_none());
            assert!(!crate::voix::occupee(&c.app));
            // Kobi coupé par l'utilisateur : rien n'est dit juste après.
            c.voix = Some((10, crate::voix::Reponse::new(&c.app)));
            c.validation_ouverte("v3".into(), "Créer budget.md");
            c.interrompre();
            c.tick();
            assert!(c.validation_a_dire.is_none());
            assert!(!crate::voix::occupee(&c.app));
            c.validation_fermee("v3", crate::validation::Reponse::Refuse);
        });
    }

    /// KOB-388 : une carte MCP de la conversation s'annonce comme une carte
    /// de Kobi : « Je le fais ? » une fois la voix libre ; pas si elle est
    /// tranchée avant ou si l'utilisateur a coupé la voix.
    #[test]
    fn carte_mcp_de_la_conversation_annoncee() {
        let (app, _dir) = chef_en_essai();
        crate::voix::installer_factice(app.handle());
        let carte = |c: &mut Chef<tauri::test::MockRuntime>, n: u64| {
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 3,
                carte: n,
                outil: "mcp__carnet__ecrire_note".into(),
                entree: serde_json::json!({ "texte": "relire le budget" }),
            }));
        };
        let attendre = |c: &mut Chef<tauri::test::MockRuntime>| {
            let limite = Instant::now() + Duration::from_secs(5);
            while crate::voix::occupee(&c.app) {
                assert!(Instant::now() < limite, "voix restée occupée");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        avec_chef(&app, |c| {
            carte(c, 1);
            assert!(c.carte_a_dire.is_none(), "dit tout de suite");
            assert!(crate::voix::est_ma_voix(&c.app, "Je le fais ?"));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 1,
                accepte: Some(true),
            }));
            attendre(c);
            // Pendant la réponse d'un autre tour : dite après, sans la couper.
            c.voix = Some((7, crate::voix::Reponse::new(&c.app)));
            carte(c, 2);
            c.tick();
            assert_eq!(c.carte_a_dire.map(|v| v.0), Some(2));
            c.voix = None;
            c.tick();
            assert!(c.carte_a_dire.is_none());
            assert!(crate::voix::occupee(&c.app));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 2,
                accepte: Some(false),
            }));
            attendre(c);
            // Tranchée avant que la voix soit libre : rien n'est dit.
            c.voix = Some((8, crate::voix::Reponse::new(&c.app)));
            carte(c, 3);
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 3,
                accepte: Some(true),
            }));
            c.voix = None;
            c.tick();
            assert!(c.carte_a_dire.is_none());
            assert!(!crate::voix::occupee(&c.app));
            // Voix coupée par l'utilisateur : rien n'est dit juste après.
            c.voix = Some((9, crate::voix::Reponse::new(&c.app)));
            carte(c, 4);
            c.interrompre();
            c.tick();
            assert!(c.carte_a_dire.is_none());
            assert!(!crate::voix::occupee(&c.app));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 4,
                accepte: Some(false),
            }));
            attendre(c);
            // Une carte de Kobi attend : « oui » irait à elle, la carte MCP
            // n'est annoncée qu'après sa fermeture.
            c.validation_ouverte("v1".into(), "Créer budget.md");
            attendre(c);
            carte(c, 5);
            c.tick();
            assert!(
                !crate::voix::occupee(&c.app),
                "rien par-dessus la carte de Kobi"
            );
            assert!(c.carte_a_dire.is_none());
            c.validation_fermee("v1", crate::validation::Reponse::Accepte);
            assert!(
                crate::voix::occupee(&c.app),
                "carte MCP annoncée à son tour"
            );
            attendre(c);
            // Deux cartes MCP : la plus récente est annoncée, l'autre à sa
            // fermeture.
            carte(c, 6);
            attendre(c);
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 6,
                accepte: Some(true),
            }));
            assert!(
                crate::voix::occupee(&c.app),
                "la carte 5 annoncée de nouveau"
            );
            attendre(c);
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 5,
                accepte: Some(true),
            }));
            assert!(!crate::voix::occupee(&c.app));
            // En réunion : une carte des notes plus récente est affichée, et
            // « oui » irait à elle : la carte de la conversation ne s'annonce
            // pas, la carte des notes reste muette.
            c.commande(Intention::DemarrerReunion);
            attendre(c);
            c.voix = Some((10, crate::voix::Reponse::new(&c.app)));
            carte(c, 7);
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 0,
                carte: 8,
                outil: "mcp__carnet__ecrire_note".into(),
                entree: serde_json::json!({ "texte": "proposée par les notes" }),
            }));
            c.voix = None;
            c.tick();
            assert!(c.carte_a_dire.is_none());
            assert!(!crate::voix::occupee(&c.app));
            // La carte des notes tranchée : celle de la conversation, de
            // nouveau affichée, s'annonce.
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 8,
                accepte: Some(false),
            }));
            c.tick();
            assert!(crate::voix::occupee(&c.app));
        });
    }

    #[test]
    fn resume_de_la_carte_mcp() {
        let ui = carte_ui(
            1,
            "mcp__carnet__ecrire_note",
            &serde_json::json!({ "title": "Relire le budget" }),
        );
        assert_eq!(resume_carte(&ui), "Carnet, ecrire note : Relire le budget");
        let ui = carte_ui(
            2,
            "mcp__carnet__ecrire_note",
            &serde_json::json!({ "texte": "x" }),
        );
        assert_eq!(resume_carte(&ui), "Carnet, ecrire note");
    }

    /// KOB-385 : « Je m'en occupe » n'est pas dit pendant qu'une carte
    /// attend la décision ; dit si la tâche tarde encore après l'acceptation,
    /// oublié si son résultat arrive avant.
    #[test]
    fn patience_remise_pendant_la_carte() {
        let (app, _dir) = chef_en_essai();
        crate::voix::installer_factice(app.handle());
        let patience = crate::pannes::patience();
        let attendre = |c: &mut Chef<tauri::test::MockRuntime>| {
            let limite = Instant::now() + Duration::from_secs(5);
            while crate::voix::occupee(&c.app) {
                assert!(Instant::now() < limite, "voix restée occupée");
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        avec_chef(&app, |c| {
            c.question("ajoute un rappel pour relire le budget".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            attendre(c);
            c.recevoir(Message::Cerveau(Retour::Patience { tour }));
            assert_eq!(c.patience_reportee, Some((tour, None)));
            assert!(!crate::voix::occupee(&c.app), "rien pendant la carte");
            assert!(!crate::voix::est_ma_voix(&c.app, &patience));
            // Acceptée : la tâche tourne encore, rien tout de suite.
            c.validation_fermee("v1", crate::validation::Reponse::Accepte);
            c.tick();
            assert!(matches!(c.patience_reportee, Some((t, Some(_))) if t == tour));
            assert!(!crate::voix::est_ma_voix(&c.app, &patience));
            // Toujours rien `PATIENCE` après : dit cette fois.
            c.patience_reportee = Some((tour, Some(Instant::now() - cerveau::PATIENCE)));
            c.tick();
            assert!(c.patience_reportee.is_none());
            assert!(crate::voix::est_ma_voix(&c.app, &patience));
            attendre(c);
            // Résultat arrivé avant : oubliée.
            c.question("ajoute un rappel pour appeler Paul".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            c.validation_ouverte("v2".into(), "Ajoute un rappel");
            c.recevoir(Message::Cerveau(Retour::Patience { tour }));
            c.validation_fermee("v2", crate::validation::Reponse::Accepte);
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour,
                texte: "Rappel ajouté.".into(),
            }));
            c.tick();
            assert!(c.patience_reportee.is_none());
            // Carte MCP de la conversation (tour détaché sans annonce) :
            // remise, le délai repart tant qu'une carte attend, puis dite.
            c.question("écris dans le carnet".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour,
                carte: 1,
                outil: "mcp__carnet__ecrire_note".into(),
                entree: serde_json::json!({ "texte": "relire" }),
            }));
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: false,
            }));
            attendre(c);
            c.recevoir(Message::Cerveau(Retour::Patience { tour }));
            c.tick();
            assert_eq!(c.patience_reportee, Some((tour, None)));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 1,
                accepte: Some(true),
            }));
            c.tick();
            assert!(matches!(c.patience_reportee, Some((t, Some(_))) if t == tour));
            c.validation_ouverte("v3".into(), "Ajoute un rappel");
            c.tick();
            assert_eq!(c.patience_reportee, Some((tour, None)), "le délai repart");
            c.validation_fermee("v3", crate::validation::Reponse::Accepte);
            attendre(c);
            c.patience_reportee = Some((tour, Some(Instant::now() - cerveau::PATIENCE)));
            c.tick();
            assert!(c.patience_reportee.is_none());
            assert!(crate::voix::est_ma_voix(&c.app, &patience));
            attendre(c);
            // Carte refusée : la patience remise est oubliée.
            c.question("ajoute un rappel pour le dentiste".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            c.validation_ouverte("v4".into(), "Ajoute un rappel");
            c.recevoir(Message::Cerveau(Retour::Patience { tour }));
            assert!(c.patience_reportee.is_some());
            c.validation_fermee("v4", crate::validation::Reponse::Refuse);
            assert!(c.patience_reportee.is_none());
        });
    }

    /// KOB-392 : une erreur pendant une réponse en cours, puis la fin de la
    /// réponse : l'Oups garde son acquittement et Kobi revient au calme.
    #[test]
    fn oups_pendant_une_reponse_acquitte() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.question("résume le projet Hermès".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour,
                texte: "Le projet avance. ".into(),
            }));
            assert_eq!(c.activite(), Some("repond"));
            c.erreur("Notes", "Les notes n'ont pas pu être écrites.");
            assert_eq!(c.activite(), Some("oups"));
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "Le projet avance. La revue est vendredi.".into(),
            }));
            assert!(
                matches!(c.calme_a, Some((_, Evenement::Acquitter))),
                "l'acquittement de l'Oups reste armé"
            );
            // Délai de l'Oups passé : retour au calme.
            c.calme_a = Some((Instant::now(), Evenement::Acquitter));
            c.tick();
            assert_eq!(c.activite(), Some("calme"));
            // Oups du réseau déjà acquitté par la réponse (KOB-386) : sa fin
            // arme la lecture, pas l'ancien acquittement, et Kobi revient au
            // calme.
            c.recevoir(Message::Interpelle);
            c.question("et la revue ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            assert_eq!(c.activite(), Some("oups"));
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour,
                texte: "Vendredi. ".into(),
            }));
            assert_eq!(c.activite(), Some("repond"));
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "Vendredi.".into(),
            }));
            assert!(matches!(c.calme_a, Some((_, Evenement::ReponseTerminee))));
            c.calme_a = Some((Instant::now(), Evenement::ReponseTerminee));
            c.tick();
            assert_eq!(c.activite(), Some("calme"));
        });
    }

    /// KOB-394 : un Oups d'une autre erreur que le réseau s'efface au bout de
    /// son délai même si une carte s'ouvre ou un tour se détache pendant
    /// qu'il est affiché ; la carte ou la tâche reprend ensuite la main.
    #[test]
    fn oups_d_une_autre_erreur_toujours_acquitte() {
        // Délai de l'Oups passé (armé au tic au besoin) : son acquittement
        // tombe au tic suivant.
        let echu = |c: &mut Chef<tauri::test::MockRuntime>| {
            c.tick();
            assert_eq!(c.activite(), Some("oups"), "l'Oups reste son délai");
            if let Some((_, e)) = c.calme_a {
                c.calme_a = Some((Instant::now(), e));
            }
            c.tick();
        };
        // Carte de Kobi ouverte pendant l'Oups.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.question("résume le projet".into(), true);
            c.erreur("Disque", "plein");
            assert_eq!(c.activite(), Some("oups"));
            c.validation_ouverte("v1".into(), "Écrire la note");
            c.tick();
            assert_eq!(c.activite(), Some("oups"), "l'Oups reste son délai");
            assert!(
                matches!(c.calme_a, Some((_, Evenement::Acquitter))),
                "l'acquittement de l'Oups reste armé"
            );
            echu(c);
            c.tick();
            assert_eq!(c.activite(), Some("propose"), "la carte reprend la main");
        });
        // Carte MCP de la conversation ouverte pendant l'Oups.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.question("crée le ticket".into(), true);
            let tour = c.tour;
            c.erreur("Disque", "plein");
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour,
                carte: 1,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({ "title": "Corriger la connexion" }),
            }));
            assert_eq!(c.activite(), Some("oups"));
            echu(c);
            c.tick();
            assert_eq!(c.activite(), Some("propose"));
        });
        // Tour détaché pendant l'Oups.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.question("résume le projet".into(), true);
            let tour = c.tour;
            c.erreur("Disque", "plein");
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            assert_eq!(c.activite(), Some("oups"));
            echu(c);
            c.tick();
            assert_eq!(c.activite(), Some("occupe"), "la tâche reprend la main");
        });
    }

    /// KOB-415 (KOB-395 E) : le tour a commencé à répondre (« répond »), puis
    /// se détache sans annonce (permission refusée sans carte) ; son
    /// résultat arrive à part : il est livré, et Kobi revient au calme.
    #[test]
    fn resultat_d_un_tour_detache_apres_un_debut_de_reponse() {
        for reunion in [false, true] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                if reunion {
                    c.commande(Intention::DemarrerReunion);
                }
                c.recevoir(Message::Interpelle);
                c.question("résume le projet".into(), true);
                let tour = c.tour;
                c.recevoir(Message::Cerveau(Retour::Texte {
                    tour,
                    texte: "Je regarde. ".into(),
                }));
                assert_eq!(c.activite(), Some("repond"), "réunion : {reunion}");
                c.recevoir(Message::Cerveau(Retour::Detache {
                    tour,
                    annonce: false,
                }));
                c.recevoir(Message::Cerveau(Retour::Resultat {
                    tour,
                    texte: "Le projet avance.".into(),
                }));
                for _ in 0..5 {
                    if let Some((_, e)) = c.calme_a {
                        c.calme_a = Some((Instant::now(), e));
                    }
                    c.livrable_a = None;
                    c.tick();
                }
                assert!(
                    c.a_livrer.is_empty(),
                    "résultat livré (réunion : {reunion})"
                );
                if reunion {
                    assert_eq!(etat(&c.app).etat, "reunion");
                }
                assert!(
                    matches!(c.activite(), Some("calme") | None),
                    "{:?} (réunion : {reunion})",
                    c.activite()
                );
            });
        }
    }

    /// KOB-416 (KOB-395 B) : une question attend le réseau (Oups, voix
    /// reportée), puis sa réponse arrive d'un coup (`Fin` sans `Texte`) :
    /// Kobi revient au calme au lieu de rester figé en « répond ».
    #[test]
    fn reponse_d_un_coup_apres_l_attente_du_reseau() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.question("quelle est la prochaine étape ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            assert_eq!(c.activite(), Some("oups"));
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "La revue est vendredi.".into(),
            }));
            assert!(
                matches!(c.calme_a, Some((_, Evenement::ReponseTerminee))),
                "la lecture est armée, pas l'ancien acquittement"
            );
            for _ in 0..3 {
                if let Some((_, e)) = c.calme_a {
                    c.calme_a = Some((Instant::now(), e));
                }
                c.tick();
            }
            assert!(
                matches!(c.activite(), Some("calme") | None),
                "{:?}",
                c.activite()
            );
        });
    }

    /// KOB-417 (KOB-395 D) : la tâche d'une question plus ancienne finit en
    /// erreur pendant que Kobi est « au travail » pour elle : il revient au
    /// calme au lieu d'y rester.
    #[test]
    fn tache_d_une_ancienne_question_en_erreur() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.question("résume le projet".into(), true);
            let tache = c.tour;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour: tache,
                annonce: true,
            }));
            // Une autre question, répondue : la tâche n'est plus la dernière.
            c.recevoir(Message::Interpelle);
            c.question("et la revue ?".into(), true);
            let autre = c.tour;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: autre,
                texte: "Vendredi.".into(),
            }));
            let avancer = |c: &mut Chef<tauri::test::MockRuntime>| {
                if let Some((_, e)) = c.calme_a {
                    c.calme_a = Some((Instant::now(), e));
                }
                c.tick();
            };
            avancer(c);
            avancer(c);
            assert_eq!(c.activite(), Some("occupe"), "la tâche tourne encore");
            c.recevoir(Message::Cerveau(Retour::Erreur {
                tour: tache,
                message: "erreur d'essai".into(),
            }));
            for _ in 0..3 {
                avancer(c);
            }
            assert!(
                matches!(c.activite(), Some("calme") | None),
                "{:?}",
                c.activite()
            );
        });
    }

    /// KOB-418 (KOB-395 A) : la question en cours tourne encore (attente du
    /// réseau, Oups acquitté) et deux cartes attendent : Kobi ne paraît pas
    /// au repos, il est « au travail ».
    #[test]
    fn question_en_cours_jamais_au_repos() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.validation_ouverte("v1".into(), "Écrire la note");
            c.validation_ouverte("v2".into(), "Écrire une autre note");
            c.question("et la revue ?".into(), false);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            for _ in 0..5 {
                if let Some((_, e)) = c.calme_a {
                    c.calme_a = Some((Instant::now(), e));
                }
                c.tick();
            }
            assert!(c.question_en_cours());
            assert!(
                !matches!(c.activite(), Some("calme") | None),
                "{:?} alors que la question tourne et que deux cartes attendent",
                c.activite()
            );
            // La réponse arrive : « répond », puis la carte est reproposée.
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "Vendredi.".into(),
            }));
            assert_eq!(c.activite(), Some("repond"));
            for _ in 0..3 {
                if let Some((_, e)) = c.calme_a {
                    c.calme_a = Some((Instant::now(), e));
                }
                c.tick();
            }
            assert_eq!(c.activite(), Some("propose"), "les cartes attendent");
        });
    }

    /// KOB-422 : le résultat d'une ancienne tâche arrive pendant qu'une
    /// question attend le réseau. Il passe après sa réponse, qui est montrée
    /// et lue ; avant, il prenait sa place de dernière question et la réponse
    /// n'apparaissait que dans l'historique.
    #[test]
    fn resultat_d_une_tache_pendant_l_attente_du_reseau() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.question("résume le projet".into(), true);
            let tache = c.tour;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour: tache,
                annonce: true,
            }));
            c.recevoir(Message::Interpelle);
            c.question("et la revue ?".into(), true);
            let autre = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente {
                tour: autre,
                nombre: 1,
            }));
            let avancer = |c: &mut Chef<tauri::test::MockRuntime>| {
                if let Some((_, e)) = c.calme_a {
                    c.calme_a = Some((Instant::now(), e));
                }
                c.livrable_a = None;
                c.tick();
            };
            for _ in 0..3 {
                avancer(c);
            }
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: tache,
                texte: "Le projet avance.".into(),
            }));
            for _ in 0..5 {
                avancer(c);
            }
            assert_eq!(c.dernier, autre, "la question qui attend reste la dernière");
            assert!(c.question_en_cours());
            assert_eq!(c.a_livrer.len(), 1, "le résultat attend la réponse");
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: autre,
                texte: "Vendredi.".into(),
            }));
            assert!(
                matches!(c.calme_a, Some((_, Evenement::ReponseTerminee))),
                "la réponse est montrée et lue : {:?}",
                c.activite()
            );
            for _ in 0..8 {
                avancer(c);
            }
            assert!(c.a_livrer.is_empty(), "le résultat est livré ensuite");
            assert_eq!(c.dernier, tache);
            assert!(
                matches!(c.activite(), Some("calme") | None),
                "{:?}",
                c.activite()
            );
        });
    }

    /// KOB-422, trouvé aussi par le test de propriété (graine 1) : la
    /// réunion démarre pendant qu'une question attend sa réponse et ramène
    /// Kobi au calme ; le résultat d'une ancienne tâche arrive. Il passe
    /// après la réponse, qui est montrée et lue.
    #[test]
    fn resultat_d_une_tache_apres_le_debut_d_une_reunion() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.question("résume le projet".into(), true);
            let tache = c.tour;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour: tache,
                annonce: true,
            }));
            c.recevoir(Message::Interpelle);
            c.question("non".into(), true);
            c.recevoir(Message::Interpelle);
            c.question("oui".into(), true);
            let autre = c.tour;
            c.commande(Intention::DemarrerReunion);
            let avancer = |c: &mut Chef<tauri::test::MockRuntime>| {
                if let Some((_, e)) = c.calme_a {
                    c.calme_a = Some((Instant::now(), e));
                }
                c.livrable_a = None;
                c.tick();
            };
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: tache,
                texte: "Le projet avance.".into(),
            }));
            for _ in 0..5 {
                avancer(c);
            }
            assert_eq!(c.dernier, autre, "la question qui attend reste la dernière");
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: autre,
                texte: "D'accord.".into(),
            }));
            assert!(
                matches!(c.calme_a, Some((_, Evenement::ReponseTerminee))),
                "la réponse est montrée et lue : {:?}",
                c.activite()
            );
            for _ in 0..8 {
                avancer(c);
            }
            assert!(c.a_livrer.is_empty(), "le résultat est livré ensuite");
            assert_eq!(c.dernier, tache);
            assert_eq!(etat(&c.app).etat, "reunion");
        });
    }

    /// KOB-419 (KOB-395 C) : en réunion, deux cartes des notes arrivent
    /// pendant un Oups ; « Kobi, c'est ok » accepte l'une, Kobi repropose
    /// l'autre depuis le calme. À sa fermeture, il revient au calme au lieu
    /// de rester figé en « propose ».
    #[test]
    fn carte_des_notes_reproposee_ramene_au_calme() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.erreur("Disque", "plein");
            assert_eq!(c.activite(), Some("oups"));
            let notes = |carte| {
                Message::Cerveau(Retour::Carte {
                    tour: 0,
                    carte,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Corriger la connexion"}),
                })
            };
            c.recevoir(notes(1));
            std::thread::sleep(Duration::from_millis(2));
            c.recevoir(notes(2));
            c.recevoir(Message::Interpelle);
            c.recevoir(Message::Question {
                texte: "c'est ok".into(),
                voix: true,
                dit_a: None,
            });
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 2,
                accepte: Some(true),
            }));
            assert_eq!(c.activite(), Some("propose"), "la carte 1 est reproposée");
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 1,
                accepte: None,
            }));
            assert_eq!(c.activite(), Some("calme"));
        });
        // Trois cartes : la reproposée se ferme avant la dernière (relecture
        // de kobi-a), l'ordre de fermeture ne compte pas.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.erreur("Disque", "plein");
            let notes = |carte| {
                Message::Cerveau(Retour::Carte {
                    tour: 0,
                    carte,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Corriger la connexion"}),
                })
            };
            for carte in [1, 2, 3] {
                c.recevoir(notes(carte));
                std::thread::sleep(Duration::from_millis(2));
            }
            c.recevoir(Message::Interpelle);
            c.recevoir(Message::Question {
                texte: "c'est ok".into(),
                voix: true,
                dit_a: None,
            });
            for (carte, accepte) in [(3, Some(true)), (2, None), (1, None)] {
                c.recevoir(Message::Cerveau(Retour::CarteFermee { carte, accepte }));
            }
            assert_eq!(c.activite(), Some("calme"));
        });
    }

    /// KOB-377 : une carte ouverte pendant qu'une question attend le réseau
    /// est annoncée (la voix est libre) ; la réponse, arrivée pendant
    /// « Je le fais ? », passe après, sur un énoncé neuf, sans le couper.
    #[test]
    fn carte_annoncee_pendant_l_attente_du_reseau() {
        let (app, _dir) = chef_en_essai();
        crate::voix::installer_factice(app.handle());
        avec_chef(&app, |c| {
            c.question("résume le projet Hermès".into(), true);
            let tour = c.tour;
            assert_eq!(c.voix.as_ref().map(|v| v.0), Some(tour));
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            assert!(c.voix.is_none(), "la question en attente lâche son énoncé");
            assert_eq!(c.voix_reportee.map(|v| v.0), Some(tour));
            c.validation_ouverte("v1".into(), "Créer budget.md");
            assert!(c.validation_a_dire.is_none(), "dit tout de suite");
            assert!(crate::voix::est_ma_voix(&c.app, "Je le fais ?"));
            // Le réseau revient pendant « Je le fais ? » : la réponse attend
            // (chaque son factice dure 50 ms : vérifié s'il joue encore).
            let occupee = crate::voix::occupee(&c.app);
            let debut = "Le projet Hermès avance bien. Puis ";
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour,
                texte: debut.into(),
            }));
            if occupee {
                assert!(c.voix.is_none());
                assert_eq!(c.voix_reportee.map(|v| v.0), Some(tour));
            }
            let limite = Instant::now() + Duration::from_secs(5);
            while c.voix.is_none() {
                assert!(Instant::now() < limite, "réponse jamais reprise");
                std::thread::sleep(Duration::from_millis(20));
                c.tick();
            }
            assert!(c.voix_reportee.is_none());
            let texte = "Le projet Hermès avance bien. Puis la revue est vendredi.";
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: texte.into(),
            }));
            assert!(crate::voix::est_ma_voix(
                &c.app,
                "Le projet Hermès avance bien."
            ));
            assert!(crate::voix::est_ma_voix(&c.app, "la revue est vendredi"));
            c.validation_fermee("v1", crate::validation::Reponse::Accepte);
            while crate::voix::occupee(&c.app) {
                assert!(Instant::now() < limite, "voix restée occupée");
                std::thread::sleep(Duration::from_millis(20));
            }
            // Réponse arrivée entière pendant une annonce : dite après.
            c.question("et le budget ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            c.validation_ouverte("v2".into(), "Créer budget.md");
            let occupee = crate::voix::occupee(&c.app);
            let texte = "Le budget tient en une page.";
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: texte.into(),
            }));
            if occupee {
                assert_eq!(c.voix_reportee.map(|v| v.0), Some(tour));
                assert!(c.calme_a.is_none(), "la lecture attend la réponse dite");
            }
            let limite = Instant::now() + Duration::from_secs(5);
            while c.voix_reportee.is_some() {
                assert!(Instant::now() < limite, "réponse jamais dite");
                std::thread::sleep(Duration::from_millis(20));
                c.tick();
            }
            assert!(crate::voix::est_ma_voix(&c.app, texte));
            assert!(c.calme_a.is_some());
            c.validation_fermee("v2", crate::validation::Reponse::Refuse);
            while crate::voix::occupee(&c.app) {
                assert!(Instant::now() < limite, "voix restée occupée");
                std::thread::sleep(Duration::from_millis(20));
            }
            // L'utilisateur coupe la voix pendant l'attente (« Kobi… ») : la
            // réponse arrivée ensuite reste à l'écran, sans voix.
            c.question("et le planning de mars ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            crate::voix::taire(&c.app, "essai");
            let texte = "Le planning de mars est prêt.";
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: texte.into(),
            }));
            c.tick();
            assert!(c.voix_reportee.is_none());
            assert!(!crate::voix::occupee(&c.app));
            assert!(!crate::voix::est_ma_voix(&c.app, texte));
            assert!(c.calme_a.is_some());
            // Réponse arrivée pendant « Je le fais ? », puis l'utilisateur
            // parle par-dessus : rien n'est dit, mais Kobi revient au calme.
            c.question("et le planning d'avril ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            c.validation_ouverte("v3".into(), "Créer budget.md");
            let texte = "Le planning d'avril est prêt.";
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: texte.into(),
            }));
            if c.voix_reportee.is_some() {
                assert!(c.calme_a.is_none());
                crate::voix::taire(&c.app, "essai");
                c.tick();
                assert!(c.voix_reportee.is_none());
                assert!(!crate::voix::est_ma_voix(&c.app, texte));
            }
            assert!(c.calme_a.is_some(), "Kobi ne reste pas en « répond »");
            c.validation_fermee("v3", crate::validation::Reponse::Refuse);
            // Erreur après l'attente (réseau jamais revenu) : rien à reprendre.
            c.question("et le planning ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            c.recevoir(Message::Cerveau(Retour::Erreur {
                tour,
                message: "toujours pas de réseau".into(),
            }));
            assert!(c.voix_reportee.is_none());
        });
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
        // Le fil du chef tourne aussi (`boucle`, un tic toutes les 250 ms) :
        // entre deux `avec_chef`, il a pu clore la fin lui-même et la réunion
        // n'est plus là. Ce qui compte, « Annuler » encore possible ou non,
        // se lit pareil (KOB-408).
        let expire = |c: &mut Essai, depuis: Duration| {
            if let Some(r) = c.reunion.as_mut() {
                r.annulable_jusqu = Some(Instant::now() - depuis);
            }
            c.tick();
            c.fin_annulable()
        };
        for rattrape in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                c.commande(Intention::DemarrerReunion);
                c.recevoir(Message::Avancement(
                    kobi_stt::Source::Moi,
                    Some(Instant::now() - Duration::from_secs(15)),
                ));
                c.commande(Intention::TerminerReunion);
            });
            assert!(avec_chef(&app, |c| expire(c, Duration::from_millis(1))));
            if rattrape {
                // Le micro rattrape le délai : la fin est définitive.
                avec_chef(&app, |c| {
                    c.recevoir(Message::Avancement(
                        kobi_stt::Source::Moi,
                        Some(Instant::now()),
                    ))
                });
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

    /// KOB-364 : un énoncé commencé avant l'arrêt au clic et fini après
    /// entre au compte rendu, en entier ; un énoncé commencé après l'arrêt
    /// n'y entre pas.
    #[test]
    fn enonce_a_cheval_sur_l_arret_garde() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            // Pistes en retard : le compte rendu attend leur transcription.
            let avant = Instant::now() - Duration::from_secs(30);
            c.recevoir(Message::Avancement(kobi_stt::Source::Moi, Some(avant)));
            c.commande(Intention::TerminerReunion);
            let arret = c.reunion.as_ref().unwrap().arret.unwrap();
            let a_cheval = Segment {
                start: 0.0,
                end: 12.0,
                source: kobi_stt::Source::Moi,
                text: "Point un validé, point deux reporté.".into(),
                is_final: true,
            };
            c.recevoir(Message::Segment(a_cheval, arret + Duration::from_secs(1)));
            c.recevoir(Message::Segment(
                dit("Après l'arrêt."),
                arret + Duration::from_secs(3),
            ));
            c.recevoir(Message::Avancement(kobi_stt::Source::Moi, None));
            c.recevoir(Message::Avancement(kobi_stt::Source::Eux, None));
            c.tick();
            let r = c.reunion.as_ref().unwrap();
            assert!(r.compte_rendu_demande);
            assert!(
                r.pour_compte_rendu
                    .contains("Point un validé, point deux reporté."),
                "{}",
                r.pour_compte_rendu
            );
            assert!(
                !r.pour_compte_rendu.contains("Après l'arrêt."),
                "{}",
                r.pour_compte_rendu
            );
        });
    }

    /// KOB-364 : sous charge, l'énoncé à cheval sur l'arrêt arrive après
    /// « Annuler » : il reste à la réunion reprise, comme s'il était arrivé
    /// avant. La règle, partagée avec la réunion close : un segment fini
    /// après l'arrêt en est s'il a commencé moins de [`MARGE_FIN`] avant.
    #[test]
    fn enonce_a_cheval_arrive_apres_annuler() {
        let a_cheval = |arret: Instant, dit_a: Instant| Segment {
            start: 0.0,
            end: (dit_a - arret).as_secs_f64() + 20.0,
            source: kobi_stt::Source::Moi,
            text: "Vingt secondes de décisions.".into(),
            is_final: true,
        };
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.commande(Intention::TerminerReunion);
            let arret = c.reunion.as_ref().unwrap().arret.unwrap();
            std::thread::sleep(Duration::from_millis(20));
            c.commande(Intention::Reprendre);
            let dit_a = arret + Duration::from_millis(10);
            c.recevoir(Message::Segment(a_cheval(arret, dit_a), dit_a));
            let r = c.reunion.as_ref().unwrap();
            assert!(
                r.a_noter.contains("Vingt secondes de décisions."),
                "{}",
                r.a_noter
            );
        });
        let arret = Instant::now();
        let s = |x: u64| Duration::from_secs(x);
        assert!(!dit_apres_l_arret(arret - s(20), arret + s(1), arret));
        assert!(!dit_apres_l_arret(arret - s(2), arret - s(1), arret));
        assert!(dit_apres_l_arret(arret - s(1), arret + s(1), arret));
        assert!(dit_apres_l_arret(arret + s(1), arret + s(2), arret));
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

    /// KOB-72 : en continu, le bloc part dès sa troisième phrase, avant
    /// l'heure, et demande des propositions ; en mode blocs, il attend
    /// l'heure.
    #[test]
    fn notes_en_continu_par_trois_phrases() {
        for continu in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                c.commande(Intention::DemarrerReunion);
                c.reunion.as_mut().unwrap().continu = continu;
                for texte in ["Budget validé.", "Lieu : Annecy."] {
                    c.recevoir(Message::Segment(dit(texte), Instant::now()));
                }
                c.tick();
                assert!(c.reunion.as_ref().unwrap().envoye.is_none());
                c.recevoir(Message::Segment(
                    dit("Il faut envoyer le devis."),
                    Instant::now(),
                ));
                c.tick();
                let r = c.reunion.as_ref().unwrap();
                assert_eq!(r.envoye.is_some(), continu, "continu : {continu}");
                if continu {
                    assert!(r.envoye.as_ref().unwrap().contains("devis"));
                    assert!(r.a_noter.is_empty());
                }
            });
        }
    }

    /// KOB-72 : une carte proposée par les notes (tour 0) met Kobi en
    /// proposition depuis le calme et l'y ramène une fois décidée ; tant
    /// qu'une autre carte attend, il reste en proposition.
    #[test]
    fn carte_des_notes_en_proposition_puis_au_calme() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            assert_eq!(c.activite(), Some("calme"));
            let carte = |carte| {
                Message::Cerveau(Retour::Carte {
                    tour: 0,
                    carte,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Envoyer le devis"}),
                })
            };
            let fermee = |carte, accepte| Message::Cerveau(Retour::CarteFermee { carte, accepte });
            c.recevoir(carte(7));
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(fermee(7, Some(true)));
            assert_eq!(c.activite(), Some("calme"));
            // KOB-331 : acceptée, sa trace va aux notes à venir.
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert!(
                notes.contains("action acceptée] Envoyer le devis"),
                "{notes}"
            );
            // Deux cartes : la première décidée, Kobi attend la seconde.
            c.recevoir(carte(8));
            c.recevoir(carte(9));
            c.recevoir(fermee(8, Some(false)));
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(fermee(9, Some(false)));
            assert_eq!(c.activite(), Some("calme"));
            assert!(c.cartes_des_notes.is_empty());
            // Refusées : pas de trace.
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert_eq!(notes.matches("action acceptée").count(), 1, "{notes}");
            // Jamais décidée : à l'échéance de kobi-perms, refusée et fermée.
            c.recevoir(carte(10));
            assert_eq!(c.activite(), Some("propose"));
            c.tick();
            assert_eq!(c.activite(), Some("propose"));
            *c.cartes_ouvertes.get_mut(&10).unwrap() =
                Instant::now() - kobi_perms::DEFAULT_TIMEOUT - Duration::from_secs(6);
            c.tick();
            assert_eq!(c.activite(), Some("calme"));
            assert!(c.cartes.is_empty() && c.cartes_ouvertes.is_empty());
        });
    }

    /// KOB-380 : une carte de Kobi et une carte MCP de la conversation
    /// attendent ensemble : la première tranchée ne fait pas quitter la
    /// proposition, quelle qu'elle soit ; la dernière décide.
    #[test]
    fn carte_de_kobi_et_carte_mcp_ensemble() {
        let carte = |carte| {
            Message::Cerveau(Retour::Carte {
                tour: 5,
                carte,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Corriger la connexion"}),
            })
        };
        let fermee = |id: &str, reponse| Message::ValidationFermee {
            id: id.into(),
            reponse,
        };
        // La carte de Kobi refusée d'abord : la carte MCP reste proposée.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            c.recevoir(carte(1));
            c.recevoir(fermee("v1", crate::validation::Reponse::Refuse));
            assert_eq!(c.activite(), Some("propose"), "la carte MCP attend");
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 1,
                accepte: Some(true),
            }));
            assert_eq!(c.activite(), Some("reflechit"));
        });
        // La carte MCP refusée d'abord : la carte de Kobi reste proposée.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            c.recevoir(carte(1));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 1,
                accepte: Some(false),
            }));
            assert_eq!(c.activite(), Some("propose"), "la carte de Kobi attend");
            c.recevoir(fermee("v1", crate::validation::Reponse::Refuse));
            assert_eq!(c.activite(), Some("calme"));
        });
    }

    /// KOB-380 : une carte des notes venue du calme et une carte de Kobi
    /// attendent ensemble : la première fermée ne fait pas quitter la
    /// proposition ; la carte de Kobi acceptée fait repartir le tour.
    #[test]
    fn carte_des_notes_et_carte_de_kobi_ensemble() {
        let notes = Message::Cerveau(Retour::Carte {
            tour: 0,
            carte: 3,
            outil: "mcp__linear__save_issue".into(),
            entree: serde_json::json!({"title": "Corriger la connexion"}),
        });
        let ouvrir = |c: &mut Chef<_>| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(notes.clone());
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            assert_eq!(c.activite(), Some("propose"));
        };
        let fermee = |reponse| Message::ValidationFermee {
            id: "v1".into(),
            reponse,
        };
        // La carte des notes fermée d'abord : la carte de Kobi reste
        // proposée, puis son « oui » fait repartir le tour.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            ouvrir(c);
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 3,
                accepte: Some(false),
            }));
            assert_eq!(c.activite(), Some("propose"), "la carte de Kobi attend");
            c.recevoir(fermee(crate::validation::Reponse::Accepte));
            assert_eq!(c.activite(), Some("reflechit"));
        });
        // La carte de Kobi refusée d'abord : la carte des notes reste
        // proposée, puis sa fermeture ramène au calme.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            ouvrir(c);
            c.recevoir(fermee(crate::validation::Reponse::Refuse));
            assert_eq!(c.activite(), Some("propose"), "la carte des notes attend");
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 3,
                accepte: Some(false),
            }));
            assert_eq!(c.activite(), Some("calme"));
        });
        // Une seconde carte des notes arrive pendant que la carte de Kobi
        // attend : la carte de Kobi refusée, puis la première des notes, la
        // dernière fermée ramène au calme.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            ouvrir(c);
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 0,
                carte: 4,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Relancer le client"}),
            }));
            c.recevoir(fermee(crate::validation::Reponse::Refuse));
            for carte in [3, 4] {
                assert_eq!(c.activite(), Some("propose"));
                c.recevoir(Message::Cerveau(Retour::CarteFermee {
                    carte,
                    accepte: Some(false),
                }));
            }
            assert_eq!(c.activite(), Some("calme"));
        });
    }

    /// KOB-387 : la question en cours demande une carte (son tour est
    /// détaché) ; acceptée, Kobi réfléchit, et la réponse du tour, livrée par
    /// `Resultat`, passe tout de suite au lieu d'attendre pour toujours, même
    /// derrière le résultat d'une question plus ancienne, qui suit ensuite.
    #[test]
    fn reponse_livree_apres_une_carte_acceptee() {
        for reunion in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                if reunion {
                    c.commande(Intention::DemarrerReunion);
                }
                c.recevoir(Message::Interpelle);
                c.envoyer("Crée le ticket".into(), None, true);
                let tour = c.dernier;
                c.recevoir(Message::Cerveau(Retour::Detache {
                    tour,
                    annonce: false,
                }));
                c.recevoir(Message::Cerveau(Retour::Carte {
                    tour,
                    carte: 1,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Corriger la connexion"}),
                }));
                c.recevoir(Message::Cerveau(Retour::CarteFermee {
                    carte: 1,
                    accepte: Some(true),
                }));
                assert_eq!(c.activite(), Some("reflechit"));
                // Le résultat d'une autre question, plus ancienne, attend.
                c.recevoir(Message::Cerveau(Retour::Resultat {
                    tour: tour + 100,
                    texte: "Autre chose.".into(),
                }));
                assert_eq!(c.a_livrer.len(), 1, "pas pendant la réflexion");
                assert_eq!(c.activite(), Some("reflechit"));
                // La sienne passe devant ; l'ancienne suit ensuite.
                c.recevoir(Message::Cerveau(Retour::Resultat {
                    tour,
                    texte: "C'est fait : le ticket est créé.".into(),
                }));
                assert_eq!(c.activite(), Some("repond"), "réunion : {reunion}");
                assert_eq!(c.a_livrer.len(), 1);
                assert_eq!(c.a_livrer[0].0, tour + 100);
            });
        }
    }

    /// KOB-381 : deux cartes de la question en cours, l'une acceptée, la
    /// dernière refusée : le tour continue avec l'action acceptée, Kobi
    /// réfléchit au lieu de retomber au calme. Mais pas si ce tour a fini
    /// entre-temps, ni sur une carte échue pendant l'écoute.
    #[test]
    fn une_carte_acceptee_puis_la_derniere_refusee() {
        let carte = |tour, carte| {
            Message::Cerveau(Retour::Carte {
                tour,
                carte,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Corriger la connexion"}),
            })
        };
        let fermee = |carte, accepte| Message::Cerveau(Retour::CarteFermee { carte, accepte });
        // La question 5 est en cours, ses deux cartes s'ouvrent.
        let ouvrir = |c: &mut Chef<_>| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.dernier = 5;
            c.ajouter(Bulle {
                tour: 5,
                question: "Crée les deux tickets".into(),
                reponse: String::new(),
                statut: "reponse",
                clavier: false,
                etapes: Vec::new(),
            });
            c.recevoir(carte(5, 1));
            std::thread::sleep(Duration::from_millis(2));
            c.recevoir(carte(5, 2));
        };
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            ouvrir(c);
            c.recevoir(fermee(2, Some(true)));
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(fermee(1, Some(false)));
            assert_eq!(c.activite(), Some("reflechit"), "la carte 2 a été acceptée");
        });
        // Tout refusé : retour au calme, comme avant.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            ouvrir(c);
            c.recevoir(fermee(2, Some(false)));
            c.recevoir(fermee(1, Some(false)));
            assert_eq!(c.activite(), Some("calme"));
        });
        // La carte acceptée était celle d'une question finie depuis : rien
        // ne continue, la dernière refusée ramène au calme.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            ouvrir(c);
            c.recevoir(fermee(2, Some(true)));
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: 5,
                texte: "Le second ticket est créé.".into(),
            }));
            c.recevoir(fermee(1, None));
            assert_eq!(c.activite(), Some("calme"), "pas figé en réflexion");
        });
        // Une carte échue pendant l'écoute : Kobi attend la question.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            ouvrir(c);
            c.recevoir(fermee(2, Some(true)));
            c.recevoir(Message::Interpelle);
            c.recevoir(fermee(1, None));
            assert_ne!(c.activite(), Some("reflechit"));
        });
        // Comme avec le vrai cerveau : la question part, son tour est détaché
        // par ses cartes, et sa réponse arrive par `Resultat` (KOB-387).
        for reunion in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                if reunion {
                    c.commande(Intention::DemarrerReunion);
                }
                c.recevoir(Message::Interpelle);
                c.envoyer("Crée les deux tickets".into(), None, true);
                let tour = c.dernier;
                c.recevoir(Message::Cerveau(Retour::Detache {
                    tour,
                    annonce: false,
                }));
                c.recevoir(carte(tour, 1));
                std::thread::sleep(Duration::from_millis(2));
                c.recevoir(carte(tour, 2));
                c.recevoir(fermee(2, Some(true)));
                c.recevoir(fermee(1, Some(false)));
                assert_eq!(c.activite(), Some("reflechit"), "réunion : {reunion}");
                c.recevoir(Message::Cerveau(Retour::Resultat {
                    tour,
                    texte: "J'ai créé le second ticket.".into(),
                }));
                assert!(c.a_livrer.is_empty());
                assert_eq!(c.activite(), Some("repond"));
            });
        }
    }

    /// KOB-384 : une carte de Kobi attend, l'utilisateur pose une autre
    /// question ; une fois sa réponse lue, Kobi repropose la carte au lieu de
    /// rester au calme ou « au travail ».
    #[test]
    fn carte_reproposee_apres_une_autre_question() {
        for reunion in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                if reunion {
                    c.commande(Intention::DemarrerReunion);
                }
                c.recevoir(Message::Interpelle);
                c.envoyer("ajoute un rappel".into(), None, true);
                let tour = c.dernier;
                c.recevoir(Message::Cerveau(Retour::Detache {
                    tour,
                    annonce: true,
                }));
                c.validation_ouverte("v1".into(), "Ajoute un rappel");
                assert_eq!(c.activite(), Some("propose"));
                // Une autre question, sa réponse, puis la fin de la lecture.
                c.recevoir(Message::Interpelle);
                c.envoyer("Quelle est la date de la démo ?".into(), None, true);
                let autre = c.dernier;
                c.recevoir(Message::Cerveau(Retour::Fin {
                    tour: autre,
                    texte: "La démo est jeudi.".into(),
                }));
                assert_eq!(c.activite(), Some("repond"));
                c.calme_a = c.calme_a.map(|(_, e)| (Instant::now(), e));
                c.tick();
                assert_eq!(c.activite(), Some("propose"), "réunion : {reunion}");
                // La carte acceptée : son tour, parti en tâche, livre son
                // résultat, qui passe « au travail » puis est dit.
                c.recevoir(Message::ValidationFermee {
                    id: "v1".into(),
                    reponse: crate::validation::Reponse::Accepte,
                });
                c.recevoir(Message::Cerveau(Retour::Resultat {
                    tour,
                    texte: "J'ai ajouté le rappel.".into(),
                }));
                for _ in 0..3 {
                    c.livrable_a = None;
                    c.tick();
                }
                assert!(c.a_livrer.is_empty(), "réunion : {reunion}");
                assert_eq!(c.activite(), Some("repond"));
            });
        }
        // La question posée pendant la carte part en tâche : Kobi reste « au
        // travail » et livre son résultat sans attendre la carte.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.envoyer("ajoute un rappel".into(), None, true);
            let tour = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            c.recevoir(Message::Interpelle);
            c.envoyer("Résume le projet".into(), None, true);
            let autre = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour: autre,
                annonce: true,
            }));
            c.tick();
            assert_eq!(c.activite(), Some("occupe"));
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: autre,
                texte: "Le projet avance.".into(),
            }));
            assert!(c.a_livrer.is_empty());
            assert_eq!(c.activite(), Some("repond"));
        });
        // Sans autre question : un faux appel (« Kobi » puis rien) ramène au
        // calme puis « au travail » ; la carte, demandée par la question en
        // cours partie en tâche, est reproposée.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.envoyer("ajoute un rappel".into(), None, true);
            let tour = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            c.recevoir(Message::Interpelle);
            c.recevoir(Message::SansQuestion);
            c.tick();
            assert_eq!(c.activite(), Some("propose"));
        });
        // Une carte MCP déjà refusée à l'échéance n'est pas reproposée.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 5,
                carte: 1,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Corriger la connexion"}),
            }));
            c.transition(&[Evenement::PermissionRefusee]);
            assert_eq!(c.activite(), Some("calme"));
            c.cartes_ouvertes
                .insert(1, Instant::now() - kobi_perms::DEFAULT_TIMEOUT);
            c.tick();
            assert_eq!(c.activite(), Some("calme"));
        });
        // Une carte des notes venue pendant une réponse n'y ramène pas.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.envoyer("Quelle est la date de la démo ?".into(), None, true);
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 0,
                carte: 3,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Corriger la connexion"}),
            }));
            let tour = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "La démo est jeudi.".into(),
            }));
            c.calme_a = c.calme_a.map(|(_, e)| (Instant::now(), e));
            c.tick();
            assert_eq!(c.activite(), Some("calme"));
        });
    }

    /// KOB-379 : « Kobi, oui » sur une carte quand une autre attend : Kobi
    /// revient en proposition, pas figé en écoute ; la dernière tranchée, il
    /// repart sur le tour.
    #[test]
    fn kobi_oui_quand_une_autre_carte_attend() {
        let (app, _dir) = chef_en_essai();
        let dite = |texte: &str| Message::Question {
            texte: texte.into(),
            voix: true,
            dit_a: None,
        };
        // Deux cartes de Kobi.
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            c.validation_ouverte("v2".into(), "Ajoute un autre rappel");
            c.recevoir(Message::Interpelle);
            assert_eq!(c.activite(), Some("ecoute"));
            c.recevoir(dite("oui"));
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(Message::ValidationFermee {
                id: "v1".into(),
                reponse: crate::validation::Reponse::Accepte,
            });
            assert_eq!(c.activite(), Some("propose"), "v2 attend");
            c.recevoir(Message::Interpelle);
            c.recevoir(dite("non"));
            assert_eq!(
                c.activite(),
                Some("ecoute"),
                "dernière carte : sa fermeture décide"
            );
            c.recevoir(Message::ValidationFermee {
                id: "v2".into(),
                reponse: crate::validation::Reponse::Refuse,
            });
            assert_eq!(c.activite(), None, "hors réunion : retour en veille");
        });
        // Deux cartes MCP de la conversation.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            let carte = |carte| {
                Message::Cerveau(Retour::Carte {
                    tour: 5,
                    carte,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Corriger la connexion"}),
                })
            };
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.recevoir(carte(1));
            std::thread::sleep(Duration::from_millis(2));
            c.recevoir(carte(2));
            c.recevoir(Message::Interpelle);
            c.recevoir(dite("oui"));
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 2,
                accepte: Some(true),
            }));
            assert_eq!(c.activite(), Some("propose"), "la carte 1 attend");
            assert_eq!(c.carte_affichee(), Some(1));
        });
    }

    /// KOB-336 : « Kobi, oui » sur une carte de Kobi (rappel) : Kobi ne reste
    /// pas figé en écoute, il repart sur le tour ; le « oui » est un échange
    /// fini dans la conversation.
    #[test]
    fn kobi_oui_sur_une_carte_de_kobi_ne_fige_pas() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.transition(&[Evenement::QuestionEnvoyee]);
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(Message::Interpelle);
            assert_eq!(c.activite(), Some("ecoute"));
            c.recevoir(Message::Question {
                texte: "oui".into(),
                voix: true,
                dit_a: None,
            });
            c.recevoir(Message::ValidationFermee {
                id: "v1".into(),
                reponse: crate::validation::Reponse::Accepte,
            });
            assert_eq!(c.activite(), Some("reflechit"));
            let dernier = c.historique.last().unwrap();
            assert_eq!((dernier.question.as_str(), dernier.statut), ("oui", "fini"));
        });
    }

    /// KOB-336 : une carte des notes le dit en tête ; « Kobi, oui » lui
    /// répond comme un clic (la plus récente d'abord), Kobi revient au calme ;
    /// une phrase ordinaire ne touche pas à la carte.
    #[test]
    fn carte_mcp_tranchee_a_la_voix() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let carte = |tour, carte| {
                Message::Cerveau(Retour::Carte {
                    tour,
                    carte,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Corriger la connexion"}),
                })
            };
            let dite = |texte: &str| Message::Question {
                texte: texte.into(),
                voix: true,
                dit_a: None,
            };
            c.recevoir(carte(0, 3));
            let kicker = &c.cartes[&3].kicker;
            assert!(kicker.starts_with("Depuis les notes · Linear"), "{kicker}");
            std::thread::sleep(Duration::from_millis(2));
            c.recevoir(carte(0, 4));
            assert_eq!(
                c.carte_affichee(),
                Some(4),
                "la plus récente, comme à l'écran"
            );
            // Une phrase qui n'est pas une réponse ne décide rien, ni « ok »
            // dit à autre chose, ni « stop » qui fait taire Kobi.
            let tour = c.tour;
            c.recevoir(Message::Interpelle);
            c.recevoir(dite("quelle heure est-il ?"));
            assert_eq!(c.tour, tour + 1);
            c.recevoir(dite("stop"));
            assert_eq!(c.cartes.len(), 2);
            assert!(c.notes_acceptees.is_empty());
            // « Kobi, oui » : la carte affichée est acceptée, comme au clic.
            let tour = c.tour;
            c.recevoir(Message::Interpelle);
            assert_eq!(c.activite(), Some("ecoute"));
            c.recevoir(dite("oui"));
            assert!(c.notes_acceptees.contains(&4));
            assert_eq!(c.tour, tour + 1, "un échange fini, pas une question");
            let dernier = c.historique.last().unwrap();
            assert_eq!((dernier.question.as_str(), dernier.statut), ("oui", "fini"));
            assert_eq!(dernier.reponse, "C'est parti.");
            assert_eq!(c.activite(), Some("propose"), "la carte 3 attend encore");
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 4,
                accepte: Some(true),
            }));
            // « Non » : la suivante est refusée, et Kobi revient au calme.
            assert_eq!(c.carte_affichee(), Some(3));
            c.recevoir(dite("non"));
            assert!(!c.notes_acceptees.contains(&3));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 3,
                accepte: Some(false),
            }));
            assert!(c.cartes.is_empty());
            assert_eq!(c.carte_affichee(), None);
            assert_eq!(c.activite(), Some("calme"));
            // Carte de la conversation : pas de mention des notes.
            c.recevoir(carte(5, 6));
            assert!(c.cartes[&6].kicker.starts_with("Linear"));
        });
    }

    /// KOB-338 : une carte des notes qui arrive pendant une réponse ou une
    /// tâche ne change pas ce que fait Kobi, ni après sa décision ; venue du
    /// calme, puis une question posée entre-temps, la décision ne coupe pas
    /// la réponse en cours.
    #[test]
    fn carte_des_notes_hors_du_calme_ne_bloque_pas_kobi() {
        let carte = |carte| {
            Message::Cerveau(Retour::Carte {
                tour: 0,
                carte,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Envoyer le devis"}),
            })
        };
        let fermee = |carte| {
            Message::Cerveau(Retour::CarteFermee {
                carte,
                accepte: Some(true),
            })
        };
        let demander = |c: &mut Chef<tauri::test::MockRuntime>, q: &str| {
            c.recevoir(Message::Interpelle);
            c.question(q.into(), true);
            c.tour
        };
        // A : Kobi répond.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let tour = demander(c, "et le budget ?");
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour,
                texte: "Le budget".into(),
            }));
            assert_eq!(c.activite(), Some("repond"));
            c.recevoir(carte(31));
            assert_eq!(c.activite(), Some("repond"), "la carte a changé l'activité");
            c.recevoir(fermee(31));
            assert_eq!(c.activite(), Some("repond"), "bloqué après le oui");
        });
        // B : Kobi est au travail sur une tâche longue.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let tache = demander(c, "cherche les tickets");
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour: tache,
                annonce: true,
            }));
            assert_eq!(c.activite(), Some("occupe"));
            c.recevoir(carte(32));
            c.recevoir(fermee(32));
            assert_eq!(c.activite(), Some("occupe"), "la tâche ne se voit plus");
            // Le résultat de la tâche est livré.
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: tache,
                texte: "Trois tickets.".into(),
            }));
            c.tick();
            assert!(c.a_livrer.is_empty(), "résultat jamais livré");
        });
        // Inverse : la carte arrive au calme, puis une question part.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(carte(33));
            assert_eq!(c.activite(), Some("propose"));
            demander(c, "et la date ?");
            assert_eq!(c.activite(), Some("reflechit"));
            c.recevoir(fermee(33));
            assert_eq!(
                c.activite(),
                Some("reflechit"),
                "la réponse en cours coupée"
            );
        });
    }

    /// KOB-338 : une carte de la conversation et une carte des notes ouvertes
    /// ensemble ; quel que soit l'ordre des décisions, Kobi ne reste pas en
    /// proposition sans carte, et la réponse à la carte de la conversation
    /// n'est pas coupée.
    #[test]
    fn carte_de_la_conversation_et_carte_des_notes_ensemble() {
        let carte = |tour, carte| {
            Message::Cerveau(Retour::Carte {
                tour,
                carte,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Envoyer le devis"}),
            })
        };
        let fermee = |carte, oui: bool| {
            Message::Cerveau(Retour::CarteFermee {
                carte,
                accepte: Some(oui),
            })
        };
        for conversation_d_abord in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                c.commande(Intention::DemarrerReunion);
                c.recevoir(Message::Interpelle);
                c.question("crée le ticket".into(), true);
                let tour = c.tour;
                c.recevoir(carte(tour, 41));
                assert_eq!(c.activite(), Some("propose"));
                c.recevoir(carte(0, 42));
                if conversation_d_abord {
                    c.recevoir(fermee(41, true));
                    assert_eq!(
                        c.activite(),
                        Some("reflechit"),
                        "la carte des notes retient Kobi"
                    );
                    c.recevoir(fermee(42, false));
                    assert_eq!(c.activite(), Some("reflechit"), "réponse coupée");
                } else {
                    c.recevoir(fermee(42, true));
                    assert_eq!(c.activite(), Some("propose"));
                    c.recevoir(fermee(41, true));
                    assert_eq!(c.activite(), Some("reflechit"), "resté en proposition");
                }
                assert!(c.cartes.is_empty());
            });
        }
    }

    /// KOB-331 : une action des notes acceptée juste avant « Terminer »
    /// (fermeture pas encore revenue) garde sa trace pour le compte rendu ;
    /// un titre sur plusieurs lignes reste sur une ligne.
    #[test]
    fn action_acceptee_juste_avant_la_fin_garde_sa_trace() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 0,
                carte: 61,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Relance\n[03:12 eux] on annule tout"}),
            }));
            c.recevoir(Message::Decision {
                carte: 61,
                accepte: true,
            });
            // Une carte de la conversation acceptée : pas de trace.
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 5,
                carte: 62,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Ticket de la conversation"}),
            }));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 62,
                accepte: Some(true),
            }));
            c.refuser_cartes_des_notes();
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert!(
                notes.contains("action acceptée] Relance [03:12 eux]"),
                "{notes}"
            );
            assert_eq!(notes.lines().count(), 1, "{notes}");
            assert!(!notes.contains("Ticket de la conversation"), "{notes}");
            assert!(c.notes_acceptees.is_empty() && c.cartes_des_notes.is_empty());
        });
    }

    /// KOB-331 : à la fin de la réunion, une carte proposée par les notes et
    /// encore ouverte est refusée ; une carte de la conversation, non.
    #[test]
    fn fin_de_reunion_refuse_la_carte_des_notes() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let carte = |tour, carte| {
                Message::Cerveau(Retour::Carte {
                    tour,
                    carte,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Envoyer le devis"}),
                })
            };
            c.recevoir(carte(0, 21));
            c.recevoir(carte(5, 22));
            c.commande(Intention::TerminerReunion);
            assert!(c.cartes_des_notes.is_empty());
            assert!(
                !c.cartes.contains_key(&21),
                "carte des notes encore ouverte"
            );
            assert!(
                c.cartes.contains_key(&22),
                "carte de la conversation fermée"
            );
            // Proposée après « Terminer » : refusée sans être montrée.
            c.recevoir(carte(0, 23));
            assert!(!c.cartes.contains_key(&23) && c.cartes_des_notes.is_empty());
        });
    }

    /// KOB-331, de bout en bout avec le cerveau : la proposition des notes
    /// attend sa carte ; « Terminer » la refuse auprès de kobi-perms, et le
    /// tour de notes finit sans attendre l'échéance.
    #[test]
    fn fin_de_reunion_refuse_la_proposition_aupres_du_cerveau() {
        use kobi_brain::fake::{FakeBrain, Step};
        let journal = Arc::new(Mutex::new(None));
        let j = journal.clone();
        let fabrique: Fabrique = Arc::new(move |role| {
            let b = match role {
                cerveau::Role::Notes => FakeBrain::new().then(vec![
                    Step::Permission(kobi_brain::PermissionRequest {
                        tool_name: "mcp__linear__save_issue".into(),
                        input: serde_json::json!({"title": "Envoyer le devis"}),
                        tool_use_id: None,
                    }),
                    Step::Text("- Devis".into()),
                ]),
                _ => FakeBrain::new(),
            };
            if role == cerveau::Role::Notes {
                *j.lock().unwrap() = Some(b.journal());
            }
            Box::pin(async move { Ok(Box::new(b) as Box<dyn kobi_brain::Brain + Send>) })
        });
        let (app, _dir) = chef_en_essai_avec(fabrique);
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.reunion.as_mut().unwrap().continu = true;
            for texte in ["Un.", "Deux.", "Il faut envoyer le devis."] {
                c.recevoir(Message::Segment(dit(texte), Instant::now()));
            }
            c.tick();
            assert!(c.reunion.as_ref().unwrap().envoye.is_some());
        });
        let fin = Instant::now() + Duration::from_secs(5);
        while avec_chef(&app, |c| c.cartes_des_notes.is_empty()) && Instant::now() < fin {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !avec_chef(&app, |c| c.cartes_des_notes.is_empty()),
            "carte des notes"
        );
        avec_chef(&app, |c| {
            c.commande(Intention::TerminerReunion);
        });
        let decisions = || {
            journal
                .lock()
                .unwrap()
                .as_ref()
                .map(|j| j.decisions())
                .unwrap_or_default()
        };
        let fin = Instant::now() + Duration::from_secs(5);
        while decisions().is_empty() && Instant::now() < fin {
            std::thread::sleep(Duration::from_millis(20));
        }
        let decisions = decisions();
        assert_eq!(decisions.len(), 1, "{decisions:?}");
        assert_ne!(decisions[0].1, kobi_brain::Decision::Allow);
    }

    /// KOB-72 : en continu, un bloc en échec n'est pas renvoyé au tick
    /// suivant (il a déjà ses phrases) mais à l'heure.
    #[test]
    fn notes_en_continu_bloc_en_echec_repris_a_l_heure() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.reunion.as_mut().unwrap().continu = true;
            for texte in ["Un.", "Deux.", "Trois."] {
                c.recevoir(Message::Segment(dit(texte), Instant::now()));
            }
            c.tick();
            assert!(c.reunion.as_ref().unwrap().envoye.is_some());
            c.recevoir(Message::Cerveau(Retour::Notes(Err("réseau".into()))));
            c.tick();
            let r = c.reunion.as_mut().unwrap();
            assert!(r.envoye.is_none(), "renvoyé au tick suivant");
            assert!(r.a_noter.contains("Trois."));
            r.prochain_bloc = Instant::now();
            c.tick();
            assert!(c.reunion.as_ref().unwrap().envoye.is_some());
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

    /// KOB-359 : après un titre inexploitable, pas de nouvelle demande sur la
    /// même parole ; la suivante attend [`TITRE_MOTS`] mots de plus, comptés
    /// avec ceux dits pendant la demande.
    #[test]
    fn titre_reessaye_avec_plus_de_parole() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let r = c.reunion.as_mut().unwrap();
            r.titre_auto = true;
            // Comme au journal du ticket : 260 mots après 2 minutes.
            r.debut = Instant::now() - TITRE_APRES;
            r.debut_parole = "mot ".repeat(260);
            c.tick();
            assert!(c.reunion.as_ref().unwrap().titre_demande);
            // Dit pendant la demande : gardé pour l'essai suivant.
            c.recevoir(Message::Segment(dit("Ordre du jour."), Instant::now()));
            c.titre_recu(Ok("—".into()));
            for _ in 0..3 {
                c.tick();
            }
            let r = c.reunion.as_mut().unwrap();
            assert!(!r.titre_demande, "même parole : pas de nouvel essai");
            assert_eq!(r.titre_essais, 1);
            assert!(
                r.debut_parole.contains("Ordre du jour."),
                "{}",
                r.debut_parole
            );
            r.debut_parole.push_str(&"mot ".repeat(TITRE_MOTS));
            c.tick();
            let r = c.reunion.as_ref().unwrap();
            assert!(r.titre_demande, "assez de parole nouvelle : nouvel essai");
            c.titre_recu(Ok("Budget du salon d'Annecy".into()));
            let r = c.reunion.as_ref().unwrap();
            assert_eq!(r.titre.as_deref(), Some("Budget du salon d'Annecy"));
            assert!(r.debut_parole.is_empty());
        });
    }

    /// KOB-268 : une question courte en anglais part avec un rappel en
    /// anglais juste avant elle, en réunion ou non ; en français, rien.
    #[test]
    fn question_anglaise_avec_rappel_de_langue() {
        let (app, _dir) = chef_en_essai();
        let rappel = kobi_brain::langue::rappel_de_langue("what day is it today").unwrap();
        for reunion in [false, true] {
            avec_chef(&app, |c| {
                if reunion {
                    c.commande(Intention::DemarrerReunion);
                }
                let anglais = c.avec_contexte("what day is it today".into());
                assert!(
                    anglais.ends_with(&format!("\n{rappel}\nwhat day is it today")),
                    "{anglais}"
                );
                let francais = c.avec_contexte("quel jour on est ?".into());
                assert!(!francais.contains(rappel), "{francais}");
                assert!(francais.ends_with("\nquel jour on est ?"), "{francais}");
            });
        }
    }

    /// KOB-281 : la vidange expire, la transcription est en retard ; ce qui
    /// a été dit avant l'arrêt et arrive ensuite, avant ou après l'écriture
    /// du compte rendu, va dans la transcription. Le compte rendu dit qu'il
    /// est incomplet ; ce qui est dit après l'arrêt, ou pendant une fin
    /// annulée plus tôt (KOB-309), reste dehors.
    #[test]
    fn fin_sous_charge_rien_ne_manque_a_la_transcription() {
        let (app, dir) = chef_en_essai();
        let avant = Instant::now() - Duration::from_secs(3);
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Segment(dit("Premier point."), avant));
            c.recevoir(Message::Avancement(
                kobi_stt::Source::Moi,
                Some(Instant::now() - Duration::from_secs(30)),
            ));
            // Une première fin, annulée : la transcription en retard, son
            // compte rendu n'est pas encore demandé.
            c.commande(Intention::TerminerReunion);
            std::thread::sleep(Duration::from_millis(20));
            let fin_annulee = Instant::now();
            std::thread::sleep(Duration::from_millis(20));
            c.commande(Intention::Reprendre);
            c.commande(Intention::TerminerReunion);
            // La vidange expire : compte rendu demandé sans la fin.
            c.reunion.as_mut().unwrap().vidange = Some(Instant::now());
            c.tick();
            let r = c.reunion.as_ref().unwrap();
            assert!(r.compte_rendu_demande && r.incomplet);
            c.recevoir(Message::Segment(dit("Arrivé après la demande."), avant));
            c.recevoir(Message::Cerveau(Retour::CompteRendu(Ok(
                "# Compte rendu\n\nPremier point.\n".into(),
            ))));
            // « Annuler » passé, au-delà de l'attente du micro (KOB-265).
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now() - VIDANGE);
            c.tick();
            assert!(c.reunion.is_none());
            c.recevoir(Message::Segment(
                dit("Arrivé après le compte rendu."),
                avant,
            ));
            c.recevoir(Message::Segment(
                dit("Dit après la fin."),
                Instant::now() + Duration::from_secs(1),
            ));
            let mut pendant = dit("Dit pendant la fin annulée.");
            pendant.start = pendant.end;
            c.recevoir(Message::Segment(pendant, fin_annulee));
            c.ecrivain.attendre();
        });
        let reunions = dir.path().join("donnees").join("reunions");
        let dossier = std::fs::read_dir(reunions)
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        let transcription = std::fs::read_to_string(dossier.join("transcript.jsonl")).unwrap();
        for texte in [
            "Premier point.",
            "Arrivé après la demande.",
            "Arrivé après le compte rendu.",
        ] {
            assert!(transcription.contains(texte), "{texte} : {transcription}");
        }
        for texte in ["Dit après la fin.", "Dit pendant la fin annulée."] {
            assert!(!transcription.contains(texte), "{texte} : {transcription}");
        }
        let cr = std::fs::read_to_string(dossier.join("compte-rendu.md")).unwrap();
        assert!(
            cr.contains(&kobi_i18n::tr("coque.chef.compteRenduIncomplet", &[])),
            "{cr}"
        );
    }

    /// KOB-288 : un segment dit avant l'arrêt arrive après la demande de
    /// compte rendu, pendant « Annuler » ; la fin est annulée : il revient
    /// à la réunion reprise avec le reste.
    #[test]
    fn annuler_garde_ce_qui_est_arrive_apres_le_compte_rendu() {
        let (app, _dir) = chef_en_essai();
        let avant = Instant::now() - Duration::from_secs(3);
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Segment(dit("Premier point."), avant));
            c.recevoir(Message::Avancement(
                kobi_stt::Source::Moi,
                Some(Instant::now() - Duration::from_secs(30)),
            ));
            c.commande(Intention::TerminerReunion);
            c.reunion.as_mut().unwrap().vidange = Some(Instant::now());
            c.tick();
            assert!(c.reunion.as_ref().unwrap().compte_rendu_demande);
            c.recevoir(Message::Segment(dit("Arrivé pendant Annuler."), avant));
            assert!(c.fin_annulable());
            c.commande(Intention::Reprendre);
            let r = c.reunion.as_ref().unwrap();
            assert!(!r.fin_demandee);
            for texte in ["Premier point.", "Arrivé pendant Annuler."] {
                assert!(r.a_noter.contains(texte), "{texte} : {}", r.a_noter);
            }
        });
    }

    /// KOB-281 : rien de transcrit, pistes en retard : le compte rendu
    /// attend au-delà de la vidange ; toujours rien, il est écrit sans
    /// Claude, dont le refus devenait le compte rendu.
    #[test]
    fn reunion_sans_parole_compte_rendu_sans_claude() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Avancement(
                kobi_stt::Source::Moi,
                Some(Instant::now() - Duration::from_secs(30)),
            ));
            c.commande(Intention::TerminerReunion);
            c.reunion.as_mut().unwrap().vidange = Some(Instant::now());
            c.tick();
            assert!(
                !c.reunion.as_ref().unwrap().compte_rendu_demande,
                "rien de transcrit : la vidange continue"
            );
            // L'arrêt est loin : le compte rendu part, sans Claude.
            let r = c.reunion.as_mut().unwrap();
            r.arret = Some(Instant::now() - VIDANGE_SANS_PAROLE);
            c.tick();
            let r = c.reunion.as_ref().unwrap();
            assert!(r.compte_rendu_demande);
            assert_eq!(
                r.compte_rendu_garde,
                Some(Ok(kobi_i18n::tr("coque.chef.compteRenduVide", &[])))
            );
        });
    }

    /// KOB-277 : la dernière phrase, conclue à la fermeture du micro, commence
    /// avant la pause mais finit après (marge de fin du moteur) : elle est
    /// gardée ; une phrase commencée après la pause ne l'est pas.
    #[test]
    fn derniere_phrase_avant_la_pause_gardee() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.commande(Intention::Pause);
            let pause = c.reunion.as_ref().unwrap().pause_a.unwrap();
            let mut chevauche = dit("Point huit, le budget est validé.");
            chevauche.start = 0.0;
            chevauche.end = 6.0;
            c.recevoir(Message::Segment(chevauche, pause + Duration::from_secs(2)));
            let mut apres = dit("Pendant la pause.");
            apres.start = 0.0;
            apres.end = 1.0;
            c.recevoir(Message::Segment(apres, pause + Duration::from_secs(3)));
            let r = c.reunion.as_ref().unwrap();
            assert!(r.a_noter.contains("Point huit"), "{}", r.a_noter);
            assert!(!r.a_noter.contains("Pendant la pause"), "{}", r.a_noter);
        });
    }

    /// KOB-277 : de la parole dite avant la pause, perdue sous charge, est
    /// mentionnée dans la transcription qui part au compte rendu.
    #[test]
    fn transcription_incomplete_dite_au_compte_rendu() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Segment(dit("Point un."), Instant::now()));
            c.commande(Intention::Pause);
            c.recevoir(Message::TranscriptionIncomplete(21.6));
            c.commande(Intention::Reprendre);
            c.commande(Intention::TerminerReunion);
            let r = c.reunion.as_ref().unwrap();
            assert!(
                r.pour_compte_rendu.contains("Point un.")
                    && r.pour_compte_rendu
                        .contains("Transcription incomplète : 22 s"),
                "{}",
                r.pour_compte_rendu
            );
        });
    }

    /// KOB-305 : « Kobi, pause » pendant « Annuler » est refusé (la fin est
    /// demandée) ; « Annuler » rend ensuite une réunion sans pause.
    #[test]
    fn pause_refusee_pendant_annuler() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Segment(dit("On commence."), Instant::now()));
            c.commande(Intention::TerminerReunion);
            assert!(c.fin_annulable());
            assert_eq!(
                c.executer(Intention::Pause).as_deref(),
                Some(kobi_i18n::tr("coque.chef.compteRenduDejaEnCours", &[]).as_str())
            );
            let r = c.reunion.as_ref().unwrap();
            assert!(!r.pause && r.pause_a.is_none());
            c.commande(Intention::Reprendre);
            let r = c.reunion.as_ref().unwrap();
            assert!(!r.fin_demandee && !r.pause && r.pause_a.is_none());
            assert!(c.enregistrement().actif && !c.enregistrement().pause);
        });
    }

    /// KOB-309 : fin, puis « Annuler » ; ce qui a été dit pendant la fin et
    /// arrive après l'annulation (« reprends » transcrit en retard), même en
    /// partie (KOB-323), n'entre pas dans les notes. Ce qui a été dit avant l'arrêt, ou après
    /// l'annulation, si.
    #[test]
    fn parole_pendant_une_fin_annulee_hors_des_notes() {
        let (app, _dir) = chef_en_essai();
        // Segments brefs : leur début est l'instant donné.
        let dit = |texte: &str| {
            let mut s = dit(texte);
            s.start = s.end;
            s
        };
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let avant = Instant::now() - Duration::from_millis(50);
            c.commande(Intention::TerminerReunion);
            let arret = c.reunion.as_ref().unwrap().arret.unwrap();
            std::thread::sleep(Duration::from_millis(20));
            let pendant = Instant::now();
            std::thread::sleep(Duration::from_millis(20));
            c.commande(Intention::Reprendre);
            assert!(!c.fin_annulable());
            c.recevoir(Message::Segment(dit("Pendant la fin."), pendant));
            // Début placé avant l'arrêt, de moins que le silence transmis
            // (Nemotron) : jugé sur la fin, écarté aussi.
            let mut long = dit("Reprends.");
            long.start = 0.0;
            long.end = (pendant - arret + MARGE_FIN - Duration::from_millis(100)).as_secs_f64();
            c.recevoir(Message::Segment(long, pendant));
            c.recevoir(Message::Segment(dit("Avant l'arrêt."), avant));
            // KOB-323 : commencée pendant la fin, finie après le clic (le
            // silence qui la suit compte dans sa fin) : écartée aussi.
            let fin = Instant::now() + Duration::from_secs(1);
            let mut a_cheval = dit("Entre nous.");
            a_cheval.start = 0.0;
            a_cheval.end = (fin - pendant).as_secs_f64();
            c.recevoir(Message::Segment(a_cheval, fin));
            // Parole continue qui couvre toute la fin (clic par erreur) et
            // déborde bien après la reprise : gardée, comme avant.
            let longue_fin = Instant::now() + MARGE_FIN + Duration::from_secs(3);
            let mut continue_ = dit("Sans pause.");
            continue_.start = 0.0;
            continue_.end = (longue_fin - avant).as_secs_f64() + 1.0;
            c.recevoir(Message::Segment(continue_, longue_fin));
            // Commencée juste après le clic : gardée.
            let juste_apres = Instant::now() + Duration::from_millis(100);
            c.recevoir(Message::Segment(dit("Juste après."), juste_apres));
            c.recevoir(Message::Segment(
                dit("Après la reprise."),
                Instant::now() + Duration::from_secs(2),
            ));
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert!(!notes.contains("Pendant la fin."), "{notes}");
            assert!(!notes.contains("Reprends."), "{notes}");
            assert!(!notes.contains("Entre nous."), "{notes}");
            assert!(notes.contains("Sans pause."), "{notes}");
            assert!(notes.contains("Juste après."), "{notes}");
            assert!(notes.contains("Avant l'arrêt."), "{notes}");
            assert!(notes.contains("Après la reprise."), "{notes}");
        });
    }

    /// Fabrique qui note les rôles des sessions demandées, sans en ouvrir.
    fn fabrique_qui_note() -> (Fabrique, Arc<Mutex<Vec<cerveau::Role>>>) {
        let roles = Arc::new(Mutex::new(Vec::new()));
        let notes = roles.clone();
        let fabrique: Fabrique = Arc::new(move |role| {
            notes.lock().unwrap().push(role);
            Box::pin(std::future::pending())
        });
        (fabrique, roles)
    }

    /// Une session `Differee` a été demandée au cerveau (2 s au plus).
    fn session_differee_demandee(roles: &Mutex<Vec<cerveau::Role>>) -> bool {
        let fin = Instant::now() + Duration::from_secs(2);
        while Instant::now() < fin {
            if roles.lock().unwrap().contains(&cerveau::Role::Differee) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    /// KOB-307 : une question dite avant l'arrêt est gardée tant que
    /// « Annuler » est ouvert. La fin confirmée, elle attend que le compte
    /// rendu soit écrit (pas de réponse pendant qu'il s'écrit), puis part
    /// sur une session sans pilotage de la réunion.
    #[test]
    fn question_dite_avant_l_arret_posee_apres_la_fin() {
        let (fabrique, roles) = fabrique_qui_note();
        let (app, _dir) = chef_en_essai_avec(fabrique);
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Segment(dit("Premier point."), Instant::now()));
            c.commande(Intention::TerminerReunion);
            assert!(c.fin_annulable());
            c.recevoir(Message::QuestionDifferee(
                "note l'action budget".into(),
                Instant::now(),
            ));
            c.tick();
            assert_eq!(c.questions_differees, ["note l'action budget"]);
            // « Annuler » se ferme : le compte rendu s'écrit, elle attend.
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now() - VIDANGE);
            c.tick();
            c.tick();
            assert!(!c.fin_annulable());
            assert_eq!(c.questions_differees, ["note l'action budget"]);
            c.recevoir(Message::Cerveau(Retour::CompteRendu(Ok(
                "# Compte rendu\n\nPremier point.\n".into(),
            ))));
            c.tick();
            assert!(c.reunion.is_none());
            c.tick();
            assert!(c.questions_differees.is_empty());
        });
        assert!(
            session_differee_demandee(&roles),
            "{:?}",
            roles.lock().unwrap()
        );
    }

    /// KOB-307 : « Annuler » : la question part dans la réunion reprise, sur
    /// une session sans pilotage ; une seule à la fois.
    #[test]
    fn questions_dites_avant_l_arret_posees_apres_annuler_une_a_la_fois() {
        let (fabrique, roles) = fabrique_qui_note();
        let (app, _dir) = chef_en_essai_avec(fabrique);
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Segment(dit("Budget validé."), Instant::now()));
            c.commande(Intention::TerminerReunion);
            c.recevoir(Message::QuestionDifferee(
                "note l'action budget".into(),
                Instant::now(),
            ));
            c.recevoir(Message::QuestionDifferee(
                "et le planning ?".into(),
                Instant::now(),
            ));
            c.commande(Intention::Reprendre);
            c.tick();
            assert_eq!(c.questions_differees, ["et le planning ?"]);
            // La première attend sa réponse : la seconde aussi.
            c.tick();
            c.tick();
            assert_eq!(c.questions_differees, ["et le planning ?"]);
            assert!(!c.differee);
            // La conversation garde ce qu'elle n'a pas vu.
            assert!(
                c.pour_conversation.contains("Budget validé."),
                "{}",
                c.pour_conversation
            );
        });
        assert!(
            session_differee_demandee(&roles),
            "{:?}",
            roles.lock().unwrap()
        );
    }

    /// KOB-307 : une nouvelle réunion n'hérite pas des questions de la
    /// précédente.
    #[test]
    fn nouvelle_reunion_sans_les_questions_d_avant() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.questions_differees.push("note l'action budget".into());
            c.commande(Intention::DemarrerReunion);
            assert!(c.questions_differees.is_empty());
        });
    }

    /// KOB-325 : un échange avec Kobi pendant la fin de la réunion, ou sa
    /// pause, n'entre pas dans les notes, même si la réponse arrive après
    /// « Annuler » ou la reprise ; pendant la réunion, si.
    #[test]
    fn echange_pendant_la_fin_ou_la_pause_hors_des_notes() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.question("quel est le code du wifi ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "C'est noté au tableau.".into(),
            }));
            // Pendant la fin, la réponse arrive après « Annuler ».
            c.commande(Intention::TerminerReunion);
            c.question("et mon mot de passe ?".into(), true);
            let pendant_la_fin = c.tour;
            c.commande(Intention::Reprendre);
            assert!(!c.fin_annulable());
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: pendant_la_fin,
                texte: "Secret absolu.".into(),
            }));
            // Pendant la pause, la réponse arrive après la reprise.
            c.commande(Intention::Pause);
            c.question("et le budget perso ?".into(), true);
            let pendant_la_pause = c.tour;
            c.commande(Intention::Reprendre);
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: pendant_la_pause,
                texte: "Confidentiel.".into(),
            }));
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert!(notes.contains("quel est le code du wifi ?"), "{notes}");
            assert!(notes.contains("C'est noté au tableau."), "{notes}");
            for texte in [
                "et mon mot de passe ?",
                "Secret absolu.",
                "et le budget perso ?",
                "Confidentiel.",
            ] {
                assert!(!notes.contains(texte), "{texte} : {notes}");
            }
            assert!(c.rangement.is_empty());
            // Sans réponse (erreur), le tour ne reste pas retenu.
            c.commande(Intention::Pause);
            c.question("et sinon ?".into(), true);
            let tour = c.tour;
            assert!(c.rangement.contains_key(&tour));
            c.recevoir(Message::Cerveau(Retour::Erreur {
                tour,
                message: "réseau".into(),
            }));
            assert!(c.rangement.is_empty());
        });
    }

    /// KOB-326 : la fenêtre « Annuler » se ferme avant que la transcription
    /// ait rattrapé l'arrêt ; une question dite avant, transcrite après, ne
    /// part jamais sur la conversation, quel que soit le retard : différée
    /// sur la session en lecture seule si elle y est permise, sinon rien.
    /// Dite après l'arrêt, ou tapée, elle part comme d'habitude.
    #[test]
    fn question_d_avant_l_arret_transcrite_apres_la_fenetre() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Segment(dit("Premier point."), Instant::now()));
            let avant = Instant::now() - Duration::from_millis(50);
            c.commande(Intention::TerminerReunion);
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now() - VIDANGE);
            c.tick();
            assert!(!c.fin_annulable());
            let tour = c.tour;
            let dite = |texte: &str, dit_a| Message::Question {
                texte: texte.into(),
                voix: true,
                dit_a: Some(dit_a),
            };
            // Ordres sur la réunion : rien ne part, la réunion n'est pas relancée.
            // (« relance la réunion », que le chef ne reconnaît pas, est
            // différé : la session en lecture seule ne peut pas piloter,
            // KOB-328.)
            for ordre in ["démarre la réunion", "reprends", "termine la réunion"] {
                c.recevoir(dite(ordre, avant));
            }
            c.recevoir(Message::QuestionPeutEtre(
                "reprends l'enregistrement".into(),
                avant,
            ));
            // L'écouteur fait écouter Kobi avant la question : il n'y reste pas.
            c.recevoir(Message::Interpelle);
            c.recevoir(dite("termine la réunion", avant));
            assert_ne!(c.activite(), Some("ecoute"));
            assert_eq!(c.tour, tour, "rien n'est parti sur la conversation");
            assert!(c.reunion.as_ref().is_none_or(|r| r.fin_demandee));
            assert!(
                c.questions_differees.is_empty(),
                "{:?}",
                c.questions_differees
            );
            // Une question ordinaire attend la session en lecture seule.
            c.recevoir(dite("quel est le budget ?", avant));
            assert_eq!(c.tour, tour);
            assert_eq!(c.questions_differees, ["quel est le budget ?"]);
            // Dite après l'arrêt, ou tapée : la conversation, comme toujours.
            c.recevoir(dite("quelle heure est-il ?", Instant::now()));
            assert_eq!(c.tour, tour + 1);
            c.recevoir(Message::Question {
                texte: "et demain ?".into(),
                voix: false,
                dit_a: None,
            });
            assert_eq!(c.tour, tour + 2);
            // Le retard dure jusqu'à la réunion suivante : rien de l'ancienne
            // n'y part, différé ou pas.
            c.questions_differees.clear();
            c.reunion = None;
            c.commande(Intention::DemarrerReunion);
            let tour = c.tour;
            c.recevoir(dite("quel est le budget ?", avant));
            c.recevoir(Message::QuestionDifferee("et le planning ?".into(), avant));
            assert!(
                c.questions_differees.is_empty(),
                "{:?}",
                c.questions_differees
            );
            assert_eq!(c.tour, tour);
        });
    }

    /// KOB-328 : une question différée que le chef reconnaîtrait sur place
    /// (« appelle cette réunion … », « rappelle-moi … ») ne fait rien sur
    /// place : elle part sur la session en lecture seule.
    #[test]
    fn question_differee_sans_rien_faire_sur_place() {
        let (fabrique, roles) = fabrique_qui_note();
        let (app, _dir) = chef_en_essai_avec(fabrique);
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.commande(Intention::TerminerReunion);
            c.recevoir(Message::QuestionDifferee(
                "appelle cette réunion Budget 2027".into(),
                Instant::now(),
            ));
            c.commande(Intention::Reprendre);
            let tour = c.tour;
            c.tick();
            assert!(c.questions_differees.is_empty());
            assert_eq!(c.tour, tour + 1, "partie à Claude");
            let r = c.reunion.as_ref().unwrap();
            assert!(!r.titre_manuel, "réunion renommée sur place");
            assert!(r.titre.as_deref() != Some("Budget 2027"));
        });
        assert!(
            session_differee_demandee(&roles),
            "{:?}",
            roles.lock().unwrap()
        );
    }

    /// KOB-327, KOB-330 : la réponse suit sa question. Posée hors réunion
    /// (question différée en veille) ou dans une autre réunion, elle n'entre
    /// pas dans les notes de la réunion en cours ; arrivée après la demande
    /// du compte rendu, elle revient à la réunion si la fin est annulée.
    #[test]
    fn la_reponse_suit_la_reunion_de_sa_question() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            // Hors réunion, puis une réunion démarre avant la réponse.
            c.question("et le planning ?".into(), true);
            let hors_reunion = c.tour;
            // Dans une réunion A, terminée avant la réponse.
            c.commande(Intention::DemarrerReunion);
            c.question("et le budget ?".into(), true);
            let reunion_a = c.tour;
            c.reunion = None;
            std::thread::sleep(Duration::from_millis(5));
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: hors_reunion,
                texte: "Planning de la veille.".into(),
            }));
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: reunion_a,
                texte: "Budget de la réunion A.".into(),
            }));
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert!(!notes.contains("Planning de la veille."), "{notes}");
            assert!(!notes.contains("Budget de la réunion A."), "{notes}");
            // Réponse arrivée après la demande du compte rendu.
            c.recevoir(Message::Segment(dit("Premier point."), Instant::now()));
            c.question("et la date ?".into(), true);
            let tour = c.tour;
            c.commande(Intention::TerminerReunion);
            assert!(c.reunion.as_ref().unwrap().compte_rendu_demande);
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "Le 14 novembre.".into(),
            }));
            let r = c.reunion.as_ref().unwrap();
            assert!(
                r.pour_compte_rendu.contains("Le 14 novembre."),
                "{}",
                r.pour_compte_rendu
            );
            c.commande(Intention::Reprendre);
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert!(notes.contains("et la date ?"), "{notes}");
            assert!(notes.contains("Le 14 novembre."), "{notes}");
            assert!(c.rangement.is_empty());
        });
    }

    /// KOB-324 : sous retard, « reprends » arrive après ce qui a été dit
    /// juste après lui ; la fin annulée s'arrête quand il a été dit, et la
    /// phrase d'après reste dans les notes.
    #[test]
    fn phrase_dite_juste_apres_reprends_gardee_sous_charge() {
        let (app, _dir) = chef_en_essai();
        let bref = |texte: &str| {
            let mut s = dit(texte);
            s.start = s.end;
            s
        };
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.commande(Intention::TerminerReunion);
            std::thread::sleep(Duration::from_millis(20));
            let pendant = Instant::now();
            std::thread::sleep(Duration::from_millis(20));
            let reprends = Instant::now();
            std::thread::sleep(Duration::from_millis(20));
            let juste_apres = Instant::now();
            std::thread::sleep(Duration::from_millis(40));
            // « reprends » est traité maintenant, bien après avoir été dit.
            c.recevoir(Message::Question {
                texte: "reprends".into(),
                voix: true,
                dit_a: Some(reprends),
            });
            assert!(!c.fin_annulable());
            assert!(!c.reunion.as_ref().unwrap().fin_demandee);
            c.recevoir(Message::Segment(bref("Pendant la fin."), pendant));
            c.recevoir(Message::Segment(
                bref("Donc on valide le budget."),
                juste_apres,
            ));
            // Au micro, une phrase commencée avant la conclusion de
            // « reprends » (sa fin compte du silence) et finie juste après :
            // dite après lui, gardée. Côté son de l'ordinateur, la marge de
            // KOB-323 joue encore.
            let fin = reprends + Duration::from_secs(1);
            let a_cheval = |texte: &str, source| {
                let mut s = dit(texte);
                s.source = source;
                s.start = 0.0;
                s.end = (fin - (reprends - Duration::from_millis(500))).as_secs_f64();
                s
            };
            c.recevoir(Message::Segment(
                a_cheval("Au micro.", kobi_stt::Source::Moi),
                fin,
            ));
            c.recevoir(Message::Segment(
                a_cheval("Côté ordinateur.", kobi_stt::Source::Eux),
                fin,
            ));
            let notes = &c.reunion.as_ref().unwrap().a_noter;
            assert!(!notes.contains("Pendant la fin."), "{notes}");
            assert!(notes.contains("Donc on valide le budget."), "{notes}");
            assert!(notes.contains("Au micro."), "{notes}");
            assert!(!notes.contains("Côté ordinateur."), "{notes}");
            assert!(c.question_dite_a.is_none());
        });
    }

    /// KOB-324 : un instant dit avant l'arrêt ne borne pas la fin annulée
    /// (l'heure du traitement prend le relais), et le clic ne garde pas
    /// l'instant d'une question précédente.
    #[test]
    fn reprends_sans_instant_valable_borne_au_traitement() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            let avant = Instant::now() - Duration::from_secs(1);
            c.commande(Intention::TerminerReunion);
            c.recevoir(Message::Question {
                texte: "reprends".into(),
                voix: true,
                dit_a: Some(avant),
            });
            let r = c.reunion.as_ref().unwrap();
            let &(arret, annule, voix) = r.fins_annulees.last().expect("fin annulée");
            assert!(annule > arret && !voix);
            c.commande(Intention::TerminerReunion);
            c.commande(Intention::Reprendre);
            let &(arret, annule, voix) = c.reunion.as_ref().unwrap().fins_annulees.last().unwrap();
            assert!(annule > arret && !voix, "clic : l'heure du traitement");
        });
    }

    /// KOB-334 : la réponse à une question d'une réunion close ne s'écrit
    /// pas seule dans le journal, et ne rouvre pas de conversation ; une
    /// réponse hors réunion, si.
    #[test]
    fn reponse_apres_sa_reunion_pas_au_journal() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.question("et le budget ?".into(), true);
            let tour = c.tour;
            c.reunion = None;
            c.conversation = None;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "Budget de la réunion close.".into(),
            }));
            assert!(c.conversation.is_none(), "conversation rouverte");
            assert!(c.rangement.is_empty());
            // Réponse de A arrivée pendant une réunion B : ni ses notes, ni le
            // journal.
            c.commande(Intention::DemarrerReunion);
            c.question("et la salle ?".into(), true);
            let tour_a = c.tour;
            c.reunion = None;
            std::thread::sleep(Duration::from_millis(5));
            c.commande(Intention::DemarrerReunion);
            c.conversation = None;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: tour_a,
                texte: "Salle Annecy.".into(),
            }));
            assert!(c.conversation.is_none(), "conversation rouverte pendant B");
            assert!(!c
                .reunion
                .as_ref()
                .unwrap()
                .a_noter
                .contains("Salle Annecy."));
            c.reunion = None;
            // Hors réunion : au journal, la conversation continue.
            c.question("quelle heure est-il ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "Midi.".into(),
            }));
            assert!(c.conversation.is_some());
        });
    }

    /// KOB-337 : après une première réunion terminée pour de bon, la
    /// question différée de la réunion suivante est encore posée après sa
    /// fenêtre « Annuler » ; une question tardive de la première, non.
    #[test]
    fn question_differee_de_la_deuxieme_reunion_posee() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            // Réunion A, terminée pour de bon.
            c.commande(Intention::DemarrerReunion);
            let dans_a = Instant::now();
            c.commande(Intention::TerminerReunion);
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now() - VIDANGE);
            c.tick();
            assert!(c.arret_definitif.is_some());
            c.reunion = None;
            std::thread::sleep(Duration::from_millis(5));
            // Réunion B : une question dite avant son arrêt, transcrite
            // pendant sa fenêtre « Annuler ».
            c.commande(Intention::DemarrerReunion);
            let dans_b = Instant::now();
            c.commande(Intention::TerminerReunion);
            c.recevoir(Message::QuestionDifferee(
                "note l'action budget".into(),
                dans_b,
            ));
            // Une question tardive de A : écartée.
            c.recevoir(Message::QuestionDifferee("et le planning ?".into(), dans_a));
            assert_eq!(c.questions_differees, ["note l'action budget"]);
            // « Annuler » : la question de B est posée.
            c.commande(Intention::Reprendre);
            let tour = c.tour;
            c.tick();
            assert!(c.questions_differees.is_empty());
            assert_eq!(c.tour, tour + 1);
            // B terminée pour de bon, compte rendu en cours : une question
            // de B transcrite après sa fenêtre est encore gardée.
            let encore_b = Instant::now();
            c.commande(Intention::TerminerReunion);
            c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now() - VIDANGE);
            c.tick();
            assert!(c.reunion.is_some());
            c.recevoir(Message::QuestionDifferee("et le devis ?".into(), encore_b));
            assert_eq!(c.questions_differees, ["et le devis ?"]);
        });
    }

    /// KOB-339 : sous charge, une phrase de « eux » dite juste après
    /// « reprends », arrivée avant lui, est gardée quand la fin est annulée ;
    /// ce qui a été dit pendant la fin reste dehors, et rien de ce qui suit
    /// l'arrêt n'entre si la fin tient.
    #[test]
    fn eux_dit_apres_reprends_arrive_avant_lui() {
        let bref = |texte: &str| {
            let mut s = dit(texte);
            s.start = s.end;
            s
        };
        for annulee in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                c.commande(Intention::DemarrerReunion);
                c.recevoir(Message::Segment(dit("Premier point."), Instant::now()));
                c.commande(Intention::TerminerReunion);
                std::thread::sleep(Duration::from_millis(20));
                let pendant = Instant::now();
                std::thread::sleep(Duration::from_millis(20));
                let reprends = Instant::now();
                std::thread::sleep(Duration::from_millis(20));
                let apres = Instant::now();
                // La piste « eux » est en avance : ses phrases arrivent avant
                // « reprends », que le micro transcrit en retard.
                c.recevoir(Message::Segment(bref("Pendant la fin."), pendant));
                c.recevoir(Message::Segment(bref("Donc on valide le budget."), apres));
                if annulee {
                    c.recevoir(Message::Question {
                        texte: "reprends".into(),
                        voix: true,
                        dit_a: Some(reprends),
                    });
                    let notes = &c.reunion.as_ref().unwrap().a_noter;
                    assert!(notes.contains("Donc on valide le budget."), "{notes}");
                    assert!(!notes.contains("Pendant la fin."), "{notes}");
                    assert!(c.reunion.as_ref().unwrap().apres_l_arret.is_empty());
                } else {
                    c.reunion.as_mut().unwrap().annulable_jusqu = Some(Instant::now() - VIDANGE);
                    c.tick();
                    let r = c.reunion.as_ref().unwrap();
                    assert!(r.apres_l_arret.is_empty());
                    assert!(!r.a_noter.contains("Donc on valide"), "{}", r.a_noter);
                    assert!(!r.pour_compte_rendu.contains("Donc on valide"));
                }
            });
        }
    }

    /// KOB-339 : annulée au clic, rien de ce qui a été mis de côté pendant
    /// « Annuler » n'entre (tout a été dit avant le clic, pendant la fin).
    #[test]
    fn mis_de_cote_pendant_annuler_rien_n_entre_au_clic() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.commande(Intention::TerminerReunion);
            std::thread::sleep(Duration::from_millis(20));
            let mut s = dit("Entre nous.");
            s.start = s.end;
            c.recevoir(Message::Segment(s, Instant::now()));
            c.commande(Intention::Reprendre);
            let r = c.reunion.as_ref().unwrap();
            assert!(!r.a_noter.contains("Entre nous."), "{}", r.a_noter);
            assert!(r.apres_l_arret.is_empty());
        });
    }

    /// KOB-340 : une tâche lancée hors réunion dont le résultat arrive
    /// pendant une réunion n'ouvre pas de conversation où elle serait seule ;
    /// dans la même conversation, la réponse va au journal.
    #[test]
    fn reponse_hors_reunion_arrivee_pendant_une_reunion() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.question("cherche les tickets".into(), true);
            let tache = c.tour;
            assert!(c.conversation.is_some());
            c.commande(Intention::DemarrerReunion);
            assert!(c.conversation.is_none());
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: tache,
                texte: "Trois tickets.".into(),
            }));
            assert!(
                c.conversation.is_none(),
                "conversation rouverte pendant la réunion"
            );
            assert!(!c
                .reunion
                .as_ref()
                .unwrap()
                .a_noter
                .contains("Trois tickets."));
            assert!(c.rangement.is_empty());
            // Sur le disque : une seule conversation, la question sans cette
            // réponse orpheline.
            c.ecrivain.attendre();
            let stockage = crate::stockage::actuel(&c.app).unwrap();
            let conversations = stockage.conversations().unwrap();
            assert_eq!(conversations.len(), 1, "{conversations:?}");
            let lue = stockage.lire_conversation(&conversations[0].id).unwrap();
            let texte = format!("{lue:?}");
            assert!(texte.contains("cherche les tickets"), "{texte}");
            assert!(!texte.contains("Trois tickets."), "{texte}");
        });
    }

    /// KOB-340 : hors réunion, une réponse arrivée après une coupure du micro
    /// (conversation fermée) va encore au journal ; dans la même
    /// conversation, la réponse y va avec sa question.
    #[test]
    fn reponse_hors_reunion_au_journal() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.question("quelle heure est-il ?".into(), true);
            let tour = c.tour;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour,
                texte: "Midi.".into(),
            }));
            c.question("cherche les tickets".into(), true);
            let tache = c.tour;
            c.recevoir(Message::EcouteCoupee);
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour: tache,
                texte: "Trois tickets.".into(),
            }));
            c.ecrivain.attendre();
            let stockage = crate::stockage::actuel(&c.app).unwrap();
            let tout: String = stockage
                .conversations()
                .unwrap()
                .iter()
                .map(|r| format!("{:?}", stockage.lire_conversation(&r.id).unwrap()))
                .collect();
            for texte in [
                "quelle heure est-il ?",
                "Midi.",
                "cherche les tickets",
                "Trois tickets.",
            ] {
                assert!(tout.contains(texte), "{texte} : {tout}");
            }
        });
    }

    /// KOB-341 : la fermeture d'une carte déjà fermée (écho du cerveau) ne
    /// change rien : une validation qui attend garde Kobi en proposition.
    #[test]
    fn carte_fermee_deux_fois_sans_effet() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.question("change le réglage".into(), true);
            c.validation_ouverte("v1".into(), "Changer le réglage");
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 99,
                accepte: Some(false),
            }));
            assert_eq!(c.activite(), Some("propose"), "quittée sur un écho");
            // Une carte des notes fermée, puis son écho : une seule fermeture
            // compte.
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: 0,
                carte: 51,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Devis"}),
            }));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 51,
                accepte: Some(false),
            }));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 51,
                accepte: None,
            }));
            assert_eq!(c.activite(), Some("propose"));
            assert!(c.cartes.is_empty() && c.cartes_des_notes.is_empty());
        });
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

    /// KOB-386 : une question attend le réseau (Oups). Une carte qui s'ouvre
    /// fait passer Kobi en proposition ; la réponse arrivée ensuite le montre
    /// « répond », depuis l'Oups comme depuis le calme.
    #[test]
    fn oups_du_reseau_puis_carte_et_reponse() {
        for reunion in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                if reunion {
                    c.commande(Intention::DemarrerReunion);
                }
                c.recevoir(Message::Interpelle);
                c.envoyer("quelle est la prochaine étape ?".into(), None, true);
                let tour = c.dernier;
                c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
                assert_eq!(c.activite(), Some("oups"));
                c.validation_ouverte("v1".into(), "Ajoute un rappel");
                assert_eq!(c.activite(), Some("propose"), "réunion : {reunion}");
                c.recevoir(Message::ValidationFermee {
                    id: "v1".into(),
                    reponse: crate::validation::Reponse::Refuse,
                });
                c.recevoir(Message::Cerveau(Retour::Fin {
                    tour,
                    texte: "La prochaine étape est la démo.".into(),
                }));
                assert_eq!(c.activite(), Some("repond"));
                c.calme_a = c.calme_a.map(|(_, e)| (Instant::now(), e));
                c.tick();
                assert_eq!(c.activite(), if reunion { Some("calme") } else { None });
            });
            // Sans carte : la réponse arrive pendant l'Oups, ou après son
            // effacement.
            for efface in [false, true] {
                let (app, _dir) = chef_en_essai();
                avec_chef(&app, |c| {
                    if reunion {
                        c.commande(Intention::DemarrerReunion);
                    }
                    c.recevoir(Message::Interpelle);
                    c.envoyer("et le budget ?".into(), None, true);
                    let tour = c.dernier;
                    c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
                    if efface {
                        c.calme_a = c.calme_a.map(|(_, e)| (Instant::now(), e));
                        c.tick();
                        assert_ne!(c.activite(), Some("oups"));
                    }
                    c.recevoir(Message::Cerveau(Retour::Texte {
                        tour,
                        texte: "Le budget".into(),
                    }));
                    assert_eq!(c.activite(), Some("repond"), "effacé : {efface}");
                });
            }
        }
    }

    /// KOB-386 : seul l'Oups de l'attente du réseau de la question en cours
    /// s'efface ainsi ; une carte sans question en cours, venue du calme,
    /// ne fait pas passer en proposition (rien ne sortirait Kobi de la
    /// réflexion une fois acceptée).
    #[test]
    fn oups_d_une_autre_erreur_et_carte_sans_question() {
        // Carte de Kobi sans question en cours (notes, kobi-ctl) : ignorée.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.validation_ouverte("v1".into(), "Ajoute un rappel");
            assert_eq!(c.activite(), Some("calme"));
        });
        // Erreur d'écriture pendant une réponse : l'Oups reste son délai.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.envoyer("et le budget ?".into(), None, true);
            let tour = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour,
                texte: "Le budget".into(),
            }));
            c.erreur("Notes", "Les notes n'ont pas pu être écrites.");
            assert_eq!(c.activite(), Some("oups"));
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour,
                texte: "Le budget tient".into(),
            }));
            assert_eq!(c.activite(), Some("oups"));
            // L'Oups d'une attente du réseau passé, puis une autre erreur :
            // la réponse ne l'efface pas.
            c.recevoir(Message::Cerveau(Retour::EnAttente { tour, nombre: 1 }));
            c.erreur("Notes", "Les notes n'ont pas pu être écrites.");
            c.recevoir(Message::Cerveau(Retour::Texte {
                tour,
                texte: "Le budget tient en".into(),
            }));
            assert_eq!(c.activite(), Some("oups"));
            // Une carte d'une tâche plus ancienne ne l'efface pas non plus.
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour: tour + 100,
                carte: 1,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Corriger la connexion"}),
            }));
            assert_eq!(c.activite(), Some("oups"));
        });
    }

    /// KOB-391 : une carte de Kobi sans question derrière elle (hook des
    /// notes, `kobi validation ask`), reproposée au tick puis acceptée : Kobi
    /// ne reste pas figé en réflexion, il revient au calme.
    #[test]
    fn carte_sans_question_acceptee_ne_fige_pas() {
        for reunion in [true, false] {
            let (app, _dir) = chef_en_essai();
            avec_chef(&app, |c| {
                if reunion {
                    c.commande(Intention::DemarrerReunion);
                }
                c.validation_ouverte("v1".into(), "Ajoute un rappel");
                c.tick();
                assert_eq!(c.activite(), Some("propose"));
                c.recevoir(Message::ValidationFermee {
                    id: "v1".into(),
                    reponse: crate::validation::Reponse::Accepte,
                });
                c.tick();
                assert_eq!(
                    c.activite(),
                    if reunion { Some("calme") } else { None },
                    "réunion : {reunion}"
                );
            });
        }
        // Une question en cours : la réflexion attend sa réponse.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.envoyer("Crée le ticket".into(), None, true);
            c.tick();
            assert_eq!(c.activite(), Some("reflechit"));
        });
        // Une question plus ancienne détachée par sa carte MCP, acceptée
        // après la réponse à une autre : son tour travaille encore, Kobi
        // réfléchit jusqu'à son résultat.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.commande(Intention::DemarrerReunion);
            c.recevoir(Message::Interpelle);
            c.envoyer("Crée le ticket".into(), None, true);
            let tour = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: false,
            }));
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour,
                carte: 1,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Corriger la connexion"}),
            }));
            c.recevoir(Message::Interpelle);
            c.envoyer("Quelle est la date de la démo ?".into(), None, true);
            let autre = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Fin {
                tour: autre,
                texte: "La démo est jeudi.".into(),
            }));
            c.calme_a = c.calme_a.map(|(_, e)| (Instant::now(), e));
            c.tick();
            assert_eq!(c.activite(), Some("propose"));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 1,
                accepte: Some(true),
            }));
            assert_eq!(c.activite(), Some("reflechit"));
            c.tick();
            assert_eq!(c.activite(), Some("reflechit"), "son tour travaille");
            c.recevoir(Message::Cerveau(Retour::Resultat {
                tour,
                texte: "Le ticket est créé.".into(),
            }));
            for _ in 0..3 {
                c.livrable_a = None;
                c.tick();
            }
            assert!(c.a_livrer.is_empty());
            assert_eq!(c.activite(), Some("repond"));
        });
    }

    /// KOB-396 : deux cartes de la même tâche, la première acceptée, la
    /// dernière refusée : la tâche continue, sa patience remise n'est pas
    /// oubliée et sera dite si elle tarde. Toutes refusées, elle l'est.
    #[test]
    fn patience_gardee_quand_une_carte_a_ete_acceptee() {
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            let carte = |tour, carte| {
                Message::Cerveau(Retour::Carte {
                    tour,
                    carte,
                    outil: "mcp__linear__save_issue".into(),
                    entree: serde_json::json!({"title": "Corriger la connexion"}),
                })
            };
            let fermee = |carte, accepte| {
                Message::Cerveau(Retour::CarteFermee {
                    carte,
                    accepte: Some(accepte),
                })
            };
            c.recevoir(Message::Interpelle);
            c.envoyer("Écris deux notes".into(), None, true);
            let tour = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            c.recevoir(carte(tour, 1));
            c.recevoir(fermee(1, true));
            c.recevoir(carte(tour, 2));
            c.recevoir(Message::Cerveau(Retour::Patience { tour }));
            assert!(c.patience_reportee.is_some(), "remise : une carte attend");
            c.recevoir(fermee(2, false));
            assert!(
                c.patience_reportee.is_some(),
                "la première carte acceptée : la tâche continue"
            );
            // La tâche tarde : sa patience est dite.
            c.patience_reportee = Some((tour, Some(Instant::now() - cerveau::PATIENCE)));
            c.tick();
            assert!(c.patience_reportee.is_none());
            assert!(c.annoncees.contains(&tour), "« Je m'en occupe » dit");
        });
        // Une seule carte, refusée : la patience est oubliée.
        let (app, _dir) = chef_en_essai();
        avec_chef(&app, |c| {
            c.recevoir(Message::Interpelle);
            c.envoyer("Écris une note".into(), None, true);
            let tour = c.dernier;
            c.recevoir(Message::Cerveau(Retour::Detache {
                tour,
                annonce: true,
            }));
            c.recevoir(Message::Cerveau(Retour::Carte {
                tour,
                carte: 1,
                outil: "mcp__linear__save_issue".into(),
                entree: serde_json::json!({"title": "Corriger la connexion"}),
            }));
            c.recevoir(Message::Cerveau(Retour::Patience { tour }));
            c.recevoir(Message::Cerveau(Retour::CarteFermee {
                carte: 1,
                accepte: Some(false),
            }));
            assert!(c.patience_reportee.is_none());
        });
    }
}

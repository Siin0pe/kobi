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
use kobi_session::stockage::{maintenant_ms, CompteRendu, Qui};
use kobi_stt::Segment;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::cerveau::{self, Cerveau, Demande, Fabrique, Retour, MESSAGE_ANNONCE};
use crate::ecrivain::{Ecrivain, Nouvelle};
use crate::intentions::{intention, Intention};
use crate::interpellation::{EVENEMENT_ERREUR, EVENEMENT_QUESTION, EVENEMENT_REPONSE};
use crate::session::{appliquer, etat};

/// État de l'enregistrement (contrat avec l'interface, KOB-137).
pub const EVENEMENT_ENREGISTREMENT: &str = "enregistrement";
/// Historique de la conversation (dernières bulles, KOB-133).
pub const EVENEMENT_CONVERSATION: &str = "conversation";
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
/// Un arrêt de la réunion à la voix ou par Claude s'annule pendant ce temps
/// (« reprends », KOB-165) : le compte rendu attend jusque-là.
pub const ANNULATION: Duration = Duration::from_secs(30);

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
    /// Phrase dite qui cite Kobi sans qu'on sache si elle lui parle
    /// (KOB-168) : Claude peut ne rien répondre, Kobi reste alors silencieux.
    QuestionPeutEtre(String),
    /// L'interpellation n'a pas été suivie d'une question.
    SansQuestion,
    /// Segment final de la transcription continue.
    Segment(Segment),
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
    /// Arrêt encore annulable par « reprends » (KOB-165, KOB-174) : REC en
    /// pause, rien n'est rédigé avant la confirmation.
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
    notes: String,
    derniere_note: Option<String>,
    /// Transcription pas encore passée aux notes.
    a_noter: String,
    /// Bloc envoyé, en attente de réponse (remis en tête s'il échoue).
    envoye: Option<String>,
    prochain_bloc: Instant,
    fin_demandee: bool,
    /// Arrêt annulable (KOB-165) : le compte rendu est demandé à cet
    /// instant, sauf « reprends » d'ici là.
    fin_differee: Option<Instant>,
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
    /// Tours abandonnés : ce qui en revient encore est ignoré.
    interrompus: HashSet<u64>,
    /// Carte de validation ouverte (KOB-156) : « oui » ou « non » lui répond.
    validation: Option<String>,
    /// La question en cours de traitement ne s'adresse peut-être pas à Kobi.
    peut_etre: bool,
    /// Tour d'une telle question : sa réponse n'est montrée qu'une fois
    /// finie, et une réponse « rien » le laisse silencieux (KOB-168).
    tour_peut_etre: Option<u64>,
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

/// Envoie un message au chef.
pub fn publier<R: Runtime>(app: &AppHandle<R>, message: Message) {
    if let Some(bus) = app.try_state::<Bus>() {
        let _ = bus.0.lock().map(|tx| tx.send(message));
    }
}

/// Lance le chef et l'acteur Claude ; renvoie le canal du chef.
pub fn installer<R: Runtime>(app: &AppHandle<R>, fabrique: Fabrique, intervalle_min: u8) {
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
        Cerveau::lancer(fabrique, sortie)
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
const SANS_REPONSE: &str = "Je n'ai encore rien répondu.";

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
            conversation: None,
            voix: None,
            suite_apres_voix: false,
            derniere: None,
            taches: BTreeMap::new(),
            interrompus: HashSet::new(),
            validation: None,
            peut_etre: false,
            tour_peut_etre: None,
        }
    }

    /// État de l'enregistrement.
    /// Dès la fin demandée, l'enregistrement est annoncé arrêté (REC
    /// s'éteint tout de suite, KOB-166), même si la réunion reste ouverte le
    /// temps d'écrire le compte rendu.
    pub fn enregistrement(&self) -> Enregistrement {
        match &self.reunion {
            None => Enregistrement::default(),
            Some(r) if r.fin_demandee && r.fin_differee.is_none() => Enregistrement::default(),
            Some(r) => Enregistrement {
                actif: true,
                pause: r.pause || r.fin_differee.is_some(),
                debut_ms: r.debut_ms,
                notes: compter_notes(&r.notes),
                titre: None,
                derniere_note: r.derniere_note.clone(),
                annulable: r.fin_differee.is_some(),
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
            Message::Question { texte, voix } => self.question(texte, voix),
            Message::QuestionPeutEtre(texte) => {
                self.peut_etre = true;
                self.question(texte, true);
                self.peut_etre = false;
            }
            Message::Segment(s) => self.segment(s),
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
                crate::voix::dire(&self.app, &format!("Rappel : {}", r.texte));
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
        if let Some(id) = self.validation.clone() {
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
                let _ = self.app.emit(EVENEMENT_REPONSE, confirmation);
                crate::voix::dire(&self.app, confirmation);
                self.tour += 1;
                self.ajouter(Bulle {
                    tour: self.tour,
                    question: texte,
                    reponse: confirmation.into(),
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
        if let Some(i) = intention(&texte).filter(|_| consigne.is_none()) {
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
    /// MCP de Kobi (KOB-165) : pas de carte, Kobi agit tout de suite, et un
    /// arrêt s'annule pendant [`ANNULATION`]. `annoncer` : Kobi affiche et
    /// dit la confirmation (Claude, lui, l'annonce dans sa réponse).
    pub fn piloter(&mut self, intention: Intention, annoncer: bool) -> String {
        let reponse = match intention {
            Intention::TerminerReunion => Some(self.arreter_annulable()),
            i => self.executer(i),
        };
        let Some(reponse) = reponse else {
            return String::new();
        };
        self.emettre_enregistrement();
        if annoncer {
            let _ = self.app.emit(EVENEMENT_REPONSE, &reponse);
            crate::voix::dire(&self.app, &reponse);
        }
        reponse
    }

    /// Un arrêt de la réunion peut-il encore s'annuler ?
    pub fn fin_annulable(&self) -> bool {
        self.reunion
            .as_ref()
            .is_some_and(|r| r.fin_differee.is_some())
    }

    fn executer(&mut self, intention: Intention) -> Option<String> {
        Some(match intention {
            Intention::DemarrerReunion => self.demarrer(),
            Intention::Pause => match &mut self.reunion {
                Some(r) if !r.pause => {
                    r.pause = true;
                    self.transition(&[Evenement::Pause]);
                    "Réunion en pause : je n'enregistre plus.".into()
                }
                Some(_) => "La réunion est déjà en pause.".into(),
                None => "Aucune réunion en cours.".into(),
            },
            Intention::Reprendre if self.fin_annulable() => self.annuler_fin(),
            Intention::Reprendre => match &mut self.reunion {
                Some(r) if r.pause => {
                    r.pause = false;
                    self.transition(&[Evenement::Reprendre]);
                    "J'enregistre de nouveau.".into()
                }
                Some(_) => "J'enregistre déjà.".into(),
                None => "Aucune réunion en cours.".into(),
            },
            Intention::TerminerReunion => self.terminer(),
            Intention::Interrompre => {
                self.interrompre();
                return None;
            }
            Intention::OuvrirCarnet => match crate::carnet::ouvrir(&self.app) {
                Ok(()) => "J'ouvre le Carnet.".into(),
                Err(e) => {
                    tracing::warn!(erreur = %e, "carnet");
                    format!("Je n'arrive pas à ouvrir le Carnet : {e}")
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
                "J'écris encore le compte rendu de la réunion : un instant.".into()
            } else {
                "J'enregistre déjà la réunion.".into()
            };
        }
        let Some(stockage) = crate::stockage::actuel(&self.app) else {
            return "Dossier personnel introuvable : impossible d'enregistrer.".into();
        };
        let debut = Instant::now();
        let debut_ms = maintenant_ms();
        self.ecrivain.ouvrir_reunion(stockage, debut_ms);
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
            fin_differee: None,
        });
        "C'est parti : j'enregistre la réunion et je prends des notes.".into()
    }

    fn terminer(&mut self) -> String {
        let Some(r) = &mut self.reunion else {
            return "Aucune réunion en cours.".into();
        };
        if r.fin_demandee {
            // Arrêt annulable en cours : le raccourci ou le menu le confirment.
            if r.fin_differee.is_some() {
                self.confirmer_fin();
                return "Réunion terminée : j'écris le compte rendu.".into();
            }
            return "J'écris déjà le compte rendu.".into();
        }
        r.fin_demandee = true;
        self.transition(&[Evenement::TerminerReunion]);
        self.emettre_enregistrement();
        self.demander_compte_rendu();
        "Réunion terminée : j'écris le compte rendu.".into()
    }

    /// Arrêt à la voix ou par Claude (KOB-165) : l'enregistrement s'arrête
    /// tout de suite, le compte rendu attend [`ANNULATION`] ; « reprends »
    /// d'ici là reprend la même réunion.
    fn arreter_annulable(&mut self) -> String {
        let Some(r) = &mut self.reunion else {
            return "Aucune réunion en cours.".into();
        };
        if r.fin_demandee {
            return "J'ai déjà arrêté l'enregistrement.".into();
        }
        r.fin_demandee = true;
        r.fin_differee = Some(Instant::now() + ANNULATION);
        // Pendant l'annulation, la session reste en réunion (KOB-174) : le
        // micro reste ouvert pour entendre « reprends » (en pause, il est
        // coupé), et rien n'est rédigé. Les segments ne sont plus notés, et
        // l'enregistrement est annoncé arrêté et annulable.
        // « Reprends » s'entend sans redire « Kobi » pendant l'annulation.
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Annulable);
        tracing::info!("réunion arrêtée, annulable");
        "J'ai arrêté l'enregistrement. Dis « reprends » si c'était une erreur.".into()
    }

    /// Fin confirmée après un arrêt annulable (délai écoulé, raccourci ou
    /// menu) : la réunion passe en fin, et le compte rendu part.
    fn confirmer_fin(&mut self) {
        let Some(r) = &mut self.reunion else { return };
        r.fin_differee = None;
        self.transition(&[Evenement::TerminerReunion]);
        self.emettre_enregistrement();
        self.demander_compte_rendu();
    }

    /// « Reprends » pendant l'annulation : la réunion continue, rien n'est
    /// perdu de ce qui restait à noter.
    fn annuler_fin(&mut self) -> String {
        let Some(r) = &mut self.reunion else {
            return "Aucune réunion en cours.".into();
        };
        r.fin_demandee = false;
        r.fin_differee = None;
        // La session n'a pas quitté la réunion (ni sa pause) : rien à changer
        // côté état.
        tracing::info!("arrêt de la réunion annulé");
        "J'enregistre de nouveau : la réunion continue.".into()
    }

    /// Envoie à Claude la demande de compte rendu de la réunion.
    fn demander_compte_rendu(&mut self) {
        let Some(r) = &mut self.reunion else { return };
        let transcript = std::mem::take(&mut r.a_noter);
        let notes = r.notes.clone();
        self.cerveau
            .demander(Demande::CompteRendu { transcript, notes });
    }

    /// Segment final : écrit tout de suite dans `transcript.jsonl`.
    fn segment(&mut self, mut s: Segment) {
        let Some(r) = &mut self.reunion else { return };
        if r.pause || r.fin_demandee || !s.is_final {
            return;
        }
        // Horodaté sur la réunion : l'horloge du moteur repart à chaque question.
        let duree = (s.end - s.start).max(0.0);
        s.end = r.debut.elapsed().as_secs_f64();
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
            SANS_REPONSE.into()
        } else {
            self.avancement()
        }
    }

    /// Avancement des tâches longues.
    fn avancement(&self) -> String {
        if self.taches.is_empty() {
            return "Aucune tâche en cours.".into();
        }
        let taches: Vec<String> = self
            .taches
            .values()
            .map(|(debut, q)| format!("« {q} », depuis {} s", debut.elapsed().as_secs()))
            .collect();
        format!("Je travaille encore sur {}.", taches.join(" ; "))
    }

    /// Annule la dernière tâche longue lancée.
    fn annuler(&mut self) -> String {
        let Some((&tour, (_, question))) = self.taches.iter().next_back() else {
            return "Aucune tâche à annuler.".into();
        };
        let question = question.clone();
        self.taches.remove(&tour);
        self.interrompus.insert(tour);
        self.cerveau.demander(Demande::Annuler { tour });
        if let Some(b) = self.bulle(tour) {
            b.statut = "fini";
            b.reponse = "Tâche annulée.".into();
        }
        self.emettre_historique();
        format!("J'ai annulé « {question} ».")
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

    /// Une action qui écrit attend un « oui » : Kobi passe en proposition,
    /// relit la demande si la voix est active, et écoute la réponse sans
    /// qu'on redise « Kobi » (hors réunion).
    fn validation_ouverte(&mut self, id: String, resume: &str) {
        self.validation = Some(id);
        self.calme_a = None;
        self.transition(&[Evenement::PermissionDemandee]);
        crate::voix::dire(&self.app, &format!("{resume}. Je le fais ?"));
        let _ = crate::ecoute::commander(&self.app, crate::ecoute::Commande::Suite);
    }

    fn validation_fermee(&mut self, id: &str, reponse: crate::validation::Reponse) {
        if self.validation.as_deref() == Some(id) {
            self.validation = None;
        }
        self.transition(&[if reponse.acceptee() {
            Evenement::PermissionAccordee
        } else {
            Evenement::PermissionRefusee
        }]);
        if reponse == crate::validation::Reponse::Delai {
            let message = "Pas de réponse : je n'ai rien fait.";
            let _ = self.app.emit(EVENEMENT_REPONSE, message);
            crate::voix::dire(&self.app, message);
        }
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
                        if b.reponse.is_empty() {
                            b.reponse = MESSAGE_ANNONCE.into();
                        }
                    }
                    if tour == self.dernier {
                        let _ = self.app.emit(EVENEMENT_REPONSE, MESSAGE_ANNONCE);
                        // L'annonce remplace ce qui se disait du tour.
                        if self.voix.as_ref().is_some_and(|(t, _)| *t == tour) {
                            self.voix = None;
                        }
                        crate::voix::dire(&self.app, MESSAGE_ANNONCE);
                        self.lire(MESSAGE_ANNONCE);
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
                self.taches.remove(&tour);
                self.retenir(tour, &texte);
                self.ranger(Qui::Kobi, &texte, false);
                let question = self.bulle(tour).map(|b| {
                    b.reponse.clone_from(&texte);
                    b.statut = "fini";
                    b.question.clone()
                });
                self.emettre_historique();
                // Livré dans la bulle si Kobi n'est pas en train d'écouter.
                if matches!(
                    self.activite(),
                    Some("calme" | "fait" | "reflechit" | "repond" | "occupe") | None
                ) {
                    if let Some(q) = question {
                        let _ = self.app.emit(EVENEMENT_QUESTION, &q);
                    }
                    self.dernier = tour;
                    self.transition(&[Evenement::Interpellation]);
                    self.transition(&[Evenement::QuestionEnvoyee, Evenement::ReponseCommence]);
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                    crate::voix::dire(&self.app, &texte);
                    self.lire(&texte);
                }
            }
            Retour::Erreur { tour, message } => {
                tracing::warn!(tour, %message, "tour en erreur");
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
            Retour::Relance { tour } => self.patienter(tour, crate::pannes::RELANCE),
            Retour::Patience { tour } => self.patienter(tour, crate::pannes::PATIENCE),
            Retour::EnAttente { tour, nombre } => {
                let (titre, explication) =
                    crate::pannes::expliquer(&crate::pannes::Panne::HorsLigne, "");
                if let Some(b) = self.bulle(tour) {
                    b.reponse = format!(
                        "En attente du réseau ({nombre} question{}).",
                        if nombre > 1 { "s" } else { "" }
                    );
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
        let Some(r) = self.reunion.take() else {
            return;
        };
        let texte = match resultat {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(erreur = %e, "compte rendu en échec : notes gardées");
                format!("# Compte rendu\n\nLe compte rendu n'a pas pu être écrit ({e}).\n\n## Notes\n\n{}", r.notes)
            }
        };
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
                    }
                    (_, Err(e)) => tracing::warn!(erreur = %e, "compte rendu illisible"),
                    _ => {}
                }
                self.tour += 1;
                self.ajouter(Bulle {
                    tour: self.tour,
                    question: "Fin de la réunion".into(),
                    reponse: format!("Compte rendu écrit : {}", chemin.display()),
                    statut: "fini",
                    clavier: false,
                    etapes: Vec::new(),
                });
            }
            Nouvelle::Erreur(message) => {
                tracing::warn!(%message, "écriture sur le disque");
                self.erreur("Enregistrement sur le disque", &message);
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
            None => return format!("{maintenant}
(Aucune réunion en cours.)
{question}"),
            Some(r) if r.fin_demandee => "arrêté, compte rendu à venir",
            Some(r) if r.pause => "en pause",
            Some(_) => "en cours",
        };
        format!("{maintenant}\n(Réunion : enregistrement {etat}.)\n{question}")
    }

    fn fermer_conversation(&mut self) {
        if self.conversation.take().is_some() {
            self.ecrivain.fermer_conversation();
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

    /// Tour qui tarde ou relancé : la bulle le dit, et Kobi aussi si c'est
    /// la question en cours (KOB-68).
    fn patienter(&mut self, tour: u64, phrase: &str) {
        tracing::info!(tour, phrase, "tour : patience");
        if let Some(b) = self.bulle(tour) {
            if b.reponse.is_empty() || b.reponse == MESSAGE_ANNONCE || b.statut == "tache" {
                b.reponse = phrase.into();
            }
        }
        self.emettre_historique();
        if tour == self.dernier {
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
        let Some(r) = &mut self.reunion else { return };
        // Arrêt annulable : sans « reprends » à temps, le compte rendu part.
        if r.fin_differee.is_some_and(|quand| Instant::now() >= quand) {
            tracing::info!("arrêt confirmé : compte rendu demandé");
            self.confirmer_fin();
            return;
        }
        if r.pause || r.fin_demandee || r.envoye.is_some() || Instant::now() < r.prochain_bloc {
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
}

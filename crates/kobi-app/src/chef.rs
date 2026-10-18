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
use kobi_session::stockage::{maintenant_ms, Conversation, Qui, ReunionOuverte};
use kobi_stt::Segment;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::cerveau::{self, Cerveau, Demande, Fabrique, Retour};
use crate::intentions::{intention, Intention};
use crate::interpellation::{EVENEMENT_ERREUR, EVENEMENT_QUESTION, EVENEMENT_REPONSE};
use crate::session::{appliquer, etat};

/// État de l'enregistrement (contrat avec l'interface, KOB-137).
pub const EVENEMENT_ENREGISTREMENT: &str = "enregistrement";
/// Historique de la conversation (dernières bulles, KOB-133).
pub const EVENEMENT_CONVERSATION: &str = "conversation";
/// Carte de confirmation (bulle d'action).
pub const EVENEMENT_CARTE: &str = "carte";

/// Premier bloc de notes après ce délai, puis toutes les `interval_min`.
const PREMIER_BLOC: Duration = Duration::from_secs(60);
/// Temps de lecture minimal d'une réponse avant le retour au calme.
const LECTURE_MIN: Duration = Duration::from_secs(5);
/// Temps d'affichage d'une erreur.
const ERREUR_AFFICHEE: Duration = Duration::from_secs(6);
/// Bulles gardées dans l'historique.
const HISTORIQUE: usize = 12;

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
    Cerveau(Retour),
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

/// Fichiers de la réunion, créés par un fil à part : l'état « réunion » se
/// voit tout de suite, même quand le disque est lent.
enum Fichiers {
    /// En création ; les segments arrivés entre-temps attendent.
    EnCreation(Receiver<std::io::Result<ReunionOuverte>>, Vec<Segment>),
    Ouverts(ReunionOuverte),
    Echec(String),
}

impl Fichiers {
    /// Les fichiers s'ils sont prêts ; avec `attendre`, jusqu'à la fin de
    /// leur création.
    fn prets(&mut self, attendre: bool) -> Option<&mut ReunionOuverte> {
        if let Fichiers::EnCreation(rx, attente) = self {
            let cree = if attendre {
                rx.recv().map_err(|_| mpsc::TryRecvError::Disconnected)
            } else {
                rx.try_recv()
            };
            match cree {
                Err(mpsc::TryRecvError::Empty) => return None,
                Ok(Ok(mut reunion)) => {
                    for segment in attente.drain(..) {
                        if let Err(e) = reunion.ecrire_segment(&segment) {
                            tracing::warn!(erreur = %e, "transcript.jsonl");
                        }
                    }
                    *self = Fichiers::Ouverts(reunion);
                }
                Ok(Err(e)) => *self = Fichiers::Echec(e.to_string()),
                Err(mpsc::TryRecvError::Disconnected) => {
                    *self = Fichiers::Echec("création interrompue".into())
                }
            }
        }
        match self {
            Fichiers::Ouverts(reunion) => Some(reunion),
            _ => None,
        }
    }
}

struct EnCours {
    fichiers: Fichiers,
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
}

/// Le chef, partagé entre son fil et les commandes de l'interface.
pub struct Chef<R: Runtime> {
    app: AppHandle<R>,
    cerveau: Cerveau,
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
    /// Conversation du quotidien en cours, rangée par le stockage (KOB-141).
    conversation: Option<Conversation>,
    /// Tours abandonnés : ce qui en revient encore est ignoré.
    interrompus: HashSet<u64>,
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
    let sortie_tx = Mutex::new(tx);
    let sortie: cerveau::Sortie = Arc::new(move |retour| {
        let _ = sortie_tx.lock().map(|tx| tx.send(Message::Cerveau(retour)));
    });
    let cerveau = {
        let _guard = rt.enter();
        Cerveau::lancer(fabrique, sortie)
    };
    app.manage(ChefPartage(Mutex::new(Chef::new(
        app.clone(),
        cerveau,
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
    fn new(app: AppHandle<R>, cerveau: Cerveau, intervalle: Duration) -> Self {
        Self {
            app,
            cerveau,
            reunion: None,
            intervalle,
            tour: 0,
            dernier: 0,
            historique: Vec::new(),
            cartes: BTreeMap::new(),
            pour_conversation: String::new(),
            calme_a: None,
            conversation: None,
            interrompus: HashSet::new(),
        }
    }

    /// État de l'enregistrement.
    pub fn enregistrement(&self) -> Enregistrement {
        match &self.reunion {
            None => Enregistrement::default(),
            Some(r) => Enregistrement {
                actif: true,
                pause: r.pause,
                debut_ms: r.debut_ms,
                notes: compter_notes(&r.notes),
                titre: match &r.fichiers {
                    Fichiers::Ouverts(f) => f.meta().titre.clone(),
                    _ => None,
                },
                derniere_note: r.derniere_note.clone(),
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
            Message::Segment(s) => self.segment(s),
            Message::Commande(i) => {
                self.commande(i);
            }
            Message::Decision { carte, accepte } => {
                self.cerveau.demander(Demande::Decision { carte, accepte })
            }
            Message::Interrompre => self.interrompre(),
            Message::Cerveau(r) => self.retour(r),
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
        if intention(&texte) == Some(Intention::Interrompre) {
            self.interrompre();
            return;
        }
        if let Some(i) = intention(&texte) {
            tracing::info!(intention = ?i, "ordre vocal");
            self.transition(&[Evenement::PasDeQuestion]);
            let reponse = self.commande(i);
            self.tour += 1;
            self.ajouter(Bulle {
                tour: self.tour,
                question: texte,
                reponse: reponse.clone(),
                statut: "fini",
                clavier: !voix,
            });
            return;
        }
        self.tour += 1;
        self.dernier = self.tour;
        self.calme_a = None;
        self.transition(&[Evenement::QuestionEnvoyee]);
        self.ranger(Qui::Moi, &texte, voix);
        let notes = self
            .reunion
            .as_ref()
            .map(|r| r.notes.clone())
            .unwrap_or_default();
        self.cerveau.demander(Demande::Question {
            tour: self.tour,
            question: texte.clone(),
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
        });
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
        if self.conversation.is_none() {
            let Some(stockage) = crate::stockage::actuel(&self.app) else {
                return;
            };
            self.conversation = Some(stockage.nouvelle_conversation(maintenant_ms()));
        }
        if let Some(c) = &mut self.conversation {
            if let Err(e) = c.ajouter(qui, texte, voix) {
                tracing::warn!(erreur = %e, "conversation non rangée");
            }
        }
    }

    fn ajouter(&mut self, bulle: Bulle) {
        self.historique.push(bulle);
        let trop = self.historique.len().saturating_sub(HISTORIQUE);
        self.historique.drain(..trop);
        self.emettre_historique();
    }

    /// Ordre (voix, menu ou interface) ; renvoie la phrase de confirmation.
    pub fn commande(&mut self, intention: Intention) -> String {
        let reponse = match intention {
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
                return String::new();
            }
        };
        self.emettre_enregistrement();
        let _ = self.app.emit(EVENEMENT_REPONSE, &reponse);
        reponse
    }

    fn demarrer(&mut self) -> String {
        if self.reunion.is_some() {
            return "J'enregistre déjà la réunion.".into();
        }
        let Some(stockage) = crate::stockage::actuel(&self.app) else {
            return "Dossier personnel introuvable : impossible d'enregistrer.".into();
        };
        let debut = Instant::now();
        let debut_ms = maintenant_ms();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(stockage.nouvelle_reunion(debut_ms, None));
        });
        // Depuis un échange hors réunion, on revient d'abord en veille.
        if etat(&self.app).etat == "assistant" {
            self.transition(&[Evenement::PasDeQuestion]);
            self.transition(&[Evenement::ReponseTerminee]);
        }
        self.transition(&[Evenement::DemarrerReunion]);
        // La conversation du quotidien s'arrête là ; la réunion prend le relais.
        self.conversation = None;
        tracing::info!(ms = debut.elapsed().as_millis() as u64, "réunion démarrée");
        self.reunion = Some(EnCours {
            fichiers: Fichiers::EnCreation(rx, Vec::new()),
            debut,
            debut_ms,
            pause: false,
            notes: String::new(),
            derniere_note: None,
            a_noter: String::new(),
            envoye: None,
            prochain_bloc: debut + PREMIER_BLOC,
            fin_demandee: false,
        });
        "C'est parti : j'enregistre la réunion et je prends des notes.".into()
    }

    fn terminer(&mut self) -> String {
        let Some(r) = &mut self.reunion else {
            return "Aucune réunion en cours.".into();
        };
        if r.fin_demandee {
            return "J'écris déjà le compte rendu.".into();
        }
        r.fin_demandee = true;
        let transcript = std::mem::take(&mut r.a_noter);
        let notes = r.notes.clone();
        self.transition(&[Evenement::TerminerReunion]);
        self.cerveau
            .demander(Demande::CompteRendu { transcript, notes });
        "Réunion terminée : j'écris le compte rendu.".into()
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
        match r.fichiers.prets(false) {
            Some(f) => {
                if let Err(e) = f.ecrire_segment(&s) {
                    tracing::warn!(erreur = %e, "transcript.jsonl");
                }
            }
            None => {
                if let Fichiers::EnCreation(_, attente) = &mut r.fichiers {
                    attente.push(s.clone());
                }
            }
        }
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

    fn retour(&mut self, retour: Retour) {
        let tour_du_retour = match &retour {
            Retour::Texte { tour, .. }
            | Retour::Fin { tour, .. }
            | Retour::Detache { tour, .. }
            | Retour::Resultat { tour, .. }
            | Retour::Erreur { tour, .. } => Some(*tour),
            _ => None,
        };
        if tour_du_retour.is_some_and(|t| self.interrompus.contains(&t)) {
            return;
        }
        match retour {
            Retour::Texte { tour, texte } => {
                if let Some(b) = self.bulle(tour) {
                    b.reponse.clone_from(&texte);
                    b.statut = "reponse";
                }
                if tour == self.dernier {
                    self.transition(&[Evenement::ReponseCommence]);
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                }
                self.emettre_historique();
            }
            Retour::Fin { tour, texte } => {
                tracing::info!(tour, "réponse terminée");
                self.ranger(Qui::Kobi, &texte, false);
                if let Some(b) = self.bulle(tour) {
                    b.reponse.clone_from(&texte);
                    b.statut = "fini";
                }
                if tour == self.dernier {
                    self.transition(&[Evenement::ReponseCommence]);
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                    self.lire(&texte);
                }
                self.emettre_historique();
            }
            Retour::Detache { tour } => {
                tracing::info!(tour, "tour détaché");
            }
            Retour::Resultat { tour, texte } => {
                tracing::info!(tour, "résultat livré");
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
                    Some("calme" | "fait" | "reflechit" | "repond") | None
                ) {
                    if let Some(q) = question {
                        let _ = self.app.emit(EVENEMENT_QUESTION, &q);
                    }
                    self.dernier = tour;
                    self.transition(&[Evenement::Interpellation]);
                    self.transition(&[Evenement::QuestionEnvoyee, Evenement::ReponseCommence]);
                    let _ = self.app.emit(EVENEMENT_REPONSE, &texte);
                    self.lire(&texte);
                }
            }
            Retour::Erreur { tour, message } => {
                tracing::warn!(tour, %message, "tour en erreur");
                if let Some(b) = self.bulle(tour) {
                    b.reponse.clone_from(&message);
                    b.statut = "erreur";
                }
                self.emettre_historique();
                if tour == self.dernier {
                    self.erreur("Claude n'a pas répondu", &message);
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
            Retour::Notes(resultat) => self.notes(resultat),
            Retour::CompteRendu(resultat) => self.compte_rendu(resultat),
        }
    }

    fn notes(&mut self, resultat: Result<String, String>) {
        let Some(r) = &mut self.reunion else { return };
        let envoye = r.envoye.take().unwrap_or_default();
        match resultat {
            Ok(notes) if !notes.trim().is_empty() => {
                r.derniere_note = derniere_note(&r.notes, &notes).or(r.derniere_note.take());
                r.notes = notes;
                if let Some(f) = r.fichiers.prets(false) {
                    if let Err(e) = f.ecrire_notes(&r.notes) {
                        tracing::warn!(erreur = %e, "notes.md");
                    }
                }
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
        let Some(mut r) = self.reunion.take() else {
            return;
        };
        let texte = match resultat {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(erreur = %e, "compte rendu en échec : notes gardées");
                format!("# Compte rendu\n\nLe compte rendu n'a pas pu être écrit ({e}).\n\n## Notes\n\n{}", r.notes)
            }
        };
        let notes = r.notes.clone();
        let Some(f) = r.fichiers.prets(true) else {
            tracing::warn!("compte rendu non rangé : fichiers de la réunion absents");
            self.transition(&[Evenement::CompteRenduEcrit]);
            self.emettre_enregistrement();
            return;
        };
        let chemin: PathBuf = f.dossier().join("compte-rendu.md");
        if let Err(e) = f.ecrire_compte_rendu(&texte) {
            tracing::warn!(erreur = %e, "compte-rendu.md");
        }
        if let Err(e) = f.ecrire_notes(&notes) {
            tracing::warn!(erreur = %e, "notes.md");
        }
        if let Err(e) = f.terminer(maintenant_ms()) {
            tracing::warn!(erreur = %e, "reunion.json");
        }
        tracing::info!(fichier = %chemin.display(), "compte rendu écrit");
        self.transition(&[Evenement::CompteRenduEcrit]);
        self.emettre_enregistrement();
        let message = format!("Compte rendu écrit : {}", chemin.display());
        self.tour += 1;
        self.ajouter(Bulle {
            tour: self.tour,
            question: "Fin de la réunion".into(),
            reponse: message,
            statut: "fini",
            clavier: false,
        });
    }

    /// Kobi a fini de répondre : temps de lecture, puis retour au calme.
    fn lire(&mut self, texte: &str) {
        let lecture = LECTURE_MIN + Duration::from_millis(60 * texte.chars().count() as u64);
        self.calme_a = Some((
            Instant::now() + lecture.min(Duration::from_secs(25)),
            Evenement::ReponseTerminee,
        ));
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
        if let Some((quand, evenement)) = self.calme_a {
            if Instant::now() >= quand {
                self.calme_a = None;
                self.transition(&[evenement]);
            }
        }
        // Retour en veille : la prochaine question ouvre une autre conversation.
        if self.conversation.is_some() && etat(&self.app).etat == "veille" {
            self.conversation = None;
        }
        let Some(r) = &mut self.reunion else { return };
        if let Fichiers::EnCreation(..) = r.fichiers {
            if let Some(f) = r.fichiers.prets(false) {
                tracing::info!(
                    dossier = %f.dossier().display(),
                    ms = r.debut.elapsed().as_millis() as u64,
                    "fichiers de la réunion prêts"
                );
            } else if let Fichiers::Echec(e) = &r.fichiers {
                let message = format!("Je n'arrive pas à créer le dossier de la réunion : {e}");
                tracing::warn!(%message, "réunion sans fichiers");
                self.erreur("Réunion non enregistrée", &message);
                return;
            }
        }
        let Some(r) = &mut self.reunion else { return };
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

/// Sessions Claude Code réelles, en lecture seule (KOB-138) : Read, Grep et
/// Glob sur les `codebases`, sans MCP ni serveur de permission, modèle
/// rapide, prompt système de Kobi.
pub fn fabrique_claude(modele: &'static str, codebases: Vec<PathBuf>) -> Fabrique {
    let args = Arc::new(crate::lecture::arguments(
        modele,
        kobi_brain::SYSTEM_PROMPT,
        &codebases,
    ));
    let programme: Arc<tokio::sync::OnceCell<Result<PathBuf, String>>> = Arc::default();
    Arc::new(move |role| {
        let (args, programme) = (args.clone(), programme.clone());
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
            let cwd = kobi_session::config::kobi_dir();
            if let Some(cwd) = &cwd {
                let _ = std::fs::create_dir_all(cwd);
            }
            let config = kobi_brain::MeetingConfig {
                program: programme,
                cwd,
                fixed_args: Some(args.as_ref().clone()),
                ..Default::default()
            };
            let debut = Instant::now();
            let session = kobi_brain::ClaudeCodeBrain::start(config).await?;
            tracing::info!(
                ?role,
                ms = debut.elapsed().as_millis() as u64,
                "session Claude ouverte"
            );
            Ok(Box::new(session) as Box<dyn kobi_brain::Brain + Send>)
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compteur_de_notes() {
        let notes = "## Décisions\n- budget validé\n- séminaire à Annecy\n\n## Actions\n* Sophie : devis\ntexte libre\n";
        assert_eq!(compter_notes(notes), 3);
        assert_eq!(compter_notes(""), 0);
    }

    #[test]
    fn segments_gardes_pendant_la_creation_des_fichiers() {
        let dir = tempfile::tempdir().unwrap();
        let stockage = kobi_session::stockage::Stockage::new(dir.path());
        let (tx, rx) = mpsc::channel();
        let segment = |t: f64, texte: &str| Segment {
            text: texte.into(),
            start: t,
            end: t + 1.0,
            is_final: true,
            source: kobi_stt::Source::Moi,
        };
        let mut fichiers = Fichiers::EnCreation(rx, Vec::new());
        assert!(fichiers.prets(false).is_none());
        if let Fichiers::EnCreation(_, attente) = &mut fichiers {
            attente.push(segment(0.0, "on commence"));
        }
        tx.send(stockage.nouvelle_reunion(1_000, None)).unwrap();
        let f = fichiers.prets(false).expect("fichiers ouverts");
        f.ecrire_segment(&segment(2.0, "le budget")).unwrap();
        let transcript = std::fs::read_to_string(f.dossier().join("transcript.jsonl")).unwrap();
        let textes: Vec<_> = transcript
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str::<Segment>(l).unwrap().text)
            .collect();
        assert_eq!(textes, ["on commence", "le budget"]);
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

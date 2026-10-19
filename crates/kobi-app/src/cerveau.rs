//! Acteur Claude (KOB-134, KOB-135) : interpellations, notes et compte rendu
//! sont des tours séparés qui ne bloquent ni l'écoute ni l'enregistrement.
//!
//! Deux sessions tournent en parallèle, chacune avec sa file :
//!
//! * la **conversation** répond aux questions, une à la fois, dans l'ordre.
//!   Un tour qui n'a rien écrit au bout de [`ANNONCE`], ou qui appelle un
//!   outil, devient une tâche longue : Kobi l'annonce, le tour continue en
//!   arrière-plan sur sa session, et la conversation passe sur une session
//!   de réserve déjà chaude. Une carte de confirmation détache aussi le
//!   tour : la conversation continue pendant que la carte attend son clic.
//! * les **notes** mettent à jour les notes de la réunion par blocs et
//!   écrivent le compte rendu.
//!
//! Tout ce qui revient (texte en flux, fin, cartes, notes) part par
//! [`Retour`] vers le chef d'orchestre, qui seul touche à l'état et à
//! l'interface.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kobi_brain::{
    Action, ApprovalRequest, Ask, Brain, BrainError, BrainEvent, DemandeOutil, NotesUpdate,
    ReportRequest, Turn,
};
use kobi_session::stockage::maintenant_ms;
use tokio::sync::mpsc;

use crate::pannes::{classer, Panne};

/// Sans texte après ce délai (ou dès un appel d'outil), le tour est annoncé
/// comme tâche longue.
pub const ANNONCE: Duration = Duration::from_secs(4);
/// Délai maximal d'un bloc de notes ou du compte rendu.
const NOTES_MAX: Duration = Duration::from_secs(180);
/// Message affiché à l'annonce d'une tâche longue.
pub const MESSAGE_ANNONCE: &str = "Je m'en occupe, je te donne le résultat dès qu'il est prêt.";
/// Tour détaché toujours sans texte après ce délai : « je cherche encore »
/// (KOB-68).
pub const PATIENCE: Duration = Duration::from_secs(8);
/// Un tour sans aucune activité (texte, outil) pendant ce délai est
/// abandonné et sa session fermée (KOB-68).
pub const ABANDON: Duration = Duration::from_secs(45);
/// Hors ligne, la première question en attente est reposée à ce rythme.
pub const REESSAI_RESEAU: Duration = Duration::from_secs(20);
/// Outils internes de Claude Code, qui ne font pas d'un tour une tâche.
const OUTILS_INTERNES: &[&str] = &["ToolSearch", "TodoWrite"];

/// Rôle d'une session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Conversation,
    Notes,
}

type Session = Box<dyn Brain + Send>;
/// Ouvre une session Claude (ou une doublure de test).
pub type Fabrique = Arc<
    dyn Fn(Role) -> Pin<Box<dyn Future<Output = Result<Session, BrainError>> + Send>> + Send + Sync,
>;

/// Ce qu'on demande au cerveau.
#[derive(Debug, Clone, PartialEq)]
pub enum Demande {
    Question {
        tour: u64,
        question: String,
        /// Transcription que la conversation n'a pas encore vue.
        transcript: String,
        notes: String,
    },
    Notes {
        transcript: String,
        notes: String,
    },
    CompteRendu {
        transcript: String,
        notes: String,
    },
    /// Clic sur une carte de confirmation.
    Decision {
        carte: u64,
        accepte: bool,
    },
    /// « Stop » ou Échap : le tour est abandonné, la conversation continue
    /// sur une autre session.
    Interrompre {
        tour: u64,
    },
    /// « Annule » : la tâche longue s'arrête et sa session est fermée.
    Annuler {
        tour: u64,
    },
    /// Les dossiers de code ont changé (KOB-153) : les sessions (et la
    /// réserve) sont rouvertes ; la question suivante les utilise. Les tours
    /// détachés finissent sur leur session.
    Renouveler,
}

/// Ce que le cerveau renvoie.
#[derive(Debug, Clone, PartialEq)]
pub enum Retour {
    /// Texte de la réponse, accumulé.
    Texte {
        tour: u64,
        texte: String,
    },
    /// Fin d'une réponse ordinaire.
    Fin {
        tour: u64,
        texte: String,
    },
    /// Le tour est devenu une tâche longue (annoncée) ou attend une carte
    /// (détaché sans annonce) ; il continue en arrière-plan.
    Detache {
        tour: u64,
        annonce: bool,
    },
    /// Fin d'un tour détaché.
    Resultat {
        tour: u64,
        texte: String,
    },
    Erreur {
        tour: u64,
        message: String,
    },
    /// Claude a planté pendant le tour : session relancée, question renvoyée.
    Relance {
        tour: u64,
    },
    /// Le tour tarde : « je cherche encore ».
    Patience {
        tour: u64,
    },
    /// Pas d'Internet : la question attend le retour du réseau.
    EnAttente {
        tour: u64,
        /// Questions en attente, celle-ci comprise.
        nombre: usize,
    },
    /// Carte de confirmation à afficher.
    Carte {
        tour: u64,
        carte: u64,
        outil: String,
        entree: serde_json::Value,
    },
    /// Outil appelé par le tour (fil d'activité, KOB-152), reçu à `at_ms`
    /// (ms depuis l'époque Unix).
    Activite {
        demande: Box<DemandeOutil>,
        at_ms: u64,
    },
    /// Carte décidée, ou expirée (`accepte = None`).
    CarteFermee {
        carte: u64,
        accepte: Option<bool>,
    },
    Notes(Result<String, String>),
    CompteRendu(Result<String, String>),
}

/// Destination des retours.
pub type Sortie = Arc<dyn Fn(Retour) + Send + Sync>;

type Cartes = Arc<Mutex<HashMap<u64, ApprovalRequest>>>;
/// Tâches longues en arrière-plan, par tour.
type Taches = Arc<Mutex<HashMap<u64, tokio::task::AbortHandle>>>;

/// Poignée de l'acteur : on lui envoie des [`Demande`].
#[derive(Clone)]
pub struct Cerveau {
    questions: mpsc::UnboundedSender<Demande>,
    arrets: mpsc::UnboundedSender<u64>,
    notes: mpsc::UnboundedSender<Demande>,
    cartes: Cartes,
    taches: Taches,
    sortie: Sortie,
}

impl Cerveau {
    /// Lance les deux sessions sur le runtime courant.
    pub fn lancer(fabrique: Fabrique, sortie: Sortie) -> Self {
        let cartes: Cartes = Arc::default();
        let taches: Taches = Arc::default();
        let (questions, rx_q) = mpsc::unbounded_channel();
        let (notes, rx_n) = mpsc::unbounded_channel();
        let (arrets, rx_a) = mpsc::unbounded_channel();
        tokio::spawn(conversation(
            fabrique.clone(),
            rx_q,
            rx_a,
            sortie.clone(),
            cartes.clone(),
            taches.clone(),
        ));
        tokio::spawn(prise_de_notes(fabrique, rx_n, sortie.clone()));
        Self {
            questions,
            arrets,
            notes,
            cartes,
            taches,
            sortie,
        }
    }

    pub fn demander(&self, demande: Demande) {
        match demande {
            Demande::Decision { carte, accepte } => {
                let attente = self
                    .cartes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&carte);
                let accepte = attente.map(|a| {
                    if a.is_abandoned() {
                        return None;
                    }
                    if accepte {
                        a.allow();
                    } else {
                        a.deny("L'utilisateur a refusé.");
                    }
                    Some(accepte)
                });
                (self.sortie)(Retour::CarteFermee {
                    carte,
                    accepte: accepte.flatten(),
                });
            }
            Demande::Interrompre { tour } => {
                let _ = self.arrets.send(tour);
            }
            Demande::Annuler { tour } => {
                let tache = self
                    .taches
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .remove(&tour);
                tracing::info!(tour, existait = tache.is_some(), "tâche annulée");
                // La tâche et sa session tombent : le process `claude` est tué.
                if let Some(t) = tache {
                    t.abort();
                }
            }
            Demande::Renouveler => {
                let _ = self.questions.send(Demande::Renouveler);
                let _ = self.notes.send(Demande::Renouveler);
            }
            d @ Demande::Question { .. } => {
                let _ = self.questions.send(d);
            }
            d => {
                let _ = self.notes.send(d);
            }
        }
    }
}

/// Numéro des cartes, unique pour toute l'application.
fn nouvelle_carte() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(1);
    N.fetch_add(1, Ordering::SeqCst)
}

/// Session prête, ouverte en avance pour ne pas faire attendre.
type Reserve = tokio::task::JoinHandle<Result<Session, BrainError>>;

fn reserve(fabrique: &Fabrique) -> Reserve {
    tokio::spawn(fabrique(Role::Conversation))
}

/// Une question, telle qu'on peut la reposer (plantage, réseau).
#[derive(Debug, Clone)]
struct Question {
    tour: u64,
    ask: Ask,
    /// Déjà relancée après un plantage : un second plantage est une erreur.
    relancee: bool,
    /// Relancée : résumé des derniers échanges, que la session neuve n'a pas vus.
    resume: Option<String>,
}

/// Échanges gardés pour le résumé d'une relance.
const ECHANGES: usize = 5;
/// Longueur gardée de chaque question et réponse dans ce résumé.
const ECHANGE_MAX: usize = 300;

fn couper(texte: &str) -> String {
    let t: String = texte.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > ECHANGE_MAX {
        format!("{}…", t.chars().take(ECHANGE_MAX).collect::<String>())
    } else {
        t
    }
}

/// La question à reposer sur une session neuve après un plantage, avec le
/// résumé des derniers échanges (KOB-68).
fn relancee(q: Question, echanges: &[(String, String)]) -> Question {
    let resume = (!echanges.is_empty()).then(|| {
        let lignes: Vec<String> = echanges
            .iter()
            .map(|(question, reponse)| {
                format!("- Question : {}\n  Réponse : {}", couper(question), couper(reponse))
            })
            .collect();
        format!(
            "[Claude a été relancé après un arrêt. Derniers échanges de cette conversation, pour le contexte :\n{}]",
            lignes.join("\n")
        )
    });
    Question {
        relancee: true,
        resume,
        ..q
    }
}

/// Retient un échange fini pour une éventuelle relance.
fn retenir(echanges: &mut VecDeque<(String, String)>, question: &str, reponse: String) {
    echanges.push_back((question.to_owned(), reponse));
    while echanges.len() > ECHANGES {
        echanges.pop_front();
    }
}

/// Contexte d'un tour détaché, pour le reposer s'il plante ou s'il perd le
/// réseau.
#[derive(Clone)]
struct Reprise {
    fabrique: Fabrique,
    question: Question,
    /// Vers la file d'attente hors ligne de la conversation.
    attente: mpsc::UnboundedSender<Question>,
    /// Derniers échanges, pour le résumé d'une relance.
    echanges: Vec<(String, String)>,
}

async fn conversation(
    fabrique: Fabrique,
    mut rx: mpsc::UnboundedReceiver<Demande>,
    mut arrets: mpsc::UnboundedReceiver<u64>,
    sortie: Sortie,
    cartes: Cartes,
    taches: Taches,
) {
    let mut session: Option<Session> = None;
    let mut suivante = reserve(&fabrique);
    // Hors ligne (KOB-68) : questions gardées dans l'ordre, la première est
    // reposée toutes les `REESSAI_RESEAU` ; dès qu'elle passe, les suivantes
    // partent aussitôt.
    let mut en_attente: VecDeque<Question> = VecDeque::new();
    let (attente_tx, mut attente_rx) = mpsc::unbounded_channel::<Question>();
    let mut reessai =
        tokio::time::interval_at(tokio::time::Instant::now() + REESSAI_RESEAU, REESSAI_RESEAU);
    reessai.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // Question à traiter tout de suite (relance après un plantage, suite de
    // la file d'attente).
    let mut prochaine: Option<Question> = None;
    let mut echanges: VecDeque<(String, String)> = VecDeque::new();
    loop {
        let q = if let Some(q) = prochaine.take() {
            q
        } else {
            tokio::select! {
                demande = rx.recv() => match demande {
                    Some(Demande::Question { tour, question, transcript, notes }) => {
                        let q = Question {
                            tour,
                            ask: Ask { question, transcript, notes },
                            relancee: false,
                            resume: None,
                        };
                        if en_attente.is_empty() {
                            q
                        } else {
                            // Le réseau manque encore : elle prend son tour.
                            mettre_en_attente(&mut en_attente, q, false, &sortie);
                            reessai_depuis(&mut reessai, &en_attente);
                            continue;
                        }
                    }
                    Some(Demande::Renouveler) => {
                        // Dossiers de code changés (KOB-153) : sessions neuves.
                        session = None;
                        suivante.abort();
                        suivante = reserve(&fabrique);
                        tracing::info!("sessions Claude renouvelées");
                        continue;
                    }
                    Some(_) => continue,
                    None => break,
                },
                Some(q) = attente_rx.recv() => {
                    mettre_en_attente(&mut en_attente, q, true, &sortie);
                    reessai_depuis(&mut reessai, &en_attente);
                    continue;
                }
                _ = reessai.tick(), if !en_attente.is_empty() => {
                    tracing::info!(attente = en_attente.len(), "réseau : question reposée");
                    en_attente.pop_front().expect("file non vide")
                }
            }
        };
        let tour = q.tour;
        // Le délai d'annonce court dès la question, ouverture de session comprise.
        let annonce = tokio::time::Instant::now() + ANNONCE;
        if session.is_none() {
            let mut prete = std::mem::replace(&mut suivante, reserve(&fabrique));
            let ouverte = tokio::select! {
                ouverte = &mut prete => Some(ouverte),
                () = tokio::time::sleep_until(annonce) => None,
            };
            let Some(ouverte) = ouverte else {
                // Claude tarde à démarrer : la question est annoncée et part
                // en arrière-plan dès que sa session est prête ; la
                // conversation continue sur la réserve suivante.
                sortie(Retour::Detache {
                    tour,
                    annonce: true,
                });
                let reprise = Reprise {
                    fabrique: fabrique.clone(),
                    question: q.clone(),
                    attente: attente_tx.clone(),
                    echanges: echanges.iter().cloned().collect(),
                };
                let (sortie, cartes) = (sortie.clone(), cartes.clone());
                detacher(&taches, tour, async move {
                    match prete
                        .await
                        .unwrap_or_else(|e| Err(BrainError::Crashed(e.to_string())))
                    {
                        Ok(s) => {
                            // La session vit jusqu'à la fin du tour.
                            let mut s = Some(s);
                            let turn = poser(&mut s, &reprise.question);
                            finir(turn, String::new(), reprise, &sortie, &cartes).await;
                        }
                        Err(e) => echec_d_ouverture(e, reprise.question, &reprise.attente, &sortie),
                    }
                });
                continue;
            };
            match ouverte.unwrap_or_else(|e| Err(BrainError::Crashed(e.to_string()))) {
                Ok(s) => session = Some(s),
                Err(e) => {
                    echec_d_ouverture(e, q, &attente_tx, &sortie);
                    continue;
                }
            }
        }
        let turn = poser(&mut session, &q);
        // Les arrêts reçus entre deux tours visaient un tour déjà fini.
        while arrets.try_recv().is_ok() {}
        match suivre(tour, turn, annonce, &sortie, &cartes, &mut arrets).await {
            Suivi::Fini(reponse) => {
                if let Some(reponse) = reponse {
                    retenir(&mut echanges, &q.ask.question, reponse);
                }
                // Le réseau est revenu : les questions en attente partent.
                prochaine = en_attente.pop_front();
            }
            Suivi::Erreur(e) => {
                let casse = !matches!(e, BrainError::Api(_));
                if casse {
                    session = None;
                }
                match classer(&e) {
                    Panne::Plantage if !q.relancee => {
                        tracing::warn!(tour, erreur = %e, "Claude a planté : session relancée, question renvoyée");
                        sortie(Retour::Relance { tour });
                        let echanges: Vec<_> = echanges.iter().cloned().collect();
                        prochaine = Some(relancee(q, &echanges));
                    }
                    Panne::HorsLigne => {
                        mettre_en_attente(&mut en_attente, q, true, &sortie);
                        reessai_depuis(&mut reessai, &en_attente);
                    }
                    _ => sortie(Retour::Erreur {
                        tour,
                        message: e.to_string(),
                    }),
                }
            }
            Suivi::Interrompu(turn) => {
                // La session est fermée avec son tour (le process `claude`
                // s'arrête) ; la conversation continue sur la réserve.
                drop(turn);
                session = None;
            }
            Suivi::Detache(turn, texte) => {
                // Le tour garde sa session jusqu'au bout ; la conversation
                // continue sur la réserve.
                let detachee = session.take();
                let reprise = Reprise {
                    fabrique: fabrique.clone(),
                    question: q,
                    attente: attente_tx.clone(),
                    echanges: echanges.iter().cloned().collect(),
                };
                let (sortie, cartes) = (sortie.clone(), cartes.clone());
                detacher(&taches, tour, async move {
                    let _session = detachee;
                    finir(turn, texte, reprise, &sortie, &cartes).await;
                });
            }
        }
    }
}

/// Lance une tâche longue en arrière-plan, annulable par son tour.
fn detacher(taches: &Taches, tour: u64, tache: impl Future<Output = ()> + Send + 'static) {
    let liste = taches.clone();
    let mut taches = taches.lock().unwrap_or_else(|e| e.into_inner());
    // Inscrite avant de démarrer : la fin de la tâche la retire.
    let poignee = tokio::spawn(async move {
        tache.await;
        liste
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&tour);
    });
    taches.insert(tour, poignee.abort_handle());
}

/// Pose la question sur la session (ouverte), précédée du résumé si elle
/// est relancée.
fn poser(session: &mut Option<Session>, q: &Question) -> Turn {
    let mut ask = q.ask.clone();
    if let Some(resume) = &q.resume {
        ask.question = format!("{resume}\n\n{}", ask.question);
    }
    session.as_mut().expect("session ouverte").ask(ask)
}

/// La file d'attente vient de se remplir : le prochain essai est dans
/// `REESSAI_RESEAU`, pas tout de suite (le minuteur a pu prendre de l'avance).
fn reessai_depuis(reessai: &mut tokio::time::Interval, en_attente: &VecDeque<Question>) {
    if en_attente.len() == 1 {
        reessai.reset();
    }
}

/// Hors ligne : la question attend, et l'interface le sait.
fn mettre_en_attente(
    en_attente: &mut VecDeque<Question>,
    q: Question,
    en_tete: bool,
    sortie: &Sortie,
) {
    let tour = q.tour;
    if en_tete {
        en_attente.push_front(q);
    } else {
        en_attente.push_back(q);
    }
    tracing::warn!(
        tour,
        attente = en_attente.len(),
        "pas d'Internet : question en attente"
    );
    sortie(Retour::EnAttente {
        tour,
        nombre: en_attente.len(),
    });
}

/// La session n'a pas pu s'ouvrir : hors ligne, la question attend ; sinon
/// l'erreur part vers l'interface (Claude absent, déconnecté…).
fn echec_d_ouverture(
    e: BrainError,
    q: Question,
    attente: &mpsc::UnboundedSender<Question>,
    sortie: &Sortie,
) {
    if classer(&e) == Panne::HorsLigne {
        let _ = attente.send(q);
    } else {
        sortie(Retour::Erreur {
            tour: q.tour,
            message: e.to_string(),
        });
    }
}

enum Suivi {
    /// Fini, avec la réponse s'il y en a une.
    Fini(Option<String>),
    /// Le tour a échoué ; l'appelant décide (relance, attente, erreur).
    Erreur(BrainError),
    /// Tâche longue ou carte : le reste du tour et le texte déjà reçu.
    Detache(Turn, String),
    /// Abandonné par l'utilisateur : le reste du tour, à ignorer.
    Interrompu(Turn),
}

/// Suit un tour au premier plan jusqu'à sa fin ou son détachement.
async fn suivre(
    tour: u64,
    mut turn: Turn,
    annonce: tokio::time::Instant,
    sortie: &Sortie,
    cartes: &Cartes,
    arrets: &mut mpsc::UnboundedReceiver<u64>,
) -> Suivi {
    let mut texte = String::new();
    let delai = tokio::time::sleep_until(annonce);
    tokio::pin!(delai);
    // Sans aucune activité pendant `ABANDON`, le tour est abandonné.
    let inactif = tokio::time::sleep(ABANDON);
    tokio::pin!(inactif);
    loop {
        tokio::select! {
            evenement = turn.next() => {
                inactif.as_mut().reset(tokio::time::Instant::now() + ABANDON);
                match evenement {
                    Some(BrainEvent::Text(t)) => {
                        texte.push_str(&t);
                        sortie(Retour::Texte { tour, texte: texte.clone() });
                    }
                    Some(BrainEvent::ToolCall { id, name, action, .. }) if !interne(&action) => {
                        journaliser_outil(&name);
                        activite(tour, id, action, sortie);
                        sortie(Retour::Detache { tour, annonce: true });
                        return Suivi::Detache(turn, texte);
                    }
                    Some(BrainEvent::ToolCall { .. }) => {}
                    Some(BrainEvent::Permission(demande)) => {
                        carte(tour, demande, sortie, cartes);
                        sortie(Retour::Detache { tour, annonce: false });
                        return Suivi::Detache(turn, texte);
                    }
                    Some(BrainEvent::Done(fin)) => {
                        let texte = if fin.text.is_empty() { texte } else { fin.text };
                        sortie(Retour::Fin { tour, texte: texte.clone() });
                        return Suivi::Fini(Some(texte));
                    }
                    Some(BrainEvent::Error(e)) => return Suivi::Erreur(e),
                    None => return Suivi::Fini(None),
                }
            },
            Some(arret) = arrets.recv() => {
                if arret == tour {
                    tracing::info!(tour, "tour interrompu");
                    return Suivi::Interrompu(turn);
                }
            }
            () = &mut delai, if texte.is_empty() => {
                sortie(Retour::Detache { tour, annonce: true });
                return Suivi::Detache(turn, texte);
            }
            () = &mut inactif => {
                tracing::warn!(tour, "tour sans activité : abandon");
                return Suivi::Erreur(BrainError::Timeout(ABANDON));
            }
        }
    }
}

/// Termine un tour détaché : ses cartes s'affichent, son résultat est livré.
/// Toujours sans texte après `PATIENCE` : « je cherche encore » ; sans
/// aucune activité pendant `ABANDON` : abandon. S'il plante, la question est
/// reposée une fois sur une session neuve ; hors ligne, elle part en attente.
async fn finir(
    mut turn: Turn,
    mut texte: String,
    reprise: Reprise,
    sortie: &Sortie,
    cartes: &Cartes,
) {
    let tour = reprise.question.tour;
    let patience = tokio::time::sleep(PATIENCE.saturating_sub(ANNONCE));
    tokio::pin!(patience);
    let mut patient = !texte.is_empty();
    let inactif = tokio::time::sleep(ABANDON);
    tokio::pin!(inactif);
    loop {
        let evenement = tokio::select! {
            e = turn.next() => e,
            () = &mut patience, if !patient => {
                patient = true;
                sortie(Retour::Patience { tour });
                continue;
            }
            () = &mut inactif => {
                tracing::warn!(tour, "tâche sans activité : abandon");
                sortie(Retour::Erreur { tour, message: BrainError::Timeout(ABANDON).to_string() });
                return;
            }
        };
        inactif
            .as_mut()
            .reset(tokio::time::Instant::now() + ABANDON);
        let Some(evenement) = evenement else { return };
        match evenement {
            BrainEvent::Text(t) => {
                patient = true;
                texte.push_str(&t);
            }
            BrainEvent::Permission(demande) => carte(tour, demande, sortie, cartes),
            BrainEvent::ToolCall {
                id, name, action, ..
            } => {
                journaliser_outil(&name);
                if !interne(&action) {
                    activite(tour, id, action, sortie);
                }
            }
            BrainEvent::Done(fin) => {
                let texte = if fin.text.is_empty() { texte } else { fin.text };
                sortie(Retour::Resultat { tour, texte });
                return;
            }
            BrainEvent::Error(e) => match classer(&e) {
                Panne::Plantage if !reprise.question.relancee => {
                    tracing::warn!(tour, erreur = %e, "tâche : Claude a planté, question renvoyée");
                    sortie(Retour::Relance { tour });
                    match (reprise.fabrique)(Role::Conversation).await {
                        Ok(s) => {
                            let mut s = Some(s);
                            let mut reprise = reprise;
                            reprise.question = relancee(reprise.question, &reprise.echanges);
                            let turn = poser(&mut s, &reprise.question);
                            return Box::pin(finir_relance(turn, reprise, sortie, cartes, s)).await;
                        }
                        Err(e) => {
                            sortie(Retour::Erreur {
                                tour,
                                message: e.to_string(),
                            });
                            return;
                        }
                    }
                }
                Panne::HorsLigne => {
                    let _ = reprise.attente.send(reprise.question.clone());
                    return;
                }
                _ => {
                    sortie(Retour::Erreur {
                        tour,
                        message: e.to_string(),
                    });
                    return;
                }
            },
        }
    }
}

/// Outil interne du harness (liste de tâches, recherche d'outils) : ni tâche
/// longue, ni ligne du fil d'activité.
fn interne(action: &Action) -> bool {
    matches!(action, Action::Autre { nom, .. } if OUTILS_INTERNES.contains(&nom.as_str()))
}

fn activite(tour: u64, id: String, action: Action, sortie: &Sortie) {
    sortie(Retour::Activite {
        demande: Box::new(DemandeOutil { id, tour, action }),
        at_ms: maintenant_ms(),
    });
}

/// Suite d'un tour détaché relancé : même suivi, la session neuve vit
/// jusqu'au bout.
async fn finir_relance(
    turn: Turn,
    reprise: Reprise,
    sortie: &Sortie,
    cartes: &Cartes,
    _session: Option<Session>,
) {
    finir(turn, String::new(), reprise, sortie, cartes).await;
}

/// Chaque outil appelé est journalisé : seuls Read, Grep, Glob (KOB-138),
/// le pilotage de la réunion (KOB-165) et les outils MCP (KOB-155) doivent
/// apparaître.
fn journaliser_outil(outil: &str) {
    if crate::lecture::permis(outil) {
        tracing::info!(%outil, "outil de lecture");
    } else if kobi_ctl::mcp::outils_de_reunion(crate::mcp_config::SERVEUR_KOBI)
        .iter()
        .any(|o| o == outil)
    {
        tracing::info!(%outil, "pilotage de la réunion par Claude");
    } else if outil.starts_with("mcp__") {
        tracing::info!(%outil, "outil MCP");
    } else if !OUTILS_INTERNES.contains(&outil) {
        tracing::warn!(%outil, "outil hors lecture seule demandé par Claude");
    }
}

fn carte(tour: u64, demande: ApprovalRequest, sortie: &Sortie, cartes: &Cartes) {
    // Hors MCP, Kobi ne fait que lire, et seulement dans les dossiers de
    // code : un Read ailleurs est refusé sans carte (KOB-138, KOB-155).
    if !demande.request.tool_name.starts_with("mcp__") {
        tracing::info!(outil = %demande.request.tool_name, "refusé sans carte : hors MCP");
        demande.deny("Hors des outils MCP, Kobi ne fait que lire dans les dossiers de code.");
        return;
    }
    let carte = nouvelle_carte();
    let (outil, entree) = (
        demande.request.tool_name.clone(),
        demande.request.input.clone(),
    );
    cartes
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(carte, demande);
    sortie(Retour::Carte {
        tour,
        carte,
        outil,
        entree,
    });
}

async fn prise_de_notes(
    fabrique: Fabrique,
    mut rx: mpsc::UnboundedReceiver<Demande>,
    sortie: Sortie,
) {
    let mut session: Option<Session> = None;
    while let Some(demande) = rx.recv().await {
        if demande == Demande::Renouveler {
            session = None;
            continue;
        }
        if session.is_none() {
            match fabrique(Role::Notes).await {
                Ok(s) => session = Some(s),
                Err(e) => {
                    let e = Err(e.to_string());
                    sortie(match demande {
                        Demande::CompteRendu { .. } => Retour::CompteRendu(e),
                        _ => Retour::Notes(e),
                    });
                    continue;
                }
            }
        }
        let s = session.as_mut().expect("session ouverte");
        let (turn, compte_rendu) = match demande {
            Demande::Notes { transcript, notes } => {
                (s.update_notes(NotesUpdate { transcript, notes }), false)
            }
            Demande::CompteRendu { transcript, notes } => (
                s.final_report(ReportRequest {
                    transcript,
                    notes,
                    pending_actions: Vec::new(),
                }),
                true,
            ),
            _ => continue,
        };
        let resultat = turn.wait_timeout(NOTES_MAX).await;
        if matches!(
            resultat,
            Err(BrainError::Crashed(_) | BrainError::Timeout(_))
        ) {
            session = None;
        }
        let resultat = resultat.map(|r| r.text).map_err(|e| e.to_string());
        sortie(if compte_rendu {
            Retour::CompteRendu(resultat)
        } else {
            Retour::Notes(resultat)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kobi_brain::fake::{FakeBrain, Step};
    use kobi_brain::PermissionRequest;
    use std::time::Instant;

    /// Fabrique de doublures : chaque session ouverte prend le script
    /// suivant de la liste.
    fn fabrique(scripts: Vec<Vec<Vec<Step>>>) -> Fabrique {
        let scripts = Arc::new(Mutex::new(std::collections::VecDeque::from(scripts)));
        Arc::new(move |_role| {
            let script = scripts.lock().unwrap().pop_front().unwrap_or_default();
            Box::pin(async move {
                let mut b = FakeBrain::new();
                for tour in script {
                    b = b.then(tour);
                }
                Ok(Box::new(b) as Session)
            })
        })
    }

    type Recus = Arc<Mutex<Vec<(Instant, Retour)>>>;

    fn sortie() -> (Sortie, Recus) {
        let recus = Arc::new(Mutex::new(Vec::new()));
        let r = recus.clone();
        (
            Arc::new(move |retour| r.lock().unwrap().push((Instant::now(), retour))),
            recus,
        )
    }

    fn question(tour: u64, texte: &str) -> Demande {
        Demande::Question {
            tour,
            question: texte.into(),
            transcript: String::new(),
            notes: String::new(),
        }
    }

    /// Attend un retour, 60 s au plus. Sous temps figé (`start_paused`), ce
    /// temps est virtuel : il doit couvrir les tâches longues simulées.
    async fn attendre(recus: &Recus, ok: impl Fn(&Retour) -> bool) {
        for _ in 0..2400 {
            if recus.lock().unwrap().iter().any(|(_, r)| ok(r)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("attendu, reçu : {:?}", recus.lock().unwrap());
    }

    /// Fabrique dont l'ouverture échoue (Claude absent, déconnecté).
    fn fabrique_en_echec(e: BrainError) -> Fabrique {
        Arc::new(move |_role| {
            let e = e.clone();
            Box::pin(async move { Err(e) })
        })
    }

    fn recu(recus: &Recus, ok: impl Fn(&Retour) -> bool) -> bool {
        recus.lock().unwrap().iter().any(|(_, r)| ok(r))
    }

    /// Ordre des retours, pour vérifier les enchaînements.
    fn rang(recus: &Recus, ok: impl Fn(&Retour) -> bool) -> usize {
        recus
            .lock()
            .unwrap()
            .iter()
            .position(|(_, r)| ok(r))
            .expect("retour attendu")
    }

    // ----- KOB-68 : un test par panne -----

    /// Plantage : la session est relancée et la question renvoyée.
    #[tokio::test]
    async fn plantage_session_relancee_et_question_renvoyee() {
        let (s, recus) = sortie();
        let plante = vec![vec![Step::Fail(BrainError::Crashed(
            "exit status 3".into(),
        ))]];
        let c = Cerveau::lancer(fabrique(vec![plante]), s);
        c.demander(question(1, "un"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 1, .. })).await;
        assert!(recu(&recus, |r| *r == Retour::Relance { tour: 1 }));
        assert!(recu(&recus, |r| *r
            == Retour::Fin {
                tour: 1,
                texte: "reçu : un".into()
            }));
        assert!(!recu(&recus, |r| matches!(r, Retour::Erreur { .. })));
    }

    /// Plantage après un premier échange : la session neuve reçoit le résumé
    /// des échanges précédents avec la question.
    #[tokio::test]
    async fn plantage_relance_avec_le_resume() {
        let (s, recus) = sortie();
        let premiere = vec![
            vec![Step::Text("Dans api/auth.rs, ligne 88.".into())],
            vec![Step::Fail(BrainError::Crashed("exit status 3".into()))],
        ];
        let c = Cerveau::lancer(fabrique(vec![premiere]), s);
        c.demander(question(1, "où est géré le refresh token ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 1, .. })).await;
        c.demander(question(2, "et côté mobile ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        assert!(recu(&recus, |r| *r == Retour::Relance { tour: 2 }));
        let texte = recus
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, r)| match r {
                Retour::Fin { tour: 2, texte } => Some(texte.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            texte,
            "reçu : [Claude a été relancé après un arrêt. Derniers échanges de cette conversation, pour le contexte :\n\
             - Question : où est géré le refresh token ?\n  Réponse : Dans api/auth.rs, ligne 88.]\n\n\
             et côté mobile ?"
        );
    }

    /// Deux plantages de suite : erreur, et la question suivante passe.
    #[tokio::test]
    async fn deux_plantages_erreur_puis_kobi_repond_encore() {
        let (s, recus) = sortie();
        let plante = || {
            vec![vec![Step::Fail(BrainError::Crashed(
                "exit status 3".into(),
            ))]]
        };
        let c = Cerveau::lancer(fabrique(vec![plante(), plante()]), s);
        c.demander(question(1, "un"));
        attendre(&recus, |r| matches!(r, Retour::Erreur { tour: 1, .. })).await;
        c.demander(question(2, "deux"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        let message = recus
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, r)| match r {
                Retour::Erreur { message, .. } => Some(message.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            crate::pannes::pour_l_utilisateur(&message).0,
            "Claude s'est arrêté"
        );
    }

    /// Lenteur : annonce, puis « je cherche encore » à 8 s, puis le résultat.
    #[tokio::test(start_paused = true)]
    async fn lenteur_je_cherche_encore_a_8_s() {
        let (s, recus) = sortie();
        let debut = tokio::time::Instant::now();
        let quand = Arc::new(Mutex::new(Vec::new()));
        let q = quand.clone();
        let sortie: Sortie = Arc::new(move |r: Retour| {
            q.lock().unwrap().push((debut.elapsed(), r.clone()));
            s(r)
        });
        let lent = vec![vec![
            Step::Wait(Duration::from_secs(20)),
            Step::Text("trouvé".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![lent]), sortie);
        c.demander(question(1, "longue"));
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 1, .. })).await;
        let quand = quand.lock().unwrap();
        let a = |ok: &dyn Fn(&Retour) -> bool| quand.iter().find(|(_, r)| ok(r)).unwrap().0;
        let annonce = a(&|r| matches!(r, Retour::Detache { annonce: true, .. }));
        let patience = a(&|r| *r == Retour::Patience { tour: 1 });
        assert!(annonce >= ANNONCE && annonce < PATIENCE, "{annonce:?}");
        assert!(
            patience >= PATIENCE && patience < PATIENCE + Duration::from_secs(1),
            "{patience:?}"
        );
        assert!(recu(&recus, |r| *r
            == Retour::Resultat {
                tour: 1,
                texte: "trouvé".into()
            }));
    }

    /// Lenteur : rien pendant 45 s, la question est abandonnée.
    #[tokio::test(start_paused = true)]
    async fn lenteur_abandon_a_45_s_sans_activite() {
        let (s, recus) = sortie();
        let muet = vec![vec![
            Step::Wait(Duration::from_secs(600)),
            Step::Text("trop tard".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![muet]), s);
        c.demander(question(1, "longue"));
        attendre(&recus, |r| matches!(r, Retour::Erreur { tour: 1, .. })).await;
        let message = recus
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, r)| match r {
                Retour::Erreur { message, .. } => Some(message.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(
            crate::pannes::pour_l_utilisateur(&message).0,
            "Claude ne répond pas"
        );
        assert!(
            rang(&recus, |r| matches!(r, Retour::Patience { .. }))
                < rang(&recus, |r| matches!(r, Retour::Erreur { .. }))
        );
        assert!(!recu(&recus, |r| matches!(r, Retour::Resultat { .. })));
    }

    /// Limite d'usage : erreur expliquée, la session reste utilisable.
    #[tokio::test]
    async fn limite_d_usage_expliquee() {
        let (s, recus) = sortie();
        let limite = vec![vec![Step::Fail(BrainError::Api(
            "Claude AI usage limit reached|1792339200".into(),
        ))]];
        let c = Cerveau::lancer(fabrique(vec![limite]), s);
        c.demander(question(1, "un"));
        attendre(&recus, |r| matches!(r, Retour::Erreur { tour: 1, .. })).await;
        let message = recus
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, r)| match r {
                Retour::Erreur { message, .. } => Some(message.clone()),
                _ => None,
            })
            .unwrap();
        let (titre, explication) = crate::pannes::pour_l_utilisateur(&message);
        assert_eq!(titre, "Limite d'usage atteinte");
        assert!(explication.contains("L'écoute et l'enregistrement continuent."));
        assert!(!recu(&recus, |r| matches!(
            r,
            Retour::Relance { .. } | Retour::EnAttente { .. }
        )));
        c.demander(question(2, "deux"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
    }

    /// Claude déconnecté : la cause et la marche à suivre.
    #[tokio::test]
    async fn claude_deconnecte_explique() {
        let (s, recus) = sortie();
        let c = Cerveau::lancer(
            fabrique_en_echec(BrainError::Unavailable(
                "Claude Code n'est pas connecté : lancer « claude auth login »".into(),
            )),
            s,
        );
        c.demander(question(1, "un"));
        attendre(&recus, |r| matches!(r, Retour::Erreur { tour: 1, .. })).await;
        let message = recus
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, r)| match r {
                Retour::Erreur { message, .. } => Some(message.clone()),
                _ => None,
            })
            .unwrap();
        let (titre, explication) = crate::pannes::pour_l_utilisateur(&message);
        assert_eq!(titre, "Claude Code n'est pas connecté");
        assert!(explication.contains("claude auth login"));
    }

    /// Pas d'Internet : les questions attendent, dans l'ordre, et partent au
    /// retour du réseau.
    #[tokio::test(start_paused = true)]
    async fn hors_ligne_questions_en_attente_puis_posees() {
        let (s, recus) = sortie();
        let coupure = || {
            vec![Step::Fail(BrainError::Api(
                "API Error: Connection error.".into(),
            ))]
        };
        // Deux échecs réseau, puis le réseau revient (réponses par défaut).
        let c = Cerveau::lancer(fabrique(vec![vec![coupure(), coupure()]]), s);
        c.demander(question(1, "un"));
        attendre(&recus, |r| {
            matches!(r, Retour::EnAttente { tour: 1, nombre: 1 })
        })
        .await;
        c.demander(question(2, "deux"));
        attendre(&recus, |r| {
            matches!(r, Retour::EnAttente { tour: 2, nombre: 2 })
        })
        .await;
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        assert!(
            rang(&recus, |r| matches!(r, Retour::Fin { tour: 1, .. }))
                < rang(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. }))
        );
        assert!(recu(&recus, |r| *r
            == Retour::Fin {
                tour: 1,
                texte: "reçu : un".into()
            }));
        assert!(!recu(&recus, |r| matches!(r, Retour::Erreur { .. })));
    }

    #[tokio::test]
    async fn questions_dans_l_ordre_et_texte_en_flux() {
        let (s, recus) = sortie();
        let c = Cerveau::lancer(fabrique(vec![]), s);
        c.demander(question(1, "un"));
        c.demander(question(2, "deux"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        let fins: Vec<_> = recus
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, r)| match r {
                Retour::Fin { tour, texte } => Some((*tour, texte.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(fins, [(1, "reçu : un".into()), (2, "reçu : deux".into())]);
        assert!(recus.lock().unwrap().iter().any(|(_, r)| *r
            == Retour::Texte {
                tour: 1,
                texte: "reçu : ".into()
            }));
    }

    #[tokio::test(start_paused = true)]
    async fn tache_longue_annoncee_et_kobi_reste_disponible() {
        let (s, recus) = sortie();
        // Première session : un tour qui travaille 30 s sans rien écrire.
        let longue = vec![vec![
            Step::Wait(Duration::from_secs(30)),
            Step::Text("résultat".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![longue]), s);
        c.demander(question(1, "lis la codebase"));
        c.demander(question(2, "quelle heure ?"));
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 1, .. })).await;
        let recus = recus.lock().unwrap();
        let position =
            |ok: &dyn Fn(&Retour) -> bool| recus.iter().position(|(_, r)| ok(r)).unwrap();
        let annonce = position(&|r| {
            *r == Retour::Detache {
                tour: 1,
                annonce: true,
            }
        });
        let reponse = position(&|r| matches!(r, Retour::Fin { tour: 2, .. }));
        let resultat = position(&|r| {
            *r == Retour::Resultat {
                tour: 1,
                texte: "résultat".into(),
            }
        });
        assert!(annonce < reponse && reponse < resultat, "{recus:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn session_lente_annoncee_a_temps() {
        let (s, recus) = sortie();
        let ouvertes = Arc::new(Mutex::new(0));
        let n = ouvertes.clone();
        // La première session met 20 s à s'ouvrir (Claude Code qui démarre).
        let fabrique: Fabrique = Arc::new(move |_role| {
            let lente = {
                let mut n = n.lock().unwrap();
                *n += 1;
                *n == 1
            };
            Box::pin(async move {
                if lente {
                    tokio::time::sleep(Duration::from_secs(20)).await;
                }
                Ok(Box::new(FakeBrain::new()) as Session)
            })
        });
        let c = Cerveau::lancer(fabrique, s);
        c.demander(question(1, "un"));
        c.demander(question(2, "deux"));
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 1, .. })).await;
        let recus = recus.lock().unwrap();
        let position =
            |ok: &dyn Fn(&Retour) -> bool| recus.iter().position(|(_, r)| ok(r)).unwrap();
        let annonce = position(&|r| {
            *r == Retour::Detache {
                tour: 1,
                annonce: true,
            }
        });
        let reponse = position(&|r| matches!(r, Retour::Fin { tour: 2, .. }));
        let resultat = position(&|r| matches!(r, Retour::Resultat { tour: 1, .. }));
        assert!(annonce < reponse && reponse < resultat, "{recus:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn tache_longue_annulee() {
        let (s, recus) = sortie();
        let longue = vec![vec![
            Step::Wait(Duration::from_secs(30)),
            Step::Text("résultat".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![longue]), s);
        c.demander(question(1, "lis toute la codebase"));
        attendre(&recus, |r| matches!(r, Retour::Detache { tour: 1, .. })).await;
        c.demander(Demande::Annuler { tour: 1 });
        // Kobi reste disponible, et le résultat de la tâche n'arrive jamais.
        c.demander(question(2, "quelle heure ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        tokio::time::sleep(Duration::from_secs(60)).await;
        assert!(
            !recus
                .lock()
                .unwrap()
                .iter()
                .any(|(_, r)| matches!(r, Retour::Resultat { tour: 1, .. })),
            "{:?}",
            recus.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn dossiers_changes_la_question_suivante_les_utilise() {
        let (s, recus) = sortie();
        // La fabrique lit les dossiers courants à l'ouverture de chaque
        // session, comme la vraie ; la doublure répond avec eux.
        let dossiers = Arc::new(Mutex::new("ancien".to_string()));
        let d = dossiers.clone();
        let fabrique: Fabrique = Arc::new(move |_role| {
            let lus = d.lock().unwrap().clone();
            Box::pin(async move {
                let b = FakeBrain::new().then(vec![Step::Text(format!("dossiers {lus}"))]);
                Ok(Box::new(b) as Session)
            })
        });
        let c = Cerveau::lancer(fabrique, s);
        c.demander(question(1, "où est fabrique_claude ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 1, .. })).await;
        // Changés pendant que Kobi tourne.
        *dossiers.lock().unwrap() = "nouveau".into();
        c.demander(Demande::Renouveler);
        c.demander(question(2, "et maintenant ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        let fins: Vec<_> = recus
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, r)| match r {
                Retour::Fin { texte, .. } => Some(texte.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(fins, ["dossiers ancien", "dossiers nouveau"]);
    }

    #[tokio::test]
    async fn carte_ne_fige_pas_la_conversation() {
        let (s, recus) = sortie();
        let action = vec![vec![
            Step::Permission(PermissionRequest {
                tool_name: "mcp__linear__save_issue".into(),
                input: serde_json::json!({"title": "Relancer le devis"}),
                tool_use_id: None,
            }),
            Step::Text("Ticket créé.".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![action]), s);
        c.demander(question(1, "crée un ticket"));
        attendre(&recus, |r| matches!(r, Retour::Carte { tour: 1, .. })).await;
        c.demander(question(2, "et sinon ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        let carte = recus
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, r)| match r {
                Retour::Carte { carte, outil, .. } => {
                    assert_eq!(outil, "mcp__linear__save_issue");
                    Some(*carte)
                }
                _ => None,
            })
            .unwrap();
        c.demander(Demande::Decision {
            carte,
            accepte: true,
        });
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 1, .. })).await;
        assert!(recus.lock().unwrap().iter().any(|(_, r)| *r
            == Retour::CarteFermee {
                carte,
                accepte: Some(true)
            }));
    }

    #[tokio::test]
    async fn chaque_outil_part_au_fil_d_activite() {
        let (s, recus) = sortie();
        let outil = |id: &str, name: &str, input: serde_json::Value| Step::ToolCall {
            id: id.into(),
            name: name.into(),
            input,
        };
        let travail = vec![vec![
            outil(
                "t1",
                "Grep",
                serde_json::json!({"pattern": "fabrique_claude"}),
            ),
            outil("t2", "TodoWrite", serde_json::json!({})),
            outil(
                "t3",
                "Read",
                serde_json::json!({"file_path": "C:/k/chef.rs"}),
            ),
            Step::Text("Dans chef.rs.".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![travail]), s);
        c.demander(question(1, "où est fabrique_claude ?"));
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 1, .. })).await;
        let recus = recus.lock().unwrap();
        let ids: Vec<_> = recus
            .iter()
            .filter_map(|(_, r)| match r {
                Retour::Activite { demande, at_ms } => {
                    assert_eq!(demande.tour, 1);
                    assert!(*at_ms > 0);
                    Some(demande.id.as_str())
                }
                _ => None,
            })
            .collect();
        // L'outil interne n'apparaît pas ; l'ordre est celui des appels.
        assert_eq!(ids, ["t1", "t3"]);
        let premiere = recus
            .iter()
            .position(|(_, r)| matches!(r, Retour::Activite { .. }))
            .unwrap();
        let annonce = recus
            .iter()
            .position(|(_, r)| matches!(r, Retour::Detache { annonce: true, .. }))
            .unwrap();
        assert!(premiere < annonce, "{recus:?}");
        assert!(recus.iter().any(|(_, r)| matches!(
            r,
            Retour::Activite { demande, .. }
                if demande.action == Action::LireFichier { chemin: "C:/k/chef.rs".into() }
        )));
    }

    #[tokio::test]
    async fn notes_en_parallele_des_questions() {
        let (s, recus) = sortie();
        let c = Cerveau::lancer(fabrique(vec![]), s);
        c.demander(Demande::Notes {
            transcript: "[00:10 moi] budget".into(),
            notes: "- budget".into(),
        });
        c.demander(question(1, "un"));
        attendre(&recus, |r| matches!(r, Retour::Notes(Ok(_)))).await;
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 1, .. })).await;
        c.demander(Demande::CompteRendu {
            transcript: String::new(),
            notes: "- budget".into(),
        });
        attendre(
            &recus,
            |r| matches!(r, Retour::CompteRendu(Ok(t)) if t.contains("budget")),
        )
        .await;
    }
}

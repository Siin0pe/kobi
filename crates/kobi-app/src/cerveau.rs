//! Acteur Claude (KOB-134, KOB-135) : interpellations, notes et compte rendu
//! sont des tours séparés qui ne bloquent ni l'écoute ni l'enregistrement.
//!
//! Deux sessions tournent en parallèle, chacune avec sa file :
//!
//! * la **conversation** répond aux questions, une à la fois, dans l'ordre.
//!   Un tour qui n'a rien écrit au bout de [`ANNONCE`], ou qui appelle un
//!   outil, devient une tâche longue (annoncée seulement si elle tarde,
//!   [`PATIENCE`]) : le tour continue en
//!   arrière-plan sur sa session, et la conversation passe sur une session
//!   de réserve déjà chaude. Une carte de confirmation détache aussi le
//!   tour : la conversation continue pendant que la carte attend son clic.
//!   Fini, le tour rend sa session à la conversation, qui la reprend avec
//!   son contexte. Une question qui suit de près ([`RAFALE`]) une tâche
//!   encore en cours l'attend ([`ATTENTE_RAFALE`] au plus) et reprend sa
//!   session : une rafale ne fait plus une session et une tâche détachée
//!   par question (KOB-238).
//! * les **notes** mettent à jour les notes de la réunion par blocs et
//!   écrivent le compte rendu.
//!
//! Tout ce qui revient (texte en flux, fin, cartes, notes) part par
//! [`Retour`] vers le chef d'orchestre, qui seul touche à l'état et à
//! l'interface.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
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
/// Tour détaché toujours sans texte après ce délai : Kobi dit, une seule
/// fois, qu'il s'en occupe (KOB-68). Avant, Kobi au travail se voit et ne dit
/// rien : la plupart des tâches finissent avant (KOB-215).
pub const PATIENCE: Duration = Duration::from_secs(15);
/// Un tour sans aucun signe de vie pendant ce délai est abandonné et sa
/// session fermée (KOB-68). Un outil pas encore revenu ou une carte en
/// attente ne comptent jamais comme inactivité (KOB-243). Réglable :
/// `taches.abandon_s`.
pub const ABANDON: Duration = Duration::from_secs(300);
/// Durée maximale d'un tour, même actif (KOB-243). Réglable :
/// `taches.duree_max_s`.
pub const DUREE_MAX: Duration = Duration::from_secs(15 * 60);
/// Une question qui arrive moins de ce délai après la précédente, encore en
/// cours en arrière-plan, est de la même rafale (KOB-238).
pub const RAFALE: Duration = Duration::from_secs(10);
/// Dans une rafale, une question attend au plus ce délai la session de la
/// tâche en cours avant d'en prendre une de réserve.
pub const ATTENTE_RAFALE: Duration = Duration::from_secs(20);
/// Après une attente de rafale, temps laissé à une réponse rapide avant
/// que la question ne passe en arrière-plan.
const APRES_ATTENTE: Duration = Duration::from_secs(1);
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

/// Une carte de Kobi lui-même attend sa réponse (KOB-156, hook de ses
/// commandes et fichiers) : Claude attend alors sans rien émettre.
pub type Attente = Arc<dyn Fn() -> bool + Send + Sync>;

/// Cartes en attente de réponse : celles du serveur de permission (MCP), et
/// celles de Kobi lui-même par `externe`.
#[derive(Clone)]
struct Cartes {
    mcp: Arc<Mutex<HashMap<u64, ApprovalRequest>>>,
    externe: Attente,
}

impl Cartes {
    fn new(externe: Attente) -> Self {
        Self {
            mcp: Arc::default(),
            externe,
        }
    }

    fn lock(
        &self,
    ) -> std::sync::LockResult<std::sync::MutexGuard<'_, HashMap<u64, ApprovalRequest>>> {
        self.mcp.lock()
    }

    /// Une carte attend sa réponse : le tour ne s'abandonne pas (KOB-191).
    /// Chaque carte a sa propre échéance, après quoi Claude reprend.
    fn attend(&self) -> bool {
        let mut mcp = self.lock().unwrap_or_else(|e| e.into_inner());
        // Une demande échue (refusée seule à l'échéance) n'attend plus rien.
        mcp.retain(|_, demande| !demande.is_abandoned());
        !mcp.is_empty() || (self.externe)()
    }
}
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
    /// Lance les deux sessions sur le runtime courant, sans carte de Kobi
    /// (tests).
    #[cfg(test)]
    pub fn lancer(fabrique: Fabrique, sortie: Sortie) -> Self {
        Self::lancer_avec(fabrique, sortie, Arc::new(|| false), Delais::default())
    }

    /// Lance les deux sessions sur le runtime courant, en sachant quand une
    /// carte de Kobi lui-même attend sa réponse (`attente`).
    pub fn lancer_avec(
        fabrique: Fabrique,
        sortie: Sortie,
        attente: Attente,
        delais: Delais,
    ) -> Self {
        let cartes = Cartes::new(attente);
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
            delais,
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
    delais: Delais,
}

async fn conversation(
    fabrique: Fabrique,
    mut rx: mpsc::UnboundedReceiver<Demande>,
    mut arrets: mpsc::UnboundedReceiver<u64>,
    sortie: Sortie,
    cartes: Cartes,
    taches: Taches,
    delais: Delais,
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
    // Fins des tâches détachées, avec leur session si elle peut resservir
    // (KOB-238). Une session d'avant un renouvellement n'est jamais reprise :
    // chaque fin porte la génération des sessions de son départ.
    let (rendue_tx, mut rendues) = mpsc::unbounded_channel::<Rendue>();
    let mut generation: u64 = 0;
    // Tâches détachées qui tournent sur une session ouverte : seules elles
    // valent d'être attendues par une rafale (pas une session lente à
    // s'ouvrir).
    let en_cours = Arc::new(AtomicUsize::new(0));
    // Moment de la dernière question prise : une suivante proche est de la
    // même rafale.
    let mut derniere: Option<tokio::time::Instant> = None;
    // Demandes reçues pendant une attente de rafale, traitées ensuite dans
    // l'ordre.
    let mut differees: VecDeque<Demande> = VecDeque::new();
    loop {
        // `neuve` : posée à l'instant (pas une relance ni une question qui
        // attendait le réseau) ; seule une question neuve fait une rafale.
        let (q, neuve) = if let Some(q) = prochaine.take() {
            (q, false)
        } else {
            let differee = differees.pop_front();
            tokio::select! {
                biased;
                demande = async {
                    match differee {
                        Some(d) => Some(d),
                        None => rx.recv().await,
                    }
                } => match demande {
                    Some(Demande::Question { tour, question, transcript, notes }) => {
                        let q = Question {
                            tour,
                            ask: Ask { question, transcript, notes },
                            relancee: false,
                            resume: None,
                        };
                        if en_attente.is_empty() {
                            (q, true)
                        } else {
                            // Le réseau manque encore : elle prend son tour.
                            mettre_en_attente(&mut en_attente, q, false, &sortie);
                            reessai_depuis(&mut reessai, &en_attente);
                            continue;
                        }
                    }
                    Some(Demande::Renouveler) => {
                        renouveler(&mut session, &mut generation, &mut suivante, &fabrique);
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
                Some((g, s)) = rendues.recv() => {
                    // Une tâche finie rend sa session : la conversation la
                    // reprend si elle n'en a pas (sinon elle se ferme).
                    if let Some(s) = s.filter(|_| g == generation && session.is_none()) {
                        tracing::info!("session de la tâche reprise par la conversation");
                        session = Some(s);
                    }
                    continue;
                }
                _ = reessai.tick(), if !en_attente.is_empty() => {
                    tracing::info!(attente = en_attente.len(), "réseau : question reposée");
                    (en_attente.pop_front().expect("file non vide"), false)
                }
            }
        };
        let tour = q.tour;
        let arrivee = tokio::time::Instant::now();
        let en_rafale = neuve && derniere.is_some_and(|d| arrivee.duration_since(d) < RAFALE);
        // La question a attendu la tâche de sa rafale : déjà annoncée avec
        // elle, elle ne l'est pas une seconde fois.
        let mut attendu = false;
        if neuve {
            derniere = Some(arrivee);
        }
        if session.is_none() {
            while let Ok((g, s)) = rendues.try_recv() {
                if g == generation && s.is_some() {
                    session = s;
                    break;
                }
            }
        }
        if session.is_none() {
            // Une tâche arrêtée sur une carte peut attendre longtemps son
            // clic : la question ne l'attend pas.
            if en_rafale && !cartes.attend() && en_cours.load(Ordering::SeqCst) > 0 {
                // Rafale (KOB-238) : la question attend la tâche en cours et
                // reprend sa session, au lieu d'en ouvrir une de plus.
                tracing::info!(
                    tour,
                    "rafale : la question attend la session de la tâche en cours"
                );
                // Kobi « réfléchit » déjà (question envoyée). Un arrêt de
                // cette question pendant l'attente l'annule : elle n'est pas
                // posée (relecture de kobi-qualite).
                let fin_attente = tokio::time::Instant::now() + ATTENTE_RAFALE;
                let mut annulee = false;
                attendu = true;
                loop {
                    tokio::select! {
                        Some((g, s)) = rendues.recv() => {
                            // Une tâche finie, réussie ou non : la question
                            // n'attend plus (la réserve sert si la session
                            // ne peut pas resservir).
                            if g == generation {
                                session = s;
                                break;
                            }
                        }
                        Some(arret) = arrets.recv() => {
                            if arret == tour {
                                annulee = true;
                                break;
                            }
                        }
                        demande = rx.recv() => match demande {
                            // Sessions neuves demandées : celle de la tâche
                            // ne resservira pas, la question n'attend plus.
                            Some(Demande::Renouveler) => {
                                renouveler(&mut session, &mut generation, &mut suivante, &fabrique);
                                break;
                            }
                            Some(d) => differees.push_back(d),
                            None => break,
                        },
                        () = tokio::time::sleep_until(fin_attente) => break,
                    }
                }
                if annulee {
                    tracing::info!(tour, "rafale : question arrêtée avant d'être posée");
                    continue;
                }
            }
        }
        // Le délai d'annonce court dès la question, attente de rafale et
        // ouverture de session comprises ; après une attente, une réponse
        // rapide a encore le temps de venir.
        let annonce = (arrivee + ANNONCE).max(tokio::time::Instant::now() + APRES_ATTENTE);
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
                    delais,
                };
                let (sortie, cartes) = (sortie.clone(), cartes.clone());
                let mut rendue = Rendre::new(generation, &rendue_tx, None);
                detacher(&taches, tour, async move {
                    match prete
                        .await
                        .unwrap_or_else(|e| Err(BrainError::Crashed(e.to_string())))
                    {
                        Ok(s) => {
                            // La session vit jusqu'à la fin du tour, puis
                            // revient à la conversation.
                            let mut s = Some(s);
                            let turn = poser(&mut s, &reprise.question);
                            let vigie = Vigie::new(reprise.delais);
                            if finir(turn, String::new(), vigie, reprise, &sortie, &cartes).await {
                                rendue.garder(s);
                            }
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
        match suivre(
            tour,
            turn,
            (annonce, !attendu),
            delais,
            &sortie,
            &cartes,
            &mut arrets,
        )
        .await
        {
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
            Suivi::Detache(turn, texte, vigie) => {
                // Le tour garde sa session jusqu'au bout ; la conversation
                // continue sur la réserve.
                let detachee = session.take();
                let reprise = Reprise {
                    fabrique: fabrique.clone(),
                    question: q,
                    attente: attente_tx.clone(),
                    echanges: echanges.iter().cloned().collect(),
                    delais,
                };
                let (sortie, cartes) = (sortie.clone(), cartes.clone());
                let mut rendue = Rendre::new(generation, &rendue_tx, Some(&en_cours));
                detacher(&taches, tour, async move {
                    // La session vit jusqu'à la fin du tour, puis revient à
                    // la conversation (KOB-238).
                    if finir(turn, texte, vigie, reprise, &sortie, &cartes).await {
                        rendue.garder(detachee);
                    }
                });
            }
        }
    }
}

/// Dossiers de code changés (KOB-153) : sessions neuves. Une session rendue
/// ensuite par une tâche d'avant n'est plus reprise (génération suivante).
fn renouveler(
    session: &mut Option<Session>,
    generation: &mut u64,
    suivante: &mut Reserve,
    fabrique: &Fabrique,
) {
    *session = None;
    *generation += 1;
    suivante.abort();
    *suivante = reserve(fabrique);
    tracing::info!("sessions Claude renouvelées");
}

/// Fin d'une tâche détachée : génération des sessions à son départ, et sa
/// session si elle peut resservir.
type Rendue = (u64, Option<Session>);

/// Signale la fin d'une tâche détachée à la conversation, quelle qu'elle
/// soit (réussie, en erreur, arrêtée) : une question de la rafale qui
/// l'attend n'attend pas pour rien (KOB-238).
struct Rendre {
    generation: u64,
    tx: mpsc::UnboundedSender<Rendue>,
    /// Session à rendre, posée quand le tour s'est fini normalement.
    session: Option<Session>,
    /// Compte des tâches qu'une rafale peut attendre, si celle-ci en est.
    en_cours: Option<Arc<AtomicUsize>>,
}

impl Rendre {
    fn new(
        generation: u64,
        tx: &mpsc::UnboundedSender<Rendue>,
        en_cours: Option<&Arc<AtomicUsize>>,
    ) -> Self {
        if let Some(n) = en_cours {
            n.fetch_add(1, Ordering::SeqCst);
        }
        Self {
            generation,
            tx: tx.clone(),
            session: None,
            en_cours: en_cours.cloned(),
        }
    }

    /// Le tour s'est fini normalement : sa session sera rendue. Par une
    /// méthode, la tâche capture toute la garde (pas le seul champ), qui ne
    /// signale la fin qu'avec elle.
    fn garder(&mut self, session: Option<Session>) {
        self.session = session;
    }
}

impl Drop for Rendre {
    fn drop(&mut self) {
        // Décomptée avant d'être signalée : la conversation qui reçoit la
        // fin voit déjà le bon compte.
        if let Some(n) = &self.en_cours {
            n.fetch_sub(1, Ordering::SeqCst);
        }
        let _ = self.tx.send((self.generation, self.session.take()));
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

/// Délais d'un tour (KOB-243).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delais {
    /// Sans signe de vie pendant ce temps, le tour est abandonné.
    pub abandon: Duration,
    /// Au-delà, le tour est arrêté, même s'il vit encore.
    pub duree_max: Duration,
}

impl Default for Delais {
    fn default() -> Self {
        Self {
            abandon: ABANDON,
            duree_max: DUREE_MAX,
        }
    }
}

/// Ce qui fait vivre un tour : les outils appelés et pas encore revenus, et
/// son début, pour la durée maximale (KOB-243). Passe du premier plan à la
/// tâche détachée.
struct Vigie {
    delais: Delais,
    debut: tokio::time::Instant,
    /// Dernier signe de vie : l'abandon se compte depuis lui, détachement
    /// compris.
    vu: tokio::time::Instant,
    outils: std::collections::HashSet<String>,
}

impl Vigie {
    fn new(delais: Delais) -> Self {
        let maintenant = tokio::time::Instant::now();
        Self {
            delais,
            debut: maintenant,
            vu: maintenant,
            outils: Default::default(),
        }
    }

    fn voir(&mut self, evenement: &BrainEvent) {
        self.vu = tokio::time::Instant::now();
        match evenement {
            BrainEvent::ToolCall { id, .. } => {
                self.outils.insert(id.clone());
            }
            BrainEvent::OutilRendu { id } => {
                self.outils.remove(id);
            }
            _ => {}
        }
    }

    /// Sans nouvelle depuis `abandon`, mais un outil tourne encore ou une
    /// carte attend son clic : ce n'est pas de l'inactivité.
    fn occupe(&self, cartes: &Cartes) -> bool {
        !self.outils.is_empty() || cartes.attend()
    }

    fn fin_max(&self) -> tokio::time::Instant {
        self.debut + self.delais.duree_max
    }

    /// Message de l'arrêt à la durée maximale, pour l'utilisateur.
    fn depassee(&self) -> String {
        format!(
            "durée maximale d'une tâche atteinte ({} min)",
            self.delais.duree_max.as_secs().div_ceil(60)
        )
    }
}

enum Suivi {
    /// Fini, avec la réponse s'il y en a une.
    Fini(Option<String>),
    /// Le tour a échoué ; l'appelant décide (relance, attente, erreur).
    Erreur(BrainError),
    /// Tâche longue ou carte : le reste du tour, le texte déjà reçu et ce
    /// qui le fait vivre.
    Detache(Turn, String, Vigie),
    /// Abandonné par l'utilisateur : le reste du tour, à ignorer.
    Interrompu(Turn),
}

/// Suit un tour au premier plan jusqu'à sa fin ou son détachement.
async fn suivre(
    tour: u64,
    mut turn: Turn,
    // Échéance d'annonce, et si le détachement à l'échéance est annoncé.
    (annonce, annoncer): (tokio::time::Instant, bool),
    delais: Delais,
    sortie: &Sortie,
    cartes: &Cartes,
    arrets: &mut mpsc::UnboundedReceiver<u64>,
) -> Suivi {
    let mut texte = String::new();
    let mut vigie = Vigie::new(delais);
    let delai = tokio::time::sleep_until(annonce);
    tokio::pin!(delai);
    // Sans aucun signe de vie pendant `abandon`, le tour est abandonné.
    let inactif = tokio::time::sleep(delais.abandon);
    tokio::pin!(inactif);
    let fin_max = tokio::time::sleep_until(vigie.fin_max());
    tokio::pin!(fin_max);
    loop {
        tokio::select! {
            evenement = turn.next() => {
                inactif.as_mut().reset(tokio::time::Instant::now() + delais.abandon);
                if let Some(e) = &evenement {
                    vigie.voir(e);
                }
                match evenement {
                    Some(BrainEvent::Text(t)) => {
                        texte.push_str(&t);
                        sortie(Retour::Texte { tour, texte: texte.clone() });
                    }
                    Some(BrainEvent::ToolCall { id, name, action, .. }) if !interne(&action) => {
                        journaliser_outil(&name);
                        activite(tour, id, action, sortie);
                        sortie(Retour::Detache { tour, annonce: true });
                        return Suivi::Detache(turn, texte, vigie);
                    }
                    Some(BrainEvent::ToolCall { .. } | BrainEvent::OutilRendu { .. } | BrainEvent::Vie) => {}
                    Some(BrainEvent::Permission(demande)) => {
                        carte(tour, demande, sortie, cartes);
                        sortie(Retour::Detache { tour, annonce: false });
                        return Suivi::Detache(turn, texte, vigie);
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
                sortie(Retour::Detache { tour, annonce: annoncer });
                return Suivi::Detache(turn, texte, vigie);
            }
            () = &mut inactif => {
                if vigie.occupe(cartes) {
                    inactif.as_mut().reset(tokio::time::Instant::now() + delais.abandon);
                    continue;
                }
                tracing::warn!(tour, "tour sans activité : abandon");
                return Suivi::Erreur(BrainError::Timeout(delais.abandon));
            }
            () = &mut fin_max => {
                tracing::warn!(tour, "tour arrêté : durée maximale atteinte");
                sortie(Retour::Erreur { tour, message: vigie.depassee() });
                // Session fermée avec son tour, comme un arrêt.
                return Suivi::Interrompu(turn);
            }
        }
    }
}

/// Termine un tour détaché : ses cartes s'affichent, son résultat est livré.
/// Toujours sans texte après `PATIENCE` : Kobi dit qu'il s'en occupe ; sans
/// signe de vie pendant `Delais::abandon` (ni outil en cours ni carte en
/// attente) : abandon ; au-delà de `Delais::duree_max` : arrêt. S'il plante,
/// la question est reposée une fois sur une session neuve ; hors ligne, elle
/// part en attente.
/// `true` si le tour s'est fini normalement : sa session peut resservir.
async fn finir(
    mut turn: Turn,
    mut texte: String,
    mut vigie: Vigie,
    reprise: Reprise,
    sortie: &Sortie,
    cartes: &Cartes,
) -> bool {
    let tour = reprise.question.tour;
    let abandon = vigie.delais.abandon;
    let patience = tokio::time::sleep(PATIENCE.saturating_sub(ANNONCE));
    tokio::pin!(patience);
    let mut patient = !texte.is_empty();
    let inactif = tokio::time::sleep_until(vigie.vu + abandon);
    tokio::pin!(inactif);
    let fin_max = tokio::time::sleep_until(vigie.fin_max());
    tokio::pin!(fin_max);
    loop {
        let evenement = tokio::select! {
            e = turn.next() => e,
            () = &mut patience, if !patient => {
                patient = true;
                sortie(Retour::Patience { tour });
                continue;
            }
            () = &mut inactif => {
                if vigie.occupe(cartes) {
                    inactif.as_mut().reset(tokio::time::Instant::now() + abandon);
                    continue;
                }
                tracing::warn!(tour, "tâche sans activité : abandon");
                sortie(Retour::Erreur { tour, message: BrainError::Timeout(abandon).to_string() });
                return false;
            }
            () = &mut fin_max => {
                tracing::warn!(tour, "tâche arrêtée : durée maximale atteinte");
                sortie(Retour::Erreur { tour, message: vigie.depassee() });
                return false;
            }
        };
        inactif
            .as_mut()
            .reset(tokio::time::Instant::now() + abandon);
        let Some(evenement) = evenement else {
            return false;
        };
        vigie.voir(&evenement);
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
            BrainEvent::OutilRendu { .. } | BrainEvent::Vie => {}
            BrainEvent::Done(fin) => {
                let texte = if fin.text.is_empty() { texte } else { fin.text };
                sortie(Retour::Resultat { tour, texte });
                return true;
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
                            Box::pin(finir_relance(turn, reprise, sortie, cartes, s)).await;
                            return false;
                        }
                        Err(e) => {
                            sortie(Retour::Erreur {
                                tour,
                                message: e.to_string(),
                            });
                            return false;
                        }
                    }
                }
                Panne::HorsLigne => {
                    let _ = reprise.attente.send(reprise.question.clone());
                    return false;
                }
                _ => {
                    sortie(Retour::Erreur {
                        tour,
                        message: e.to_string(),
                    });
                    return false;
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
    // Session neuve d'une relance : elle se ferme avec le tour.
    let vigie = Vigie::new(reprise.delais);
    finir(turn, String::new(), vigie, reprise, sortie, cartes).await;
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
    } else if crate::commandes::OUTILS_FICHIER.contains(&outil) {
        // Le hook de Kobi demande la carte avant l'écriture (KOB-156).
        tracing::info!(%outil, "écriture de fichier, validée par la carte");
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

    /// Lenteur : détaché, puis la seule annonce à `PATIENCE`, puis le résultat.
    #[tokio::test(start_paused = true)]
    async fn lenteur_annoncee_une_fois_a_15_s() {
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

    /// Abandon à 45 s pour les essais : bien en dessous de l'outil de 2 min
    /// ou de la carte de 55 s, qui ne doivent pas le déclencher.
    fn court() -> Delais {
        Delais {
            abandon: Duration::from_secs(45),
            duree_max: DUREE_MAX,
        }
    }

    /// Attend un retour jusqu'à `au_plus` de temps (virtuel sous `start_paused`).
    async fn attendre_jusqu_a(recus: &Recus, au_plus: Duration, ok: impl Fn(&Retour) -> bool) {
        let fin = tokio::time::Instant::now() + au_plus;
        while tokio::time::Instant::now() < fin {
            if recus.lock().unwrap().iter().any(|(_, r)| ok(r)) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("attendu, reçu : {:?}", recus.lock().unwrap());
    }

    fn message_d_erreur(recus: &Recus) -> String {
        recus
            .lock()
            .unwrap()
            .iter()
            .find_map(|(_, r)| match r {
                Retour::Erreur { message, .. } => Some(message.clone()),
                _ => None,
            })
            .unwrap()
    }

    /// Vrai blocage (rien n'arrive, aucun outil, aucune carte) : abandon
    /// propre au délai par défaut (5 min), avec la durée dans le message.
    #[tokio::test(start_paused = true)]
    async fn blocage_abandon_apres_5_min_sans_signe_de_vie() {
        let (s, recus) = sortie();
        let muet = vec![vec![
            Step::Wait(Duration::from_secs(900)),
            Step::Text("trop tard".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![muet]), s);
        let debut = tokio::time::Instant::now();
        c.demander(question(1, "longue"));
        attendre_jusqu_a(&recus, Duration::from_secs(400), |r| {
            matches!(r, Retour::Erreur { tour: 1, .. })
        })
        .await;
        let ecoule = debut.elapsed();
        assert!(
            ecoule >= ABANDON && ecoule < ABANDON + Duration::from_secs(2),
            "{ecoule:?}"
        );
        let (titre, explication) = crate::pannes::pour_l_utilisateur(&message_d_erreur(&recus));
        assert_eq!(titre, "Claude ne répond pas");
        assert!(
            explication.contains("Rien depuis 5 minutes"),
            "{explication}"
        );
        assert!(
            rang(&recus, |r| matches!(r, Retour::Patience { .. }))
                < rang(&recus, |r| matches!(r, Retour::Erreur { .. }))
        );
        assert!(!recu(&recus, |r| matches!(r, Retour::Resultat { .. })));
    }

    /// Réflexion longue (4 min sans un mot) : sous le délai par défaut, la
    /// tâche est menée à terme.
    #[tokio::test(start_paused = true)]
    async fn reflexion_longue_menee_a_terme() {
        let (s, recus) = sortie();
        let pense = vec![vec![
            Step::Wait(Duration::from_secs(240)),
            Step::Text("voilà".into()),
        ]];
        let c = Cerveau::lancer(fabrique(vec![pense]), s);
        c.demander(question(1, "réfléchis"));
        attendre_jusqu_a(&recus, Duration::from_secs(300), |r| {
            matches!(r, Retour::Resultat { tour: 1, .. })
        })
        .await;
        assert!(!recu(&recus, |r| matches!(r, Retour::Erreur { .. })));
    }

    /// Un outil qui tourne 2 min (recherche, Bash) n'est pas de
    /// l'inactivité, même avec un délai d'abandon de 45 s ; revenu, le
    /// silence qui suit compte de nouveau (KOB-243).
    #[tokio::test(start_paused = true)]
    async fn outil_de_2_min_pas_abandonne() {
        let (s, recus) = sortie();
        let recherche = vec![vec![
            Step::ToolCall {
                id: "t1".into(),
                name: "WebSearch".into(),
                input: serde_json::json!({ "query": "kobi" }),
            },
            Step::Wait(Duration::from_secs(120)),
            Step::Rendu("t1".into()),
            Step::Text("trouvé".into()),
        ]];
        let c = Cerveau::lancer_avec(fabrique(vec![recherche]), s, Arc::new(|| false), court());
        c.demander(question(1, "cherche"));
        attendre_jusqu_a(&recus, Duration::from_secs(200), |r| {
            matches!(r, Retour::Resultat { tour: 1, .. })
        })
        .await;
        assert!(
            !recu(&recus, |r| matches!(r, Retour::Erreur { .. })),
            "{:?}",
            recus.lock().unwrap()
        );

        // L'outil rendu, plus rien : abandon au délai.
        let (s, recus) = sortie();
        let fige = vec![vec![
            Step::ToolCall {
                id: "t1".into(),
                name: "WebSearch".into(),
                input: serde_json::json!({ "query": "kobi" }),
            },
            Step::Rendu("t1".into()),
            Step::Wait(Duration::from_secs(600)),
            Step::Text("trop tard".into()),
        ]];
        let c = Cerveau::lancer_avec(fabrique(vec![fige]), s, Arc::new(|| false), court());
        c.demander(question(1, "cherche"));
        attendre_jusqu_a(&recus, Duration::from_secs(100), |r| {
            matches!(r, Retour::Erreur { tour: 1, .. })
        })
        .await;
    }

    /// KOB-191, KOB-243 : une carte de Kobi attend sa réponse (Claude
    /// bloqué dans le hook, rien n'arrive) : validée après 55 s, le tour
    /// n'est pas abandonné, même avec un délai de 45 s.
    #[tokio::test(start_paused = true)]
    async fn carte_en_attente_le_tour_n_est_pas_abandonne() {
        let (s, recus) = sortie();
        let carte = vec![vec![
            Step::Wait(Duration::from_secs(55)),
            Step::Text("Fichier créé.".into()),
        ]];
        let c = Cerveau::lancer_avec(fabrique(vec![carte]), s, Arc::new(|| true), court());
        c.demander(question(1, "crée salut.txt"));
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 1, .. })).await;
        assert!(!recu(&recus, |r| matches!(r, Retour::Erreur { .. })));
    }

    /// Durée maximale (KOB-243) : une tâche qui vit encore est arrêtée à sa
    /// limite, au premier plan comme détachée, avec un message clair.
    #[tokio::test(start_paused = true)]
    async fn duree_maximale_annoncee() {
        let delais = Delais {
            abandon: Duration::from_secs(45),
            duree_max: Duration::from_secs(120),
        };
        // Au premier plan : du texte toutes les 30 s.
        let (s, recus) = sortie();
        let bavard: Vec<Step> = (0..10)
            .flat_map(|i| {
                [
                    Step::Text(format!("{i} ")),
                    Step::Wait(Duration::from_secs(30)),
                ]
            })
            .collect();
        let c = Cerveau::lancer_avec(fabrique(vec![vec![bavard]]), s, Arc::new(|| false), delais);
        let debut = tokio::time::Instant::now();
        c.demander(question(1, "parle"));
        attendre_jusqu_a(&recus, Duration::from_secs(200), |r| {
            matches!(r, Retour::Erreur { tour: 1, .. })
        })
        .await;
        assert!(
            debut.elapsed() < Duration::from_secs(122),
            "{:?}",
            debut.elapsed()
        );
        let (titre, explication) = crate::pannes::pour_l_utilisateur(&message_d_erreur(&recus));
        assert_eq!(titre, "Tâche trop longue");
        assert!(explication.contains("(2 min)"), "{explication}");
        assert!(!recu(&recus, |r| matches!(
            r,
            Retour::Fin { .. } | Retour::Resultat { .. }
        )));

        // Détachée (outil long) : arrêtée de même.
        let (s, recus) = sortie();
        let outil = vec![vec![
            Step::ToolCall {
                id: "t1".into(),
                name: "WebSearch".into(),
                input: serde_json::json!({ "query": "kobi" }),
            },
            Step::Wait(Duration::from_secs(600)),
            Step::Rendu("t1".into()),
        ]];
        let c = Cerveau::lancer_avec(fabrique(vec![outil]), s, Arc::new(|| false), delais);
        c.demander(question(1, "cherche"));
        attendre_jusqu_a(&recus, Duration::from_secs(200), |r| {
            matches!(r, Retour::Erreur { tour: 1, .. })
        })
        .await;
        assert_eq!(
            crate::pannes::pour_l_utilisateur(&message_d_erreur(&recus)).0,
            "Tâche trop longue"
        );
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

    /// KOB-238 : la rafale du journal (3 questions en 13 s, la première
    /// longue) passe dans une seule session, dans l'ordre ; la réserve ne
    /// sert pas, et rien n'est annoncé (la tâche finit avant `PATIENCE`).
    #[tokio::test(start_paused = true)]
    async fn rafale_dans_une_seule_session() {
        let (s, recus) = sortie();
        let a = vec![
            vec![Step::Wait(Duration::from_secs(9)), Step::Text("r1".into())],
            vec![Step::Text("r2".into())],
            vec![Step::Text("r3".into())],
        ];
        let reserve = vec![vec![Step::Text("autre session".into())]; 3];
        let c = Cerveau::lancer(fabrique(vec![a, reserve.clone(), reserve]), s);
        c.demander(question(1, "longue phrase prise pour une question"));
        tokio::time::sleep(Duration::from_secs(5)).await;
        c.demander(question(2, "et ça ?"));
        tokio::time::sleep(Duration::from_secs(8)).await;
        c.demander(question(3, "et encore ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 3, .. })).await;
        let recus: Vec<Retour> = recus
            .lock()
            .unwrap()
            .iter()
            .map(|(_, r)| r.clone())
            .collect();
        let fins: Vec<(u64, &str)> = recus
            .iter()
            .filter_map(|r| match r {
                Retour::Resultat { tour, texte } | Retour::Fin { tour, texte } => {
                    Some((*tour, texte.as_str()))
                }
                _ => None,
            })
            .collect();
        assert_eq!(fins, [(1, "r1"), (2, "r2"), (3, "r3")], "{recus:?}");
        // Au plus l'annonce de la première ; aucune annonce de patience.
        let detaches = recus
            .iter()
            .filter(|r| matches!(r, Retour::Detache { annonce: true, .. }))
            .count();
        assert!(detaches <= 1, "{recus:?}");
        assert!(!recus.iter().any(|r| matches!(r, Retour::Patience { .. })));
    }

    /// Arrêtée pendant qu'elle attend la tâche de la rafale, la question
    /// n'est jamais posée ; la suivante l'est (relecture de kobi-qualite).
    #[tokio::test(start_paused = true)]
    async fn rafale_arret_pendant_l_attente() {
        let (s, recus) = sortie();
        let a = vec![
            vec![Step::Wait(Duration::from_secs(9)), Step::Text("r1".into())],
            vec![Step::Text("r3".into())],
        ];
        let c = Cerveau::lancer(fabrique(vec![a]), s);
        c.demander(question(1, "longue"));
        tokio::time::sleep(Duration::from_secs(5)).await;
        c.demander(question(2, "arrêtée"));
        tokio::time::sleep(Duration::from_secs(1)).await;
        c.demander(Demande::Interrompre { tour: 2 });
        tokio::time::sleep(Duration::from_secs(1)).await;
        c.demander(question(3, "suivante"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 3, .. })).await;
        let recus = recus.lock().unwrap();
        assert!(
            !recus.iter().any(|(_, r)| matches!(
                r,
                Retour::Fin { tour: 2, .. } | Retour::Texte { tour: 2, .. }
            )),
            "{recus:?}"
        );
        assert!(
            recus
                .iter()
                .any(|(_, r)| matches!(r, Retour::Fin { tour: 3, texte } if texte == "r3")),
            "{recus:?}"
        );
    }

    /// Une question posée bien après (hors rafale) n'attend pas la tâche
    /// longue : Kobi reste disponible (KOB-152).
    #[tokio::test(start_paused = true)]
    async fn hors_rafale_la_question_n_attend_pas() {
        let (s, recus) = sortie();
        let longue = vec![vec![
            Step::Wait(Duration::from_secs(60)),
            Step::Text("r1".into()),
        ]];
        let autre = vec![vec![Step::Text("tout de suite".into())]];
        let c = Cerveau::lancer(fabrique(vec![longue, autre]), s);
        c.demander(question(1, "lis toute la codebase"));
        tokio::time::sleep(RAFALE + Duration::from_secs(2)).await;
        // Temps de tokio (en pause dans ce test), pas l'horloge du système.
        let posee = tokio::time::Instant::now();
        c.demander(question(2, "quelle heure ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        assert!(
            posee.elapsed() < Duration::from_secs(5),
            "{:?}",
            recus.lock().unwrap()
        );
    }

    /// Une tâche lancée avant un renouvellement (dossiers de code changés,
    /// KOB-153) ne rend pas sa session : la question suivante part sur une
    /// session neuve (relecture de kobi-qualite).
    #[tokio::test(start_paused = true)]
    async fn renouveler_la_session_d_avant_n_est_pas_reprise() {
        let (s, recus) = sortie();
        let ancienne = vec![
            vec![Step::Wait(Duration::from_secs(9)), Step::Text("r1".into())],
            vec![Step::Text("ancienne session".into())],
        ];
        let abandonnee = vec![vec![Step::Text("réserve abandonnée".into())]];
        let neuve = vec![vec![Step::Text("session neuve".into())]];
        let c = Cerveau::lancer(fabrique(vec![ancienne, abandonnee, neuve]), s);
        c.demander(question(1, "longue"));
        tokio::time::sleep(Duration::from_secs(5)).await;
        c.demander(Demande::Renouveler);
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 1, .. })).await;
        tokio::time::sleep(RAFALE).await;
        c.demander(question(2, "et maintenant ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        assert!(
            recu(
                &recus,
                |r| matches!(r, Retour::Fin { tour: 2, texte } if texte == "session neuve")
            ),
            "{:?}",
            recus.lock().unwrap()
        );
    }

    /// La tâche attendue par la rafale échoue : la question part aussitôt sur
    /// la réserve, sans attendre `ATTENTE_RAFALE` (relecture de kobi-qualite).
    #[tokio::test(start_paused = true)]
    async fn rafale_tache_en_echec_la_question_n_attend_pas() {
        let (s, recus) = sortie();
        let echoue = vec![vec![
            Step::Wait(Duration::from_secs(6)),
            Step::Fail(BrainError::Api("surcharge".into())),
        ]];
        let reserve = vec![vec![Step::Text("r2".into())]];
        let c = Cerveau::lancer(fabrique(vec![echoue, reserve]), s);
        c.demander(question(1, "longue"));
        tokio::time::sleep(Duration::from_secs(5)).await;
        let posee = tokio::time::Instant::now();
        c.demander(question(2, "et ça ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        assert!(
            posee.elapsed() < Duration::from_secs(3),
            "{:?} {:?}",
            posee.elapsed(),
            recus.lock().unwrap()
        );
        assert!(recu(&recus, |r| matches!(
            r,
            Retour::Erreur { tour: 1, .. }
        )));
    }

    /// Le délai d'annonce d'une question de la rafale court dès son arrivée,
    /// attente de la tâche comprise (relecture de kobi-qualite).
    #[tokio::test(start_paused = true)]
    async fn rafale_annonce_comptee_des_l_arrivee() {
        let (s, recus) = sortie();
        let a = vec![
            vec![Step::Wait(Duration::from_secs(9)), Step::Text("r1".into())],
            vec![Step::Wait(Duration::from_secs(10)), Step::Text("r2".into())],
        ];
        let c = Cerveau::lancer(fabrique(vec![a]), s);
        c.demander(question(1, "longue"));
        tokio::time::sleep(Duration::from_secs(5)).await;
        let posee = tokio::time::Instant::now();
        c.demander(question(2, "et ça ?"));
        attendre(&recus, |r| matches!(r, Retour::Detache { tour: 2, .. })).await;
        // Attente jusqu'à 9 s ; l'échéance (5 + 4 s) est passée, la question
        // a encore `APRES_ATTENTE` puis passe en arrière-plan, sans seconde
        // annonce (celle de la rafale vaut pour elle).
        assert!(
            recu(&recus, |r| *r
                == Retour::Detache {
                    tour: 2,
                    annonce: false
                }),
            "{:?}",
            recus.lock().unwrap()
        );
        assert!(
            posee.elapsed() <= ANNONCE + APRES_ATTENTE + Duration::from_millis(100),
            "{:?} {:?}",
            posee.elapsed(),
            recus.lock().unwrap()
        );
        attendre(&recus, |r| matches!(r, Retour::Resultat { tour: 2, .. })).await;
    }

    /// Une question relancée après un plantage ne compte pas comme rafale :
    /// elle n'attend pas une tâche sans rapport (relecture de kobi-qualite).
    #[tokio::test(start_paused = true)]
    async fn relance_hors_rafale() {
        let (s, recus) = sortie();
        let longue = vec![vec![
            Step::Wait(Duration::from_secs(60)),
            Step::Text("r1".into()),
        ]];
        let plante = vec![vec![Step::Fail(BrainError::Crashed(
            "exit status 3".into(),
        ))]];
        let relance = vec![vec![Step::Text("après relance".into())]];
        let c = Cerveau::lancer(fabrique(vec![longue, plante, relance]), s);
        c.demander(question(1, "lis toute la codebase"));
        tokio::time::sleep(RAFALE + Duration::from_secs(1)).await;
        let posee = tokio::time::Instant::now();
        c.demander(question(2, "quelle heure ?"));
        attendre(&recus, |r| matches!(r, Retour::Fin { tour: 2, .. })).await;
        assert!(
            posee.elapsed() < Duration::from_secs(3),
            "{:?} {:?}",
            posee.elapsed(),
            recus.lock().unwrap()
        );
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

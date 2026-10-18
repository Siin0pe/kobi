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

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kobi_brain::{
    ApprovalRequest, Ask, Brain, BrainError, BrainEvent, NotesUpdate, ReportRequest, Turn,
};
use tokio::sync::mpsc;

/// Sans texte après ce délai (ou dès un appel d'outil), le tour est annoncé
/// comme tâche longue.
pub const ANNONCE: Duration = Duration::from_secs(4);
/// Délai maximal d'un bloc de notes ou du compte rendu.
const NOTES_MAX: Duration = Duration::from_secs(180);
/// Message affiché à l'annonce d'une tâche longue.
pub const MESSAGE_ANNONCE: &str = "Je m'en occupe, je te donne le résultat dès qu'il est prêt.";
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
    /// Carte de confirmation à afficher.
    Carte {
        tour: u64,
        carte: u64,
        outil: String,
        entree: serde_json::Value,
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

/// Poignée de l'acteur : on lui envoie des [`Demande`].
#[derive(Clone)]
pub struct Cerveau {
    questions: mpsc::UnboundedSender<Demande>,
    arrets: mpsc::UnboundedSender<u64>,
    notes: mpsc::UnboundedSender<Demande>,
    cartes: Cartes,
    sortie: Sortie,
}

impl Cerveau {
    /// Lance les deux sessions sur le runtime courant.
    pub fn lancer(fabrique: Fabrique, sortie: Sortie) -> Self {
        let cartes: Cartes = Arc::default();
        let (questions, rx_q) = mpsc::unbounded_channel();
        let (notes, rx_n) = mpsc::unbounded_channel();
        let (arrets, rx_a) = mpsc::unbounded_channel();
        tokio::spawn(conversation(
            fabrique.clone(),
            rx_q,
            rx_a,
            sortie.clone(),
            cartes.clone(),
        ));
        tokio::spawn(prise_de_notes(fabrique, rx_n, sortie.clone()));
        Self {
            questions,
            arrets,
            notes,
            cartes,
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

async fn conversation(
    fabrique: Fabrique,
    mut rx: mpsc::UnboundedReceiver<Demande>,
    mut arrets: mpsc::UnboundedReceiver<u64>,
    sortie: Sortie,
    cartes: Cartes,
) {
    let mut session: Option<Session> = None;
    let mut suivante = reserve(&fabrique);
    while let Some(demande) = rx.recv().await {
        let Demande::Question {
            tour,
            question,
            transcript,
            notes,
        } = demande
        else {
            continue;
        };
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
                let (sortie, cartes) = (sortie.clone(), cartes.clone());
                tokio::spawn(async move {
                    match prete
                        .await
                        .unwrap_or_else(|e| Err(BrainError::Crashed(e.to_string())))
                    {
                        Ok(mut s) => {
                            let turn = s.ask(Ask {
                                question,
                                transcript,
                                notes,
                            });
                            finir(tour, turn, String::new(), &sortie, &cartes).await;
                        }
                        Err(e) => sortie(Retour::Erreur {
                            tour,
                            message: e.to_string(),
                        }),
                    }
                });
                continue;
            };
            match ouverte.unwrap_or_else(|e| Err(BrainError::Crashed(e.to_string()))) {
                Ok(s) => session = Some(s),
                Err(e) => {
                    sortie(Retour::Erreur {
                        tour,
                        message: e.to_string(),
                    });
                    continue;
                }
            }
        }
        let s = session.as_mut().expect("session ouverte");
        let turn = s.ask(Ask {
            question,
            transcript,
            notes,
        });
        // Les arrêts reçus entre deux tours visaient un tour déjà fini.
        while arrets.try_recv().is_ok() {}
        match suivre(tour, turn, annonce, &sortie, &cartes, &mut arrets).await {
            Suivi::Fini => {}
            Suivi::Casse => session = None,
            Suivi::Interrompu(mut turn) => {
                // Le tour finit en silence sur sa session ; la conversation
                // continue sur la réserve.
                let detachee = session.take();
                tokio::spawn(async move {
                    let _session = detachee;
                    while turn.next().await.is_some() {}
                });
            }
            Suivi::Detache(turn, texte) => {
                // Le tour garde sa session jusqu'au bout ; la conversation
                // continue sur la réserve.
                let detachee = session.take();
                let (sortie, cartes) = (sortie.clone(), cartes.clone());
                tokio::spawn(async move {
                    let _session = detachee;
                    finir(tour, turn, texte, &sortie, &cartes).await;
                });
            }
        }
    }
}

enum Suivi {
    Fini,
    /// Le tour a échoué et la session est à rouvrir.
    Casse,
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
    loop {
        tokio::select! {
            evenement = turn.next() => match evenement {
                Some(BrainEvent::Text(t)) => {
                    texte.push_str(&t);
                    sortie(Retour::Texte { tour, texte: texte.clone() });
                }
                Some(BrainEvent::ToolCall { name, .. }) if !OUTILS_INTERNES.contains(&name.as_str()) => {
                    journaliser_outil(&name);
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
                    sortie(Retour::Fin { tour, texte });
                    return Suivi::Fini;
                }
                Some(BrainEvent::Error(e)) => {
                    let casse = !matches!(e, BrainError::Api(_));
                    sortie(Retour::Erreur { tour, message: e.to_string() });
                    return if casse { Suivi::Casse } else { Suivi::Fini };
                }
                None => return Suivi::Fini,
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
        }
    }
}

/// Termine un tour détaché : ses cartes s'affichent, son résultat est livré.
async fn finir(tour: u64, mut turn: Turn, mut texte: String, sortie: &Sortie, cartes: &Cartes) {
    while let Some(evenement) = turn.next().await {
        match evenement {
            BrainEvent::Text(t) => texte.push_str(&t),
            BrainEvent::Permission(demande) => carte(tour, demande, sortie, cartes),
            BrainEvent::ToolCall { name, .. } => journaliser_outil(&name),
            BrainEvent::Done(fin) => {
                let texte = if fin.text.is_empty() { texte } else { fin.text };
                sortie(Retour::Resultat { tour, texte });
                return;
            }
            BrainEvent::Error(e) => {
                sortie(Retour::Erreur {
                    tour,
                    message: e.to_string(),
                });
                return;
            }
        }
    }
}

/// Chaque outil appelé est journalisé : seuls Read, Grep et Glob doivent
/// apparaître (KOB-138).
fn journaliser_outil(outil: &str) {
    if crate::lecture::permis(outil) {
        tracing::info!(%outil, "outil de lecture");
    } else if !OUTILS_INTERNES.contains(&outil) {
        tracing::warn!(%outil, "outil hors lecture seule demandé par Claude");
    }
}

fn carte(tour: u64, demande: ApprovalRequest, sortie: &Sortie, cartes: &Cartes) {
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

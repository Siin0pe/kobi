//! Interface commune aux backends Claude Code et clé API.
//!
//! Chaque appel ([`Brain::ask`], [`Brain::update_notes`],
//! [`Brain::final_report`]) ouvre un [`Turn`] : un flux d'événements
//! ([`BrainEvent`]) qui se termine toujours par exactement un
//! [`BrainEvent::Done`] ou un [`BrainEvent::Error`].

use std::time::Duration;

use kobi_perms::ApprovalRequest;
use serde_json::Value;
use tokio::sync::mpsc;

use crate::claude_code::Usage;
use crate::outils::Action;

/// Interpellation : « Kobi, … ».
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ask {
    pub question: String,
    /// Morceau de transcription pas encore vu par le cerveau, déjà horodaté
    /// et attribué à sa source par la session.
    pub transcript: String,
    /// Notes courantes.
    pub notes: String,
}

/// Bloc de notes : toutes les 3 min ou après une longue pause.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NotesUpdate {
    /// Transcription depuis le bloc précédent.
    pub transcript: String,
    pub notes: String,
}

/// Fin de réunion : demande du compte rendu.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReportRequest {
    /// Fin de transcription pas encore vue.
    pub transcript: String,
    pub notes: String,
    /// Actions non faites ou échouées (tirées de `actions.json`), à garder
    /// dans le compte rendu.
    pub pending_actions: Vec<String>,
}

/// Erreur d'un tour, sous une forme que la session peut garder et afficher
/// (état Oups).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BrainError {
    /// Backend indisponible : Claude Code absent, déconnecté, clé manquante.
    #[error("cerveau indisponible : {0}")]
    Unavailable(String),
    /// Le process ou la connexion s'est arrêté pendant le tour.
    #[error("cerveau arrêté : {0}")]
    Crashed(String),
    /// Limite d'usage ou erreur renvoyée par l'API.
    #[error("erreur d'API : {0}")]
    Api(String),
    /// Pas de fin de tour dans le délai.
    #[error("pas de réponse en {} s", .0.as_secs())]
    Timeout(Duration),
    /// Un tour est déjà en cours sur cette session.
    #[error("un tour est déjà en cours")]
    Busy,
}

/// Fin d'un tour réussi.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TurnSummary {
    /// Texte complet : la réponse, les notes complètes ou le compte rendu.
    pub text: String,
    pub usage: Option<Usage>,
}

/// Événement d'un tour.
#[derive(Debug)]
pub enum BrainEvent {
    /// Morceau de texte, à afficher dès qu'il arrive.
    Text(String),
    /// Outil appelé (lecture MCP, ou écriture autorisée). `name` et `input`
    /// sont ceux du harness ; `action` est leur traduction normalisée, seule
    /// à servir aux règles de Kobi (KOB-157).
    ToolCall {
        id: String,
        name: String,
        input: Value,
        action: Action,
    },
    /// Un outil appelé a rendu son résultat (KOB-243) : tant qu'un outil
    /// n'est pas revenu, le tour n'est pas inactif.
    OutilRendu { id: String },
    /// Signe de vie sans rien à montrer (réflexion, message intermédiaire) :
    /// le tour n'est pas bloqué (KOB-243).
    Vie,
    /// Le cerveau attend une confirmation avant un outil. Abandonner la
    /// demande (la laisser tomber) vaut refus.
    Permission(ApprovalRequest),
    /// Fin du tour.
    Done(TurnSummary),
    /// Le tour a échoué.
    Error(BrainError),
}

impl BrainEvent {
    fn is_terminal(&self) -> bool {
        matches!(self, BrainEvent::Done(_) | BrainEvent::Error(_))
    }
}

/// Flux d'événements d'un tour.
#[derive(Debug)]
pub struct Turn {
    events: mpsc::Receiver<BrainEvent>,
    finished: bool,
}

impl Turn {
    /// Canal d'un nouveau tour : le backend écrit dans l'émetteur.
    pub fn channel() -> (mpsc::Sender<BrainEvent>, Self) {
        let (tx, events) = mpsc::channel(64);
        (
            tx,
            Self {
                events,
                finished: false,
            },
        )
    }

    /// Tour qui échoue tout de suite.
    pub fn failed(error: BrainError) -> Self {
        let (tx, turn) = Self::channel();
        tx.try_send(BrainEvent::Error(error))
            .expect("canal neuf et vide");
        turn
    }

    /// Prochain événement ; `None` après l'événement de fin. Si le backend
    /// disparaît sans fin de tour, un [`BrainError::Crashed`] est émis.
    pub async fn next(&mut self) -> Option<BrainEvent> {
        if self.finished {
            return None;
        }
        let event = self.events.recv().await.unwrap_or_else(|| {
            BrainEvent::Error(BrainError::Crashed(
                "flux interrompu sans fin de tour".into(),
            ))
        });
        self.finished = event.is_terminal();
        Some(event)
    }

    /// Lit le tour jusqu'au bout. Les demandes de permission sont refusées
    /// (abandonnées) ; les morceaux de texte sont ignorés au profit du
    /// résumé final.
    pub async fn wait(mut self) -> Result<TurnSummary, BrainError> {
        while let Some(event) = self.next().await {
            match event {
                BrainEvent::Done(summary) => return Ok(summary),
                BrainEvent::Error(error) => return Err(error),
                _ => {}
            }
        }
        unreachable!("next émet toujours une fin de tour")
    }

    /// Comme [`Turn::wait`], avec un délai global.
    pub async fn wait_timeout(self, timeout: Duration) -> Result<TurnSummary, BrainError> {
        tokio::time::timeout(timeout, self.wait())
            .await
            .unwrap_or(Err(BrainError::Timeout(timeout)))
    }
}

/// Un cerveau : Claude Code, clé API ou doublure de test.
///
/// Les méthodes rendent la main tout de suite ; le travail se fait dans des
/// tâches tokio (appeler depuis un runtime). Un seul tour à la fois par
/// session : un appel pendant un tour en cours donne [`BrainError::Busy`].
pub trait Brain: Send {
    /// Nom court du backend (`claude-code`, `api`, `fake`).
    fn name(&self) -> &str;

    /// Interpellation : la réponse arrive en flux.
    fn ask(&mut self, ask: Ask) -> Turn;

    /// Nouveau bloc de notes : le tour se termine sur les notes complètes.
    fn update_notes(&mut self, update: NotesUpdate) -> Turn;

    /// Fin de réunion : le tour se termine sur le compte rendu markdown.
    fn final_report(&mut self, report: ReportRequest) -> Turn;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn flux_interrompu_donne_une_erreur_puis_rien() {
        let (tx, mut turn) = Turn::channel();
        tx.send(BrainEvent::Text("a".into())).await.unwrap();
        drop(tx);
        assert!(matches!(turn.next().await, Some(BrainEvent::Text(t)) if t == "a"));
        assert!(matches!(
            turn.next().await,
            Some(BrainEvent::Error(BrainError::Crashed(_)))
        ));
        assert!(turn.next().await.is_none());
    }

    #[tokio::test]
    async fn rien_apres_la_fin_du_tour() {
        let (tx, mut turn) = Turn::channel();
        tx.send(BrainEvent::Done(TurnSummary::default()))
            .await
            .unwrap();
        tx.send(BrainEvent::Text("trop tard".into())).await.unwrap();
        assert!(matches!(turn.next().await, Some(BrainEvent::Done(_))));
        assert!(turn.next().await.is_none());
    }

    #[tokio::test]
    async fn delai_depasse() {
        let (_tx, turn) = Turn::channel();
        let err = turn
            .wait_timeout(Duration::from_millis(20))
            .await
            .unwrap_err();
        assert_eq!(err, BrainError::Timeout(Duration::from_millis(20)));
    }

    #[tokio::test]
    async fn tour_echoue_d_emblee() {
        let turn = Turn::failed(BrainError::Unavailable("claude introuvable".into()));
        assert_eq!(
            turn.wait().await,
            Err(BrainError::Unavailable("claude introuvable".into()))
        );
    }
}

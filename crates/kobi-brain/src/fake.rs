//! Cerveau factice pour les tests de la session : rejoue des tours écrits
//! à l'avance et garde la trace de ce qu'il a reçu.
//!
//! Sans script, chaque tour répond « reçu : <question> » (ou les notes, ou
//! le compte rendu) en deux morceaux.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kobi_perms::{ApprovalRequest, Decision, PermissionRequest};
use serde_json::Value;
use tokio::sync::mpsc;

use crate::brain::{
    Ask, Brain, BrainError, BrainEvent, NotesUpdate, ReportRequest, Turn, TurnSummary,
};

/// Une étape d'un tour scripté.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
    /// Demande une confirmation ; si elle est accordée, l'outil est appelé
    /// (événement `ToolCall`), sinon le tour continue sans lui.
    Permission(PermissionRequest),
    /// L'outil de cet identifiant a rendu son résultat (KOB-243).
    Rendu(String),
    /// Pause, pour tester les délais.
    Wait(Duration),
    /// Termine le tour en erreur (les étapes suivantes sont ignorées).
    Fail(BrainError),
}

/// Appel reçu par le cerveau factice.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Ask(Ask),
    Notes(NotesUpdate),
    Report(ReportRequest),
}

/// Trace partagée : reste lisible une fois le cerveau confié à la session.
#[derive(Debug, Clone, Default)]
pub struct Journal {
    requests: Arc<Mutex<Vec<Request>>>,
    decisions: Arc<Mutex<Vec<(String, Decision)>>>,
}

impl Journal {
    /// Appels reçus, dans l'ordre.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().expect("verrou journal").clone()
    }

    /// Décisions de permission reçues : (outil, décision).
    pub fn decisions(&self) -> Vec<(String, Decision)> {
        self.decisions.lock().expect("verrou journal").clone()
    }
}

#[derive(Default)]
pub struct FakeBrain {
    script: VecDeque<Vec<Step>>,
    journal: Journal,
    busy: Arc<AtomicBool>,
}

impl FakeBrain {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ajoute un tour scripté ; les tours sont consommés dans l'ordre, quel
    /// que soit l'appel (`ask`, `update_notes`, `final_report`).
    pub fn then(mut self, steps: Vec<Step>) -> Self {
        self.script.push_back(steps);
        self
    }

    pub fn journal(&self) -> Journal {
        self.journal.clone()
    }

    fn turn(&mut self, request: Request) -> Turn {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Turn::failed(BrainError::Busy);
        }
        let steps = self.script.pop_front().unwrap_or_else(|| echo(&request));
        self.journal
            .requests
            .lock()
            .expect("verrou journal")
            .push(request);
        let (tx, turn) = Turn::channel();
        let busy = self.busy.clone();
        let decisions = self.journal.decisions.clone();
        tokio::spawn(async move {
            let end = play(steps, &tx, &decisions).await;
            // Libéré avant la fin du tour : un appel fait dès réception de
            // `Done` doit être accepté.
            busy.store(false, Ordering::SeqCst);
            let _ = tx.send(end).await;
        });
        turn
    }
}

impl Brain for FakeBrain {
    fn name(&self) -> &str {
        "fake"
    }

    fn ask(&mut self, ask: Ask) -> Turn {
        self.turn(Request::Ask(ask))
    }

    fn update_notes(&mut self, update: NotesUpdate) -> Turn {
        self.turn(Request::Notes(update))
    }

    fn final_report(&mut self, report: ReportRequest) -> Turn {
        self.turn(Request::Report(report))
    }
}

fn echo(request: &Request) -> Vec<Step> {
    let content = match request {
        Request::Ask(a) => &a.question,
        Request::Notes(n) => &n.notes,
        Request::Report(r) => &r.notes,
    };
    vec![Step::Text("reçu : ".into()), Step::Text(content.clone())]
}

/// Joue les étapes et renvoie l'événement de fin.
async fn play(
    steps: Vec<Step>,
    tx: &mpsc::Sender<BrainEvent>,
    decisions: &Mutex<Vec<(String, Decision)>>,
) -> BrainEvent {
    let mut text = String::new();
    for step in steps {
        let event = match step {
            Step::Text(t) => {
                text.push_str(&t);
                BrainEvent::Text(t)
            }
            Step::ToolCall { id, name, input } => BrainEvent::ToolCall {
                action: crate::claude_code::outils::action(&name, &input),
                id,
                name,
                input,
            },
            Step::Permission(request) => {
                let tool = request.tool_name.clone();
                let (pending, answer) = ApprovalRequest::new(request.clone());
                if tx.send(BrainEvent::Permission(pending)).await.is_err() {
                    break;
                }
                let decision = answer
                    .await
                    .unwrap_or_else(|_| Decision::Deny("Demande abandonnée.".into()));
                decisions
                    .lock()
                    .expect("verrou journal")
                    .push((tool, decision.clone()));
                if decision != Decision::Allow {
                    continue;
                }
                BrainEvent::ToolCall {
                    id: request.tool_use_id.unwrap_or_default(),
                    action: crate::claude_code::outils::action(&request.tool_name, &request.input),
                    name: request.tool_name,
                    input: request.input,
                }
            }
            Step::Rendu(id) => BrainEvent::OutilRendu { id },
            Step::Wait(d) => {
                tokio::time::sleep(d).await;
                continue;
            }
            Step::Fail(error) => return BrainEvent::Error(error),
        };
        if tx.send(event).await.is_err() {
            break;
        }
    }
    BrainEvent::Done(TurnSummary {
        text,
        usage: None,
        avis: None,
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn write_request() -> PermissionRequest {
        PermissionRequest {
            tool_name: "mcp__linear__save_issue".into(),
            input: json!({ "title": "Corriger la connexion" }),
            tool_use_id: Some("toolu_1".into()),
        }
    }

    async fn texts(turn: &mut Turn) -> (Vec<String>, BrainEvent) {
        let mut texts = Vec::new();
        loop {
            match turn.next().await.expect("fin de tour") {
                BrainEvent::Text(t) => texts.push(t),
                end @ (BrainEvent::Done(_) | BrainEvent::Error(_)) => return (texts, end),
                other => panic!("inattendu : {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn repond_en_echo_par_defaut_et_garde_les_appels() {
        let mut brain = FakeBrain::new();
        let journal = brain.journal();
        let ask = Ask {
            question: "combien de tickets ?".into(),
            transcript: "[00:01 moi] bonjour".into(),
            notes: String::new(),
        };
        let mut turn = brain.ask(ask.clone());
        let (texts, end) = texts(&mut turn).await;
        assert_eq!(texts, ["reçu : ", "combien de tickets ?"]);
        let BrainEvent::Done(summary) = end else {
            panic!("attendu Done")
        };
        assert_eq!(summary.text, "reçu : combien de tickets ?");
        assert_eq!(journal.requests(), [Request::Ask(ask)]);
    }

    #[tokio::test]
    async fn les_trois_appels_passent_par_un_dyn_brain() {
        let fake = FakeBrain::new();
        let journal = fake.journal();
        let mut brain: Box<dyn Brain> = Box::new(fake);
        assert_eq!(brain.name(), "fake");
        let notes = brain
            .update_notes(NotesUpdate {
                transcript: "…".into(),
                notes: "- décision A".into(),
                ..Default::default()
            })
            .wait()
            .await
            .unwrap();
        assert_eq!(notes.text, "reçu : - décision A");
        let report = brain
            .final_report(ReportRequest {
                notes: "# Compte rendu".into(),
                pending_actions: vec!["ticket Linear non créé".into()],
                ..Default::default()
            })
            .wait()
            .await
            .unwrap();
        assert_eq!(report.text, "reçu : # Compte rendu");
        assert!(matches!(
            journal.requests().as_slice(),
            [Request::Notes(_), Request::Report(r)] if r.pending_actions.len() == 1
        ));
    }

    #[tokio::test]
    async fn ecriture_autorisee_puis_outil_appele() {
        let mut brain = FakeBrain::new().then(vec![
            Step::ToolCall {
                id: "toolu_0".into(),
                name: "mcp__linear__list_issues".into(),
                input: json!({}),
            },
            Step::Permission(write_request()),
            Step::Text("Ticket créé.".into()),
        ]);
        let journal = brain.journal();
        let mut turn = brain.ask(Ask::default());
        assert!(matches!(
            turn.next().await,
            Some(BrainEvent::ToolCall { name, .. }) if name == "mcp__linear__list_issues"
        ));
        let Some(BrainEvent::Permission(pending)) = turn.next().await else {
            panic!("attendu une demande de permission")
        };
        assert_eq!(pending.request, write_request());
        pending.allow();
        assert!(matches!(
            turn.next().await,
            Some(BrainEvent::ToolCall { id, name, .. })
                if id == "toolu_1" && name == "mcp__linear__save_issue"
        ));
        assert_eq!(turn.wait().await.unwrap().text, "Ticket créé.");
        assert_eq!(
            journal.decisions(),
            [("mcp__linear__save_issue".into(), Decision::Allow)]
        );
    }

    #[tokio::test]
    async fn ecriture_refusee_sans_appel_d_outil() {
        let mut brain = FakeBrain::new().then(vec![
            Step::Permission(write_request()),
            Step::Text("D'accord, je ne crée rien.".into()),
        ]);
        let journal = brain.journal();
        let mut turn = brain.ask(Ask::default());
        let Some(BrainEvent::Permission(pending)) = turn.next().await else {
            panic!("attendu une demande de permission")
        };
        pending.deny("Pas maintenant.");
        let (texts, end) = texts(&mut turn).await;
        assert_eq!(texts, ["D'accord, je ne crée rien."]);
        assert!(matches!(end, BrainEvent::Done(_)));
        assert_eq!(
            journal.decisions(),
            [(
                "mcp__linear__save_issue".into(),
                Decision::Deny("Pas maintenant.".into())
            )]
        );
    }

    #[tokio::test]
    async fn wait_abandonne_les_permissions_ce_qui_vaut_refus() {
        let mut brain = FakeBrain::new().then(vec![Step::Permission(write_request())]);
        let journal = brain.journal();
        brain.ask(Ask::default()).wait().await.unwrap();
        assert!(matches!(
            journal.decisions().as_slice(),
            [(_, Decision::Deny(_))]
        ));
    }

    #[tokio::test]
    async fn erreur_en_cours_de_tour() {
        let mut brain = FakeBrain::new().then(vec![
            Step::Text("Je cherche".into()),
            Step::Fail(BrainError::Api("limite d'usage atteinte".into())),
            Step::Text("jamais envoyé".into()),
        ]);
        let mut turn = brain.ask(Ask::default());
        let (texts, end) = texts(&mut turn).await;
        assert_eq!(texts, ["Je cherche"]);
        assert!(matches!(
            end,
            BrainEvent::Error(BrainError::Api(m)) if m.contains("limite")
        ));
        assert!(turn.next().await.is_none());
        // La session reste utilisable après une erreur.
        assert_eq!(
            brain.ask(Ask::default()).wait().await.unwrap().text,
            "reçu : "
        );
    }

    #[tokio::test]
    async fn un_seul_tour_a_la_fois() {
        let mut brain = FakeBrain::new().then(vec![Step::Wait(Duration::from_millis(50))]);
        let first = brain.ask(Ask::default());
        assert_eq!(
            brain.ask(Ask::default()).wait().await,
            Err(BrainError::Busy)
        );
        first.wait().await.unwrap();
        assert!(brain.ask(Ask::default()).wait().await.is_ok());
    }

    #[tokio::test]
    async fn tour_trop_lent() {
        let mut brain = FakeBrain::new().then(vec![
            Step::Wait(Duration::from_secs(60)),
            Step::Text("trop tard".into()),
        ]);
        let err = brain
            .ask(Ask::default())
            .wait_timeout(Duration::from_millis(20))
            .await
            .unwrap_err();
        assert_eq!(err, BrainError::Timeout(Duration::from_millis(20)));
    }
}

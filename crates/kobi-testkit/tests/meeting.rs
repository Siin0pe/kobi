//! Session Claude Code de réunion ([`ClaudeCodeBrain`]) contre le faux
//! `claude` : un seul process pour toute la réunion, permissions par le
//! serveur « kobi », fermeture à la fin.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kobi_brain::{
    Ask, Brain, BrainError, BrainEvent, ClaudeCodeBrain, MeetingConfig, ReportRequest, Turn,
};
use serde_json::{json, Map};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");
const FAKE_MCP: &str = env!("CARGO_BIN_EXE_fake-mcp");

fn work_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kobi-meeting-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn config(scenario: &str, dir: &Path) -> MeetingConfig {
    let mut servers = Map::new();
    servers.insert(
        "notes".into(),
        json!({ "command": FAKE_MCP, "args": ["--journal", dir.join("journal.txt")] }),
    );
    MeetingConfig {
        program: PathBuf::from(FAKE_CLAUDE),
        cwd: Some(dir.to_owned()),
        mcp_servers: servers,
        add_dirs: vec![dir.to_owned()],
        extra_args: vec![
            "--scenario".into(),
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("scenarios")
                .join(format!("{scenario}.json"))
                .display()
                .to_string(),
        ],
        ..Default::default()
    }
}

fn ask(question: &str) -> Ask {
    Ask {
        question: question.into(),
        transcript: "[00:12 eux] on livre vendredi ?".into(),
        notes: "- Ordre du jour : livraison".into(),
    }
}

/// Texte du tour et outils appelés ; les permissions éventuelles sont
/// accordées si `allow`, refusées sinon.
async fn play(mut turn: Turn, allow: bool) -> Result<(String, Vec<String>), BrainError> {
    let mut tools = Vec::new();
    while let Some(event) = turn.next().await {
        match event {
            BrainEvent::ToolCall { name, .. } => tools.push(name),
            BrainEvent::Permission(pending) if allow => pending.allow(),
            BrainEvent::Permission(pending) => pending.deny("Pas maintenant."),
            BrainEvent::Done(summary) => return Ok((summary.text, tools)),
            BrainEvent::Error(error) => return Err(error),
            BrainEvent::Text(_) => {}
        }
    }
    unreachable!("un tour finit toujours par Done ou Error")
}

#[tokio::test]
async fn trois_questions_un_seul_process() {
    let dir = work_dir("trois");
    let mut brain = ClaudeCodeBrain::start(config("reunion", &dir))
        .await
        .unwrap();
    assert_eq!(brain.name(), "claude-code");
    let pid = brain.pid().expect("process lancé");
    for n in 1..=3 {
        let (text, _) = play(brain.ask(ask("on livre quand ?")), false)
            .await
            .unwrap();
        // Le faux claude numérote ses tours : un nouveau process repartirait de 1.
        assert_eq!(text, format!("Réponse {n} du process {pid}."));
    }
    let report = brain
        .final_report(ReportRequest {
            notes: "- Livraison vendredi".into(),
            pending_actions: vec!["Créer le ticket de livraison".into()],
            ..Default::default()
        })
        .wait()
        .await
        .unwrap();
    assert_eq!(report.text, format!("# Compte rendu (process {pid})"));
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn lecture_mcp_sans_outils_internes() {
    let dir = work_dir("lecture");
    let mut brain = ClaudeCodeBrain::start(config("interpellation", &dir))
        .await
        .unwrap();
    let (text, tools) = play(brain.ask(ask("qu'est-ce qu'on a décidé ?")), false)
        .await
        .unwrap();
    assert!(text.contains("vendredi"));
    // `ToolSearch` (chargement des outils MCP) n'est pas une source.
    assert_eq!(tools, ["mcp__notes__read_notes"]);
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn ecriture_confirmee_par_la_carte() {
    let dir = work_dir("confirmee");
    let mut brain = ClaudeCodeBrain::start(config("permission", &dir))
        .await
        .unwrap();
    let (text, tools) = play(brain.ask(ask("note qu'il faut envoyer le devis")), true)
        .await
        .unwrap();
    assert!(text.contains("C'est noté"));
    assert!(tools.contains(&"mcp__notes__write_note".to_owned()));
    assert_eq!(
        std::fs::read_to_string(dir.join("journal.txt")).unwrap(),
        "- Il faut envoyer le devis au client.\n"
    );
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn ecriture_refusee_par_la_carte() {
    let dir = work_dir("refusee");
    let mut brain = ClaudeCodeBrain::start(config("permission", &dir))
        .await
        .unwrap();
    let (text, _) = play(brain.ask(ask("note qu'il faut envoyer le devis")), false)
        .await
        .unwrap();
    assert!(text.contains("je n'ajoute rien"));
    assert!(!dir.join("journal.txt").exists());
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn une_question_a_la_fois() {
    let dir = work_dir("occupe");
    let mut brain = ClaudeCodeBrain::start(config("reunion", &dir))
        .await
        .unwrap();
    let first = brain.ask(ask("un"));
    assert_eq!(brain.ask(ask("deux")).wait().await, Err(BrainError::Busy));
    first.wait().await.unwrap();
    // Libre dès la fin du tour.
    assert!(brain.ask(ask("trois")).wait().await.is_ok());
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn plantage_puis_session_fermee() {
    let dir = work_dir("plantage");
    let mut brain = ClaudeCodeBrain::start(config("plantage", &dir))
        .await
        .unwrap();
    brain.ask(ask("premier point")).wait().await.unwrap();
    let err = brain.ask(ask("second point")).wait().await.unwrap_err();
    assert!(
        matches!(&err, BrainError::Crashed(m) if m.contains("Connection error")),
        "{err:?}"
    );
    // La session est morte : la question suivante échoue tout de suite (la
    // session relancera claude, KOB-58).
    let err = brain.ask(ask("troisième")).wait().await.unwrap_err();
    assert!(matches!(err, BrainError::Crashed(_)), "{err:?}");
}

#[tokio::test]
async fn claude_introuvable() {
    let config = MeetingConfig {
        program: PathBuf::from("kobi-claude-introuvable"),
        ..Default::default()
    };
    let err = ClaudeCodeBrain::start(config).await.err().unwrap();
    assert!(matches!(err, BrainError::Unavailable(_)), "{err:?}");
}

#[tokio::test]
async fn fermeture_a_la_fin_de_la_reunion() {
    let dir = work_dir("fermeture");
    let brain = ClaudeCodeBrain::start(config("lent", &dir)).await.unwrap();
    let pid = brain.pid().unwrap();
    brain.close(Duration::from_millis(500)).await;
    // stdin fermé : le faux claude sort de lui-même, sans tour en cours.
    assert!(!process_alive(pid));
}

#[cfg(target_os = "linux")]
fn process_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
        && !std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| s.contains(") Z "))
            .unwrap_or(true)
}

#[cfg(not(target_os = "linux"))]
fn process_alive(_pid: u32) -> bool {
    false
}

//! Serveur de permission « kobi » de bout en bout, avec le faux `claude` :
//! une écriture attend la réponse de l'utilisateur à la carte.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kobi_brain::{Ask, Brain, BrainEvent, ClaudeCodeBrain, Decision, MeetingConfig, Turn};
use kobi_perms::Cards;
use serde_json::{json, Map};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");
const FAKE_MCP: &str = env!("CARGO_BIN_EXE_fake-mcp");

fn work_dir(test: &str) -> kobi_testkit::DossierTemp {
    kobi_testkit::DossierTemp::nouveau("perms", test)
}

async fn start(dir: &Path, timeout: Duration) -> ClaudeCodeBrain {
    let mut servers = Map::new();
    servers.insert(
        "notes".into(),
        json!({ "command": FAKE_MCP, "args": ["--journal", dir.join("journal.txt")] }),
    );
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/permission.json");
    ClaudeCodeBrain::start(MeetingConfig {
        program: PathBuf::from(FAKE_CLAUDE),
        mcp_servers: servers,
        permission_timeout: timeout,
        extra_args: vec!["--scenario".into(), scenario.display().to_string()],
        ..Default::default()
    })
    .await
    .unwrap()
}

/// Lit le tour jusqu'à la demande de permission et ouvre la carte.
async fn until_card(turn: &mut Turn, cards: &Cards) -> u64 {
    loop {
        match turn.next().await.expect("le tour continue") {
            BrainEvent::Permission(pending) => return cards.open(pending).id,
            BrainEvent::Done(_) | BrainEvent::Error(_) => panic!("fin de tour sans carte"),
            _ => {}
        }
    }
}

fn ask() -> Ask {
    Ask {
        question: "note qu'il faut envoyer le devis".into(),
        ..Default::default()
    }
}

fn journal(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("journal.txt")).ok()
}

#[tokio::test]
async fn une_ecriture_attend_la_reponse() {
    let dir = work_dir("attend");
    let cards = Cards::new(Duration::from_secs(60));
    let mut brain = start(&dir, Duration::from_secs(60)).await;
    let mut turn = brain.ask(ask());
    let id = until_card(&mut turn, &cards).await;
    let card = &cards.pending()[0];
    assert_eq!(
        (card.server.as_deref(), card.tool.as_str()),
        (Some("notes"), "write_note")
    );
    assert_eq!(card.input["text"], "Il faut envoyer le devis au client.");

    // Personne n'a cliqué : Claude attend, rien n'est écrit.
    let waiting = tokio::time::timeout(Duration::from_secs(1), turn.next()).await;
    assert!(waiting.is_err(), "le tour ne doit pas avancer sans réponse");
    assert_eq!(journal(&dir), None);

    assert!(cards.answer(id, Decision::Allow));
    let summary = turn.wait().await.unwrap();
    assert!(summary.text.contains("C'est noté"));
    assert_eq!(
        journal(&dir).as_deref(),
        Some("- Il faut envoyer le devis au client.\n")
    );
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn pas_maintenant() {
    let dir = work_dir("refus");
    let cards = Cards::new(Duration::from_secs(60));
    let mut brain = start(&dir, Duration::from_secs(60)).await;
    let mut turn = brain.ask(ask());
    let id = until_card(&mut turn, &cards).await;
    assert!(cards.answer(id, Decision::Deny("Pas maintenant.".into())));
    assert!(turn.wait().await.unwrap().text.contains("je n'ajoute rien"));
    assert_eq!(journal(&dir), None);
    brain.close(Duration::from_secs(5)).await;
}

#[tokio::test]
async fn carte_expiree_refus_automatique() {
    let dir = work_dir("expiree");
    let cards = Cards::new(Duration::from_millis(300));
    let mut brain = start(&dir, Duration::from_millis(300)).await;
    let mut turn = brain.ask(ask());
    let id = until_card(&mut turn, &cards).await;
    // Sans réponse dans le délai, le serveur refuse et le tour se termine.
    assert!(turn.wait().await.unwrap().text.contains("je n'ajoute rien"));
    assert!(cards.pending().is_empty());
    assert!(!cards.answer(id, Decision::Allow), "clic trop tard");
    assert_eq!(journal(&dir), None);
    brain.close(Duration::from_secs(5)).await;
}

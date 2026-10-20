//! Règles de permission de bout en bout : annotations lues sur un vrai
//! serveur MCP (le faux serveur `notes`, rmcp), liste blanche du workspace,
//! confirmation pour le reste.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use kobi_brain::{Ask, Brain, BrainEvent, ClaudeCodeBrain, MeetingConfig};
use kobi_perms::{Rules, Verdict, DEFAULT_TIMEOUT};
use serde_json::{json, Map, Value};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");
const FAKE_MCP: &str = env!("CARGO_BIN_EXE_fake-mcp");

/// Réponse `tools/list` du faux serveur MCP, en JSON-RPC sur stdio.
fn tools_list() -> Value {
    let mut child = Command::new(FAKE_MCP)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut call = |message: Value, id: Option<u64>| {
        writeln!(stdin, "{message}").unwrap();
        let id = id?;
        let mut line = String::new();
        loop {
            line.clear();
            stdout.read_line(&mut line).unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            if reply["id"] == id {
                return Some(reply["result"].clone());
            }
        }
    };
    call(
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25",
            "capabilities":{},"clientInfo":{"name":"test","version":"0"}}}),
        Some(1),
    );
    call(
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        None,
    );
    let list = call(
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        Some(2),
    )
    .unwrap();
    drop(stdin);
    let _ = child.wait();
    list
}

fn work_dir(test: &str) -> kobi_testkit::DossierTemp {
    kobi_testkit::DossierTemp::nouveau("rules", test)
}

/// Joue le scénario d'écriture (`write_note`) avec ces règles ; renvoie le
/// nombre de cartes (toutes refusées) et ce qui a été écrit.
async fn write_with(rules: Rules, test: &str) -> (usize, Option<String>) {
    let dir = work_dir(test);
    let mut servers = Map::new();
    servers.insert(
        "notes".into(),
        json!({ "command": FAKE_MCP, "args": ["--journal", dir.join("journal.txt")] }),
    );
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR")).join("scenarios/permission.json");
    let mut brain = ClaudeCodeBrain::start(MeetingConfig {
        program: PathBuf::from(FAKE_CLAUDE),
        mcp_servers: servers,
        permission_rules: rules,
        extra_args: vec!["--scenario".into(), scenario.display().to_string()],
        ..Default::default()
    })
    .await
    .unwrap();
    let mut turn = brain.ask(Ask {
        question: "note qu'il faut envoyer le devis".into(),
        ..Default::default()
    });
    let mut cards = 0;
    while let Some(event) = turn.next().await {
        match event {
            BrainEvent::Permission(pending) => {
                cards += 1;
                pending.deny("Pas maintenant.");
            }
            BrainEvent::Done(_) => break,
            BrainEvent::Error(e) => panic!("tour en erreur : {e}"),
            _ => {}
        }
    }
    brain.close(Duration::from_secs(5)).await;
    (cards, std::fs::read_to_string(dir.join("journal.txt")).ok())
}

#[test]
fn annotations_lues_sur_le_serveur_mcp() {
    let mut rules = Rules::default();
    assert_eq!(rules.add_tools_list("notes", &tools_list()), 1);
    assert!(matches!(
        rules.verdict("mcp__notes__read_notes"),
        Verdict::Allow(_)
    ));
    // `write_note` : readOnlyHint à false, donc confirmation.
    assert_eq!(rules.verdict("mcp__notes__write_note"), Verdict::Ask);
}

#[tokio::test]
async fn ecriture_confirmee_meme_avec_les_annotations() {
    let mut rules = Rules::default();
    rules.add_tools_list("notes", &tools_list());
    let (cards, written) = write_with(rules, "annotations").await;
    assert_eq!(cards, 1);
    assert_eq!(written, None);
}

#[tokio::test]
async fn liste_blanche_du_workspace_sans_carte() {
    let rules = Rules::new(["mcp__notes__write_note"]);
    let (cards, written) = write_with(rules, "liste-blanche").await;
    assert_eq!(cards, 0);
    assert_eq!(
        written.as_deref(),
        Some("- Il faut envoyer le devis au client.\n")
    );
}

#[tokio::test]
async fn joker_de_serveur_dans_la_liste_blanche() {
    let (cards, written) = write_with(Rules::new(["mcp__notes__*"]), "joker").await;
    assert_eq!((cards, written.is_some()), (0, true));
}

#[test]
fn sans_reponse_en_60_s_c_est_un_refus() {
    // Le refus au délai est testé dans kobi-perms et dans tests/perms.rs ;
    // ici, la valeur du dossier de conception.
    assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(60));
    assert_eq!(MeetingConfig::default().permission_timeout, DEFAULT_TIMEOUT);
}

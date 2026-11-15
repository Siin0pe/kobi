//! SONDE KOB-511 (ne pas merger) : ce que `turn.next()` rend dans la seconde qui
//! suit la carte, sur 40 essais. Échoue toujours, pour que le compte rendu
//! apparaisse dans la CI.
use std::path::{Path, PathBuf};
use std::time::Duration;

use kobi_brain::{Ask, Brain, BrainEvent, ClaudeCodeBrain, Decision, MeetingConfig, Turn};
use kobi_perms::Cards;
use serde_json::{json, Map};

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");
const FAKE_MCP: &str = env!("CARGO_BIN_EXE_fake-mcp");

async fn start(dir: &Path) -> ClaudeCodeBrain {
    let mut servers = Map::new();
    servers.insert(
        "notes".into(),
        json!({ "command": FAKE_MCP, "args": ["--journal", dir.join("journal.txt")] }),
    );
    let scenario = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join("permission.json");
    ClaudeCodeBrain::start(MeetingConfig {
        program: PathBuf::from(FAKE_CLAUDE),
        mcp_servers: servers,
        permission_timeout: Duration::from_secs(60),
        extra_args: vec!["--scenario".into(), scenario.display().to_string()],
        ..Default::default()
    })
    .await
    .unwrap()
}

async fn jusqu_a_la_carte(turn: &mut Turn, cards: &Cards, avant: &mut Vec<String>) -> u64 {
    loop {
        match turn.next().await.expect("le tour continue") {
            BrainEvent::Permission(p) => return cards.open(p).id,
            e @ (BrainEvent::Done(_) | BrainEvent::Error(_)) => panic!("fin sans carte : {e:?}"),
            e => avant.push(court(&e)),
        }
    }
}

fn court(e: &BrainEvent) -> String {
    let s = format!("{e:?}");
    s.chars().take(70).collect()
}

/// Un essai : ce que `turn.next()` rend dans la seconde qui suit la carte.
async fn essai(n: usize) -> String {
    let dir = kobi_testkit::DossierTemp::nouveau("sonde511", &format!("e{n}"));
    let cards = Cards::new(Duration::from_secs(60));
    let mut brain = start(&dir).await;
    let mut turn = brain.ask(Ask {
        question: "note qu'il faut envoyer le devis".into(),
        ..Default::default()
    });
    let mut avant = Vec::new();
    let id = jusqu_a_la_carte(&mut turn, &cards, &mut avant).await;
    let attente = tokio::time::timeout(Duration::from_secs(1), turn.next()).await;
    let vu = match attente {
        Err(_) => "rien (attendu)".to_owned(),
        Ok(None) => "None (flux fermé)".to_owned(),
        Ok(Some(e)) => court(&e),
    };
    let fin_avant = avant.last().cloned().unwrap_or_default();
    cards.answer(id, Decision::Allow);
    let _ = turn.wait().await;
    brain.close(Duration::from_secs(5)).await;
    format!("essai {n:2} : {vu} | dernier avant la carte : {fin_avant}")
}

/// Sous charge : 8 essais en parallèle, 5 fois, comme quand toute la suite tourne.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sonde_kob511_sous_charge() {
    let mut rendus = Vec::new();
    for vague in 0..5 {
        let taches: Vec<_> = (0..8).map(|k| tokio::spawn(essai(vague * 8 + k))).collect();
        for t in taches {
            rendus.push(t.await.unwrap_or_else(|e| format!("tâche perdue : {e}")));
        }
    }
    let anormaux = rendus
        .iter()
        .filter(|r| !r.contains("rien (attendu)"))
        .count();
    panic!(
        "COMPTE RENDU KOB-511 sous charge : {anormaux}/40 anormaux\n{}",
        rendus.join("\n")
    );
}

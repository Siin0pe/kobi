//! Tests d'intégration du cerveau contre les doublures : le faux `claude`
//! rejoue des flux stream-json enregistrés et appelle pour de vrai le
//! serveur de permission « kobi » et le faux serveur MCP de notes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use kobi_brain::{BrainError, BrainEvent, ClaudeCode, ClaudeOptions, Error, TurnTranslator};
use kobi_perms::{PermsServer, DEFAULT_TIMEOUT, PERMISSION_PROMPT_TOOL};
use serde_json::json;

const FAKE_CLAUDE: &str = env!("CARGO_BIN_EXE_fake-claude");
const FAKE_MCP: &str = env!("CARGO_BIN_EXE_fake-mcp");

fn scenario(name: &str) -> String {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios")
        .join(format!("{name}.json"))
        .display()
        .to_string()
}

/// Dossier vide propre au test.
fn work_dir(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kobi-testkit-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Options du faux `claude` : scénario, serveur de notes et, si fourni,
/// serveur de permission, comme pour le vrai.
fn options(name: &str, journal: &Path, perms: Option<&PermsServer>) -> ClaudeOptions {
    let mut servers = json!({
        "notes": { "command": FAKE_MCP, "args": ["--journal", journal] },
    });
    let mut args = vec!["--scenario".to_owned(), scenario(name)];
    if let Some(perms) = perms {
        servers["kobi"] = perms.mcp_config()["mcpServers"]["kobi"].clone();
        args.extend([
            "--permission-prompt-tool".into(),
            PERMISSION_PROMPT_TOOL.into(),
        ]);
    }
    args.extend([
        "--mcp-config".into(),
        json!({ "mcpServers": servers }).to_string(),
    ]);
    ClaudeOptions {
        program: PathBuf::from(FAKE_CLAUDE),
        args,
        ..Default::default()
    }
}

/// Envoie un message et traduit le tour en événements Kobi, jusqu'à la fin.
async fn turn(claude: &mut ClaudeCode, text: &str) -> Result<Vec<BrainEvent>, Error> {
    claude.send(text).await?;
    let mut translator = TurnTranslator::default();
    let mut events = Vec::new();
    while let Some(event) = claude.next_event().await {
        for e in translator.push(&event) {
            let end = matches!(e, BrainEvent::Done(_) | BrainEvent::Error(_));
            events.push(e);
            if end {
                return Ok(events);
            }
        }
    }
    Err(claude.exited().await)
}

fn text_of(events: &[BrainEvent]) -> String {
    match events.last() {
        Some(BrainEvent::Done(summary)) => summary.text.clone(),
        other => panic!("attendu Done, obtenu {other:?}"),
    }
}

/// Outils MCP appelés (sans les outils internes comme `ToolSearch`).
fn mcp_calls(events: &[BrainEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            BrainEvent::ToolCall { name, .. } if name.starts_with("mcp__") => Some(name.as_str()),
            _ => None,
        })
        .collect()
}

fn journal_of(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("journal.txt")).unwrap_or_default()
}

#[tokio::test]
async fn interpellation_avec_lecture_mcp() {
    let dir = work_dir("interpellation");
    let journal = dir.join("journal.txt");
    let mut claude = ClaudeCode::spawn(&options("interpellation", &journal, None)).unwrap();
    let events = turn(&mut claude, "Kobi, qu'est-ce qu'on a décidé ?")
        .await
        .unwrap();
    assert_eq!(mcp_calls(&events), ["mcp__notes__read_notes"]);
    let deltas: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            BrainEvent::Text(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        deltas.len() > 1,
        "le texte doit arriver en flux : {deltas:?}"
    );
    assert!(text_of(&events).contains("vendredi"));
    assert!(claude.session_id().is_some());
    // Deuxième question dans la même session.
    let events = turn(&mut claude, "Kobi, merci").await.unwrap();
    assert!(!text_of(&events).is_empty());
    assert!(claude
        .close(Duration::from_secs(5))
        .await
        .unwrap()
        .success());
}

#[tokio::test]
async fn ecriture_confirmee_puis_executee() {
    let dir = work_dir("confirmee");
    let (perms, mut requests) = PermsServer::start(DEFAULT_TIMEOUT).await.unwrap();
    let mut claude = ClaudeCode::spawn(&options(
        "permission",
        &dir.join("journal.txt"),
        Some(&perms),
    ))
    .unwrap();
    let card = tokio::spawn(async move {
        let pending = requests.recv().await.expect("demande de permission");
        let request = pending.request.clone();
        pending.allow();
        request
    });
    let events = turn(&mut claude, "Kobi, note qu'il faut envoyer le devis")
        .await
        .unwrap();
    let request = card.await.unwrap();
    assert_eq!(request.tool_name, "mcp__notes__write_note");
    assert_eq!(request.input["text"], "Il faut envoyer le devis au client.");
    assert_eq!(journal_of(&dir), "- Il faut envoyer le devis au client.\n");
    assert!(mcp_calls(&events).contains(&"mcp__notes__write_note"));
    assert!(text_of(&events).contains("C'est noté"));
    perms.shutdown().await;
}

#[tokio::test]
async fn ecriture_refusee_rien_n_est_ecrit() {
    let dir = work_dir("refusee");
    let (perms, mut requests) = PermsServer::start(DEFAULT_TIMEOUT).await.unwrap();
    let mut claude = ClaudeCode::spawn(&options(
        "permission",
        &dir.join("journal.txt"),
        Some(&perms),
    ))
    .unwrap();
    tokio::spawn(async move {
        requests
            .recv()
            .await
            .expect("demande de permission")
            .deny("Pas maintenant.");
    });
    let events = turn(&mut claude, "Kobi, note qu'il faut envoyer le devis")
        .await
        .unwrap();
    assert_eq!(journal_of(&dir), "", "aucune écriture sans confirmation");
    assert!(text_of(&events).contains("je n'ajoute rien"));
    perms.shutdown().await;
}

#[tokio::test]
async fn pas_de_reponse_a_la_carte_vaut_refus() {
    let dir = work_dir("sans-reponse");
    let (perms, mut requests) = PermsServer::start(Duration::from_millis(300))
        .await
        .unwrap();
    let mut claude = ClaudeCode::spawn(&options(
        "permission",
        &dir.join("journal.txt"),
        Some(&perms),
    ))
    .unwrap();
    // La carte s'affiche mais personne ne clique.
    let card = tokio::spawn(async move {
        let pending = requests.recv().await.expect("demande de permission");
        tokio::time::sleep(Duration::from_secs(2)).await;
        drop(pending);
    });
    let events = turn(&mut claude, "Kobi, note qu'il faut envoyer le devis")
        .await
        .unwrap();
    assert_eq!(journal_of(&dir), "");
    assert!(text_of(&events).contains("je n'ajoute rien"));
    card.abort();
    perms.shutdown().await;
}

#[tokio::test]
async fn reponse_trop_lente() {
    let dir = work_dir("lent");
    let mut claude = ClaudeCode::spawn(&options("lent", &dir.join("journal.txt"), None)).unwrap();
    let late = tokio::time::timeout(
        Duration::from_millis(500),
        turn(&mut claude, "Kobi, tu es là ?"),
    )
    .await;
    assert!(late.is_err(), "le tour aurait dû dépasser le délai");
    // Le process bloqué est tué à la fermeture.
    let status = claude.close(Duration::from_millis(200)).await.unwrap();
    assert!(!status.success());
}

#[tokio::test]
async fn plantage_en_cours_de_tour() {
    let dir = work_dir("plantage");
    let mut claude =
        ClaudeCode::spawn(&options("plantage", &dir.join("journal.txt"), None)).unwrap();
    let events = turn(&mut claude, "Kobi, premier point").await.unwrap();
    assert!(!text_of(&events).is_empty());
    let err = turn(&mut claude, "Kobi, second point").await.unwrap_err();
    let Error::Exited { status, stderr } = err else {
        panic!("attendu Exited, obtenu {err:?}")
    };
    assert_eq!(status.and_then(|s| s.code()), Some(1));
    assert!(stderr.contains("Connection error"), "stderr : {stderr}");
}

#[tokio::test]
async fn limite_d_usage() {
    let dir = work_dir("limite");
    let mut claude =
        ClaudeCode::spawn(&options("limite-usage", &dir.join("journal.txt"), None)).unwrap();
    let events = turn(&mut claude, "Kobi, résume").await.unwrap();
    assert!(matches!(
        events.last(),
        Some(BrainEvent::Error(BrainError::Api(m))) if m.contains("limit")
    ));
}

#[tokio::test]
async fn message_inattendu_arrete_le_faux_claude() {
    let dir = work_dir("inattendu");
    let mut claude =
        ClaudeCode::spawn(&options("interpellation", &dir.join("journal.txt"), None)).unwrap();
    let err = turn(&mut claude, "hors sujet").await.unwrap_err();
    assert!(matches!(err, Error::Exited { stderr, .. } if stderr.contains("hors sujet")));
}

// Mode écho, sans scénario.

fn echo() -> ClaudeOptions {
    ClaudeOptions {
        program: PathBuf::from(FAKE_CLAUDE),
        ..Default::default()
    }
}

#[tokio::test]
async fn echo_trois_questions_dans_la_meme_session() {
    let mut claude = ClaudeCode::spawn(&echo()).unwrap();
    for q in ["un", "deux", "trois"] {
        let mut deltas = Vec::new();
        let turn = claude.ask(q, |d| deltas.push(d.to_owned())).await.unwrap();
        assert_eq!(deltas, ["reçu : ", q]);
        assert_eq!(turn.text, format!("reçu : {q}"));
        assert_eq!(turn.result.result.as_deref(), Some(turn.text.as_str()));
    }
    assert_eq!(claude.session_id(), Some("fake-session"));
    let status = claude.close(Duration::from_secs(5)).await.unwrap();
    assert!(status.success());
}

#[tokio::test]
async fn echo_tour_en_erreur() {
    let mut claude = ClaudeCode::spawn(&echo()).unwrap();
    let err = claude.ask("erreur", |_| {}).await.unwrap_err();
    assert!(matches!(err, Error::Turn(t) if t.result.is_error));
    // La session reste utilisable après un tour en erreur.
    assert_eq!(claude.ask("ok", |_| {}).await.unwrap().text, "reçu : ok");
}

#[tokio::test]
async fn echo_process_qui_plante() {
    let mut claude = ClaudeCode::spawn(&echo()).unwrap();
    let err = claude.ask("crash", |_| {}).await.unwrap_err();
    let Error::Exited { status, stderr } = err else {
        panic!("attendu Exited, obtenu {err:?}")
    };
    assert_eq!(status.and_then(|s| s.code()), Some(3));
    assert!(stderr.contains("faux plantage"));
}

#[tokio::test]
async fn executable_introuvable() {
    let opts = ClaudeOptions {
        program: PathBuf::from("kobi-claude-introuvable"),
        ..Default::default()
    };
    assert!(matches!(ClaudeCode::spawn(&opts), Err(Error::Spawn { .. })));
}

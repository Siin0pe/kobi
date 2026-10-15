//! Session pilotée contre le faux `claude` du crate.

use std::path::PathBuf;
use std::time::Duration;

use kobi_brain::{ClaudeCode, ClaudeOptions, Error};

fn options() -> ClaudeOptions {
    ClaudeOptions {
        program: PathBuf::from(env!("CARGO_BIN_EXE_fake-claude")),
        ..Default::default()
    }
}

#[tokio::test]
async fn trois_questions_dans_la_meme_session() {
    let mut claude = ClaudeCode::spawn(&options()).unwrap();
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
async fn tour_en_erreur() {
    let mut claude = ClaudeCode::spawn(&options()).unwrap();
    let err = claude.ask("erreur", |_| {}).await.unwrap_err();
    assert!(matches!(err, Error::Turn(t) if t.result.is_error));
    // La session reste utilisable après un tour en erreur.
    assert_eq!(claude.ask("ok", |_| {}).await.unwrap().text, "reçu : ok");
}

#[tokio::test]
async fn process_qui_plante() {
    let mut claude = ClaudeCode::spawn(&options()).unwrap();
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

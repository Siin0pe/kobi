//! Joue les 10 cas de non-régression du prompt système de Kobi.
//!
//! ```text
//! cargo build -p kobi-testkit --bins
//! target/debug/cas-prompt [--program claude] [--model sonnet] [--only <id>]
//!                         [--cases <cas.json>] [--out <rapport.json>]
//! ```
//!
//! Chaque cas ouvre une session de réunion neuve (`ClaudeCodeBrain`, prompt
//! système de kobi-brain) avec le faux serveur MCP `notes` ; toutes les
//! demandes de permission sont refusées. Le code de sortie vaut 1 si un cas
//! échoue. Contre le vrai Claude, chaque cas consomme une requête.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use kobi_brain::{
    Ask, Brain, BrainEvent, ClaudeCodeBrain, MeetingConfig, NotesUpdate, ReportRequest,
    SYSTEM_PROMPT_VERSION,
};
use kobi_testkit::cases::{evaluate, Case, CaseFile, Kind, Outcome};
use serde_json::{json, Map};

/// Délai maximal d'un cas.
const CASE_TIMEOUT: Duration = Duration::from_secs(180);

struct Args {
    program: PathBuf,
    model: Option<String>,
    only: Option<String>,
    cases: PathBuf,
    out: Option<PathBuf>,
}

fn parse_args() -> Args {
    let mut args = Args {
        program: PathBuf::from("claude"),
        model: None,
        only: None,
        cases: Path::new(env!("CARGO_MANIFEST_DIR")).join("prompt-cases/cas.json"),
        out: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || {
            it.next()
                .unwrap_or_else(|| panic!("valeur attendue après {arg}"))
        };
        match arg.as_str() {
            "--program" => {
                // Chemin relatif : rendu absolu, la session change de dossier.
                let program = PathBuf::from(value());
                args.program = if program.components().count() > 1 {
                    std::fs::canonicalize(&program).unwrap_or(program)
                } else {
                    program
                };
            }
            "--model" => args.model = Some(value()),
            "--only" => args.only = Some(value()),
            "--cases" => args.cases = value().into(),
            "--out" => args.out = Some(value().into()),
            other => panic!("argument inconnu : {other}"),
        }
    }
    args
}

/// Le faux serveur MCP est compilé à côté de ce binaire.
fn fake_mcp() -> PathBuf {
    let exe = std::env::current_exe().expect("chemin de cas-prompt");
    exe.with_file_name(format!("fake-mcp{}", std::env::consts::EXE_SUFFIX))
}

async fn run_case(args: &Args, case: &Case, dir: &Path) -> Outcome {
    let journal = dir.join("journal.txt");
    let mut servers = Map::new();
    servers.insert(
        "notes".into(),
        json!({ "command": fake_mcp(), "args": ["--journal", journal] }),
    );
    let config = MeetingConfig {
        program: args.program.clone(),
        cwd: Some(dir.to_owned()),
        mcp_servers: servers,
        model: args.model.clone(),
        // Ni réglages ni CLAUDE.md de l'utilisateur : seul le prompt de Kobi.
        extra_args: vec!["--setting-sources".into(), "project".into()],
        ..Default::default()
    };
    let mut outcome = Outcome::default();
    let mut brain = match ClaudeCodeBrain::start(config).await {
        Ok(brain) => brain,
        Err(e) => {
            outcome.error = Some(e.to_string());
            return outcome;
        }
    };
    let mut turn = match case.kind {
        Kind::Ask => brain.ask(Ask {
            question: case.question.clone(),
            transcript: case.transcript.clone(),
            notes: case.notes.clone(),
        }),
        Kind::Notes => brain.update_notes(NotesUpdate {
            transcript: case.transcript.clone(),
            notes: case.notes.clone(),
            ..Default::default()
        }),
        Kind::Report => brain.final_report(ReportRequest {
            transcript: case.transcript.clone(),
            notes: case.notes.clone(),
            pending_actions: Vec::new(),
        }),
    };
    let deadline = tokio::time::sleep(CASE_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        let event = tokio::select! {
            event = turn.next() => event,
            _ = &mut deadline => {
                outcome.error = Some(format!("pas de fin en {} s", CASE_TIMEOUT.as_secs()));
                break;
            }
        };
        match event {
            Some(BrainEvent::Text(_) | BrainEvent::OutilRendu { .. } | BrainEvent::Vie) => {}
            Some(BrainEvent::ToolCall { name, .. }) => outcome.tools.push(name),
            Some(BrainEvent::Permission(pending)) => {
                outcome.permissions.push(pending.request.tool_name.clone());
                pending.deny("Pas maintenant.");
            }
            Some(BrainEvent::Done(summary)) => {
                outcome.text = summary.text;
                break;
            }
            Some(BrainEvent::Error(e)) => {
                outcome.error = Some(e.to_string());
                break;
            }
            None => break,
        }
    }
    brain.close(Duration::from_secs(10)).await;
    outcome.written = std::fs::read_to_string(&journal).unwrap_or_default();
    outcome
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let file: CaseFile = serde_json::from_str(
        &std::fs::read_to_string(&args.cases)
            .unwrap_or_else(|e| panic!("{} : {e}", args.cases.display())),
    )
    .expect("fichier de cas");
    if file.prompt_version != SYSTEM_PROMPT_VERSION {
        eprintln!(
            "attention : cas écrits pour le prompt v{}, prompt actuel v{SYSTEM_PROMPT_VERSION}",
            file.prompt_version
        );
    }
    println!(
        "prompt système v{SYSTEM_PROMPT_VERSION}, programme {}, modèle {}",
        args.program.display(),
        args.model.as_deref().unwrap_or("par défaut")
    );
    let root = std::env::temp_dir().join(format!("kobi-cas-prompt-{}", std::process::id()));
    let mut report = Vec::new();
    let mut failed = 0;
    for case in &file.cases {
        if args.only.as_ref().is_some_and(|only| only != &case.id) {
            continue;
        }
        let dir = root.join(&case.id);
        std::fs::create_dir_all(&dir).expect("dossier du cas");
        let t0 = Instant::now();
        let outcome = run_case(&args, case, &dir).await;
        let failures = evaluate(case, &outcome);
        let seconds = t0.elapsed().as_secs_f64();
        if !failures.is_empty() {
            failed += 1;
        }
        println!(
            "\n{} {} ({seconds:.1} s) : {}",
            if failures.is_empty() {
                "OK  "
            } else {
                "ÉCHEC"
            },
            case.id,
            case.description
        );
        println!(
            "  réponse : {}",
            outcome.text.trim().replace('\n', "\n            ")
        );
        if !outcome.tools.is_empty() || !outcome.permissions.is_empty() {
            println!(
                "  outils : {:?} ; permissions : {:?}",
                outcome.tools, outcome.permissions
            );
        }
        for failure in &failures {
            println!("  ✗ {failure}");
        }
        report.push(json!({
            "id": case.id, "ok": failures.is_empty(), "failures": failures,
            "seconds": seconds, "outcome": outcome,
        }));
    }
    let _ = std::fs::remove_dir_all(&root);
    println!("\n{} cas, {} échec(s)", report.len(), failed);
    if let Some(out) = &args.out {
        let report = json!({
            "prompt_version": SYSTEM_PROMPT_VERSION,
            "program": args.program, "model": args.model, "cases": report,
        });
        std::fs::write(out, serde_json::to_string_pretty(&report).unwrap())
            .unwrap_or_else(|e| panic!("{} : {e}", out.display()));
    }
    std::process::exit(i32::from(failed > 0));
}

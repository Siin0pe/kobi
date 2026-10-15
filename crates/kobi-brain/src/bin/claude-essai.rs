//! Binaire d'essai : pose plusieurs questions à la suite dans une même
//! session Claude Code et affiche les réponses en flux.
//!
//! ```text
//! cargo run -p kobi-brain --bin claude-essai -- [questions…]
//! ```
//!
//! Sans argument, trois questions courtes dont la troisième dépend des deux
//! premières (preuve que le contexte est gardé). Variables : `KOBI_CLAUDE`
//! (exécutable, `claude` par défaut), `KOBI_CLAUDE_ARGS` (arguments en plus,
//! séparés par des espaces).

use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kobi_brain::{ClaudeCode, ClaudeOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut questions: Vec<String> = std::env::args().skip(1).collect();
    if questions.is_empty() {
        questions = [
            "Retiens le nombre 7. Réponds en une phrase courte.",
            "Retiens aussi le mot « pomme ». Réponds en une phrase courte.",
            "Quels sont le nombre et le mot que je t'ai demandé de retenir ? Une phrase.",
        ]
        .map(String::from)
        .into();
    }

    let mut opts = ClaudeOptions {
        // Aucun MCP : l'essai ne dépend pas de la config de l'utilisateur.
        args: vec!["--strict-mcp-config".into()],
        ..Default::default()
    };
    if let Some(program) = std::env::var_os("KOBI_CLAUDE") {
        opts.program = PathBuf::from(program);
    }
    if let Ok(extra) = std::env::var("KOBI_CLAUDE_ARGS") {
        opts.args.extend(extra.split_whitespace().map(String::from));
    }

    let started = Instant::now();
    let mut claude = ClaudeCode::spawn(&opts)?;
    for (i, question) in questions.iter().enumerate() {
        println!("\n> [{}] {question}", i + 1);
        let asked = Instant::now();
        let mut first: Option<Duration> = None;
        let turn = claude
            .ask(question, |delta| {
                first.get_or_insert_with(|| asked.elapsed());
                print!("{delta}");
                std::io::stdout().flush().ok();
            })
            .await?;
        println!(
            "\n  (premier token {:.2} s, tour {:.2} s, session {})",
            first.unwrap_or_default().as_secs_f64(),
            asked.elapsed().as_secs_f64(),
            turn.result.session_id.as_deref().unwrap_or("?"),
        );
    }
    let status = claude.close(Duration::from_secs(10)).await?;
    println!(
        "\nprocess terminé ({status}) après {:.2} s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

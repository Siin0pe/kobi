//! Détecte Claude Code sur cette machine, sans appeler l'API.
//!
//! ```text
//! cargo run -p kobi-brain --example detecter-claude
//! ```

use kobi_brain::claude_code::detect::{candidates, detect, PROBE_TIMEOUT};

#[tokio::main]
async fn main() {
    for program in candidates() {
        println!("candidat : {}", program.display());
    }
    let detection = detect(PROBE_TIMEOUT).await;
    println!("{detection:?}");
    if let Some(advice) = detection.advice() {
        println!("{advice}");
    }
}

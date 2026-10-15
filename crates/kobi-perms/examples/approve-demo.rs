//! Démo de `--permission-prompt-tool` : Claude Code veut écrire un fichier,
//! le serveur « kobi » bloque l'appel, le refuse, puis l'autorise.
//!
//! ```text
//! cargo run -p kobi-perms --example approve-demo -- <dossier de travail vide>
//! ```
//!
//! 1. Première demande d'écriture : retenue 3 s (l'appel attend), puis refusée.
//!    Le fichier ne doit pas exister.
//! 2. Seconde demande : autorisée. Le fichier doit exister.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use kobi_brain::{ClaudeCode, ClaudeOptions};
use kobi_perms::{Decision, PermsServer, DEFAULT_TIMEOUT, PERMISSION_PROMPT_TOOL};

const FILE: &str = "kobi-demo.txt";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("dossier de travail attendu")?,
    );
    std::fs::create_dir_all(&dir)?;
    let target = dir.join(FILE);
    let _ = std::fs::remove_file(&target);

    let (server, mut requests) = PermsServer::start(DEFAULT_TIMEOUT).await?;
    let t0 = Instant::now();
    let log = move |msg: String| println!("[{:>6.2} s] {msg}", t0.elapsed().as_secs_f64());
    log(format!("serveur kobi sur {}", server.url()));

    // Kobi : 1re demande retenue puis refusée, les suivantes autorisées.
    tokio::spawn(async move {
        let mut n = 0;
        while let Some(pending) = requests.recv().await {
            n += 1;
            let r = &pending.request;
            log(format!("carte : {} {}", r.tool_name, r.input));
            if n == 1 {
                tokio::time::sleep(Duration::from_secs(3)).await;
                log("clic : Pas maintenant".into());
                pending.decide(Decision::Deny("Refusé par l'utilisateur de Kobi.".into()));
            } else {
                log("clic : Créer".into());
                pending.allow();
            }
        }
    });

    let opts = ClaudeOptions {
        cwd: Some(dir.clone()),
        args: [
            "--setting-sources",
            "project",
            "--strict-mcp-config",
            "--mcp-config",
            &server.mcp_config().to_string(),
            // Sinon le mode peut venir des réglages ou de l'environnement
            // (`auto`, `acceptEdits`…) et l'écriture passe sans demander.
            "--permission-mode",
            "default",
            "--permission-prompt-tool",
            PERMISSION_PROMPT_TOOL,
            "--disallowedTools",
            "Bash PowerShell",
            "--model",
            "haiku",
        ]
        .map(String::from)
        .into(),
        ..Default::default()
    };
    let mut claude = ClaudeCode::spawn(&opts)?;

    let questions = [
        format!(
            "Crée le fichier {FILE} contenant « bonjour » avec l'outil Write. \
             Si c'est refusé, ne réessaie pas et dis-le en une phrase."
        ),
        format!("Réessaie maintenant de créer {FILE}, une seule fois. Réponds en une phrase."),
    ];
    for q in &questions {
        log(format!("question : {q}"));
        let turn = claude.ask(q, |_| {}).await?;
        log(format!("réponse : {}", turn.text.trim()));
        log(format!(
            "{FILE} existe : {}",
            if target.exists() { "oui" } else { "non" }
        ));
    }
    claude.close(Duration::from_secs(10)).await?;
    server.shutdown().await;
    Ok(())
}

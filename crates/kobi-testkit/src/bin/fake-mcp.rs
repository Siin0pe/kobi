//! Faux serveur MCP (stdio) : un outil de lecture, un outil d'écriture.
//!
//! ```text
//! fake-mcp --journal <fichier>
//! ```
//!
//! `write_note` ajoute une ligne au journal ; c'est ce que les tests
//! regardent pour savoir si une écriture a vraiment eu lieu. `read_notes`
//! renvoie une note fixe suivie du journal.

use std::io::Write;
use std::path::PathBuf;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt};
use serde::Deserialize;

/// Note toujours présente, pour reconnaître une vraie lecture.
const FIXED_NOTE: &str = "- Décision : livrer la v1 vendredi.";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
struct WriteNote {
    /// Texte de la note.
    text: String,
}

#[derive(Clone)]
struct Notes {
    journal: Option<PathBuf>,
    #[allow(dead_code, reason = "lu par la macro tool_handler")]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Notes {
    #[tool(
        description = "Lit les notes de la réunion.",
        annotations(read_only_hint = true)
    )]
    fn read_notes(&self) -> String {
        let written = self
            .journal
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        format!("{FIXED_NOTE}\n{written}").trim_end().to_owned()
    }

    #[tool(
        description = "Ajoute une note à la réunion.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    fn write_note(&self, Parameters(note): Parameters<WriteNote>) -> String {
        if let Some(path) = &self.journal {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .expect("journal du faux serveur MCP");
            writeln!(file, "- {}", note.text).expect("écriture du journal");
        }
        format!("Note ajoutée : {}", note.text)
    }
}

#[tool_handler]
impl ServerHandler for Notes {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Faux serveur MCP de notes, pour les tests de Kobi.")
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let mut journal = None;
    while let Some(arg) = args.next() {
        if arg == "--journal" {
            journal = args.next().map(PathBuf::from);
        }
    }
    let notes = Notes {
        journal,
        tool_router: Notes::tool_router(),
    };
    notes
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}

//! Process `claude` persistant : messages sur stdin, événements sur stdout.

use std::path::PathBuf;
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::sync::mpsc;

use super::event::{Event, TurnResult};

/// Taille maximale de stderr gardée pour les messages d'erreur.
const STDERR_MAX: usize = 8 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("impossible de lancer `{program}` : {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("écriture sur stdin de claude : {0}")]
    Io(#[from] std::io::Error),
    #[error("stdin de claude déjà fermé")]
    StdinClosed,
    #[error("le process claude s'est arrêté ({status:?}) : {stderr}")]
    Exited {
        status: Option<ExitStatus>,
        stderr: String,
    },
    #[error("le tour s'est terminé en erreur : {0:?}")]
    Turn(Box<TurnResult>),
}

/// Options de lancement de `claude`.
#[derive(Debug, Clone)]
pub struct ClaudeOptions {
    /// Exécutable, `claude` par défaut (cherché dans le PATH).
    pub program: PathBuf,
    /// Dossier de travail du process.
    pub cwd: Option<PathBuf>,
    /// Ajoute `--include-partial-messages` pour recevoir le texte en flux.
    pub partial_messages: bool,
    /// Arguments supplémentaires (`--mcp-config`, `--add-dir`, `--model`…).
    pub args: Vec<String>,
}

impl Default for ClaudeOptions {
    fn default() -> Self {
        Self {
            program: PathBuf::from("claude"),
            cwd: None,
            partial_messages: true,
            args: Vec::new(),
        }
    }
}

impl ClaudeOptions {
    /// Ligne de commande complète, sans l'exécutable.
    pub fn command_args(&self) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            // Obligatoire avec `-p --output-format stream-json`.
            "--verbose",
        ]
        .map(String::from)
        .into();
        if self.partial_messages {
            args.push("--include-partial-messages".into());
        }
        args.extend(self.args.iter().cloned());
        args
    }
}

/// Une session Claude Code : un process, plusieurs messages successifs.
pub struct ClaudeCode {
    child: Child,
    stdin: Option<ChildStdin>,
    events: mpsc::UnboundedReceiver<Event>,
    stderr: Arc<Mutex<String>>,
    session_id: Option<String>,
}

impl ClaudeCode {
    /// Lance le process. Il attend ensuite le premier message sur stdin.
    pub fn spawn(opts: &ClaudeOptions) -> Result<Self, Error> {
        let mut cmd = Command::new(&opts.program);
        cmd.args(opts.command_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        if let Some(cwd) = &opts.cwd {
            cmd.current_dir(cwd);
        }
        let mut child = cmd.spawn().map_err(|source| Error::Spawn {
            program: opts.program.display().to_string(),
            source,
        })?;

        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("stdout est capturé");
        let stderr_pipe = child.stderr.take().expect("stderr est capturé");

        let (tx, events) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                if line.trim().is_empty() {
                    continue;
                }
                let event = Event::parse(&line).unwrap_or(Event::Other(Value::String(line)));
                if tx.send(event).is_err() {
                    break;
                }
            }
        });

        let stderr = Arc::new(Mutex::new(String::new()));
        let sink = stderr.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr_pipe).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let mut buf = sink.lock().expect("verrou stderr");
                buf.push_str(&line);
                buf.push('\n');
                if buf.len() > STDERR_MAX {
                    let cut = buf.len() - STDERR_MAX;
                    let cut = (cut..buf.len())
                        .find(|&i| buf.is_char_boundary(i))
                        .unwrap_or(0);
                    buf.drain(..cut);
                }
            }
        });

        Ok(Self {
            child,
            stdin,
            events,
            stderr,
            session_id: None,
        })
    }

    /// Identifiant de session, connu après le premier événement `init`.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Envoie un message utilisateur, sans attendre la réponse.
    pub async fn send(&mut self, text: &str) -> Result<(), Error> {
        let stdin = self.stdin.as_mut().ok_or(Error::StdinClosed)?;
        let mut line = json!({
            "type": "user",
            "message": { "role": "user", "content": text },
        })
        .to_string();
        line.push('\n');
        stdin.write_all(line.as_bytes()).await?;
        stdin.flush().await?;
        Ok(())
    }

    /// Prochain événement ; `None` quand stdout est fermé.
    pub async fn next_event(&mut self) -> Option<Event> {
        let event = self.events.recv().await?;
        if let Event::System(s) = &event {
            if s.session_id.is_some() {
                self.session_id.clone_from(&s.session_id);
            }
        }
        Some(event)
    }

    /// Pose une question et lit le tour jusqu'à son `result`, en passant
    /// chaque morceau de texte à `on_delta` dès qu'il arrive.
    pub async fn ask(
        &mut self,
        text: &str,
        mut on_delta: impl FnMut(&str),
    ) -> Result<TurnResult, Error> {
        self.send(text).await?;
        let mut answer = String::new();
        loop {
            let Some(event) = self.next_event().await else {
                return Err(self.exited().await);
            };
            if let Some(delta) = event.text_delta() {
                on_delta(delta);
                answer.push_str(delta);
            }
            if let Event::Result(result) = event {
                if answer.is_empty() {
                    // Sans `--include-partial-messages`, seul le résultat porte le texte.
                    if let Some(text) = &result.result {
                        on_delta(text);
                        answer.clone_from(text);
                    }
                }
                let turn = TurnResult {
                    text: answer,
                    result,
                };
                if turn.result.is_error {
                    return Err(Error::Turn(Box::new(turn)));
                }
                return Ok(turn);
            }
        }
    }

    /// Ferme stdin : claude termine le tour en cours puis s'arrête. Au-delà
    /// de `timeout`, le process est tué.
    pub async fn close(mut self, timeout: Duration) -> Result<ExitStatus, Error> {
        drop(self.stdin.take());
        match tokio::time::timeout(timeout, self.child.wait()).await {
            Ok(status) => Ok(status?),
            Err(_) => {
                self.child.kill().await?;
                Ok(self.child.wait().await?)
            }
        }
    }

    /// Diagnostic d'un process arrêté : code de sortie et fin de stderr. À
    /// appeler quand [`ClaudeCode::next_event`] renvoie `None`.
    pub async fn exited(&mut self) -> Error {
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .ok()
            .and_then(Result::ok);
        let stderr = self.stderr.lock().expect("verrou stderr").clone();
        Error::Exited { status, stderr }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ligne_de_commande() {
        let opts = ClaudeOptions {
            args: vec!["--model".into(), "haiku".into()],
            ..Default::default()
        };
        assert_eq!(
            opts.command_args(),
            [
                "-p",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--model",
                "haiku",
            ]
        );
    }
}

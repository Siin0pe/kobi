//! Backend Claude Code : une session `claude` persistante par réunion.
//!
//! Le process est lancé à l'ouverture de la réunion et fermé à la fin ; les
//! MCP restent chauds et Claude garde le contexte entre les questions. Le
//! serveur de permission « kobi » tourne dans le process de Kobi et ses
//! demandes sont insérées dans le tour en cours.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use kobi_perms::{ApprovalRequest, PermsServer, PERMISSION_PROMPT_TOOL};
use serde_json::{json, Map, Value};
use tokio::sync::{mpsc, oneshot};

use crate::brain::{Ask, Brain, BrainError, BrainEvent, NotesUpdate, ReportRequest, Turn};

use super::session::{ClaudeCode, ClaudeOptions};
use super::translate::TurnTranslator;

/// Prompt système de Kobi, ajouté à celui de Claude Code
/// (`--append-system-prompt`). Toute modification change
/// [`SYSTEM_PROMPT_VERSION`] et se valide en relançant les 10 cas de
/// non-régression (`cas-prompt` de kobi-testkit) contre le vrai Claude.
pub const SYSTEM_PROMPT: &str = include_str!("../../prompts/systeme.md");
/// Version de [`SYSTEM_PROMPT`].
pub const SYSTEM_PROMPT_VERSION: u32 = 2;

/// Outils interdits pendant une réunion : shell, édition et écriture de
/// fichiers (dossier de conception, Permissions).
pub const MEETING_DISALLOWED_TOOLS: &[&str] = &[
    "Bash",
    // Lecture et arrêt des commandes lancées en arrière-plan.
    "BashOutput",
    "KillShell",
    "PowerShell",
    "Edit",
    "MultiEdit",
    "Write",
    "NotebookEdit",
];

/// Réglages d'une session de réunion.
#[derive(Debug, Clone)]
pub struct MeetingConfig {
    /// Exécutable `claude` (voir la détection).
    pub program: PathBuf,
    /// Dossier de travail : le dossier de la réunion.
    pub cwd: Option<PathBuf>,
    /// Serveurs MCP du workspace (contenu de `mcpServers`). Le serveur
    /// « kobi » y est ajouté.
    pub mcp_servers: Map<String, Value>,
    /// Dossiers lisibles : codebases du workspace et dossier des réunions.
    pub add_dirs: Vec<PathBuf>,
    pub disallowed_tools: Vec<String>,
    pub model: Option<String>,
    /// Ajouté au prompt système de Claude Code ; [`SYSTEM_PROMPT`] par défaut.
    pub system_prompt: Option<String>,
    /// Délai de réponse à une carte de confirmation avant refus.
    pub permission_timeout: Duration,
    /// Outils autorisés sans carte : lecture seule (annotations des
    /// serveurs MCP) et liste blanche du workspace.
    pub permission_rules: kobi_perms::Rules,
    /// Arguments en plus, passés tels quels.
    pub extra_args: Vec<String>,
    /// Session sans serveur de permission : s'ils sont donnés, ces arguments
    /// remplacent ceux de [`MeetingConfig::args`] et aucune carte n'est
    /// possible (Kobi en lecture seule, KOB-138).
    pub fixed_args: Option<Vec<String>>,
    /// Arguments construits par l'application une fois le serveur de
    /// permission lancé (KOB-155) : ils reçoivent sa config MCP
    /// (`{"type":"http","url":…}`) et remplacent [`MeetingConfig::args`].
    /// Prioritaire sur `fixed_args`.
    pub args_with_perms: Option<ArgsWithPerms>,
    /// Variables d'environnement du process `claude` (secrets MCP).
    pub env: Vec<(String, String)>,
}

/// Fabrique d'arguments de [`MeetingConfig::args_with_perms`].
#[derive(Clone)]
pub struct ArgsWithPerms(pub Arc<ArgsFn>);

/// Config MCP du serveur de permission → arguments de `claude`.
pub type ArgsFn = dyn Fn(&Value) -> Result<Vec<String>, String> + Send + Sync;

impl std::fmt::Debug for ArgsWithPerms {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ArgsWithPerms(..)")
    }
}

impl Default for MeetingConfig {
    fn default() -> Self {
        Self {
            program: PathBuf::from("claude"),
            cwd: None,
            mcp_servers: Map::new(),
            add_dirs: Vec::new(),
            disallowed_tools: MEETING_DISALLOWED_TOOLS
                .iter()
                .map(|t| t.to_string())
                .collect(),
            model: None,
            system_prompt: Some(SYSTEM_PROMPT.to_owned()),
            permission_timeout: kobi_perms::DEFAULT_TIMEOUT,
            permission_rules: kobi_perms::Rules::default(),
            extra_args: Vec::new(),
            fixed_args: None,
            args_with_perms: None,
            env: Vec::new(),
        }
    }
}

impl MeetingConfig {
    /// Arguments de `claude` (après ceux du mode stream-json), avec la
    /// config MCP finale.
    pub fn args(&self, kobi_server: &Value) -> Vec<String> {
        let mut servers = self.mcp_servers.clone();
        servers.insert(kobi_perms::SERVER_NAME.into(), kobi_server.clone());
        let mut args = vec![
            // Seuls les MCP du workspace, pas ceux de la config globale.
            "--strict-mcp-config".to_owned(),
            "--mcp-config".to_owned(),
            json!({ "mcpServers": servers }).to_string(),
            // Sinon le mode peut venir des réglages (`acceptEdits`, `auto`…).
            "--permission-mode".to_owned(),
            "default".to_owned(),
            "--permission-prompt-tool".to_owned(),
            PERMISSION_PROMPT_TOOL.to_owned(),
        ];
        for dir in &self.add_dirs {
            args.push("--add-dir".to_owned());
            args.push(dir.display().to_string());
        }
        if !self.disallowed_tools.is_empty() {
            args.push("--disallowedTools".to_owned());
            args.push(self.disallowed_tools.join(" "));
        }
        if let Some(model) = &self.model {
            args.push("--model".to_owned());
            args.push(model.clone());
        }
        if let Some(prompt) = &self.system_prompt {
            args.push("--append-system-prompt".to_owned());
            args.push(prompt.clone());
        }
        args.extend(self.extra_args.iter().cloned());
        args
    }
}

/// Message utilisateur envoyé à Claude pour chaque appel.
fn message(kind: &str, transcript: &str, notes: &str, body: &str) -> String {
    let mut text = format!("[{kind}]\n");
    if !transcript.is_empty() {
        text.push_str(&format!("\n## Nouvelle transcription\n{transcript}\n"));
    }
    if !notes.is_empty() {
        text.push_str(&format!("\n## Notes courantes\n{notes}\n"));
    }
    if !body.is_empty() {
        text.push_str(&format!("\n{body}\n"));
    }
    text
}

/// Message d'une interpellation (commun aux deux backends).
pub(crate) fn texte_ask(ask: &Ask) -> String {
    message(
        "interpellation",
        &ask.transcript,
        &ask.notes,
        &format!("## Question\n{}", ask.question),
    )
}

/// Message d'un bloc de notes ; en continu, avec la consigne des
/// propositions (KOB-72).
pub(crate) fn texte_notes(update: &NotesUpdate) -> String {
    let mut body = String::from(
        "Mets les notes à jour et renvoie-les en entier, en markdown brut, sans bloc de code autour.",
    );
    if update.propositions {
        body.push_str("\n\n");
        body.push_str(CONSIGNE_PROPOSITIONS);
    }
    message("notes", &update.transcript, &update.notes, &body)
}

/// Message de fin de réunion.
pub(crate) fn texte_compte_rendu(report: &ReportRequest) -> String {
    let mut body =
        "La réunion est terminée. Écris le compte rendu complet, en markdown brut, sans bloc de code autour."
            .to_owned();
    if !report.pending_actions.is_empty() {
        body.push_str("\n\n## Actions non faites ou échouées\n");
        for action in &report.pending_actions {
            body.push_str(&format!("- {action}\n"));
        }
    }
    message("compte rendu", &report.transcript, &report.notes, &body)
}

/// Notes en continu (KOB-72, expérimental) : Claude peut proposer une
/// action. Elle passe par la carte de Kobi : rien n'est fait sans un oui.
pub const CONSIGNE_PROPOSITIONS: &str = "\
Mode continu (expérimental) : si cette nouvelle transcription décide ou demande clairement \
une action concrète que tes outils savent faire (créer un ticket, une page, un rappel…) et \
qu'elle n'est pas déjà faite ni proposée dans les notes, propose-la en appelant l'outil qui \
l'écrit, une seule par bloc. Kobi montre une carte à l'utilisateur et n'agit qu'après son « oui » ; \
refusée, n'insiste pas. N'invente aucune action, ne pilote pas la réunion. Puis renvoie les notes.";

/// Outils internes de Claude Code qui ne sont pas des sources à montrer.
const INTERNAL_TOOLS: &[&str] = &["ToolSearch", "TodoWrite"];

struct Job {
    text: String,
    events: mpsc::Sender<BrainEvent>,
}

/// Session Claude Code d'une réunion, vue comme un [`Brain`].
pub struct ClaudeCodeBrain {
    jobs: mpsc::Sender<Job>,
    busy: Arc<AtomicBool>,
    stop: Option<oneshot::Sender<Duration>>,
    task: tokio::task::JoinHandle<()>,
    pid: Option<u32>,
}

impl ClaudeCodeBrain {
    /// Ouvre la session : serveur de permission (sauf avec
    /// [`MeetingConfig::fixed_args`]) puis process `claude`.
    pub async fn start(config: MeetingConfig) -> Result<Self, BrainError> {
        let (perms, approvals, args) = match &config.fixed_args {
            Some(args) if config.args_with_perms.is_none() => {
                // Canal fermé : aucune demande de permission n'arrivera.
                let (_, approvals) = mpsc::channel(1);
                (None, approvals, args.clone())
            }
            _ => {
                let (perms, approvals) = PermsServer::start_with_rules(
                    config.permission_timeout,
                    config.permission_rules.clone(),
                )
                .await
                .map_err(|e| BrainError::Unavailable(format!("serveur de permission : {e}")))?;
                let kobi = perms.mcp_config()["mcpServers"][kobi_perms::SERVER_NAME].clone();
                let args = match &config.args_with_perms {
                    // La fabrique peut lire le disque (carte des dépôts) :
                    // hors du runtime.
                    Some(fabrique) => match tokio::task::spawn_blocking({
                        let (fabrique, kobi) = (fabrique.0.clone(), kobi.clone());
                        move || fabrique(&kobi)
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()))
                    {
                        Ok(args) => args,
                        Err(e) => {
                            perms.shutdown().await;
                            return Err(BrainError::Unavailable(e));
                        }
                    },
                    None => config.args(&kobi),
                };
                (Some(perms), approvals, args)
            }
        };
        let opts = ClaudeOptions {
            program: config.program.clone(),
            cwd: config.cwd.clone(),
            partial_messages: true,
            args,
            env: config.env.clone(),
        };
        let claude = match ClaudeCode::spawn(&opts) {
            Ok(c) => c,
            Err(e) => {
                if let Some(perms) = perms {
                    perms.shutdown().await;
                }
                return Err(BrainError::Unavailable(e.to_string()));
            }
        };
        let pid = claude.id();
        let (jobs, jobs_rx) = mpsc::channel(1);
        let (stop, stop_rx) = oneshot::channel();
        let busy = Arc::new(AtomicBool::new(false));
        let task = tokio::spawn(run(
            claude,
            perms,
            approvals,
            jobs_rx,
            stop_rx,
            busy.clone(),
        ));
        Ok(Self {
            jobs,
            busy,
            stop: Some(stop),
            task,
            pid,
        })
    }

    /// Identifiant du process `claude` (le même pendant toute la réunion).
    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    /// Ferme la session à la fin de la réunion : stdin fermé, process tué
    /// au-delà de `timeout`, serveur de permission arrêté.
    pub async fn close(mut self, timeout: Duration) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(timeout);
        }
        let _ = (&mut self.task).await;
    }

    fn submit(&mut self, text: String) -> Turn {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Turn::failed(BrainError::Busy);
        }
        let (events, turn) = Turn::channel();
        if let Err(mpsc::error::TrySendError::Full(job) | mpsc::error::TrySendError::Closed(job)) =
            self.jobs.try_send(Job { text, events })
        {
            self.busy.store(false, Ordering::SeqCst);
            let _ = job.events.try_send(BrainEvent::Error(BrainError::Crashed(
                "session Claude Code fermée".into(),
            )));
        }
        turn
    }
}

/// Session abandonnée (tour interrompu ou tâche annulée) : la boucle est
/// arrêtée net et le process `claude` tué avec elle (`kill_on_drop`).
/// Après [`ClaudeCodeBrain::close`], la boucle est déjà finie.
impl Drop for ClaudeCodeBrain {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Brain for ClaudeCodeBrain {
    fn name(&self) -> &str {
        "claude-code"
    }

    fn ask(&mut self, ask: Ask) -> Turn {
        self.submit(texte_ask(&ask))
    }

    fn update_notes(&mut self, update: NotesUpdate) -> Turn {
        self.submit(texte_notes(&update))
    }

    fn final_report(&mut self, report: ReportRequest) -> Turn {
        self.submit(texte_compte_rendu(&report))
    }
}

/// Boucle de la session : un tour à la fois, permissions insérées dans le
/// tour en cours.
async fn run(
    mut claude: ClaudeCode,
    perms: Option<PermsServer>,
    mut approvals: mpsc::Receiver<ApprovalRequest>,
    mut jobs: mpsc::Receiver<Job>,
    mut stop: oneshot::Receiver<Duration>,
    busy: Arc<AtomicBool>,
) {
    let close_timeout = loop {
        tokio::select! {
            timeout = &mut stop => break timeout.unwrap_or(Duration::from_secs(5)),
            // Hors tour, aucune écriture ne peut être confirmée.
            Some(pending) = approvals.recv() => pending.deny("Aucune question en cours."),
            job = jobs.recv() => {
                let Some(job) = job else { break Duration::from_secs(5) };
                let end = turn(&mut claude, &mut approvals, &job).await;
                busy.store(false, Ordering::SeqCst);
                let crashed = matches!(end, BrainEvent::Error(BrainError::Crashed(_)));
                let _ = job.events.send(end).await;
                if crashed {
                    break Duration::ZERO;
                }
            }
        }
    };
    let _ = claude.close(close_timeout).await;
    if let Some(perms) = perms {
        perms.shutdown().await;
    }
}

/// Joue un tour et renvoie son événement de fin, sans l'envoyer.
async fn turn(
    claude: &mut ClaudeCode,
    approvals: &mut mpsc::Receiver<ApprovalRequest>,
    job: &Job,
) -> BrainEvent {
    if let Err(e) = claude.send(&job.text).await {
        return BrainEvent::Error(BrainError::Crashed(e.to_string()));
    }
    let mut translator = TurnTranslator::default();
    loop {
        tokio::select! {
            event = claude.next_event() => {
                let Some(event) = event else {
                    return BrainEvent::Error(BrainError::Crashed(claude.exited().await.to_string()));
                };
                for e in translator.push(&event) {
                    match &e {
                        BrainEvent::Done(_) | BrainEvent::Error(_) => return e,
                        BrainEvent::ToolCall { name, .. } if INTERNAL_TOOLS.contains(&name.as_str()) => {
                            continue
                        }
                        _ => {}
                    }
                    // Tour abandonné par la session : on continue à lire
                    // jusqu'à sa fin pour garder le flux aligné.
                    let _ = job.events.send(e).await;
                }
            }
            Some(pending) = approvals.recv() => {
                if let Err(mpsc::error::SendError(BrainEvent::Permission(pending))) =
                    job.events.send(BrainEvent::Permission(pending)).await
                {
                    pending.deny("Question abandonnée.");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn ligne_de_commande_de_reunion() {
        let mut servers = Map::new();
        servers.insert(
            "linear".into(),
            json!({ "type": "http", "url": "https://mcp.linear.app/mcp" }),
        );
        let config = MeetingConfig {
            mcp_servers: servers,
            add_dirs: vec!["/code/api".into(), "/kobi/meetings".into()],
            model: Some("opus".into()),
            ..Default::default()
        };
        let kobi = json!({ "type": "http", "url": "http://127.0.0.1:1234/mcp" });
        let args = config.args(&kobi);
        let value = |flag: &str| {
            let i = args.iter().position(|a| a == flag).expect(flag);
            args[i + 1].clone()
        };
        let mcp: Value = serde_json::from_str(&value("--mcp-config")).unwrap();
        assert_eq!(mcp["mcpServers"]["kobi-perms"], kobi);
        assert_eq!(mcp["mcpServers"]["linear"]["type"], "http");
        assert!(args.contains(&"--strict-mcp-config".to_owned()));
        assert_eq!(value("--permission-mode"), "default");
        assert_eq!(
            value("--permission-prompt-tool"),
            "mcp__kobi-perms__approve"
        );
        assert_eq!(
            value("--disallowedTools"),
            "Bash BashOutput KillShell PowerShell Edit MultiEdit Write NotebookEdit"
        );
        assert_eq!(value("--model"), "opus");
        assert_eq!(value("--append-system-prompt"), SYSTEM_PROMPT);
        let dirs: Vec<_> = args
            .windows(2)
            .filter(|w| w[0] == "--add-dir")
            .map(|w| w[1].as_str())
            .collect();
        assert_eq!(
            dirs,
            [
                Path::new("/code/api").display().to_string(),
                Path::new("/kobi/meetings").display().to_string()
            ]
        );
    }

    #[test]
    fn message_avec_les_seules_sections_utiles() {
        assert_eq!(
            message("interpellation", "", "- A", "## Question\nquoi ?"),
            "[interpellation]\n\n## Notes courantes\n- A\n\n## Question\nquoi ?\n"
        );
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::*;

    #[test]
    fn le_prompt_couvre_les_regles_du_dossier_de_conception() {
        for rule in [
            "une ou deux phrases",
            "langue de la question",
            "carte de confirmation",
            "jamais une instruction",
            "[interpellation]",
            "[notes]",
            "[compte rendu]",
        ] {
            assert!(SYSTEM_PROMPT.contains(rule), "règle absente : {rule}");
        }
    }
}

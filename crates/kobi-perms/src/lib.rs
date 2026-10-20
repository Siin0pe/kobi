//! kobi-perms : serveur MCP local « kobi-perms » branché sur
//! `claude --permission-prompt-tool mcp__kobi-perms__approve`.
//!
//! Claude Code appelle l'outil `approve` avant chaque outil qui demande une
//! autorisation. Le serveur tourne dans le process de Kobi (HTTP streamable
//! sur la boucle locale) : chaque appel devient une [`ApprovalRequest`] sur un
//! canal, et l'appel reste bloqué jusqu'à la décision. Sans décision dans le
//! délai, ou si la demande est abandonnée, la réponse est un refus.
//!
//! Côté interface, [`cards::Cards`] transforme les demandes en cartes
//! numérotées et transmet la réponse de l'utilisateur. Les [`rules::Rules`]
//! autorisent d'office la lecture seule et la liste blanche du workspace,
//! sans carte.

pub mod action;
pub mod cards;
pub mod classement;
pub mod commande;
pub mod mcp;
pub mod rules;

pub use action::{Action, DemandeOutil, Etape, Famille, Shell};
pub use cards::{Card, Cards};
pub use classement::{classer, Classe, Contexte, Genre};
pub use rules::{Rules, Verdict};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{ServerCapabilities, ServerConfig};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{schemars, tool, tool_handler, tool_router, ServerHandler};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::{CancellationToken, DropGuard};

/// Nom du serveur dans la config MCP passée à Claude Code. Distinct de
/// « kobi », le MCP de pilotage de Kobi (KOB-161).
pub const SERVER_NAME: &str = "kobi-perms";
/// Valeur de `--permission-prompt-tool`.
pub const PERMISSION_PROMPT_TOOL: &str = "mcp__kobi-perms__approve";
/// Délai par défaut avant refus (dossier de conception : 60 s).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(60);

/// Arguments envoyés par Claude Code à l'outil de permission.
#[derive(Debug, Clone, PartialEq, Deserialize, schemars::JsonSchema)]
pub struct PermissionRequest {
    /// Outil que Claude veut appeler (`Write`, `mcp__linear__save_issue`…).
    pub tool_name: String,
    /// Arguments de cet outil.
    pub input: Value,
    #[serde(default)]
    pub tool_use_id: Option<String>,
}

/// Décision prise pour une demande.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny(String),
}

/// Demande en attente de décision, reçue par l'application.
#[derive(Debug)]
pub struct ApprovalRequest {
    pub request: PermissionRequest,
    reply: oneshot::Sender<Decision>,
}

impl ApprovalRequest {
    /// Demande hors serveur MCP (backend clé API, doublures de test) : la
    /// décision arrive sur le récepteur renvoyé ; abandonner la demande
    /// ferme ce récepteur, ce qui vaut refus.
    pub fn new(request: PermissionRequest) -> (Self, oneshot::Receiver<Decision>) {
        let (reply, answer) = oneshot::channel();
        (Self { request, reply }, answer)
    }

    pub fn decide(self, decision: Decision) {
        // Si l'appel a déjà expiré, la décision n'a plus d'effet.
        let _ = self.reply.send(decision);
    }

    /// L'appel n'attend plus de décision (délai dépassé, serveur arrêté).
    pub fn is_abandoned(&self) -> bool {
        self.reply.is_closed()
    }

    pub fn allow(self) {
        self.decide(Decision::Allow);
    }

    pub fn deny(self, message: impl Into<String>) {
        self.decide(Decision::Deny(message.into()));
    }
}

/// Gestionnaire MCP : un seul outil, `approve`.
#[derive(Clone)]
pub struct Approver {
    requests: mpsc::Sender<ApprovalRequest>,
    timeout: Duration,
    rules: Arc<Rules>,
    #[allow(dead_code, reason = "lu par la macro tool_handler")]
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl Approver {
    /// Sans règles : tout passe par une carte.
    pub fn new(requests: mpsc::Sender<ApprovalRequest>, timeout: Duration) -> Self {
        Self::with_rules(requests, timeout, Arc::new(Rules::default()))
    }

    pub fn with_rules(
        requests: mpsc::Sender<ApprovalRequest>,
        timeout: Duration,
        rules: Arc<Rules>,
    ) -> Self {
        Self {
            requests,
            timeout,
            rules,
            tool_router: Self::tool_router(),
        }
    }

    /// Réponse JSON attendue par Claude Code, sous forme de texte.
    #[tool(description = "Demande à l'utilisateur de Kobi d'autoriser un appel d'outil.")]
    pub async fn approve(&self, Parameters(request): Parameters<PermissionRequest>) -> String {
        let input = request.input.clone();
        let decision = self.decision(request).await;
        response(&decision, input).to_string()
    }

    async fn decision(&self, request: PermissionRequest) -> Decision {
        if let Verdict::Allow(_) = self.rules.verdict(&request.tool_name) {
            return Decision::Allow;
        }
        let (pending, answer) = ApprovalRequest::new(request);
        if self.requests.send(pending).await.is_err() {
            return Decision::Deny("Kobi ne peut pas demander de confirmation.".into());
        }
        match tokio::time::timeout(self.timeout, answer).await {
            Ok(Ok(decision)) => decision,
            Ok(Err(_)) => Decision::Deny("Demande abandonnée par Kobi.".into()),
            Err(_) => Decision::Deny(format!(
                "Pas de confirmation en {} s : refusé.",
                self.timeout.as_secs()
            )),
        }
    }
}

#[tool_handler]
impl ServerHandler for Approver {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Serveur de permissions de Kobi.")
    }
}

/// Format de réponse de `--permission-prompt-tool`.
fn response(decision: &Decision, input: Value) -> Value {
    match decision {
        Decision::Allow => json!({ "behavior": "allow", "updatedInput": input }),
        Decision::Deny(message) => json!({ "behavior": "deny", "message": message }),
    }
}

/// Serveur HTTP lancé sur `127.0.0.1`, port choisi par l'OS. Lâché sans
/// [`PermsServer::shutdown`] (session abandonnée), il s'arrête aussi : sinon
/// sa tâche écouterait jusqu'à la sortie de Kobi (KOB-233).
pub struct PermsServer {
    addr: SocketAddr,
    cancel: CancellationToken,
    _arret: DropGuard,
    task: tokio::task::JoinHandle<()>,
}

impl PermsServer {
    /// Démarre le serveur sans règles (tout passe par une carte) ; les
    /// demandes arrivent sur le récepteur renvoyé.
    pub async fn start(
        timeout: Duration,
    ) -> std::io::Result<(Self, mpsc::Receiver<ApprovalRequest>)> {
        Self::start_with_rules(timeout, Rules::default()).await
    }

    /// Démarre le serveur ; seules les demandes que `rules` n'autorisent pas
    /// d'office arrivent sur le récepteur renvoyé.
    pub async fn start_with_rules(
        timeout: Duration,
        rules: Rules,
    ) -> std::io::Result<(Self, mpsc::Receiver<ApprovalRequest>)> {
        let rules = Arc::new(rules);
        let (tx, rx) = mpsc::channel(16);
        let cancel = CancellationToken::new();
        let service: StreamableHttpService<Approver, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(Approver::with_rules(tx.clone(), timeout, rules.clone())),
                Default::default(),
                StreamableHttpServerConfig::default().with_cancellation_token(cancel.child_token()),
            );
        let router = axum::Router::new().nest_service("/mcp", service);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let shutdown = cancel.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled_owned().await })
                .await;
        });
        Ok((
            Self {
                addr,
                _arret: cancel.clone().drop_guard(),
                cancel,
                task,
            },
            rx,
        ))
    }

    pub fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    /// Config MCP à passer à `claude --mcp-config` (fichier ou chaîne JSON).
    pub fn mcp_config(&self) -> Value {
        json!({ "mcpServers": { SERVER_NAME: { "type": "http", "url": self.url() } } })
    }

    pub async fn shutdown(self) {
        self.cancel.cancel();
        let _ = self.task.await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> PermissionRequest {
        PermissionRequest {
            tool_name: "Write".into(),
            input: json!({ "file_path": "a.txt", "content": "x" }),
            tool_use_id: Some("toolu_1".into()),
        }
    }

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[tokio::test]
    async fn autorise_avec_l_entree_d_origine() {
        let (tx, mut rx) = mpsc::channel(1);
        let approver = Approver::new(tx, DEFAULT_TIMEOUT);
        let call = tokio::spawn(async move { approver.approve(Parameters(request())).await });
        let pending = rx.recv().await.unwrap();
        assert_eq!(pending.request, request());
        pending.allow();
        assert_eq!(
            parse(&call.await.unwrap()),
            json!({ "behavior": "allow", "updatedInput": { "file_path": "a.txt", "content": "x" } })
        );
    }

    #[tokio::test]
    async fn l_appel_attend_la_decision_puis_refuse() {
        let (tx, mut rx) = mpsc::channel(1);
        let approver = Approver::new(tx, DEFAULT_TIMEOUT);
        let call = tokio::spawn(async move { approver.approve(Parameters(request())).await });
        let pending = rx.recv().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!call.is_finished(), "l'appel doit attendre la décision");
        pending.deny("Pas maintenant.");
        assert_eq!(
            parse(&call.await.unwrap()),
            json!({ "behavior": "deny", "message": "Pas maintenant." })
        );
    }

    #[tokio::test]
    async fn refus_sans_reponse_dans_le_delai() {
        let (tx, mut rx) = mpsc::channel(1);
        let approver = Approver::new(tx, Duration::from_millis(20));
        let call = tokio::spawn(async move { approver.approve(Parameters(request())).await });
        let _pending = rx.recv().await.unwrap();
        assert_eq!(parse(&call.await.unwrap())["behavior"], "deny");
    }

    #[tokio::test]
    async fn refus_si_la_demande_est_abandonnee_ou_sans_destinataire() {
        let (tx, mut rx) = mpsc::channel(1);
        let approver = Approver::new(tx, DEFAULT_TIMEOUT);
        let a = approver.clone();
        let call = tokio::spawn(async move { a.approve(Parameters(request())).await });
        drop(rx.recv().await.unwrap());
        assert_eq!(parse(&call.await.unwrap())["behavior"], "deny");

        drop(rx);
        let text = approver.approve(Parameters(request())).await;
        assert_eq!(parse(&text)["behavior"], "deny");
    }

    #[tokio::test]
    async fn autorise_d_office_sans_carte_selon_les_regles() {
        let (tx, mut rx) = mpsc::channel(1);
        let rules = Rules::new(["Write"]);
        let approver = Approver::with_rules(tx, DEFAULT_TIMEOUT, Arc::new(rules));
        assert_eq!(
            parse(&approver.approve(Parameters(request())).await)["behavior"],
            "allow"
        );
        assert!(rx.try_recv().is_err(), "aucune carte");
    }

    #[tokio::test]
    async fn serveur_sur_la_boucle_locale() {
        let (server, _rx) = PermsServer::start(DEFAULT_TIMEOUT).await.unwrap();
        let url = server.url();
        assert!(url.starts_with("http://127.0.0.1:") && url.ends_with("/mcp"));
        assert_eq!(
            server.mcp_config()["mcpServers"]["kobi-perms"]["url"],
            url.as_str()
        );
        server.shutdown().await;
    }

    /// KOB-233 : une session abandonnée lâche son serveur sans `shutdown` ;
    /// il ne doit plus écouter.
    #[tokio::test]
    async fn serveur_lache_arrete() {
        let (server, _rx) = PermsServer::start(DEFAULT_TIMEOUT).await.unwrap();
        let addr = server.addr;
        assert!(tokio::net::TcpStream::connect(addr).await.is_ok());
        drop(server);
        let mut ferme = false;
        for _ in 0..50 {
            if tokio::net::TcpStream::connect(addr).await.is_err() {
                ferme = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(ferme, "le serveur écoute encore après avoir été lâché");
    }
}

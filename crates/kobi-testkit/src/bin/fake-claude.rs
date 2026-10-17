//! Faux `claude -p --input-format stream-json --output-format stream-json`.
//!
//! `--version` et `auth status` répondent comme le vrai. Un fichier
//! `<exécutable>.mode` à côté du binaire change ce comportement, pour les
//! tests de détection : `logged-out` (pas connecté), `broken` (`--version`
//! échoue), `hang` (ne répond pas), `auth-hang` (seul `auth status` ne répond pas),
//! `old` (pas de commande `auth`).
//!
//! Sans scénario, il répond « reçu : <message> » en deux deltas : le message
//! `crash` arrête le process en erreur, `erreur` donne un `result` avec
//! `is_error`.
//!
//! Avec `--scenario <fichier.json>`, il rejoue un flux stream-json
//! enregistré, un tour par message reçu (format décrit dans
//! `scenarios/README.md`). Les directives `{"fake": …}` du scénario appellent
//! pour de vrai les serveurs MCP de `--mcp-config` (stdio ou HTTP) et l'outil
//! de `--permission-prompt-tool`, comme le ferait Claude Code. Les autres
//! arguments de la vraie ligne de commande sont acceptés et ignorés.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::Duration;

use serde_json::{json, Value};

const PROTOCOL_VERSION: &str = "2025-11-25";

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.first().map(String::as_str) == Some("--version")
        || argv.first().map(String::as_str) == Some("auth")
    {
        status_command(&argv);
    }
    let args = Args::parse();
    let mut out = Output(std::io::stdout());
    match &args.scenario {
        None => echo(&mut out),
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|e| fail(&format!("scénario {} : {e}", path.display())));
            let scenario: Value = serde_json::from_str(&text)
                .unwrap_or_else(|e| fail(&format!("scénario {} : {e}", path.display())));
            let mut player = Player {
                out,
                mcp: Mcp::new(args.mcp_config.as_deref()),
                permission_tool: args.permission_tool.clone(),
                disallowed: args.disallowed.clone(),
                vars: HashMap::new(),
            };
            player.run(&scenario);
        }
    }
}

/// `--version` et `auth status`, selon le fichier `.mode`.
fn status_command(argv: &[String]) -> ! {
    let mode = std::env::current_exe()
        .ok()
        .and_then(|exe| std::fs::read_to_string(exe.with_extension("mode")).ok())
        .unwrap_or_default();
    let mode = mode.trim();
    if mode == "hang" || (mode == "auth-hang" && argv[0] == "auth") {
        std::thread::sleep(Duration::from_secs(60));
    }
    if argv[0] == "--version" {
        if mode == "broken" {
            eprintln!("Error: Cannot find module '@anthropic-ai/claude-code/cli.js'");
            std::process::exit(1);
        }
        println!("2.1.302 (Claude Code)");
        std::process::exit(0);
    }
    match mode {
        "old" => {
            eprintln!("error: unknown command 'auth'");
            std::process::exit(1);
        }
        "logged-out" => {
            println!(
                "{}",
                json!({"loggedIn": false, "authMethod": "none", "apiProvider": "firstParty"})
            );
            std::process::exit(1);
        }
        _ => {
            println!(
                "{}",
                json!({"loggedIn": true, "authMethod": "claude.ai", "apiProvider": "firstParty"})
            );
            std::process::exit(0);
        }
    }
}

/// Arrête le process comme un vrai plantage : message sur stderr, code 2.
fn fail(message: &str) -> ! {
    eprintln!("fake-claude : {message}");
    std::process::exit(2)
}

#[derive(Default)]
struct Args {
    scenario: Option<PathBuf>,
    mcp_config: Option<String>,
    permission_tool: Option<String>,
    /// Outils de `--disallowedTools` (séparés par des espaces ou des virgules,
    /// en une ou plusieurs valeurs).
    disallowed: Vec<String>,
}

impl Args {
    fn parse() -> Self {
        let mut parsed = Self::default();
        let mut args = std::env::args().skip(1).peekable();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--scenario" => parsed.scenario = args.next().map(PathBuf::from),
                "--mcp-config" => parsed.mcp_config = args.next(),
                "--permission-prompt-tool" => parsed.permission_tool = args.next(),
                "--disallowedTools" | "--disallowed-tools" => {
                    while let Some(value) = args.next_if(|a| !a.starts_with("--")) {
                        parsed.disallowed.extend(
                            value
                                .split([' ', ','])
                                .filter(|t| !t.is_empty())
                                .map(String::from),
                        );
                    }
                }
                _ => {}
            }
        }
        parsed
    }
}

struct Output(std::io::Stdout);

impl Output {
    fn emit(&mut self, line: &str) {
        let mut out = self.0.lock();
        writeln!(out, "{line}").unwrap();
        out.flush().unwrap();
    }
}

/// Messages utilisateur lus sur stdin, un par ligne.
fn user_messages() -> impl Iterator<Item = String> {
    std::io::stdin().lock().lines().map(|line| {
        let line = line.unwrap_or_else(|e| fail(&format!("stdin : {e}")));
        let msg: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| fail(&format!("ligne non JSON : {e}")));
        if msg["type"] != "user" {
            fail(&format!("message inattendu : {line}"));
        }
        let content = &msg["message"]["content"];
        content
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| content.to_string())
    })
}

fn echo(out: &mut Output) {
    let session = "fake-session";
    for content in user_messages() {
        if content == "crash" {
            eprintln!("faux plantage");
            std::process::exit(3);
        }
        out.emit(
            &json!({"type":"system","subtype":"init","session_id":session,"model":"fake"})
                .to_string(),
        );
        // Les hooks et événements inconnus doivent être tolérés.
        out.emit(&json!({"type":"rate_limit_event","rate_limit_info":{}}).to_string());
        let answer = format!("reçu : {content}");
        for part in ["reçu : ", content.as_str()] {
            out.emit(
                &json!({"type":"stream_event","parent_tool_use_id":null,"session_id":session,
                "event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":part}}})
                .to_string(),
            );
        }
        out.emit(
            &json!({"type":"assistant","session_id":session,
            "message":{"role":"assistant","content":[{"type":"text","text":answer}]}})
            .to_string(),
        );
        let is_error = content == "erreur";
        out.emit(
            &json!({"duration_api_ms":1,"session_id":session,"type":"result",
            "subtype": if is_error { "error_during_execution" } else { "success" },
            "is_error":is_error,"result":answer,"num_turns":1})
            .to_string(),
        );
    }
}

struct Player {
    out: Output,
    mcp: Mcp,
    permission_tool: Option<String>,
    disallowed: Vec<String>,
    /// Valeurs substituées dans les lignes : `{{tool_result}}`, `{{deny_message}}`,
    /// `{{pid}}` (process du faux claude), `{{turn}}` (numéro du tour, dès 1).
    vars: HashMap<&'static str, String>,
}

impl Player {
    fn run(&mut self, scenario: &Value) {
        let turns = scenario["turns"].as_array().cloned().unwrap_or_default();
        let mut turns = turns.into_iter();
        self.vars.insert("pid", std::process::id().to_string());
        for (n, content) in user_messages().enumerate() {
            self.vars.insert("turn", (n + 1).to_string());
            let Some(turn) = turns.next() else {
                fail("scénario épuisé : message en trop");
            };
            if let Some(expected) = turn["expect"].as_str() {
                if !content.contains(expected) {
                    fail(&format!(
                        "message inattendu : « {content} » ne contient pas « {expected} »"
                    ));
                }
            }
            self.play(
                turn["lines"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
        }
    }

    fn play(&mut self, lines: &[Value]) {
        for line in lines {
            match line["fake"].as_str() {
                None => self.emit(line),
                Some("sleep") => {
                    std::thread::sleep(Duration::from_millis(line["ms"].as_u64().unwrap_or(0)))
                }
                Some("exit") => {
                    if let Some(stderr) = line["stderr"].as_str() {
                        eprintln!("{stderr}");
                    }
                    std::process::exit(line["code"].as_i64().unwrap_or(1) as i32);
                }
                Some("call") => {
                    let tool = line["tool"].as_str().unwrap_or_default();
                    let result = self.call_tool(tool, line["input"].clone());
                    self.vars.insert("tool_result", result);
                }
                Some("permission") => self.permission(line),
                Some("builtin") => self.builtin(line),
                Some(other) => fail(&format!("directive inconnue : {other}")),
            }
        }
    }

    /// Ligne enregistrée, avec substitution des variables (échappées JSON).
    fn emit(&mut self, line: &Value) {
        let mut text = line.to_string();
        for (name, value) in &self.vars {
            let escaped = serde_json::to_string(value).unwrap();
            text = text.replace(&format!("{{{{{name}}}}}"), &escaped[1..escaped.len() - 1]);
        }
        self.out.emit(&text);
    }

    /// Outil de base de Claude Code (`Bash`, `Write`…). Interdit par
    /// `--disallowedTools` : erreur « No such tool available », rejoue
    /// `blocked`, sans demande de permission. Sinon, demande l'autorisation
    /// comme Claude Code ; l'outil n'est jamais exécuté pour de vrai.
    fn builtin(&mut self, line: &Value) {
        let tool = line["tool"].as_str().unwrap_or_default();
        if self.disallowed.iter().any(|t| t == tool) {
            self.vars
                .insert("blocked_message", format!("No such tool available: {tool}"));
            self.play(
                line["blocked"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
            return;
        }
        let decision = self.ask_permission(tool, line["input"].clone(), &line["tool_use_id"]);
        if decision["behavior"] == "allow" {
            self.vars
                .insert("tool_result", format!("({tool} simulé par le faux claude)"));
            self.play(
                line["allow"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
        } else {
            let message = decision["message"].as_str().unwrap_or("refusé").to_owned();
            self.vars.insert("deny_message", message);
            self.play(
                line["deny"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
        }
    }

    /// Réponse de l'outil de `--permission-prompt-tool` (refus s'il n'y en a pas).
    fn ask_permission(&mut self, tool: &str, input: Value, tool_use_id: &Value) -> Value {
        match &self.permission_tool {
            None => json!({"behavior":"deny","message":"aucun --permission-prompt-tool"}),
            Some(prompt_tool) => {
                let answer = self.call_tool(
                    &prompt_tool.clone(),
                    json!({"tool_name": tool, "input": input, "tool_use_id": tool_use_id}),
                );
                serde_json::from_str(&answer).unwrap_or_else(|e| {
                    fail(&format!("réponse de permission illisible ({e}) : {answer}"))
                })
            }
        }
    }

    /// Demande l'autorisation, appelle l'outil si elle est accordée, puis
    /// rejoue la branche `allow` ou `deny`.
    fn permission(&mut self, line: &Value) {
        let tool = line["tool"].as_str().unwrap_or_default();
        let input = line["input"].clone();
        let decision = self.ask_permission(tool, input.clone(), &line["tool_use_id"]);
        if decision["behavior"] == "allow" {
            let input = decision.get("updatedInput").cloned().unwrap_or(input);
            let result = self.call_tool(tool, input);
            self.vars.insert("tool_result", result);
            self.play(
                line["allow"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
        } else {
            let message = decision["message"].as_str().unwrap_or("refusé").to_owned();
            self.vars.insert("deny_message", message);
            self.play(
                line["deny"]
                    .as_array()
                    .map(Vec::as_slice)
                    .unwrap_or_default(),
            );
        }
    }

    /// Appelle `mcp__<serveur>__<outil>` et renvoie le texte du résultat.
    fn call_tool(&mut self, tool: &str, input: Value) -> String {
        let Some((server, name)) = tool
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split_once("__"))
        else {
            // Outil interne de Claude Code : rien à appeler.
            return String::new();
        };
        let result = self
            .mcp
            .client(server)
            .request("tools/call", json!({"name": name, "arguments": input}))
            .unwrap_or_else(|e| fail(&format!("appel de {tool} : {e}")));
        result["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Serveurs MCP de `--mcp-config`, connectés au premier appel.
struct Mcp {
    config: Value,
    clients: HashMap<String, Box<dyn Client>>,
}

impl Mcp {
    fn new(config: Option<&str>) -> Self {
        let config = match config {
            None => json!({}),
            Some(text) if text.trim_start().starts_with('{') => {
                serde_json::from_str(text).unwrap_or_else(|e| fail(&format!("--mcp-config : {e}")))
            }
            Some(path) => serde_json::from_str(
                &std::fs::read_to_string(path)
                    .unwrap_or_else(|e| fail(&format!("--mcp-config {path} : {e}"))),
            )
            .unwrap_or_else(|e| fail(&format!("--mcp-config {path} : {e}"))),
        };
        Self {
            config,
            clients: HashMap::new(),
        }
    }

    fn client(&mut self, server: &str) -> &mut dyn Client {
        if !self.clients.contains_key(server) {
            let conf = &self.config["mcpServers"][server];
            let mut client: Box<dyn Client> = if let Some(url) = conf["url"].as_str() {
                Box::new(HttpClient::new(url))
            } else if let Some(command) = conf["command"].as_str() {
                let args: Vec<&str> = conf["args"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect();
                Box::new(StdioClient::spawn(command, &args))
            } else {
                fail(&format!("serveur MCP inconnu : {server}"))
            };
            client
                .request(
                    "initialize",
                    json!({"protocolVersion": PROTOCOL_VERSION, "capabilities": {},
                        "clientInfo": {"name": "fake-claude", "version": "0"}}),
                )
                .unwrap_or_else(|e| fail(&format!("initialisation de {server} : {e}")));
            client
                .notify("notifications/initialized")
                .unwrap_or_else(|e| fail(&format!("initialisation de {server} : {e}")));
            self.clients.insert(server.to_owned(), client);
        }
        self.clients.get_mut(server).unwrap().as_mut()
    }
}

/// Client JSON-RPC minimal, juste ce qu'il faut pour appeler un outil.
trait Client {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String>;
    fn notify(&mut self, method: &str) -> Result<(), String>;
}

/// Résultat d'une réponse JSON-RPC, ou son erreur.
fn rpc_result(response: Value) -> Result<Value, String> {
    match response.get("error") {
        Some(error) => Err(error.to_string()),
        None => Ok(response["result"].clone()),
    }
}

struct StdioClient {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

impl StdioClient {
    fn spawn(command: &str, args: &[&str]) -> Self {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| fail(&format!("lancement de {command} : {e}")));
        Self {
            stdin: child.stdin.take().unwrap(),
            stdout: BufReader::new(child.stdout.take().unwrap()),
            _child: child,
            next_id: 1,
        }
    }

    fn send(&mut self, message: &Value) -> Result<(), String> {
        writeln!(self.stdin, "{message}").map_err(|e| e.to_string())?;
        self.stdin.flush().map_err(|e| e.to_string())
    }
}

impl Client for StdioClient {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        let mut line = String::new();
        loop {
            line.clear();
            if self
                .stdout
                .read_line(&mut line)
                .map_err(|e| e.to_string())?
                == 0
            {
                return Err("serveur MCP arrêté".into());
            }
            let Ok(message) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if message["id"] == id {
                return rpc_result(message);
            }
        }
    }

    fn notify(&mut self, method: &str) -> Result<(), String> {
        self.send(&json!({"jsonrpc":"2.0","method":method}))
    }
}

/// Transport HTTP « streamable » : réponse en JSON ou en flux SSE.
struct HttpClient {
    url: String,
    session: Option<String>,
    next_id: u64,
}

impl HttpClient {
    fn new(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            session: None,
            next_id: 1,
        }
    }

    fn post(&mut self, body: &Value) -> Result<ureq::Response, String> {
        let mut request = ureq::post(&self.url)
            .set("Content-Type", "application/json")
            .set("Accept", "application/json, text/event-stream");
        if let Some(session) = &self.session {
            request = request
                .set("Mcp-Session-Id", session)
                .set("MCP-Protocol-Version", PROTOCOL_VERSION);
        }
        let response = request
            .send_string(&body.to_string())
            .map_err(|e| e.to_string())?;
        if let Some(session) = response.header("Mcp-Session-Id") {
            self.session = Some(session.to_owned());
        }
        Ok(response)
    }
}

impl Client for HttpClient {
    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id;
        self.next_id += 1;
        let response =
            self.post(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))?;
        if response.content_type() == "application/json" {
            let mut body = String::new();
            response
                .into_reader()
                .read_to_string(&mut body)
                .map_err(|e| e.to_string())?;
            return rpc_result(serde_json::from_str(&body).map_err(|e| e.to_string())?);
        }
        // SSE : on lit les événements jusqu'à la réponse qui porte notre id.
        let mut data = String::new();
        for line in BufReader::new(response.into_reader()).lines() {
            let line = line.map_err(|e| e.to_string())?;
            if let Some(chunk) = line.strip_prefix("data:") {
                data.push_str(chunk.trim_start());
            } else if line.is_empty() && !data.is_empty() {
                if let Ok(message) = serde_json::from_str::<Value>(&data) {
                    if message["id"] == id {
                        return rpc_result(message);
                    }
                }
                data.clear();
            }
        }
        Err(format!("pas de réponse à {method}"))
    }

    fn notify(&mut self, method: &str) -> Result<(), String> {
        self.post(&json!({"jsonrpc":"2.0","method":method}))
            .map(drop)
    }
}

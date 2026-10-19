//! Serveur MCP de Kobi : `kobi mcp` (stdio, JSON-RPC 2.0, un message par
//! ligne). Chaque outil est une commande de la table ; l'appel passe à l'app
//! en cours par le point d'accès local.
//!
//! `kobi mcp --agent` sert l'agent de Kobi : toutes les commandes, mais
//! l'app demande à l'utilisateur de valider celles qui écrivent (KOB-156),
//! sauf la réunion (décision du propriétaire).

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};

use crate::commandes::{self, Commande, Effet, COMMANDES};
use crate::protocole::{Erreur, Origine};
use crate::transport::Point;

/// Versions du protocole MCP comprises, la plus récente d'abord.
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

const INSTRUCTIONS: &str = "Outils de Kobi, l'assistant de bureau de l'utilisateur : \
état de Kobi, réunions enregistrées (notes, comptes rendus, transcriptions), conversations, \
rappels, réglages, et pilotage de l'app (ouvrir une section, démarrer ou arrêter une réunion). \
Chaque description commence par [lecture], [affichage] ou [écriture]. \
Ces outils ne marchent que si l'app Kobi est lancée.";

/// Qui se sert du serveur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Un agent branché par l'utilisateur (`claude mcp add kobi -- kobi mcp`).
    Externe,
    /// L'agent de Kobi : ce qui écrit est validé par l'utilisateur (KOB-156).
    Agent,
}

impl Mode {
    fn origine(self) -> Origine {
        match self {
            Mode::Externe => Origine::Mcp,
            Mode::Agent => Origine::Agent,
        }
    }

    /// Commandes exposées : toutes sauf les internes ; pour l'agent, l'app
    /// valide celles qui écrivent.
    pub fn commandes(self) -> impl Iterator<Item = &'static Commande> {
        COMMANDES.iter().filter(|c| !c.interne)
    }
}

/// Noms complets des outils de lecture, tels que Claude Code les voit
/// (`mcp__<serveur>__<outil>`).
pub fn outils_de_lecture(serveur: &str) -> Vec<String> {
    COMMANDES
        .iter()
        .filter(|c| c.effet == Effet::Lecture)
        .map(|c| format!("mcp__{serveur}__{}", c.outil()))
        .collect()
}

/// Tous les outils de l'agent, pour `--allowedTools` : le harness les laisse
/// passer, et c'est Kobi qui demande la validation de ceux qui écrivent
/// (rien ne dépend du harness).
pub fn outils_de_l_agent(serveur: &str) -> Vec<String> {
    Mode::Agent
        .commandes()
        .map(|c| format!("mcp__{serveur}__{}", c.outil()))
        .collect()
}

/// Outils de pilotage de la réunion (`meeting_start`…), lancés sans carte
/// (KOB-165) et refusés à la session des notes.
pub fn outils_de_reunion(serveur: &str) -> Vec<String> {
    COMMANDES
        .iter()
        .filter(|c| c.pilote_la_reunion())
        .map(|c| format!("mcp__{serveur}__{}", c.outil()))
        .collect()
}

fn outil(c: &Commande) -> Value {
    json!({
        "name": c.outil(),
        "title": c.mots(),
        "description": format!("{} {}", c.effet.etiquette(), c.description),
        "inputSchema": c.schema(),
        "annotations": {
            "readOnlyHint": c.effet == Effet::Lecture,
            "destructiveHint": c.destructive,
            "idempotentHint": c.effet != Effet::Ecriture,
            "openWorldHint": false,
        },
    })
}

/// Texte rendu à l'agent : un texte tel quel (markdown), sinon du JSON.
fn texte(resultat: &Value) -> String {
    match resultat {
        Value::String(s) => s.clone(),
        autre => serde_json::to_string_pretty(autre).unwrap_or_default(),
    }
}

fn reponse(id: Value, resultat: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": resultat})
}

fn erreur(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// Traite un message JSON-RPC ; `None` pour une notification.
pub fn traiter(
    message: &Value,
    mode: Mode,
    appeler: &dyn Fn(&'static Commande, Value, Origine) -> Result<Value, Erreur>,
) -> Option<Value> {
    let methode = message.get("method").and_then(Value::as_str).unwrap_or("");
    let id = message.get("id").cloned()?;
    let params = message.get("params").cloned().unwrap_or(Value::Null);
    Some(match methode {
        "initialize" => {
            let demandee = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or("");
            let version = VERSIONS
                .iter()
                .find(|v| **v == demandee)
                .unwrap_or(&VERSIONS[0]);
            reponse(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "kobi", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => reponse(id, json!({})),
        "tools/list" => reponse(
            id,
            json!({"tools": mode.commandes().map(outil).collect::<Vec<_>>()}),
        ),
        "tools/call" => {
            let nom = params.get("name").and_then(Value::as_str).unwrap_or("");
            let Some(c) =
                commandes::par_outil(nom).filter(|c| mode.commandes().any(|m| m.nom == c.nom))
            else {
                return Some(erreur(id, -32602, &format!("outil inconnu : {nom}")));
            };
            let arguments = params.get("arguments").cloned().unwrap_or(Value::Null);
            let resultat = c
                .verifier(&arguments)
                .and_then(|p| appeler(c, Value::Object(p), mode.origine()));
            let (contenu, en_erreur) = match resultat {
                Ok(v) => (texte(&v), false),
                Err(e) => (e.message, true),
            };
            reponse(
                id,
                json!({"content": [{"type": "text", "text": contenu}], "isError": en_erreur}),
            )
        }
        "resources/list" => reponse(id, json!({"resources": []})),
        "prompts/list" => reponse(id, json!({"prompts": []})),
        autre => erreur(id, -32601, &format!("méthode inconnue : {autre}")),
    })
}

/// Boucle stdio : lit les messages sur stdin, répond sur stdout.
pub fn servir(point: Point, mode: Mode) -> io::Result<()> {
    let appeler = |c: &'static Commande, params: Value, origine: Origine| {
        crate::client::appeler(&point, c.nom, params, origine)
    };
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for ligne in stdin.lock().lines() {
        let ligne = ligne?;
        if ligne.trim().is_empty() {
            continue;
        }
        let sortie = match serde_json::from_str::<Value>(&ligne) {
            Ok(Value::Array(lot)) => {
                let r: Vec<Value> = lot
                    .iter()
                    .filter_map(|m| traiter(m, mode, &appeler))
                    .collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            Ok(message) => traiter(&message, mode, &appeler),
            Err(e) => Some(erreur(
                Value::Null,
                -32700,
                &format!("JSON illisible : {e}"),
            )),
        };
        if let Some(s) = sortie {
            writeln!(stdout, "{s}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocole::Code;

    fn appel(nom: &str, args: Value) -> Value {
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": nom, "arguments": args}})
    }

    fn faux(c: &'static Commande, params: Value, origine: Origine) -> Result<Value, Erreur> {
        match c.nom {
            "notes.last" => Ok(json!("# Point budget\n\n- budget validé")),
            "status" => Ok(json!({"etat": "veille", "origine": origine})),
            "meeting.start" => Err(Erreur::new(Code::Conflict, "J'enregistre déjà la réunion.")),
            _ => Ok(params),
        }
    }

    #[test]
    fn initialisation() {
        let r = traiter(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-03-26", "capabilities": {}, "clientInfo": {"name": "claude-code", "version": "2"}}}),
            Mode::Externe,
            &faux,
        )
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "kobi");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        // Version inconnue : la plus récente.
        let r = traiter(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2099-01-01"}}),
            Mode::Externe,
            &faux,
        )
        .unwrap();
        assert_eq!(r["result"]["protocolVersion"], VERSIONS[0]);
        // Notification : pas de réponse.
        assert!(traiter(
            &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            Mode::Externe,
            &faux
        )
        .is_none());
        let r = traiter(
            &json!({"jsonrpc": "2.0", "id": 2, "method": "nimporte"}),
            Mode::Externe,
            &faux,
        )
        .unwrap();
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn outils_externes_et_agent() {
        let liste = |mode| {
            traiter(
                &json!({"jsonrpc": "2.0", "id": 3, "method": "tools/list"}),
                mode,
                &faux,
            )
            .unwrap()["result"]["tools"]
                .as_array()
                .unwrap()
                .clone()
        };
        let tous = liste(Mode::Externe);
        assert_eq!(tous.len(), COMMANDES.iter().filter(|c| !c.interne).count());
        let start = tous.iter().find(|t| t["name"] == "meeting_start").unwrap();
        assert_eq!(start["annotations"]["readOnlyHint"], false);
        assert!(start["description"]
            .as_str()
            .unwrap()
            .starts_with("[écriture]"));
        let notes = tous.iter().find(|t| t["name"] == "notes_last").unwrap();
        assert_eq!(notes["annotations"]["readOnlyHint"], true);
        assert!(notes["description"]
            .as_str()
            .unwrap()
            .starts_with("[lecture]"));

        // L'agent voit tout ; l'app valide ce qui écrit (KOB-156).
        let agent = liste(Mode::Agent);
        assert_eq!(agent.len(), tous.len());
        assert!(agent.iter().any(|t| t["name"] == "config_set"));
        assert!(outils_de_lecture("kobi").contains(&"mcp__kobi__notes_last".to_string()));
        assert!(!outils_de_lecture("kobi").contains(&"mcp__kobi__app_quit".to_string()));
        assert!(outils_de_l_agent("kobi").contains(&"mcp__kobi__config_set".to_string()));
        assert_eq!(
            outils_de_reunion("kobi"),
            [
                "mcp__kobi__meeting_start",
                "mcp__kobi__meeting_stop",
                "mcp__kobi__meeting_pause",
                "mcp__kobi__meeting_resume"
            ]
        );
        assert!(!agent.iter().any(|t| t["name"] == "validation_ask"));
        // Interne : ni listée ni appelable par un MCP.
        let r = traiter(&appel("validation_ask", json!({})), Mode::Agent, &faux).unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }

    #[test]
    fn appels() {
        let r = traiter(&appel("notes_last", json!({})), Mode::Agent, &faux).unwrap();
        assert_eq!(r["id"], 7);
        assert_eq!(r["result"]["isError"], false);
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("# Point budget"));

        let r = traiter(&appel("status", json!({})), Mode::Agent, &faux).unwrap();
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"origine\": \"agent\""));
        let r = traiter(&appel("status", json!({})), Mode::Externe, &faux).unwrap();
        assert!(r["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"origine\": \"mcp\""));

        let r = traiter(&appel("meeting_start", Value::Null), Mode::Externe, &faux).unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(
            r["result"]["content"][0]["text"],
            "J'enregistre déjà la réunion."
        );

        // Paramètres vérifiés avant l'appel.
        let r = traiter(
            &appel("meetings_get", json!({"part": "tout"})),
            Mode::Externe,
            &faux,
        )
        .unwrap();
        assert_eq!(r["result"]["isError"], true);

        // L'agent a les outils d'écriture ; l'app décide (validation).
        let r = traiter(&appel("meeting_start", json!({})), Mode::Agent, &faux).unwrap();
        assert!(r.get("error").is_none(), "{r}");
        let r = traiter(&appel("rm_rf", json!({})), Mode::Externe, &faux).unwrap();
        assert_eq!(r["error"]["code"], -32602);
    }
}

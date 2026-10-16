//! Événements émis par `claude --output-format stream-json`.
//!
//! Seuls les champs utiles à Kobi sont typés ; le reste est conservé brut
//! dans `raw` ou `Event::Other`, car le format évolue avec Claude Code.

use serde::Deserialize;
use serde_json::Value;

/// Un événement, c'est-à-dire une ligne de stdout.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// `system` : `init` au début de chaque tour, `status`, hooks…
    System(SystemEvent),
    /// `stream_event` : événement brut de l'API Messages
    /// (présent avec `--include-partial-messages`).
    Stream(StreamEvent),
    /// `assistant` : message complet de l'assistant.
    Assistant { message: Value },
    /// `user` : message utilisateur, dont les résultats d'outils.
    User { message: Value },
    /// `result` : fin d'un tour.
    Result(ResultEvent),
    /// Type inconnu ou non typé (`rate_limit_event`…).
    Other(Value),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SystemEvent {
    pub subtype: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Serveurs MCP et leur état (`connected`, `failed`, `pending`…), dans `init`.
    #[serde(default)]
    pub mcp_servers: Vec<McpServerStatus>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct StreamEvent {
    pub event: Value,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct ResultEvent {
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub is_error: bool,
    #[serde(default)]
    pub result: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub duration_api_ms: Option<u64>,
    #[serde(default)]
    pub total_cost_usd: Option<f64>,
    #[serde(default)]
    pub num_turns: Option<u32>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

/// Tokens du tour (somme des appels à l'API).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub output_tokens_details: OutputTokensDetails,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct OutputTokensDetails {
    /// Tokens de réflexion, compris dans `output_tokens`.
    #[serde(default)]
    pub thinking_tokens: u64,
}

impl Usage {
    /// Tokens d'entrée au total, servis en cache ou non.
    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

/// Résumé d'un tour complet renvoyé par [`crate::ClaudeCode::ask`].
#[derive(Debug, Clone, PartialEq)]
pub struct TurnResult {
    /// Texte de la réponse, concaténé depuis les deltas.
    pub text: String,
    /// Événement `result` final.
    pub result: ResultEvent,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Tagged {
    System(SystemEvent),
    StreamEvent(StreamEvent),
    Assistant { message: Value },
    User { message: Value },
    Result(ResultEvent),
}

impl Event {
    /// Analyse une ligne de stdout. Une ligne JSON valide d'un type inconnu
    /// donne `Event::Other` ; seule une ligne qui n'est pas du JSON échoue.
    pub fn parse(line: &str) -> Result<Self, serde_json::Error> {
        let value: Value = serde_json::from_str(line)?;
        Ok(match Tagged::deserialize(&value) {
            Ok(Tagged::System(e)) => Event::System(e),
            Ok(Tagged::StreamEvent(e)) => Event::Stream(e),
            Ok(Tagged::Assistant { message }) => Event::Assistant { message },
            Ok(Tagged::User { message }) => Event::User { message },
            Ok(Tagged::Result(e)) => Event::Result(e),
            Err(_) => Event::Other(value),
        })
    }

    /// Morceau de texte de la réponse principale (hors sous-agents), si
    /// l'événement en porte un.
    pub fn text_delta(&self) -> Option<&str> {
        let Event::Stream(s) = self else { return None };
        if s.parent_tool_use_id.is_some() {
            return None;
        }
        let ev = &s.event;
        if ev.get("type")?.as_str()? != "content_block_delta" {
            return None;
        }
        let delta = ev.get("delta")?;
        if delta.get("type")?.as_str()? != "text_delta" {
            return None;
        }
        delta.get("text")?.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn analyse_init() {
        let e = Event::parse(
            r#"{"type":"system","subtype":"init","cwd":"C:\\x","session_id":"abc","model":"claude-opus-5-5","tools":[],"mcp_servers":[{"name":"linear","status":"connected"}]}"#,
        )
        .unwrap();
        assert_eq!(
            e,
            Event::System(SystemEvent {
                subtype: "init".into(),
                session_id: Some("abc".into()),
                model: Some("claude-opus-5-5".into()),
                mcp_servers: vec![McpServerStatus {
                    name: "linear".into(),
                    status: "connected".into(),
                }],
            })
        );
    }

    #[test]
    fn extrait_un_delta_de_texte() {
        let e = Event::parse(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"un"}},"session_id":"abc","parent_tool_use_id":null}"#,
        )
        .unwrap();
        assert_eq!(e.text_delta(), Some("un"));
    }

    #[test]
    fn ignore_les_deltas_des_sous_agents() {
        let e = Event::parse(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"x"}},"parent_tool_use_id":"toolu_1"}"#,
        )
        .unwrap();
        assert_eq!(e.text_delta(), None);
    }

    #[test]
    fn analyse_result_quel_que_soit_l_ordre_des_champs() {
        let e = Event::parse(
            r#"{"duration_api_ms":1452,"stop_reason":"end_turn","session_id":"abc","total_cost_usd":0.1,"type":"result","subtype":"success","is_error":false,"result":"un","num_turns":1,"usage":{"input_tokens":2,"cache_creation_input_tokens":10,"cache_read_input_tokens":30,"output_tokens":3,"output_tokens_details":{"thinking_tokens":1},"service_tier":"standard"}}"#,
        )
        .unwrap();
        let Event::Result(r) = e else {
            panic!("attendu un result")
        };
        assert_eq!(r.result.as_deref(), Some("un"));
        assert_eq!(r.subtype.as_deref(), Some("success"));
        assert!(!r.is_error);
        assert_eq!(r.duration_api_ms, Some(1452));
        let usage = r.usage.unwrap();
        assert_eq!(usage.total_input(), 42);
        assert_eq!(usage.cache_read_input_tokens, 30);
        assert_eq!(usage.output_tokens_details.thinking_tokens, 1);
    }

    #[test]
    fn type_inconnu_donne_other() {
        let e = Event::parse(r#"{"type":"rate_limit_event","rate_limit_info":{}}"#).unwrap();
        assert!(matches!(e, Event::Other(_)));
    }

    #[test]
    fn ligne_non_json_echoue() {
        assert!(Event::parse("pas du json").is_err());
    }
}

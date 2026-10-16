//! Traduction des événements stream-json de Claude Code en [`BrainEvent`].

use crate::brain::{BrainError, BrainEvent, TurnSummary};

use super::event::Event;

/// Suit un tour et traduit chaque ligne de stdout. Les permissions ne
/// passent pas par stdout (serveur MCP « kobi ») : le backend les ajoute
/// lui-même au flux.
#[derive(Debug, Default)]
pub struct TurnTranslator {
    text: String,
}

impl TurnTranslator {
    /// Événements Kobi portés par `event` ; le dernier d'un tour est `Done`
    /// ou `Error`.
    pub fn push(&mut self, event: &Event) -> Vec<BrainEvent> {
        if let Some(delta) = event.text_delta() {
            self.text.push_str(delta);
            return vec![BrainEvent::Text(delta.to_owned())];
        }
        match event {
            Event::Assistant { message } => message
                .get("content")
                .and_then(|c| c.as_array())
                .into_iter()
                .flatten()
                .filter(|block| block.get("type").and_then(|t| t.as_str()) == Some("tool_use"))
                .map(|block| BrainEvent::ToolCall {
                    id: str_field(block, "id"),
                    name: str_field(block, "name"),
                    input: block.get("input").cloned().unwrap_or_default(),
                })
                .collect(),
            Event::Result(result) if result.is_error => {
                let message = result
                    .result
                    .clone()
                    .or_else(|| result.subtype.clone())
                    .unwrap_or_else(|| "tour en erreur".into());
                self.text.clear();
                vec![BrainEvent::Error(BrainError::Api(message))]
            }
            Event::Result(result) => {
                let mut events = Vec::new();
                if self.text.is_empty() {
                    // Sans `--include-partial-messages`, seul le résultat porte le texte.
                    if let Some(text) = result.result.as_ref().filter(|t| !t.is_empty()) {
                        events.push(BrainEvent::Text(text.clone()));
                        self.text.clone_from(text);
                    }
                }
                events.push(BrainEvent::Done(TurnSummary {
                    text: std::mem::take(&mut self.text),
                    usage: result.usage,
                }));
                events
            }
            _ => Vec::new(),
        }
    }
}

fn str_field(block: &serde_json::Value, key: &str) -> String {
    block
        .get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn push_all(lines: &[serde_json::Value]) -> Vec<BrainEvent> {
        let mut t = TurnTranslator::default();
        lines
            .iter()
            .flat_map(|l| t.push(&Event::parse(&l.to_string()).unwrap()))
            .collect()
    }

    #[test]
    fn texte_outil_puis_fin() {
        let delta = |text: &str| {
            json!({"type":"stream_event","parent_tool_use_id":null,
                "event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":text}}})
        };
        let events = push_all(&[
            json!({"type":"system","subtype":"init","session_id":"s"}),
            json!({"type":"assistant","message":{"role":"assistant","content":[
                {"type":"tool_use","id":"toolu_1","name":"mcp__linear__list_issues","input":{"project":"API"}}]}}),
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"3"}]}}),
            delta("Trois "),
            delta("tickets."),
            json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Trois tickets."}]}}),
            json!({"type":"result","subtype":"success","is_error":false,"result":"Trois tickets.",
                "usage":{"input_tokens":5,"output_tokens":3}}),
        ]);
        assert!(matches!(
            &events[0],
            BrainEvent::ToolCall { id, name, input }
                if id == "toolu_1" && name == "mcp__linear__list_issues" && input["project"] == "API"
        ));
        assert!(matches!(&events[1], BrainEvent::Text(t) if t == "Trois "));
        assert!(matches!(&events[2], BrainEvent::Text(t) if t == "tickets."));
        let BrainEvent::Done(summary) = &events[3] else {
            panic!("attendu Done, obtenu {:?}", events[3])
        };
        assert_eq!(summary.text, "Trois tickets.");
        assert_eq!(summary.usage.unwrap().output_tokens, 3);
        assert_eq!(events.len(), 4);
    }

    #[test]
    fn texte_du_resultat_sans_deltas() {
        let events = push_all(&[json!({"type":"result","is_error":false,"result":"oui"})]);
        assert!(matches!(&events[0], BrainEvent::Text(t) if t == "oui"));
        assert!(matches!(&events[1], BrainEvent::Done(s) if s.text == "oui"));
    }

    #[test]
    fn resultat_en_erreur() {
        let events = push_all(&[json!({"type":"result","subtype":"error_during_execution",
            "is_error":true,"result":"Claude AI usage limit reached"})]);
        assert!(matches!(
            &events[..],
            [BrainEvent::Error(BrainError::Api(m))] if m.contains("usage limit")
        ));
    }

    #[test]
    fn deux_tours_successifs_ne_melangent_pas_le_texte() {
        let mut t = TurnTranslator::default();
        let result = Event::parse(r#"{"type":"result","is_error":false,"result":"un"}"#).unwrap();
        t.push(&result);
        let result = Event::parse(r#"{"type":"result","is_error":false,"result":"deux"}"#).unwrap();
        assert!(matches!(&t.push(&result)[1], BrainEvent::Done(s) if s.text == "deux"));
    }
}

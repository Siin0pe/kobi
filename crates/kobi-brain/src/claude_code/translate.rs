//! Traduction des événements stream-json de Claude Code en [`BrainEvent`].

use std::time::{Duration, Instant};

use crate::brain::{BrainError, BrainEvent, TurnSummary};

use super::event::Event;

/// Suit un tour et traduit chaque ligne de stdout. Les permissions ne
/// passent pas par stdout (serveur MCP « kobi ») : le backend les ajoute
/// lui-même au flux.
#[derive(Debug, Default)]
pub struct TurnTranslator {
    text: String,
    /// Dernier signe de vie donné pour un événement de flux sans texte : un
    /// par seconde au plus (la réflexion en émet beaucoup).
    vie: Option<Instant>,
}

/// Écart minimal entre deux [`BrainEvent::Vie`] tirés du flux.
const VIE: Duration = Duration::from_secs(1);

impl TurnTranslator {
    /// Événements Kobi portés par `event` ; le dernier d'un tour est `Done`
    /// ou `Error`.
    pub fn push(&mut self, event: &Event) -> Vec<BrainEvent> {
        if let Some(delta) = event.text_delta() {
            self.text.push_str(delta);
            return vec![BrainEvent::Text(delta.to_owned())];
        }
        match event {
            Event::Assistant { message } => {
                let outils: Vec<BrainEvent> = blocs(message, "tool_use")
                    .map(|block| {
                        let name = str_field(block, "name");
                        let input = block.get("input").cloned().unwrap_or_default();
                        BrainEvent::ToolCall {
                            id: str_field(block, "id"),
                            action: super::outils::action(&name, &input),
                            name,
                            input,
                        }
                    })
                    .collect();
                if outils.is_empty() {
                    vec![BrainEvent::Vie]
                } else {
                    outils
                }
            }
            // Résultats des outils : chacun est revenu (KOB-243).
            Event::User { message } => {
                let rendus: Vec<BrainEvent> = blocs(message, "tool_result")
                    .map(|block| BrainEvent::OutilRendu {
                        id: str_field(block, "tool_use_id"),
                    })
                    .collect();
                if rendus.is_empty() {
                    vec![BrainEvent::Vie]
                } else {
                    rendus
                }
            }
            Event::Stream(_) => {
                if self.vie.is_some_and(|t| t.elapsed() < VIE) {
                    Vec::new()
                } else {
                    self.vie = Some(Instant::now());
                    vec![BrainEvent::Vie]
                }
            }
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
                    avis: None,
                }));
                events
            }
            _ => Vec::new(),
        }
    }
}

/// Blocs de contenu d'un message, du type donné.
fn blocs<'a>(
    message: &'a serde_json::Value,
    genre: &'a str,
) -> impl Iterator<Item = &'a serde_json::Value> + 'a {
    message
        .get("content")
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter(move |block| block.get("type").and_then(|t| t.as_str()) == Some(genre))
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
            BrainEvent::ToolCall { id, name, input, action }
                if id == "toolu_1" && name == "mcp__linear__list_issues" && input["project"] == "API"
                    && matches!(action, crate::Action::Mcp { serveur, .. } if serveur == "linear")
        ));
        // L'outil revenu, puis le texte ; le message complet de l'assistant
        // n'est qu'un signe de vie (KOB-243).
        assert!(matches!(&events[1], BrainEvent::OutilRendu { id } if id == "toolu_1"));
        assert!(matches!(&events[2], BrainEvent::Text(t) if t == "Trois "));
        assert!(matches!(&events[3], BrainEvent::Text(t) if t == "tickets."));
        assert!(matches!(&events[4], BrainEvent::Vie));
        let BrainEvent::Done(summary) = &events[5] else {
            panic!("attendu Done, obtenu {:?}", events[5])
        };
        assert_eq!(summary.text, "Trois tickets.");
        assert_eq!(summary.usage.unwrap().output_tokens, 3);
        assert_eq!(events.len(), 6);
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

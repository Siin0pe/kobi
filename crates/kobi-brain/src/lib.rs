//! kobi-brain : dialogue avec Claude.
//!
//! Le backend Claude Code pilote un seul process
//! `claude -p --input-format stream-json --output-format stream-json`
//! par session : chaque message est une ligne JSON écrite sur stdin, chaque
//! événement une ligne JSON lue sur stdout.

pub mod claude_code;

pub use claude_code::{ClaudeCode, ClaudeOptions, Error, Event, TurnResult, Usage};

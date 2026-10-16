//! kobi-brain : dialogue avec Claude.
//!
//! Le backend Claude Code pilote un seul process
//! `claude -p --input-format stream-json --output-format stream-json`
//! par session : chaque message est une ligne JSON écrite sur stdin, chaque
//! événement une ligne JSON lue sur stdout.
//!
//! Le reste de Kobi ne voit que le trait [`Brain`] et ses événements
//! ([`BrainEvent`]) ; [`fake::FakeBrain`] sert aux tests de la session.

pub mod brain;
pub mod claude_code;
pub mod fake;

pub use brain::{
    Ask, Brain, BrainError, BrainEvent, NotesUpdate, ReportRequest, Turn, TurnSummary,
};
pub use claude_code::{ClaudeCode, ClaudeOptions, Error, Event, TurnResult, TurnTranslator, Usage};
pub use kobi_perms::{ApprovalRequest, Decision, PermissionRequest};

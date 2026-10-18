//! kobi-brain : dialogue avec Claude.
//!
//! Le backend Claude Code pilote un seul process
//! `claude -p --input-format stream-json --output-format stream-json`
//! par session : chaque message est une ligne JSON écrite sur stdin, chaque
//! événement une ligne JSON lue sur stdout.
//!
//! Le reste de Kobi ne voit que le trait [`Brain`] et ses événements
//! ([`BrainEvent`]) ; [`fake::FakeBrain`] sert aux tests de la session.

pub mod actions;
pub mod brain;
pub mod claude_code;
pub mod fake;
pub mod outils;
pub mod transcript;

pub use brain::{
    Ask, Brain, BrainError, BrainEvent, NotesUpdate, ReportRequest, Turn, TurnSummary,
};
pub use claude_code::{
    ArgsWithPerms, ClaudeCode, ClaudeCodeBrain, ClaudeInstall, ClaudeOptions, Detection, Error,
    Event, MeetingConfig, TurnResult, TurnTranslator, Usage, SYSTEM_PROMPT, SYSTEM_PROMPT_VERSION,
};
pub use kobi_perms::{ApprovalRequest, Decision, PermissionRequest};
pub use outils::{Action, DemandeOutil, Etape, Famille, Shell};

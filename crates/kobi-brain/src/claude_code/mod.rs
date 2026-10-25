//! Backend Claude Code : un process `claude` piloté en stream-json.

pub mod detect;
mod event;
mod meeting;
pub mod outils;
mod session;
mod translate;

pub use detect::{ClaudeInstall, Detection};
pub use event::{Event, McpServerStatus, ResultEvent, StreamEvent, SystemEvent, TurnResult, Usage};
pub(crate) use meeting::{texte_ask, texte_compte_rendu, texte_notes};
pub use meeting::{
    ArgsWithPerms, ClaudeCodeBrain, MeetingConfig, MEETING_DISALLOWED_TOOLS, SYSTEM_PROMPT,
    SYSTEM_PROMPT_VERSION,
};
pub use session::{ClaudeCode, ClaudeOptions, Error};
pub use translate::TurnTranslator;

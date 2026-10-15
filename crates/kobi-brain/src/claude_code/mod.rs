//! Backend Claude Code : un process `claude` piloté en stream-json.

mod event;
mod session;

pub use event::{Event, ResultEvent, StreamEvent, SystemEvent, TurnResult};
pub use session::{ClaudeCode, ClaudeOptions, Error};

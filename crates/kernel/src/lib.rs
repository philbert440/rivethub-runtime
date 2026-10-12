pub mod harness;
pub mod memory;
pub mod model;
pub mod queue;
pub mod terminal;
pub mod tool;
pub mod transcript;

use std::path::PathBuf;
use wire::session_id::SessionId;

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolContext {
    pub agent_id: Option<String>,
    pub working_dir: Option<PathBuf>,
    pub session: Option<SessionId>,
}

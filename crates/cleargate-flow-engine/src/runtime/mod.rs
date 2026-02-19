//! Standalone execution runtime — per-execution recording without requiring
//! the full Engine/GraphDef machinery.

mod config;
mod session;

pub use config::{ExecutionConfig, ExecutionSessionBuilder};
pub use session::{ExecutionSession, SessionError, SessionHandle};

//! Multi-agent pattern library.
//!
//! This module provides building blocks for supervisor/worker delegation,
//! agent handoff, and shared blackboard state. These are patterns built
//! on top of the existing executor — no new execution primitives are
//! introduced.
//!
//! # Components
//!
//! - [`Blackboard`] — namespaced shared memory over [`StateStore`](crate::traits::StateStore)
//! - [`AgentHandoff`] — structured context transfer between agents
//! - [`AgentRole`] — semantic marker for a node's role in a multi-agent graph
//!
//! # Built-in Nodes
//!
//! The companion nodes live in [`crate::nodes::supervisor`] and
//! [`crate::nodes::worker`], registered as `"supervisor"` and `"worker"`
//! node types.

pub mod blackboard;
pub mod handoff;

pub use blackboard::Blackboard;
pub use handoff::AgentHandoff;

/// Semantic role of a node within a multi-agent graph.
///
/// Used for introspection and observability — the executor does not
/// treat roles differently at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentRole {
    /// Orchestrates work by delegating to workers.
    Supervisor,
    /// Executes a specific task on behalf of a supervisor.
    Worker,
}

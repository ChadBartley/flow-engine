//! Execution identity, run records, versioning, and tags.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use uuid::Uuid;

use super::graph::GraphDef;
use super::llm::LlmRunSummary;
use super::triggers::TriggerSource;

// ---------------------------------------------------------------------------
// Execution identity
// ---------------------------------------------------------------------------

/// Unique identifier for an execution session.
///
/// The `run_id` is always a UUID v4. The optional `execution_hash` allows
/// content-addressed deduplication when the same logical execution is
/// retried or replayed.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ExecutionId {
    pub run_id: Uuid,
    pub execution_hash: Option<String>,
}

impl ExecutionId {
    /// Create a new `ExecutionId` with a random UUID v4 and no hash.
    pub fn new() -> Self {
        Self {
            run_id: Uuid::new_v4(),
            execution_hash: None,
        }
    }

    /// Returns the run_id as a `String`.
    pub fn run_id_str(&self) -> String {
        self.run_id.to_string()
    }
}

impl Default for ExecutionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for ExecutionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.run_id)
    }
}

// ---------------------------------------------------------------------------
// Flow versioning
// ---------------------------------------------------------------------------

/// An immutable, content-addressed snapshot of a flow graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct FlowVersion {
    /// SHA-256 of canonical JSON.
    pub version_id: std::string::String,
    pub flow_id: std::string::String,
    /// Immutable snapshot (includes tool_definitions).
    pub graph: GraphDef,
    pub created_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_by: Option<std::string::String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<std::string::String>,
}

/// Pointer to the latest version of a flow.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct FlowHead {
    pub flow_id: std::string::String,
    pub name: std::string::String,
    pub current_version_id: std::string::String,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Run records
// ---------------------------------------------------------------------------

/// Summary metadata for a single execution of a flow.
///
/// This is a lightweight record containing run identity, status, timing, and
/// aggregate LLM stats. It does **not** contain the detailed event log — use
/// [`RunStore::events()`](crate::traits::RunStore::events) to retrieve the
/// full sequence of [`WriteEvent`](crate::write_event::WriteEvent) values
/// (LLM calls, tool invocations, steps, topology hints, etc.).
///
/// Together, `RunRecord` + events give the complete picture needed by
/// `DiffEngine` and `ReplayEngine`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct RunRecord {
    pub run_id: std::string::String,
    pub flow_id: std::string::String,
    /// FK to `FlowVersion`.
    pub version_id: std::string::String,
    pub status: RunStatus,
    pub triggered_by: TriggerSource,
    /// SHA-256 of resolved config.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_hash: Option<std::string::String>,
    /// Reserved for replay lineage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_run_id: Option<std::string::String>,
    /// Reserved for replay metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_config: Option<serde_json::Value>,
    /// Aggregate LLM stats for this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm_summary: Option<LlmRunSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub inputs: serde_json::Value,
}

impl Default for RunRecord {
    fn default() -> Self {
        Self {
            run_id: std::string::String::new(),
            flow_id: std::string::String::new(),
            version_id: std::string::String::new(),
            status: RunStatus::Pending,
            triggered_by: TriggerSource::Manual {
                principal: "local".into(),
            },
            config_hash: None,
            parent_run_id: None,
            replay_config: None,
            llm_summary: None,
            started_at: None,
            completed_at: None,
            duration_ms: None,
            inputs: serde_json::Value::Null,
        }
    }
}

/// Lifecycle status of a run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum RunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// A mutable tag pointing to an immutable flow version.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct FlowTag {
    pub flow_id: String,
    pub tag: String,
    pub version_id: String,
    pub updated_at: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// Export / Import bundle
// ---------------------------------------------------------------------------

/// Self-contained bundle for flow export/import.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct FlowBundle {
    pub schema_version: u16,
    pub flow_head: FlowHead,
    pub versions: Vec<FlowVersion>,
    pub tags: BTreeMap<String, String>,
}

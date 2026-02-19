//! Normalized adapter events shared across all framework adapters.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A normalized event produced by framework adapters from framework-specific
/// callback payloads. Each variant maps to one or more `ObserverSession`
/// recording calls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
#[non_exhaustive]
pub enum AdapterEvent {
    LlmStart {
        node_id: String,
        request: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
    LlmEnd {
        node_id: String,
        response: serde_json::Value,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    ToolStart {
        node_id: String,
        tool_name: String,
        inputs: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
    ToolEnd {
        node_id: String,
        tool_name: String,
        outputs: serde_json::Value,
        duration_ms: u64,
        timestamp: DateTime<Utc>,
    },
    NodeTransition {
        from: String,
        to: String,
        timestamp: DateTime<Utc>,
    },
    StepStart {
        name: String,
        data: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
    StepEnd {
        name: String,
        data: serde_json::Value,
        timestamp: DateTime<Utc>,
    },
}

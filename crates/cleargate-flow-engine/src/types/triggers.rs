//! Trigger types — how flow executions are initiated.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Payload delivered by any trigger to start a flow execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct TriggerEvent {
    pub flow_id: std::string::String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_id: Option<std::string::String>,
    pub inputs: serde_json::Value,
    pub source: TriggerSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotency_key: Option<std::string::String>,
}

/// How a flow execution was initiated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case", tag = "kind")]
#[non_exhaustive]
pub enum TriggerSource {
    Api {
        request_id: std::string::String,
    },
    Cron {
        schedule: std::string::String,
        tick: DateTime<Utc>,
    },
    Flow {
        source_run_id: std::string::String,
        source_flow_id: std::string::String,
    },
    Queue {
        topic: std::string::String,
        message_id: std::string::String,
    },
    Manual {
        principal: std::string::String,
    },
    Custom {
        trigger_type: std::string::String,
        metadata: serde_json::Value,
    },
    Replay {
        source_run_id: std::string::String,
        node_id: std::string::String,
        substitutions: ReplaySubstitutions,
    },
}

/// What to substitute when replaying a node.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct ReplaySubstitutions {
    /// Override node config keys (merged on top of original).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config_overrides: BTreeMap<std::string::String, serde_json::Value>,
    /// Override the node's input entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_override: Option<serde_json::Value>,
    /// Override specific secrets for this replay.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secret_overrides: BTreeMap<std::string::String, std::string::String>,
}

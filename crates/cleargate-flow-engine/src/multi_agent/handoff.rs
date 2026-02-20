//! Structured context transfer between agents.
//!
//! An [`AgentHandoff`] captures the task, context, and constraints that one
//! agent delegates to another. It travels as serialized edge data between
//! supervisor and worker nodes, enabling typed inter-agent communication
//! without requiring new execution primitives.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Structured context passed between agents via edge data.
///
/// Supervisors produce handoffs that describe what a worker should do;
/// workers consume handoffs and return results. The type is
/// serde-serializable so it can travel as JSON through node inputs/outputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHandoff {
    /// The agent that initiated the delegation.
    pub from_agent: String,
    /// The target agent that should handle the task.
    pub to_agent: String,
    /// The delegated task description or instruction.
    pub task: Value,
    /// Relevant context from the source agent (conversation history,
    /// intermediate results, etc.).
    pub context: Value,
    /// Optional constraints or guidelines for the worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<Value>,
}

impl AgentHandoff {
    /// Extract an `AgentHandoff` from a node's output JSON.
    ///
    /// Looks for a `_handoff` key in the output and deserializes it.
    /// Returns `None` if the key is missing or the value doesn't
    /// match the expected shape.
    pub fn from_node_output(output: &Value) -> Option<Self> {
        output
            .get("_handoff")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    /// Format this handoff as a JSON value suitable for downstream node input.
    ///
    /// The handoff is placed under a `_handoff` key so workers can
    /// reliably locate it regardless of other input data.
    pub fn to_node_input(&self) -> Value {
        serde_json::json!({
            "_handoff": serde_json::to_value(self).unwrap_or_default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_handoff() -> AgentHandoff {
        AgentHandoff {
            from_agent: "supervisor".into(),
            to_agent: "researcher".into(),
            task: json!("Find information about Rust async patterns"),
            context: json!({"previous_findings": []}),
            constraints: Some(json!({"max_sources": 5})),
        }
    }

    #[test]
    fn serde_roundtrip() {
        let handoff = sample_handoff();
        let json = serde_json::to_value(&handoff).unwrap();
        let deserialized: AgentHandoff = serde_json::from_value(json).unwrap();

        assert_eq!(deserialized.from_agent, "supervisor");
        assert_eq!(deserialized.to_agent, "researcher");
        assert_eq!(
            deserialized.task,
            json!("Find information about Rust async patterns")
        );
        assert!(deserialized.constraints.is_some());
    }

    #[test]
    fn serde_roundtrip_without_constraints() {
        let handoff = AgentHandoff {
            from_agent: "sup".into(),
            to_agent: "worker".into(),
            task: json!("do something"),
            context: json!({}),
            constraints: None,
        };
        let json_str = serde_json::to_string(&handoff).unwrap();
        assert!(!json_str.contains("constraints"));

        let deserialized: AgentHandoff = serde_json::from_str(&json_str).unwrap();
        assert!(deserialized.constraints.is_none());
    }

    #[test]
    fn from_node_output_extracts_handoff() {
        let handoff = sample_handoff();
        let output = json!({
            "_handoff": serde_json::to_value(&handoff).unwrap(),
            "other_data": "ignored"
        });

        let extracted = AgentHandoff::from_node_output(&output).unwrap();
        assert_eq!(extracted.from_agent, "supervisor");
        assert_eq!(extracted.to_agent, "researcher");
    }

    #[test]
    fn from_node_output_returns_none_when_missing() {
        let output = json!({"content": "no handoff here"});
        assert!(AgentHandoff::from_node_output(&output).is_none());
    }

    #[test]
    fn from_node_output_returns_none_on_invalid_shape() {
        let output = json!({"_handoff": "not an object"});
        assert!(AgentHandoff::from_node_output(&output).is_none());
    }

    #[test]
    fn to_node_input_wraps_under_handoff_key() {
        let handoff = sample_handoff();
        let input = handoff.to_node_input();

        assert!(input.get("_handoff").is_some());
        let extracted = AgentHandoff::from_node_output(&input).unwrap();
        assert_eq!(extracted.from_agent, "supervisor");
    }
}

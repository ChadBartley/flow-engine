//! LangGraph framework adapter.
//!
//! Translates LangGraph node transition and state events (serialized as JSON)
//! into [`AdapterEvent`] values.

use chrono::Utc;

use crate::adapter::FrameworkAdapter;
use crate::types::AdapterEvent;

/// Adapter for LangGraph callback events.
///
/// Expects JSON payloads with a `"callback"` field:
/// - `node_start` / `node_end` — graph node lifecycle
/// - `edge` — conditional or direct edge transitions
/// - `on_llm_start` / `on_llm_end` — LLM calls within nodes
/// - `state_update` — state mutations
pub struct LangGraphAdapter;

impl FrameworkAdapter for LangGraphAdapter {
    fn translate(&self, raw: serde_json::Value) -> Vec<AdapterEvent> {
        let callback = match raw.get("callback").and_then(|v| v.as_str()) {
            Some(cb) => cb,
            None => return vec![],
        };

        match callback {
            "node_start" => {
                let name = raw
                    .get("node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let data = raw.get("state").cloned().unwrap_or_default();

                vec![AdapterEvent::StepStart {
                    name,
                    data,
                    timestamp: Utc::now(),
                }]
            }
            "node_end" => {
                let name = raw
                    .get("node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let data = raw.get("state").cloned().unwrap_or_default();

                vec![AdapterEvent::StepEnd {
                    name,
                    data,
                    timestamp: Utc::now(),
                }]
            }
            "edge" => {
                let from = raw
                    .get("from")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let to = raw
                    .get("to")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                vec![AdapterEvent::NodeTransition {
                    from,
                    to,
                    timestamp: Utc::now(),
                }]
            }
            "on_llm_start" => {
                let node = raw
                    .get("node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                vec![AdapterEvent::LlmStart {
                    node_id: node,
                    request: raw.get("request").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            "on_llm_end" => {
                let node = raw
                    .get("node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                vec![AdapterEvent::LlmEnd {
                    node_id: node,
                    response: raw.get("response").cloned().unwrap_or_default(),
                    duration_ms: raw.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                    timestamp: Utc::now(),
                }]
            }
            "state_update" => {
                let node = raw
                    .get("node")
                    .and_then(|v| v.as_str())
                    .unwrap_or("state")
                    .to_string();

                vec![AdapterEvent::StepStart {
                    name: format!("{node}_state_update"),
                    data: raw.get("updates").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            _ => vec![],
        }
    }

    fn framework_name(&self) -> &str {
        "langgraph"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_node_lifecycle() {
        let adapter = LangGraphAdapter;

        let start = adapter.translate(json!({
            "callback": "node_start",
            "node": "agent",
            "state": {"messages": []}
        }));
        assert_eq!(start.len(), 1);
        assert!(matches!(&start[0], AdapterEvent::StepStart { name, .. } if name == "agent"));

        let end = adapter.translate(json!({
            "callback": "node_end",
            "node": "agent",
            "state": {"messages": ["hi"]}
        }));
        assert_eq!(end.len(), 1);
        assert!(matches!(&end[0], AdapterEvent::StepEnd { name, .. } if name == "agent"));
    }

    #[test]
    fn test_edge_transition() {
        let adapter = LangGraphAdapter;
        let events = adapter.translate(json!({
            "callback": "edge",
            "from": "agent",
            "to": "tools"
        }));
        assert_eq!(events.len(), 1);
        if let AdapterEvent::NodeTransition { from, to, .. } = &events[0] {
            assert_eq!(from, "agent");
            assert_eq!(to, "tools");
        } else {
            panic!("expected NodeTransition");
        }
    }

    #[test]
    fn test_llm_within_node() {
        let adapter = LangGraphAdapter;
        let events = adapter.translate(json!({
            "callback": "on_llm_start",
            "node": "agent",
            "request": {"model": "gpt-4", "messages": []}
        }));
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AdapterEvent::LlmStart { node_id, .. } if node_id == "agent"));
    }
}

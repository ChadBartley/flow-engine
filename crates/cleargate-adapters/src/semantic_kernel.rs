//! Semantic Kernel framework adapter.
//!
//! Translates Semantic Kernel `KernelHooks` events (serialized as JSON)
//! into [`AdapterEvent`] values.

use chrono::Utc;

use crate::adapter::FrameworkAdapter;
use crate::types::AdapterEvent;

/// Adapter for Microsoft Semantic Kernel callback events.
///
/// Expects JSON payloads with a `"callback"` field:
/// - `function_invoking` / `function_invoked` — plugin function lifecycle
/// - `prompt_rendering` / `prompt_rendered` — prompt template events
/// - `on_llm_start` / `on_llm_end` — LLM completion calls
pub struct SemanticKernelAdapter;

impl FrameworkAdapter for SemanticKernelAdapter {
    fn translate(&self, raw: serde_json::Value) -> Vec<AdapterEvent> {
        let callback = match raw.get("callback").and_then(|v| v.as_str()) {
            Some(cb) => cb,
            None => return vec![],
        };

        match callback {
            "function_invoking" => {
                let plugin = raw
                    .get("plugin_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let function = raw
                    .get("function_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let node_id = format!("{plugin}.{function}");

                vec![AdapterEvent::ToolStart {
                    node_id,
                    tool_name: function.to_string(),
                    inputs: raw.get("arguments").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            "function_invoked" => {
                let plugin = raw
                    .get("plugin_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let function = raw
                    .get("function_name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                let node_id = format!("{plugin}.{function}");

                vec![AdapterEvent::ToolEnd {
                    node_id,
                    tool_name: function.to_string(),
                    outputs: raw.get("result").cloned().unwrap_or_default(),
                    duration_ms: raw.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                    timestamp: Utc::now(),
                }]
            }
            "prompt_rendering" => {
                vec![AdapterEvent::StepStart {
                    name: "prompt_rendering".into(),
                    data: raw.get("template").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            "prompt_rendered" => {
                vec![AdapterEvent::StepEnd {
                    name: "prompt_rendering".into(),
                    data: raw.get("rendered_prompt").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            "on_llm_start" => {
                let node_id = raw
                    .get("service_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("llm")
                    .to_string();

                vec![AdapterEvent::LlmStart {
                    node_id,
                    request: raw.get("request").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            "on_llm_end" => {
                let node_id = raw
                    .get("service_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("llm")
                    .to_string();

                vec![AdapterEvent::LlmEnd {
                    node_id,
                    response: raw.get("response").cloned().unwrap_or_default(),
                    duration_ms: raw.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                    timestamp: Utc::now(),
                }]
            }
            _ => vec![],
        }
    }

    fn framework_name(&self) -> &str {
        "semantic_kernel"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_function_lifecycle() {
        let adapter = SemanticKernelAdapter;

        let invoking = adapter.translate(json!({
            "callback": "function_invoking",
            "plugin_name": "SearchPlugin",
            "function_name": "search",
            "arguments": {"query": "rust lang"}
        }));
        assert_eq!(invoking.len(), 1);
        if let AdapterEvent::ToolStart {
            node_id, tool_name, ..
        } = &invoking[0]
        {
            assert_eq!(node_id, "SearchPlugin.search");
            assert_eq!(tool_name, "search");
        } else {
            panic!("expected ToolStart");
        }

        let invoked = adapter.translate(json!({
            "callback": "function_invoked",
            "plugin_name": "SearchPlugin",
            "function_name": "search",
            "result": {"text": "Rust is a systems language"},
            "duration_ms": 150
        }));
        assert_eq!(invoked.len(), 1);
        if let AdapterEvent::ToolEnd { duration_ms, .. } = &invoked[0] {
            assert_eq!(*duration_ms, 150);
        } else {
            panic!("expected ToolEnd");
        }
    }

    #[test]
    fn test_prompt_rendering() {
        let adapter = SemanticKernelAdapter;

        let rendering = adapter.translate(json!({
            "callback": "prompt_rendering",
            "template": "Tell me about {{$topic}}"
        }));
        assert_eq!(rendering.len(), 1);
        assert!(
            matches!(&rendering[0], AdapterEvent::StepStart { name, .. } if name == "prompt_rendering")
        );

        let rendered = adapter.translate(json!({
            "callback": "prompt_rendered",
            "rendered_prompt": "Tell me about Rust"
        }));
        assert_eq!(rendered.len(), 1);
        assert!(
            matches!(&rendered[0], AdapterEvent::StepEnd { name, .. } if name == "prompt_rendering")
        );
    }
}

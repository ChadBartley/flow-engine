//! LangChain framework adapter.
//!
//! Translates LangChain `CallbackHandler` events (serialized as JSON)
//! into [`AdapterEvent`] values.

use chrono::Utc;

use crate::adapter::FrameworkAdapter;
use crate::types::AdapterEvent;

/// Extract the model name from a LangChain serialized `repr` string.
///
/// Some providers (e.g. ChatOllama) serialize without `kwargs` and only
/// embed the model name in the repr, like:
/// `"ChatOllama(callbacks=[...], model='ministral-3:3b')"`.
fn extract_model_from_repr(serialized: &serde_json::Value) -> Option<String> {
    let repr = serialized.get("repr")?.as_str()?;
    // Look for model='...' or model="..."
    let after = repr.split("model=").nth(1)?;
    let quote = after.chars().next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    let model = after[1..].split(quote).next()?;
    if model.is_empty() {
        return None;
    }
    Some(model.to_string())
}

/// Adapter for LangChain callback events.
///
/// Expects JSON payloads with a `"callback"` field matching LangChain's
/// callback handler method names:
/// - `on_llm_start` / `on_llm_end`
/// - `on_tool_start` / `on_tool_end`
/// - `on_chain_start` / `on_chain_end`
pub struct LangChainAdapter;

impl FrameworkAdapter for LangChainAdapter {
    fn translate(&self, raw: serde_json::Value) -> Vec<AdapterEvent> {
        let callback = match raw.get("callback").and_then(|v| v.as_str()) {
            Some(cb) => cb,
            None => return vec![],
        };

        let run_id = raw
            .get("run_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        match callback {
            "on_llm_start" => {
                let serialized = raw.get("serialized").cloned().unwrap_or_default();
                let prompts = raw.get("prompts").cloned().unwrap_or_default();
                let kwargs = serialized.get("kwargs");

                // Model name: try kwargs paths first (OpenAI-style serialization),
                // then parse from repr string (Ollama-style where kwargs is absent),
                // then fall back to the last element of serialized.id.
                let model = kwargs
                    .and_then(|k| {
                        k.get("model_name").or_else(|| k.get("model")).or_else(|| {
                            k.get("kwargs")
                                .and_then(|kk| kk.get("model_name").or_else(|| kk.get("model")))
                        })
                    })
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| extract_model_from_repr(&serialized))
                    .or_else(|| {
                        serialized
                            .get("id")
                            .and_then(|ids| ids.as_array()?.last()?.as_str())
                            .map(|s| s.to_string())
                    })
                    .unwrap_or_else(|| "unknown".to_string());

                // Provider: extract from serialized.id[0], stripping the
                // "langchain_" / "langchain-" prefix if present (e.g.
                // "langchain_ollama" → "ollama"). Falls back to "langchain".
                let provider = serialized
                    .get("id")
                    .and_then(|ids| ids.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .map(|s| {
                        s.strip_prefix("langchain_")
                            .or_else(|| s.strip_prefix("langchain-"))
                            .unwrap_or(s)
                    })
                    .unwrap_or("langchain");

                vec![AdapterEvent::LlmStart {
                    node_id: run_id,
                    request: serde_json::json!({
                        "provider": provider,
                        "model": model,
                        "messages": prompts,
                    }),
                    timestamp: Utc::now(),
                }]
            }
            "on_llm_end" => {
                let response = raw.get("response").cloned().unwrap_or_default();
                let generations = response.get("generations").and_then(|g| g.as_array());

                let content = generations
                    .and_then(|gens| gens.first())
                    .and_then(|gen| gen.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|g| g.get("text"))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!(""));

                let llm_output = response.get("llm_output").cloned().unwrap_or_default();
                let usage = llm_output.get("token_usage").cloned().unwrap_or_default();

                // Model: try llm_output.model_name / model, then look in the
                // first generation's generation_info.model (Ollama path).
                let model = llm_output
                    .get("model_name")
                    .or_else(|| llm_output.get("model"))
                    .or_else(|| {
                        generations
                            .and_then(|gens| gens.first())
                            .and_then(|gen| gen.as_array())
                            .and_then(|arr| arr.first())
                            .and_then(|g| g.get("generation_info"))
                            .and_then(|gi| gi.get("model"))
                    })
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");

                vec![AdapterEvent::LlmEnd {
                    node_id: run_id,
                    response: serde_json::json!({
                        "content": content,
                        "model": model,
                        "usage": {
                            "prompt_tokens": usage.get("prompt_tokens"),
                            "completion_tokens": usage.get("completion_tokens"),
                            "total_tokens": usage.get("total_tokens"),
                        },
                    }),
                    duration_ms: raw.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                    timestamp: Utc::now(),
                }]
            }
            "on_tool_start" => {
                let tool_name = raw
                    .get("serialized")
                    .and_then(|s| s.get("name"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();
                let input_str = raw
                    .get("input_str")
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!(""));

                vec![AdapterEvent::ToolStart {
                    node_id: run_id,
                    tool_name,
                    inputs: input_str,
                    timestamp: Utc::now(),
                }]
            }
            "on_tool_end" => {
                let output = raw.get("output").cloned().unwrap_or_default();

                vec![AdapterEvent::ToolEnd {
                    node_id: run_id,
                    tool_name: "unknown".into(),
                    outputs: output,
                    duration_ms: raw.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0),
                    timestamp: Utc::now(),
                }]
            }
            "on_chain_start" => {
                let name = raw
                    .get("serialized")
                    .and_then(|s| s.get("id"))
                    .and_then(|ids| ids.as_array())
                    .and_then(|arr| arr.last())
                    .and_then(|v| v.as_str())
                    .unwrap_or("chain")
                    .to_string();

                vec![AdapterEvent::StepStart {
                    name,
                    data: raw.get("inputs").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            "on_chain_end" => {
                let name = raw
                    .get("serialized")
                    .and_then(|s| s.get("id"))
                    .and_then(|ids| ids.as_array())
                    .and_then(|arr| arr.last())
                    .and_then(|v| v.as_str())
                    .unwrap_or("chain")
                    .to_string();

                vec![AdapterEvent::StepEnd {
                    name,
                    data: raw.get("outputs").cloned().unwrap_or_default(),
                    timestamp: Utc::now(),
                }]
            }
            _ => vec![],
        }
    }

    fn framework_name(&self) -> &str {
        "langchain"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_on_llm_start() {
        let adapter = LangChainAdapter;
        let raw = json!({
            "callback": "on_llm_start",
            "run_id": "run-123",
            "serialized": {
                "id": ["langchain_openai", "chat_models", "ChatOpenAI"],
                "kwargs": {"model_name": "gpt-4o"}
            },
            "prompts": ["Hello, how are you?"]
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        if let AdapterEvent::LlmStart {
            node_id, request, ..
        } = &events[0]
        {
            assert_eq!(node_id, "run-123");
            assert_eq!(request["model"], "gpt-4o");
            assert_eq!(request["provider"], "openai");
        } else {
            panic!("expected LlmStart");
        }
    }

    #[test]
    fn test_on_llm_end() {
        let adapter = LangChainAdapter;
        let raw = json!({
            "callback": "on_llm_end",
            "run_id": "run-123",
            "response": {
                "generations": [[{"text": "I'm fine!"}]],
                "llm_output": {
                    "model_name": "gpt-4o",
                    "token_usage": {
                        "prompt_tokens": 10,
                        "completion_tokens": 5,
                        "total_tokens": 15
                    }
                }
            },
            "duration_ms": 200
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        if let AdapterEvent::LlmEnd {
            response,
            duration_ms,
            ..
        } = &events[0]
        {
            assert_eq!(response["content"], "I'm fine!");
            assert_eq!(response["model"], "gpt-4o");
            assert_eq!(*duration_ms, 200);
        } else {
            panic!("expected LlmEnd");
        }
    }

    #[test]
    fn test_on_chain_start() {
        let adapter = LangChainAdapter;
        let raw = json!({
            "callback": "on_chain_start",
            "serialized": {"id": ["langchain", "chains", "RetrievalQA"]},
            "inputs": {"query": "what is rust?"}
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], AdapterEvent::StepStart { name, .. } if name == "RetrievalQA")
        );
    }

    #[test]
    fn test_on_llm_start_model_fallback() {
        let adapter = LangChainAdapter;
        let raw = json!({
            "callback": "on_llm_start",
            "run_id": "run-ollama",
            "serialized": {
                "id": ["langchain", "llms", "ollama", "ChatOllama"],
                "kwargs": {"model": "llama3"}
            },
            "prompts": ["Hello"]
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        if let AdapterEvent::LlmStart { request, .. } = &events[0] {
            assert_eq!(request["model"], "llama3");
            assert_eq!(request["provider"], "langchain");
        } else {
            panic!("expected LlmStart");
        }
    }

    #[test]
    fn test_on_llm_start_repr_extraction() {
        let adapter = LangChainAdapter;
        // ChatOllama serializes without kwargs — model is only in repr
        let raw = json!({
            "callback": "on_llm_start",
            "run_id": "run-ollama-repr",
            "serialized": {
                "lc": 1,
                "type": "not_implemented",
                "id": ["langchain_ollama", "chat_models", "ChatOllama"],
                "repr": "ChatOllama(callbacks=[...], model='ministral-3:3b')",
                "name": "ChatOllama"
            },
            "prompts": ["Hello"]
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        if let AdapterEvent::LlmStart { request, .. } = &events[0] {
            assert_eq!(request["model"], "ministral-3:3b");
            assert_eq!(request["provider"], "ollama");
        } else {
            panic!("expected LlmStart");
        }
    }

    #[test]
    fn test_on_llm_end_model_fallback() {
        let adapter = LangChainAdapter;
        let raw = json!({
            "callback": "on_llm_end",
            "run_id": "run-ollama",
            "response": {
                "generations": [[{"text": "Hello!"}]],
                "llm_output": {
                    "model": "llama3",
                    "token_usage": {
                        "prompt_tokens": 5,
                        "completion_tokens": 3,
                        "total_tokens": 8
                    }
                }
            },
            "duration_ms": 100
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        if let AdapterEvent::LlmEnd { response, .. } = &events[0] {
            assert_eq!(response["model"], "llama3");
        } else {
            panic!("expected LlmEnd");
        }
    }

    #[test]
    fn test_on_chain_end_preserves_name() {
        let adapter = LangChainAdapter;
        let raw = json!({
            "callback": "on_chain_end",
            "serialized": {"id": ["langchain", "chains", "RetrievalQA"]},
            "outputs": {"result": "Rust is a systems programming language."}
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AdapterEvent::StepEnd { name, .. } if name == "RetrievalQA"));
    }

    #[test]
    fn test_on_chain_end_fallback() {
        let adapter = LangChainAdapter;
        let raw = json!({
            "callback": "on_chain_end",
            "outputs": {"result": "done"}
        });

        let events = adapter.translate(raw);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], AdapterEvent::StepEnd { name, .. } if name == "chain"));
    }

    #[test]
    fn test_unknown_callback() {
        let adapter = LangChainAdapter;
        let events = adapter.translate(json!({"callback": "on_something_else"}));
        assert!(events.is_empty());
    }
}

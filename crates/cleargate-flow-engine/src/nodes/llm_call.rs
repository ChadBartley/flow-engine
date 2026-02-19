//! Built-in LLM Call node handler.
//!
//! This node makes LLM API calls within a flow using registered providers.
//! It supports tool calling, conversation state persistence across cycles,
//! and records all LLM invocations for replay and cost analysis.

use std::collections::BTreeMap;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};

use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Built-in node that invokes an LLM provider.
pub struct LlmCallNode;

#[async_trait]
impl NodeHandler for LlmCallNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "llm_call".into(),
            label: "LLM Call".into(),
            category: "ai".into(),
            inputs: vec![
                PortDef {
                    name: "messages".into(),
                    port_type: PortType::Json,
                    sensitivity: Sensitivity::default(),
                    required: false,
                    default: None,
                    description: None,
                },
                PortDef {
                    name: "context".into(),
                    port_type: PortType::Json,
                    sensitivity: Sensitivity::default(),
                    required: false,
                    default: None,
                    description: None,
                },
            ],
            outputs: vec![
                PortDef {
                    name: "content".into(),
                    port_type: PortType::Json,
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: None,
                },
                PortDef {
                    name: "tool_calls".into(),
                    port_type: PortType::List(Box::new(PortType::Json)),
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: None,
                },
                PortDef {
                    name: "finish_reason".into(),
                    port_type: PortType::String,
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: None,
                },
            ],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "provider": { "type": "string" },
                    "model": { "type": "string" },
                    "system_prompt": { "type": "string" },
                    "temperature": { "type": "number" },
                    "max_tokens": { "type": "integer" },
                    "tools_from_graph": { "type": "boolean" },
                    "tool_filter": { "type": "array", "items": { "type": "string" } },
                    "api_key_secret": { "type": "string" },
                    "response_format": { "type": "object", "description": "JSON schema or response format spec for structured output" },
                    "stream": { "type": "boolean", "description": "Enable token-by-token streaming via LlmChunk events" }
                },
                "required": ["provider", "model"]
            }),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        // 1. Read required config fields
        let provider_name = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required config field: provider".into(),
            })?
            .to_string();

        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required config field: model".into(),
            })?
            .to_string();

        // 2. Look up the provider
        let provider = ctx
            .llm_provider(&provider_name)
            .ok_or_else(|| NodeError::Fatal {
                message: format!("LLM provider '{}' not registered in engine", provider_name),
            })?;

        // 3. Build conversation state
        let state_key = format!("{}:messages", ctx.node_instance_id());

        let system_prompt = config.get("system_prompt").and_then(|v| v.as_str());
        let temperature = config.get("temperature").and_then(|v| v.as_f64());
        let max_tokens = config
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32);

        // Load prior conversation from state (for cycle/back-edge support)
        let prior_messages = ctx.state_get(&state_key).await?;
        let has_prior = prior_messages.is_some();
        let mut messages: Vec<Value> = if let Some(Value::Array(msgs)) = prior_messages {
            msgs
        } else {
            // First invocation: build initial messages
            let mut msgs = Vec::new();
            if let Some(sp) = system_prompt {
                msgs.push(json!({"role": "system", "content": sp}));
            }
            msgs
        };

        // Append new input messages
        if !has_prior || messages.is_empty() {
            // First time: add user messages from input
            if let Some(Value::Array(input_msgs)) = inputs.get("messages") {
                messages.extend(input_msgs.clone());
            } else if let Some(msg_val) = inputs.get("messages") {
                messages.push(msg_val.clone());
            } else {
                // Raw input as user message
                messages.push(json!({"role": "user", "content": inputs.to_string()}));
            }
        } else {
            // Back-edge: append tool results from inputs
            // The inputs come from tool execution results
            if let Some(Value::Array(tool_results)) = inputs.get("tool_results") {
                messages.extend(tool_results.clone());
            } else if inputs.get("tool_call_id").is_some() {
                // Single tool result
                messages.push(inputs.clone());
            } else if let Some(Value::Array(input_msgs)) = inputs.get("messages") {
                messages.extend(input_msgs.clone());
            } else if inputs.is_object() || inputs.is_array() {
                // Tool result objects from fan-in
                if let Value::Array(arr) = &inputs {
                    for item in arr {
                        messages.push(item.clone());
                    }
                } else {
                    messages.push(inputs.clone());
                }
            }
        }

        // 4. Build tools from graph (if enabled)
        let tools_from_graph = config
            .get("tools_from_graph")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let tool_filter: Option<Vec<String>> = config.get("tool_filter").and_then(|v| {
            v.as_array().map(|arr| {
                arr.iter()
                    .filter_map(|item| item.as_str().map(String::from))
                    .collect()
            })
        });

        let tools = if tools_from_graph {
            let available = ctx.available_tools();
            let filtered: Vec<Value> = available
                .iter()
                .filter(|t| {
                    tool_filter
                        .as_ref()
                        .is_none_or(|filter| filter.contains(&t.name))
                })
                .map(|t| {
                    json!({
                        "type": "function",
                        "function": {
                            "name": t.name,
                            "description": t.description,
                            "parameters": t.parameters
                        }
                    })
                })
                .collect();

            if filtered.is_empty() {
                None
            } else {
                Some(filtered)
            }
        } else {
            None
        };

        let response_format = config.get("response_format").cloned();

        // 5. Build LlmRequest with engine types
        let llm_request = LlmRequest {
            provider: provider_name.clone(),
            model: model.clone(),
            messages: Value::Array(messages.clone()),
            tools,
            temperature,
            top_p: None,
            max_tokens,
            stop_sequences: None,
            response_format,
            seed: None,
            extra_params: BTreeMap::new(),
        };

        // 6. Call the provider — streaming or non-streaming
        let stream_enabled = config
            .get("stream")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let llm_response = if stream_enabled && provider.supports_streaming() {
            // Streaming path: iterate chunks, accumulate, emit each chunk
            let mut stream = provider.complete_streaming(llm_request.clone()).await?;

            let start = std::time::Instant::now();
            let mut content = String::new();
            let mut tool_call_parts: BTreeMap<u32, (String, String, String)> = BTreeMap::new();
            let mut finish_reason = String::from("stop");
            let mut input_tokens = None;
            let mut output_tokens = None;

            while let Some(chunk_result) = stream.next().await {
                let chunk = chunk_result?;
                ctx.emit_llm_chunk(chunk.clone());

                match &chunk {
                    LlmChunk::TextDelta { delta } => {
                        content.push_str(delta);
                    }
                    LlmChunk::ToolCallDelta {
                        index,
                        id,
                        name,
                        arguments_delta,
                    } => {
                        let entry = tool_call_parts
                            .entry(*index)
                            .or_insert_with(|| (String::new(), String::new(), String::new()));
                        if let Some(id_val) = id {
                            entry.0 = id_val.clone();
                        }
                        if let Some(name_val) = name {
                            entry.1 = name_val.clone();
                        }
                        entry.2.push_str(arguments_delta);
                    }
                    LlmChunk::Usage {
                        input_tokens: it,
                        output_tokens: ot,
                    } => {
                        if it.is_some() {
                            input_tokens = *it;
                        }
                        if ot.is_some() {
                            output_tokens = *ot;
                        }
                    }
                    LlmChunk::Done { finish_reason: fr } => {
                        finish_reason = fr.clone();
                    }
                }
            }

            let latency_ms = start.elapsed().as_millis() as u64;

            let tool_calls: Option<Vec<LlmToolCall>> = if tool_call_parts.is_empty() {
                None
            } else {
                Some(
                    tool_call_parts
                        .into_values()
                        .map(|(id, name, args)| LlmToolCall {
                            id,
                            tool_name: name,
                            arguments: serde_json::from_str(&args)
                                .unwrap_or(Value::Object(serde_json::Map::new())),
                        })
                        .collect(),
                )
            };

            LlmResponse {
                content: json!(content),
                tool_calls,
                model_used: model.clone(),
                input_tokens,
                output_tokens,
                total_tokens: match (input_tokens, output_tokens) {
                    (Some(i), Some(o)) => Some(i + o),
                    _ => None,
                },
                finish_reason,
                latency_ms,
                provider_request_id: None,
                cost: None,
            }
        } else {
            // Non-streaming path
            provider.complete(llm_request.clone()).await?
        };

        // 7. Extract tool calls from response
        let tool_calls_value: Option<Vec<Value>> = llm_response.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| {
                    json!({
                        "id": tc.id,
                        "tool_name": tc.tool_name,
                        "arguments": tc.arguments
                    })
                })
                .collect()
        });

        // 8. Save conversation state (messages + assistant reply)
        let assistant_msg = if let Some(ref tool_calls) = llm_response.tool_calls {
            // Assistant message with tool calls
            let tc_value: Value = serde_json::to_value(tool_calls).unwrap_or(Value::Null);
            json!({
                "role": "assistant",
                "content": llm_response.content.clone(),
                "tool_calls": tc_value
            })
        } else {
            json!({
                "role": "assistant",
                "content": llm_response.content.clone()
            })
        };
        messages.push(assistant_msg);
        ctx.state_set(&state_key, Value::Array(messages.clone()))
            .await?;

        // 9. Record the LLM invocation for replay and cost tracking
        ctx.record_llm_call(llm_request, llm_response.clone())
            .await?;

        // 10. Return outputs
        Ok(json!({
            "content": llm_response.content,
            "tool_calls": tool_calls_value,
            "finish_reason": llm_response.finish_reason
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;
    use crate::traits::FlowLlmProvider;
    use async_trait::async_trait;
    use futures::Stream;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::pin::Pin;

    /// Simple mock LLM provider for testing
    struct MockLlmProvider {
        response_content: String,
    }

    impl MockLlmProvider {
        fn new(content: &str) -> Self {
            Self {
                response_content: content.to_string(),
            }
        }
    }

    #[async_trait]
    impl FlowLlmProvider for MockLlmProvider {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, NodeError> {
            Ok(LlmResponse {
                content: json!(self.response_content.clone()),
                tool_calls: None,
                model_used: "mock-model".to_string(),
                input_tokens: Some(10),
                output_tokens: Some(20),
                total_tokens: Some(30),
                finish_reason: "stop".to_string(),
                latency_ms: 100,
                provider_request_id: None,
                cost: None,
            })
        }

        fn name(&self) -> &str {
            "mock"
        }
    }

    #[tokio::test]
    async fn test_llm_call_missing_provider_config() {
        let node = LlmCallNode;
        let config = json!({
            "model": "gpt-4o"
        });
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let err = node.run(json!({}), &config, &ctx).await.unwrap_err();
        match err {
            NodeError::Validation { message } => {
                assert!(message.contains("provider"), "got: {message}");
            }
            other => panic!("expected Validation, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_llm_call_provider_not_registered() {
        let node = LlmCallNode;
        let config = json!({
            "provider": "nonexistent",
            "model": "gpt-4o"
        });
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let err = node.run(json!({}), &config, &ctx).await.unwrap_err();
        match err {
            NodeError::Fatal { message } => {
                assert!(message.contains("not registered"), "got: {message}");
            }
            other => panic!("expected Fatal, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_llm_call_with_mock_provider() {
        let node = LlmCallNode;
        let config = json!({
            "provider": "mock",
            "model": "mock-model",
            "tools_from_graph": false
        });

        let mock_provider = std::sync::Arc::new(MockLlmProvider::new("Hello, world!"));

        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock", mock_provider)
            .build();

        let inputs = json!({
            "messages": [{"role": "user", "content": "hello"}]
        });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        assert!(result.get("content").is_some());
        assert!(result.get("finish_reason").is_some());
        assert_eq!(result["finish_reason"], "stop");

        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);
    }

    /// Mock streaming LLM provider for testing
    struct MockStreamingProvider;

    #[async_trait]
    impl FlowLlmProvider for MockStreamingProvider {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, NodeError> {
            unreachable!("streaming provider should use complete_streaming");
        }

        async fn complete_streaming(
            &self,
            _request: LlmRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk, NodeError>> + Send>>, NodeError>
        {
            let chunks = vec![
                Ok(LlmChunk::TextDelta {
                    delta: "Hello".into(),
                }),
                Ok(LlmChunk::TextDelta {
                    delta: ", world!".into(),
                }),
                Ok(LlmChunk::Usage {
                    input_tokens: Some(10),
                    output_tokens: Some(5),
                }),
                Ok(LlmChunk::Done {
                    finish_reason: "stop".into(),
                }),
            ];
            Ok(Box::pin(futures::stream::iter(chunks)))
        }

        fn supports_streaming(&self) -> bool {
            true
        }

        fn name(&self) -> &str {
            "mock-streaming"
        }
    }

    #[tokio::test]
    async fn test_llm_call_streaming() {
        let node = LlmCallNode;
        let config = json!({
            "provider": "mock-streaming",
            "model": "mock-model",
            "tools_from_graph": false,
            "stream": true
        });

        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock-streaming", std::sync::Arc::new(MockStreamingProvider))
            .build();

        let inputs = json!({
            "messages": [{"role": "user", "content": "hello"}]
        });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        assert_eq!(result["content"], "Hello, world!");
        assert_eq!(result["finish_reason"], "stop");

        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn test_llm_call_streaming_fallback_when_not_supported() {
        let node = LlmCallNode;
        let config = json!({
            "provider": "mock",
            "model": "mock-model",
            "tools_from_graph": false,
            "stream": true
        });

        // MockLlmProvider does NOT support streaming, should fall back to complete()
        let mock_provider = std::sync::Arc::new(MockLlmProvider::new("Fallback response"));
        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock", mock_provider)
            .build();

        let inputs = json!({
            "messages": [{"role": "user", "content": "hello"}]
        });

        let result = node.run(inputs, &config, &ctx).await.unwrap();
        assert_eq!(result["finish_reason"], "stop");

        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn test_llm_call_with_tool_filter() {
        let node = LlmCallNode;
        let config = json!({
            "provider": "mock",
            "model": "mock-model",
            "tools_from_graph": true,
            "tool_filter": ["search"]
        });

        let mock_provider = std::sync::Arc::new(MockLlmProvider::new("Using search tool"));

        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock", mock_provider)
            .tool(ToolDef {
                name: "search".into(),
                description: "Search".into(),
                parameters: json!({"type": "object"}),
                tool_type: ToolType::Node {
                    target_node_id: "n1".into(),
                },
                metadata: BTreeMap::new(),
            })
            .tool(ToolDef {
                name: "calc".into(),
                description: "Calculate".into(),
                parameters: json!({"type": "object"}),
                tool_type: ToolType::Node {
                    target_node_id: "n2".into(),
                },
                metadata: BTreeMap::new(),
            })
            .build();

        let _result = node.run(json!({}), &config, &ctx).await.unwrap();

        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);

        let tools = calls[0]
            .request
            .tools
            .as_ref()
            .expect("tools should be set");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["function"]["name"], "search");
    }
}

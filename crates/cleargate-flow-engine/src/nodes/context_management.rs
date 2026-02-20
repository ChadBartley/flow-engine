//! Standalone context management node.
//!
//! Applies memory strategies to an input message array as an explicit
//! pipeline step. Use this when you need context management as a
//! separate node in the graph — for example, to apply different strategies
//! to different branches, or to manage context before a custom LLM node.
//!
//! For most use cases, the automatic integration in `LlmCallNode` (via
//! the `memory` config key) is simpler and sufficient.
//!
//! # Inputs
//!
//! - `messages` (required): JSON array of chat messages to trim.
//!
//! # Config
//!
//! - `memory`: Memory strategy configuration (same format as `LlmCallNode`).
//! - `provider`: Default provider name for summarization fallback.
//! - `model`: Default model name for summarization fallback.
//!
//! # Outputs
//!
//! - `messages`: The trimmed message array.

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::memory::MemoryConfig;
use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Built-in node that applies memory strategies to message arrays.
///
/// Useful for advanced graph patterns where context management happens
/// as an explicit pipeline step rather than automatically in `LlmCallNode`.
pub struct ContextManagementNode;

#[async_trait]
impl NodeHandler for ContextManagementNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "context_management".into(),
            label: "Context Management".into(),
            category: "ai".into(),
            inputs: vec![PortDef {
                name: "messages".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: Some("Array of chat messages to apply memory management to".into()),
            }],
            outputs: vec![PortDef {
                name: "messages".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: Some("Trimmed message array after applying memory strategy".into()),
            }],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "memory": {
                        "type": "object",
                        "description": "Memory strategy configuration"
                    },
                    "provider": {
                        "type": "string",
                        "description": "Default provider for summarization fallback"
                    },
                    "model": {
                        "type": "string",
                        "description": "Default model for summarization fallback"
                    }
                }
            }),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        // Read messages from inputs.
        let messages: Vec<Value> = inputs
            .get("messages")
            .and_then(|v| v.as_array())
            .cloned()
            .ok_or_else(|| NodeError::Validation {
                message: "missing required input: messages (must be a JSON array)".into(),
            })?;

        // Parse memory config.
        let memory_config: MemoryConfig = config
            .get("memory")
            .cloned()
            .map(|v| {
                serde_json::from_value(v).map_err(|e| NodeError::Validation {
                    message: format!("invalid memory config: {e}"),
                })
            })
            .transpose()?
            .unwrap_or_default();

        let result = if let Some(ref strategy) = memory_config.strategy {
            let default_provider = config
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("default");
            let default_model = config
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("default");

            ctx.memory_manager()
                .prepare_messages(
                    messages,
                    strategy,
                    ctx.token_counter().as_ref(),
                    &ctx.llm_providers_arc(),
                    default_provider,
                    default_model,
                )
                .await?
        } else {
            messages
        };

        Ok(json!({ "messages": result }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;
    use serde_json::json;

    fn make_messages(count: usize) -> Vec<Value> {
        let mut msgs = vec![json!({"role": "system", "content": "You are helpful."})];
        for i in 0..count {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            msgs.push(json!({"role": role, "content": format!("Message {i}")}));
        }
        msgs
    }

    #[tokio::test]
    async fn context_node_message_count() {
        let node = ContextManagementNode;
        let msgs = make_messages(10);
        let config = json!({
            "memory": {
                "strategy": "message_count",
                "max_messages": 3
            }
        });

        let (ctx, _) = TestNodeCtx::builder().build();
        let inputs = json!({ "messages": msgs });
        let result = node.run(inputs, &config, &ctx).await.unwrap();

        let out_msgs = result["messages"].as_array().unwrap();
        assert_eq!(out_msgs.len(), 4); // system + 3
    }

    #[tokio::test]
    async fn context_node_token_budget() {
        let node = ContextManagementNode;
        let msgs = make_messages(10);
        let config = json!({
            "memory": {
                "strategy": "token_budget",
                "max_tokens": 20,
                "reserve_system": true
            }
        });

        let (ctx, _) = TestNodeCtx::builder().build();
        let inputs = json!({ "messages": msgs });
        let result = node.run(inputs, &config, &ctx).await.unwrap();

        let out_msgs = result["messages"].as_array().unwrap();
        assert!(out_msgs.len() < 11);
        assert_eq!(out_msgs[0]["role"], "system");
    }

    #[tokio::test]
    async fn context_node_no_strategy_passthrough() {
        let node = ContextManagementNode;
        let msgs = make_messages(5);
        let config = json!({});

        let (ctx, _) = TestNodeCtx::builder().build();
        let inputs = json!({ "messages": msgs.clone() });
        let result = node.run(inputs, &config, &ctx).await.unwrap();

        let out_msgs = result["messages"].as_array().unwrap();
        assert_eq!(out_msgs.len(), msgs.len());
    }

    #[tokio::test]
    async fn context_node_missing_messages_input_error() {
        let node = ContextManagementNode;
        let config = json!({});

        let (ctx, _) = TestNodeCtx::builder().build();
        let result = node.run(json!({}), &config, &ctx).await;

        assert!(result.is_err());
        match result.unwrap_err() {
            NodeError::Validation { message } => {
                assert!(message.contains("messages"), "got: {message}");
            }
            other => panic!("expected Validation, got: {other}"),
        }
    }

    #[tokio::test]
    async fn context_node_empty_messages() {
        let node = ContextManagementNode;
        let config = json!({
            "memory": {
                "strategy": "message_count",
                "max_messages": 5
            }
        });

        let (ctx, _) = TestNodeCtx::builder().build();
        let inputs = json!({ "messages": [] });
        let result = node.run(inputs, &config, &ctx).await.unwrap();

        let out_msgs = result["messages"].as_array().unwrap();
        assert!(out_msgs.is_empty());
    }
}

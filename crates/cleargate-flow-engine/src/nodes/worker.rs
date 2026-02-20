//! Built-in worker node for multi-agent task execution.
//!
//! The worker receives an [`AgentHandoff`] from a supervisor, executes the
//! delegated task by calling an LLM, writes its result to the
//! [`Blackboard`], and returns the result for routing back to the
//! supervisor.
//!
//! ## Config
//!
//! ```json
//! {
//!   "provider": "openai",
//!   "model": "gpt-4o",
//!   "worker_name": "researcher",
//!   "system_prompt": "You are a research specialist..."
//! }
//! ```

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::multi_agent::{AgentHandoff, Blackboard};
use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Built-in worker node that executes a task from an [`AgentHandoff`].
pub struct WorkerNode;

#[async_trait]
impl NodeHandler for WorkerNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "worker".into(),
            label: "Worker".into(),
            category: "multi-agent".into(),
            inputs: vec![PortDef {
                name: "handoff".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: true,
                default: None,
                description: Some("AgentHandoff from a supervisor".into()),
            }],
            outputs: vec![
                PortDef {
                    name: "result".into(),
                    port_type: PortType::Json,
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: Some("The worker's result".into()),
                },
                PortDef {
                    name: "worker_name".into(),
                    port_type: PortType::String,
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: Some("Name of this worker".into()),
                },
            ],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "provider": { "type": "string" },
                    "model": { "type": "string" },
                    "worker_name": { "type": "string" },
                    "system_prompt": { "type": "string" }
                },
                "required": ["provider", "model", "worker_name"]
            }),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        }
    }

    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        let provider_name = config
            .get("provider")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required config field: provider".into(),
            })?;

        let model = config
            .get("model")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required config field: model".into(),
            })?;

        let worker_name = config
            .get("worker_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Validation {
                message: "missing required config field: worker_name".into(),
            })?;

        let provider = ctx
            .llm_provider(provider_name)
            .ok_or_else(|| NodeError::Fatal {
                message: format!("LLM provider '{provider_name}' not registered"),
            })?;

        // Extract handoff from inputs
        let handoff: AgentHandoff = if let Some(handoff_val) = inputs.get("_handoff") {
            serde_json::from_value(handoff_val.clone()).map_err(|e| NodeError::Fatal {
                message: format!("invalid handoff data: {e}"),
            })?
        } else if let Some(handoff_val) = inputs.get("handoff") {
            serde_json::from_value(handoff_val.clone()).map_err(|e| NodeError::Fatal {
                message: format!("invalid handoff data: {e}"),
            })?
        } else {
            // Treat the entire input as a simple task
            AgentHandoff {
                from_agent: "unknown".into(),
                to_agent: worker_name.to_string(),
                task: inputs.clone(),
                context: json!({}),
                constraints: None,
            }
        };

        // Build messages from handoff
        let system_prompt = config
            .get("system_prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("You are a specialized worker agent. Complete the assigned task.");

        let user_content = json!({
            "task": handoff.task,
            "context": handoff.context,
            "constraints": handoff.constraints,
        });

        let messages = json!([
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": serde_json::to_string_pretty(&user_content).unwrap_or_default()}
        ]);

        let llm_request = LlmRequest {
            provider: provider_name.to_string(),
            model: model.to_string(),
            messages,
            tools: None,
            temperature: Some(0.7),
            top_p: None,
            max_tokens: Some(4096),
            stop_sequences: None,
            response_format: None,
            seed: None,
            extra_params: BTreeMap::new(),
        };

        let response = provider.complete(llm_request.clone()).await?;
        ctx.record_llm_call(llm_request, response.clone()).await?;

        // Write result to blackboard
        let bb = Blackboard::new(ctx.state_store(), ctx.run_id().to_string());
        bb.write(worker_name, "result", response.content.clone())
            .await?;

        ctx.emit("worker.completed", json!({"worker_name": worker_name}));

        Ok(json!({
            "result": response.content,
            "worker_name": worker_name
        }))
    }

    fn validate_config(&self, config: &Value) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if config.get("provider").and_then(|v| v.as_str()).is_none() {
            errors.push("missing required field: provider".into());
        }
        if config.get("model").and_then(|v| v.as_str()).is_none() {
            errors.push("missing required field: model".into());
        }
        if config.get("worker_name").and_then(|v| v.as_str()).is_none() {
            errors.push("missing required field: worker_name".into());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::multi_agent::AgentHandoff;
    use crate::node_ctx::TestNodeCtx;
    use crate::traits::FlowLlmProvider;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockWorkerProvider {
        response: String,
    }

    impl MockWorkerProvider {
        fn new(response: &str) -> Self {
            Self {
                response: response.into(),
            }
        }
    }

    #[async_trait]
    impl FlowLlmProvider for MockWorkerProvider {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, NodeError> {
            Ok(LlmResponse {
                content: json!(self.response),
                tool_calls: None,
                model_used: "mock".into(),
                input_tokens: Some(20),
                output_tokens: Some(40),
                total_tokens: Some(60),
                finish_reason: "stop".into(),
                latency_ms: 150,
                provider_request_id: None,
                cost: None,
            })
        }

        fn name(&self) -> &str {
            "mock-worker"
        }
    }

    fn worker_config() -> Value {
        json!({
            "provider": "mock",
            "model": "gpt-4o",
            "worker_name": "researcher",
            "system_prompt": "You are a research specialist."
        })
    }

    #[tokio::test]
    async fn executes_handoff_task() {
        let node = WorkerNode;
        let provider = std::sync::Arc::new(MockWorkerProvider::new("Found 3 relevant papers"));
        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let handoff = AgentHandoff {
            from_agent: "supervisor".into(),
            to_agent: "researcher".into(),
            task: json!("Find papers on Rust async"),
            context: json!({}),
            constraints: None,
        };

        let result = node
            .run(handoff.to_node_input(), &worker_config(), &ctx)
            .await
            .unwrap();

        assert_eq!(result["result"], "Found 3 relevant papers");
        assert_eq!(result["worker_name"], "researcher");

        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn writes_result_to_blackboard() {
        let node = WorkerNode;
        let provider = std::sync::Arc::new(MockWorkerProvider::new("result data"));
        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let handoff = AgentHandoff {
            from_agent: "supervisor".into(),
            to_agent: "researcher".into(),
            task: json!("do work"),
            context: json!({}),
            constraints: None,
        };

        node.run(handoff.to_node_input(), &worker_config(), &ctx)
            .await
            .unwrap();

        let state = inspector.state_snapshot().await;
        assert_eq!(state.get("researcher:result"), Some(&json!("result data")));
    }

    #[tokio::test]
    async fn accepts_handoff_in_handoff_key() {
        let node = WorkerNode;
        let provider = std::sync::Arc::new(MockWorkerProvider::new("ok"));
        let (ctx, _inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let input = json!({
            "handoff": {
                "from_agent": "sup",
                "to_agent": "researcher",
                "task": "do it",
                "context": {}
            }
        });

        let result = node.run(input, &worker_config(), &ctx).await.unwrap();
        assert_eq!(result["worker_name"], "researcher");
    }

    #[tokio::test]
    async fn fallback_when_no_handoff() {
        let node = WorkerNode;
        let provider = std::sync::Arc::new(MockWorkerProvider::new("processed"));
        let (ctx, _inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let result = node
            .run(json!({"raw": "data"}), &worker_config(), &ctx)
            .await
            .unwrap();
        assert_eq!(result["worker_name"], "researcher");
    }

    #[tokio::test]
    async fn missing_provider_config() {
        let node = WorkerNode;
        let config = json!({"model": "m", "worker_name": "w"});
        let (ctx, _) = TestNodeCtx::builder().build();

        let err = node.run(json!({}), &config, &ctx).await.unwrap_err();
        match err {
            NodeError::Validation { message } => {
                assert!(message.contains("provider"), "got: {message}");
            }
            other => panic!("expected Validation, got: {other}"),
        }
    }

    #[tokio::test]
    async fn validate_config_catches_errors() {
        let node = WorkerNode;
        let errs = node.validate_config(&json!({})).unwrap_err();
        assert_eq!(errs.len(), 3);

        assert!(node.validate_config(&worker_config()).is_ok());
    }
}

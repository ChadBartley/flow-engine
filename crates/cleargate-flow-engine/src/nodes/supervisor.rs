//! Built-in supervisor node for multi-agent delegation.
//!
//! The supervisor receives a task (or worker results from a previous round),
//! consults an LLM to decide which worker to delegate to, and emits an
//! [`AgentHandoff`] with routing information. When all work is complete, it
//! sets `done: true` and provides the final aggregated result.
//!
//! ## Routing
//!
//! The output includes a `target_worker` string field that downstream
//! conditional edges can match on to route to the correct worker subgraph.
//!
//! ## Config
//!
//! ```json
//! {
//!   "provider": "openai",
//!   "model": "gpt-4o",
//!   "system_prompt": "You are a supervisor...",
//!   "workers": {
//!     "researcher": { "description": "Searches for information" },
//!     "writer": { "description": "Writes content" }
//!   }
//! }
//! ```

use std::collections::BTreeMap;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::multi_agent::{AgentHandoff, Blackboard};
use crate::node_ctx::NodeCtx;
use crate::traits::NodeHandler;
use crate::types::*;

/// Built-in supervisor node that delegates tasks to worker agents via LLM decisions.
pub struct SupervisorNode;

/// Build a system prompt that instructs the LLM to choose a worker.
fn build_supervisor_prompt(
    user_system_prompt: Option<&str>,
    workers: &BTreeMap<String, WorkerDef>,
) -> String {
    let mut prompt = user_system_prompt
        .unwrap_or("You are a supervisor agent that delegates tasks to specialized workers.")
        .to_string();

    prompt.push_str("\n\nAvailable workers:\n");
    for (name, def) in workers {
        prompt.push_str(&format!("- {name}: {}\n", def.description));
    }

    prompt.push_str(
        "\nRespond with a JSON object. If you need to delegate, use:\n\
         {\"done\": false, \"target_worker\": \"<worker_name>\", \"task\": \"<task description>\", \"context\": <relevant context>}\n\n\
         If the task is complete and you have the final result, use:\n\
         {\"done\": true, \"final_result\": <the final aggregated result>}\n\n\
         Always respond with valid JSON only, no other text.",
    );

    prompt
}

#[derive(Debug, Clone)]
struct WorkerDef {
    description: String,
}

/// Parse the `workers` config map.
fn parse_workers(config: &Value) -> Result<BTreeMap<String, WorkerDef>, NodeError> {
    let workers_val = config.get("workers").ok_or_else(|| NodeError::Validation {
        message: "missing required config field: workers".into(),
    })?;

    let map = workers_val
        .as_object()
        .ok_or_else(|| NodeError::Validation {
            message: "workers must be an object".into(),
        })?;

    let mut workers = BTreeMap::new();
    for (name, def) in map {
        let description = def
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        workers.insert(name.clone(), WorkerDef { description });
    }

    if workers.is_empty() {
        return Err(NodeError::Validation {
            message: "workers must contain at least one worker definition".into(),
        });
    }

    Ok(workers)
}

#[async_trait]
impl NodeHandler for SupervisorNode {
    fn meta(&self) -> NodeMeta {
        NodeMeta {
            node_type: "supervisor".into(),
            label: "Supervisor".into(),
            category: "multi-agent".into(),
            inputs: vec![PortDef {
                name: "task".into(),
                port_type: PortType::Json,
                sensitivity: Sensitivity::default(),
                required: false,
                default: None,
                description: Some("The work to supervise or results from workers".into()),
            }],
            outputs: vec![
                PortDef {
                    name: "handoff".into(),
                    port_type: PortType::Json,
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: Some("AgentHandoff for the target worker".into()),
                },
                PortDef {
                    name: "target_worker".into(),
                    port_type: PortType::String,
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: Some("Name of the worker to route to".into()),
                },
                PortDef {
                    name: "done".into(),
                    port_type: PortType::Bool,
                    sensitivity: Sensitivity::default(),
                    required: true,
                    default: None,
                    description: Some("Whether the supervisor considers the task complete".into()),
                },
                PortDef {
                    name: "final_result".into(),
                    port_type: PortType::Json,
                    sensitivity: Sensitivity::default(),
                    required: false,
                    default: None,
                    description: Some("The aggregated result when done=true".into()),
                },
            ],
            config_schema: json!({
                "type": "object",
                "properties": {
                    "provider": { "type": "string" },
                    "model": { "type": "string" },
                    "system_prompt": { "type": "string" },
                    "workers": {
                        "type": "object",
                        "additionalProperties": {
                            "type": "object",
                            "properties": {
                                "description": { "type": "string" }
                            }
                        }
                    }
                },
                "required": ["provider", "model", "workers"]
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

        let provider = ctx
            .llm_provider(provider_name)
            .ok_or_else(|| NodeError::Fatal {
                message: format!("LLM provider '{provider_name}' not registered"),
            })?;

        let workers = parse_workers(config)?;

        let system_prompt = build_supervisor_prompt(
            config.get("system_prompt").and_then(|v| v.as_str()),
            &workers,
        );

        // Build user message from inputs
        let user_content = if inputs.is_null() || inputs == json!({}) {
            "No input provided.".to_string()
        } else {
            serde_json::to_string_pretty(&inputs).unwrap_or_else(|_| inputs.to_string())
        };

        let messages = json!([
            {"role": "system", "content": system_prompt},
            {"role": "user", "content": user_content}
        ]);

        let llm_request = LlmRequest {
            provider: provider_name.to_string(),
            model: model.to_string(),
            messages,
            tools: None,
            temperature: Some(0.0),
            top_p: None,
            max_tokens: Some(2048),
            stop_sequences: None,
            response_format: Some(json!({"type": "json_object"})),
            seed: None,
            extra_params: BTreeMap::new(),
        };

        let response = provider.complete(llm_request.clone()).await?;
        ctx.record_llm_call(llm_request, response.clone()).await?;

        // Parse the LLM's JSON decision
        let decision = parse_llm_decision(&response.content)?;

        let done = decision
            .get("done")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if done {
            let final_result = decision.get("final_result").cloned().unwrap_or(Value::Null);
            return Ok(json!({
                "handoff": Value::Null,
                "target_worker": "",
                "done": true,
                "final_result": final_result
            }));
        }

        // Extract delegation target
        let target_worker = decision
            .get("target_worker")
            .and_then(|v| v.as_str())
            .ok_or_else(|| NodeError::Fatal {
                message: "supervisor LLM response missing target_worker".into(),
            })?
            .to_string();

        // Validate the worker exists
        if !workers.contains_key(&target_worker) {
            return Err(NodeError::Fatal {
                message: format!(
                    "supervisor delegated to unknown worker '{target_worker}'. Available: {:?}",
                    workers.keys().collect::<Vec<_>>()
                ),
            });
        }

        let task_value = decision.get("task").cloned().unwrap_or(Value::Null);
        let context_value = decision.get("context").cloned().unwrap_or(json!({}));

        let handoff = AgentHandoff {
            from_agent: "supervisor".into(),
            to_agent: target_worker.clone(),
            task: task_value,
            context: context_value,
            constraints: decision.get("constraints").cloned(),
        };

        // Persist delegation history to blackboard
        let bb = Blackboard::new(ctx.state_store(), ctx.run_id().to_string());
        let history_key = "delegations";
        let mut history: Vec<Value> = bb
            .read("supervisor", history_key)
            .await?
            .and_then(|v| serde_json::from_value(v).ok())
            .unwrap_or_default();
        history.push(json!({
            "target": &target_worker,
            "task": &handoff.task,
        }));
        bb.write("supervisor", history_key, json!(history)).await?;

        ctx.emit(
            "supervisor.delegation",
            json!({"target_worker": &target_worker}),
        );

        Ok(json!({
            "handoff": serde_json::to_value(&handoff).unwrap_or_default(),
            "target_worker": target_worker,
            "done": false,
            "final_result": Value::Null
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
        if config.get("workers").is_none() {
            errors.push("missing required field: workers".into());
        } else if let Some(w) = config.get("workers") {
            if !w.is_object() {
                errors.push("workers must be an object".into());
            } else if w.as_object().is_none_or(|m| m.is_empty()) {
                errors.push("workers must contain at least one entry".into());
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Parse LLM response content into a JSON value.
fn parse_llm_decision(content: &Value) -> Result<Value, NodeError> {
    match content {
        Value::String(s) => serde_json::from_str(s).map_err(|e| NodeError::Fatal {
            message: format!("supervisor LLM returned invalid JSON: {e}"),
        }),
        Value::Object(_) => Ok(content.clone()),
        other => Err(NodeError::Fatal {
            message: format!("supervisor LLM returned unexpected content type: {}", other),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::TestNodeCtx;
    use crate::traits::FlowLlmProvider;
    use async_trait::async_trait;
    use serde_json::json;

    /// Mock provider that returns a predetermined JSON decision.
    struct MockSupervisorProvider {
        response: String,
    }

    impl MockSupervisorProvider {
        fn delegating(worker: &str, task: &str) -> Self {
            Self {
                response: json!({
                    "done": false,
                    "target_worker": worker,
                    "task": task,
                    "context": {}
                })
                .to_string(),
            }
        }

        fn done(result: Value) -> Self {
            Self {
                response: json!({
                    "done": true,
                    "final_result": result
                })
                .to_string(),
            }
        }
    }

    #[async_trait]
    impl FlowLlmProvider for MockSupervisorProvider {
        async fn complete(&self, _request: LlmRequest) -> Result<LlmResponse, NodeError> {
            Ok(LlmResponse {
                content: json!(self.response),
                tool_calls: None,
                model_used: "mock".into(),
                input_tokens: Some(50),
                output_tokens: Some(30),
                total_tokens: Some(80),
                finish_reason: "stop".into(),
                latency_ms: 100,
                provider_request_id: None,
                cost: None,
            })
        }

        fn name(&self) -> &str {
            "mock-supervisor"
        }
    }

    fn supervisor_config() -> Value {
        json!({
            "provider": "mock",
            "model": "gpt-4o",
            "workers": {
                "researcher": { "description": "Searches for information" },
                "writer": { "description": "Writes content" }
            }
        })
    }

    #[tokio::test]
    async fn delegates_to_worker() {
        let node = SupervisorNode;
        let provider = std::sync::Arc::new(MockSupervisorProvider::delegating(
            "researcher",
            "Find Rust async patterns",
        ));
        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let result = node
            .run(
                json!({"task": "Research async"}),
                &supervisor_config(),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["done"], false);
        assert_eq!(result["target_worker"], "researcher");
        assert!(result.get("handoff").is_some());

        // Verify handoff structure
        let handoff: AgentHandoff = serde_json::from_value(result["handoff"].clone()).unwrap();
        assert_eq!(handoff.to_agent, "researcher");

        // Verify LLM call was recorded
        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);
    }

    #[tokio::test]
    async fn returns_done_with_final_result() {
        let node = SupervisorNode;
        let provider = std::sync::Arc::new(MockSupervisorProvider::done(json!({
            "summary": "Task completed successfully"
        })));
        let (ctx, _inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let result = node
            .run(
                json!({"worker_results": "some data"}),
                &supervisor_config(),
                &ctx,
            )
            .await
            .unwrap();

        assert_eq!(result["done"], true);
        assert_eq!(
            result["final_result"]["summary"],
            "Task completed successfully"
        );
    }

    #[tokio::test]
    async fn rejects_unknown_worker() {
        let node = SupervisorNode;
        let provider = std::sync::Arc::new(MockSupervisorProvider::delegating(
            "nonexistent",
            "do something",
        ));
        let (ctx, _inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let err = node
            .run(json!({}), &supervisor_config(), &ctx)
            .await
            .unwrap_err();
        match err {
            NodeError::Fatal { message } => {
                assert!(message.contains("unknown worker"), "got: {message}");
            }
            other => panic!("expected Fatal, got: {other}"),
        }
    }

    #[tokio::test]
    async fn missing_provider_config() {
        let node = SupervisorNode;
        let config = json!({"model": "gpt-4o", "workers": {"w": {"description": "d"}}});
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
    async fn missing_workers_config() {
        let node = SupervisorNode;
        let config = json!({"provider": "mock", "model": "gpt-4o"});
        let provider = std::sync::Arc::new(MockSupervisorProvider::done(json!("x")));
        let (ctx, _inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        let err = node.run(json!({}), &config, &ctx).await.unwrap_err();
        match err {
            NodeError::Validation { message } => {
                assert!(message.contains("workers"), "got: {message}");
            }
            other => panic!("expected Validation, got: {other}"),
        }
    }

    #[tokio::test]
    async fn validate_config_catches_errors() {
        let node = SupervisorNode;

        let errs = node.validate_config(&json!({})).unwrap_err();
        assert!(errs.len() >= 3);

        let ok = node.validate_config(&supervisor_config());
        assert!(ok.is_ok());
    }

    #[tokio::test]
    async fn persists_delegation_history() {
        let node = SupervisorNode;
        let provider = std::sync::Arc::new(MockSupervisorProvider::delegating(
            "researcher",
            "Find data",
        ));
        let (ctx, inspector) = TestNodeCtx::builder()
            .llm_provider("mock", provider)
            .build();

        node.run(json!({"task": "go"}), &supervisor_config(), &ctx)
            .await
            .unwrap();

        let state = inspector.state_snapshot().await;
        let history = state.get("supervisor:delegations").unwrap();
        let arr = history.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["target"], "researcher");
    }
}

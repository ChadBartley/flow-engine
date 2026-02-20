//! Graph executor — the core of the FlowEngine.
//!
//! Runs flows as tokio task DAGs with parallel execution, edge-condition
//! branching, retry, timeout, cancellation, and deadlock detection.
//!
//! Every execution produces a self-contained forensic record in the
//! RunStore via the write pipeline (E8). All events carry monotonic
//! sequence numbers for total ordering (E9).

mod edges;
mod fanout;
mod node;
pub(crate) mod run;

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;
use tokio::sync::{broadcast, oneshot};

use super::node_ctx::HumanInputRegistry;
use super::traits::{
    FlowLlmProvider, NodeHandler, ObservabilityProvider, QueueProvider, Redactor, RunStore,
    SecretsProvider, StateStore,
};
use super::triggers::RunCompletedEvent;
use super::types::*;
use super::versioning::compute_config_hash;
use super::write_event::WriteEvent;
use super::write_handle::WriteHandleError;
use super::write_pipeline::WritePipelineConfig;
use crate::runtime::ExecutionSession;

use run::RunContext;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Errors from the executor.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExecutorError {
    /// A node type referenced in the graph has no handler registered.
    #[error("node type not found in registry: {node_type}")]
    NodeNotFound { node_type: String },
    /// General execution error.
    #[error("execution error: {message}")]
    Execution { message: String },
    /// Write handle channel closed.
    #[error("write handle error: {0}")]
    WriteHandle(#[from] WriteHandleError),
}

/// Configuration for the executor.
pub struct ExecutorConfig {
    /// Default max edge traversals for cycle detection. Default: 50.
    pub max_traversals: u32,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self { max_traversals: 50 }
    }
}

/// Handle to a running execution.
pub struct ExecutionHandle {
    /// Unique identifier for this run.
    pub run_id: String,
    /// Broadcast receiver for live execution events.
    pub events: broadcast::Receiver<WriteEvent>,
    /// Send to cancel the run.
    pub cancel: oneshot::Sender<()>,
}

impl std::fmt::Debug for ExecutionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionHandle")
            .field("run_id", &self.run_id)
            .finish_non_exhaustive()
    }
}

/// The graph executor. Runs flows as tokio task DAGs.
pub(crate) struct Executor {
    config: ExecutorConfig,
    node_registry: HashMap<String, Arc<dyn NodeHandler>>,
    secrets: Arc<dyn SecretsProvider>,
    state: Arc<dyn StateStore>,
    queue: Arc<dyn QueueProvider>,
    observability: Arc<dyn ObservabilityProvider>,
    run_store: Arc<dyn RunStore>,
    redactor: Arc<dyn Redactor>,
    pipeline_config: WritePipelineConfig,
    http_client: reqwest::Client,
    human_in_loop_senders: HumanInputRegistry,
    llm_providers: Arc<HashMap<String, Arc<dyn FlowLlmProvider>>>,
    #[cfg(feature = "mcp")]
    mcp_registry: Arc<crate::mcp::McpServerRegistry>,
    /// Active run event senders for WebSocket subscription.
    /// Key: run_id, Value: broadcast sender for WriteEvent.
    run_event_senders: Arc<tokio::sync::RwLock<HashMap<String, broadcast::Sender<WriteEvent>>>>,
    /// Broadcast sender for run completion events (trigger system).
    run_completed_tx: broadcast::Sender<RunCompletedEvent>,
}

impl Executor {
    /// Create a new executor with the given configuration and providers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: ExecutorConfig,
        node_registry: HashMap<String, Arc<dyn NodeHandler>>,
        secrets: Arc<dyn SecretsProvider>,
        state: Arc<dyn StateStore>,
        queue: Arc<dyn QueueProvider>,
        observability: Arc<dyn ObservabilityProvider>,
        run_store: Arc<dyn RunStore>,
        redactor: Arc<dyn Redactor>,
        pipeline_config: WritePipelineConfig,
        llm_providers: Arc<HashMap<String, Arc<dyn FlowLlmProvider>>>,
        run_completed_tx: broadcast::Sender<RunCompletedEvent>,
        #[cfg(feature = "mcp")] mcp_registry: Arc<crate::mcp::McpServerRegistry>,
    ) -> Self {
        Self {
            config,
            node_registry,
            secrets,
            state,
            queue,
            observability,
            run_store,
            redactor,
            pipeline_config,
            http_client: reqwest::Client::new(),
            human_in_loop_senders: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            llm_providers,
            #[cfg(feature = "mcp")]
            mcp_registry,
            run_event_senders: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
            run_completed_tx,
        }
    }

    /// Execute a flow graph. Returns immediately with an [`ExecutionHandle`].
    ///
    /// The actual execution runs in a spawned tokio task. Use the handle's
    /// `events` receiver for live streaming and `cancel` sender for
    /// cancellation.
    pub async fn execute(
        &self,
        graph: &GraphDef,
        version_id: &str,
        inputs: Value,
        trigger_source: TriggerSource,
        parent_run_id: Option<&str>,
    ) -> Result<ExecutionHandle, ExecutorError> {
        // Validate all node types exist in the registry.
        for node in &graph.nodes {
            if !self.node_registry.contains_key(&node.node_type) {
                return Err(ExecutorError::NodeNotFound {
                    node_type: node.node_type.clone(),
                });
            }
        }

        let config_hash = compute_config_hash(graph);

        // Build a per-execution session.
        let mut builder = ExecutionSession::builder()
            .session_name(&graph.id)
            .run_store(Arc::clone(&self.run_store))
            .redactor(Arc::clone(&self.redactor))
            .pipeline_config(self.pipeline_config.clone())
            .version_id(version_id)
            .config_hash(config_hash.clone())
            .triggered_by(trigger_source.clone())
            .trigger_inputs(inputs.clone());
        if let Some(pid) = parent_run_id {
            builder = builder.parent_run_id(pid);
        }
        let (session, session_handle): (ExecutionSession, _) =
            builder
                .start()
                .await
                .map_err(|e| ExecutorError::Execution {
                    message: format!("session start failed: {e}"),
                })?;

        let run_id = session.run_id();
        let (cancel_tx, cancel_rx) = oneshot::channel::<()>();

        // Store the session's event sender so late subscribers (WebSocket) can subscribe.
        {
            let mut senders = self.run_event_senders.write().await;
            senders.insert(run_id.clone(), session.event_sender().clone());
        }

        let handle = ExecutionHandle {
            run_id: run_id.clone(),
            events: session_handle.events,
            cancel: cancel_tx,
        };

        // Clone everything needed by the spawned task.
        let graph = graph.clone();
        let node_registry = self.node_registry.clone();
        let secrets = Arc::clone(&self.secrets);
        let state = Arc::clone(&self.state);
        let queue = Arc::clone(&self.queue);
        let observability = Arc::clone(&self.observability);
        let llm_providers = Arc::clone(&self.llm_providers);
        #[cfg(feature = "mcp")]
        let mcp_registry = Arc::clone(&self.mcp_registry);
        let http_client = self.http_client.clone();
        let max_traversals = self.config.max_traversals;
        let human_input_registry = Arc::clone(&self.human_in_loop_senders);
        let run_event_senders = Arc::clone(&self.run_event_senders);
        let cleanup_run_id = run_id.clone();
        let run_completed_tx = self.run_completed_tx.clone();
        let flow_id_for_completion = graph.id.clone();

        tokio::spawn(async move {
            let ctx = RunContext {
                run_id,
                graph,
                inputs,
                session,
                cancel_rx,
                node_registry,
                secrets,
                state,
                queue,
                observability,
                llm_providers,
                http_client,
                max_traversals,
                human_input_registry,
                #[cfg(feature = "mcp")]
                mcp_registry,
            };
            let (final_run_id, final_status) = run::execute_run(ctx).await;

            // Broadcast run completion for trigger system.
            let _ = run_completed_tx.send(RunCompletedEvent {
                run_id: final_run_id,
                flow_id: flow_id_for_completion,
                status: final_status,
                outputs: serde_json::json!({}),
            });

            // Clean up the sender to prevent memory leaks.
            let mut senders = run_event_senders.write().await;
            senders.remove(&cleanup_run_id);
        });

        Ok(handle)
    }

    /// Subscribe to live events for a running execution.
    ///
    /// Returns `None` if the run_id is not currently executing. Late
    /// subscribers may miss early events.
    pub async fn subscribe_run(&self, run_id: &str) -> Option<broadcast::Receiver<WriteEvent>> {
        let senders = self.run_event_senders.read().await;
        senders.get(run_id).map(|tx| tx.subscribe())
    }

    /// Deliver a value to a node that's waiting for human input.
    ///
    /// The node must have previously registered a oneshot sender via
    /// [`NodeCtx::register_human_input`]. This removes the sender from the
    /// registry and sends the value.
    pub fn provide_input(
        &self,
        run_id: &str,
        node_id: &str,
        value: Value,
    ) -> Result<(), ExecutorError> {
        let key = (run_id.to_string(), node_id.to_string());
        let mut guard =
            self.human_in_loop_senders
                .try_lock()
                .map_err(|_| ExecutorError::Execution {
                    message: "human input registry lock contention".into(),
                })?;
        let sender = guard.remove(&key).ok_or_else(|| ExecutorError::Execution {
            message: format!("no human input sender registered for run={run_id} node={node_id}"),
        })?;
        sender.send(value).map_err(|_| ExecutorError::Execution {
            message: "human input receiver dropped".into(),
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::node_ctx::NodeCtx;
    use crate::traits::{NodeHandler, NoopObservability};
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Duration;

    // -- Mock node handlers -------------------------------------------------

    /// Returns inputs as outputs.
    struct PassthroughNode;

    #[async_trait]
    impl NodeHandler for PassthroughNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "passthrough".into(),
                label: "Passthrough".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            Ok(inputs)
        }
    }

    /// Always fails with configurable retryable flag.
    struct FailingNode {
        retryable: bool,
    }

    #[async_trait]
    impl NodeHandler for FailingNode {
        fn meta(&self) -> NodeMeta {
            let mut hints = ExecutionHints::default();
            if self.retryable {
                hints.retry = RetryPolicy {
                    max_attempts: 3,
                    backoff_ms: 10,
                    backoff_multiplier: 1.0,
                    retryable_errors: None,
                };
            }
            NodeMeta {
                node_type: "failing".into(),
                label: "Failing".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: hints,
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            if self.retryable {
                Err(NodeError::Retryable {
                    message: "temporary failure".into(),
                })
            } else {
                Err(NodeError::Fatal {
                    message: "permanent failure".into(),
                })
            }
        }
    }

    /// Sleeps for a configurable duration, then returns.
    struct SlowNode {
        delay_ms: u64,
    }

    #[async_trait]
    impl NodeHandler for SlowNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "slow".into(),
                label: "Slow".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints {
                    timeout_ms: 50, // Short timeout for tests.
                    ..Default::default()
                },
            }
        }

        async fn run(
            &self,
            inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
            Ok(inputs)
        }
    }

    /// Calls ctx.record_llm_call() then returns.
    struct MockLlmNode;

    #[async_trait]
    impl NodeHandler for MockLlmNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "mock_llm".into(),
                label: "Mock LLM".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            let req = LlmRequest {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                messages: json!([{"role": "user", "content": "hello"}]),
                tools: None,
                temperature: Some(0.7),
                top_p: None,
                max_tokens: Some(4096),
                stop_sequences: None,
                response_format: None,
                seed: None,
                extra_params: BTreeMap::new(),
            };
            let resp = LlmResponse {
                content: json!("Hello!"),
                tool_calls: None,
                model_used: "gpt-4o".into(),
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                finish_reason: "stop".into(),
                latency_ms: 100,
                provider_request_id: None,
                cost: Some(LlmCost {
                    input_cost_usd: 0.001,
                    output_cost_usd: 0.0005,
                    total_cost_usd: 0.0015,
                    pricing_source: "config".into(),
                }),
            };
            ctx.record_llm_call(req, resp).await?;
            Ok(json!({"finish_reason": "stop", "content": "Hello!"}))
        }
    }

    /// Returns a configured output value.
    struct BranchingNode {
        output: Value,
    }

    #[async_trait]
    impl NodeHandler for BranchingNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "branching".into(),
                label: "Branching".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            Ok(self.output.clone())
        }
    }

    /// Retryable node that fails N times then succeeds.
    struct EventuallySucceedsNode {
        fail_count: std::sync::atomic::AtomicU32,
        succeed_after: u32,
    }

    #[async_trait]
    impl NodeHandler for EventuallySucceedsNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "eventually_succeeds".into(),
                label: "Eventually Succeeds".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints {
                    retry: RetryPolicy {
                        max_attempts: 3,
                        backoff_ms: 10,
                        backoff_multiplier: 1.0,
                        retryable_errors: None,
                    },
                    ..Default::default()
                },
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            let count = self
                .fail_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if count < self.succeed_after {
                Err(NodeError::Retryable {
                    message: "not yet".into(),
                })
            } else {
                Ok(json!({"status": "success"}))
            }
        }
    }

    // -- Test helpers -------------------------------------------------------

    fn make_executor(
        registry: HashMap<String, Arc<dyn NodeHandler>>,
    ) -> (Executor, Arc<crate::defaults::InMemoryRunStore>) {
        let run_store = Arc::new(crate::defaults::InMemoryRunStore::new());
        let (run_completed_tx, _) = broadcast::channel::<RunCompletedEvent>(16);
        let executor = Executor::new(
            ExecutorConfig::default(),
            registry,
            Arc::new(crate::defaults::EnvSecretsProvider),
            Arc::new(crate::defaults::InMemoryState::new()),
            Arc::new(crate::defaults::InMemoryQueue::new()),
            Arc::new(NoopObservability),
            run_store.clone(),
            Arc::new(crate::defaults::DefaultRedactor),
            WritePipelineConfig::default(),
            Arc::new(HashMap::new()),
            run_completed_tx,
            #[cfg(feature = "mcp")]
            Arc::new(crate::mcp::McpServerRegistry::new()),
        );
        (executor, run_store)
    }

    fn make_graph_linear(nodes: Vec<(&str, &str)>) -> GraphDef {
        let node_instances: Vec<NodeInstance> = nodes
            .iter()
            .map(|(id, node_type)| NodeInstance {
                instance_id: id.to_string(),
                node_type: node_type.to_string(),
                config: json!({}),
                position: None,
            })
            .collect();

        let edges: Vec<Edge> = nodes
            .windows(2)
            .enumerate()
            .map(|(i, pair)| Edge {
                id: format!("e{i}"),
                from_node: pair[0].0.to_string(),
                from_port: "output".into(),
                to_node: pair[1].0.to_string(),
                to_port: "input".into(),
                condition: None,
            })
            .collect();

        GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "test-flow".into(),
            name: "Test Flow".into(),
            version: "1".into(),
            nodes: node_instances,
            edges,
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        }
    }

    /// Drain all events from the broadcast event receiver.
    async fn drain_events(rx: &mut broadcast::Receiver<WriteEvent>) -> Vec<WriteEvent> {
        // Give executor tasks time to complete.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        events
    }

    fn events_of_type(
        events: &[WriteEvent],
        predicate: fn(&WriteEvent) -> bool,
    ) -> Vec<&WriteEvent> {
        events.iter().filter(|e| predicate(e)).collect()
    }

    fn is_run_started(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::RunStarted { .. })
    }
    fn is_run_completed(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::RunCompleted { .. })
    }
    fn is_node_started(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::NodeStarted { .. })
    }
    fn is_node_completed(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::NodeCompleted { .. })
    }
    fn is_node_failed(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::NodeFailed { .. })
    }
    fn is_edge_evaluated(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::EdgeEvaluated { .. })
    }
    fn is_llm_invocation(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::LlmInvocation { .. })
    }
    fn is_checkpoint(e: &WriteEvent) -> bool {
        matches!(e, WriteEvent::Checkpoint { .. })
    }

    // -- Tests --------------------------------------------------------------

    #[tokio::test]
    async fn test_linear_execution() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![
            ("A", "passthrough"),
            ("B", "passthrough"),
            ("C", "passthrough"),
        ]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({"data": "hello"}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        assert!(!handle.run_id.is_empty());

        let events = drain_events(&mut handle.events).await;

        // Verify RunStarted.
        assert_eq!(events_of_type(&events, is_run_started).len(), 1);

        // Verify all 3 nodes started and completed.
        assert_eq!(events_of_type(&events, is_node_started).len(), 3);
        assert_eq!(events_of_type(&events, is_node_completed).len(), 3);

        // Verify checkpoints.
        assert_eq!(events_of_type(&events, is_checkpoint).len(), 3);

        // Verify RunCompleted with success.
        let completed = events_of_type(&events, is_run_completed);
        assert_eq!(completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }

        // Verify edge evaluations.
        assert_eq!(events_of_type(&events, is_edge_evaluated).len(), 2);
    }

    #[tokio::test]
    async fn test_branching() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "branching".into(),
            Arc::new(BranchingNode {
                output: json!({"finish_reason": "tool_calls"}),
            }),
        );
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);

        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "branch-flow".into(),
            name: "Branch Flow".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "A".into(),
                    node_type: "branching".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "B".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "C".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                Edge {
                    id: "e1".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "B".into(),
                    to_port: "input".into(),
                    condition: Some(EdgeCondition {
                        expression: r#"finish_reason == "tool_calls""#.into(),
                        label: None,
                    }),
                },
                Edge {
                    id: "e2".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "C".into(),
                    to_port: "input".into(),
                    condition: Some(EdgeCondition {
                        expression: r#"finish_reason == "stop""#.into(),
                        label: None,
                    }),
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // A should complete.
        let node_completed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeCompleted { node_id, .. } if node_id == "A"))
            .collect();
        assert_eq!(node_completed.len(), 1);

        // B should run (condition "tool_calls" matches).
        let b_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "B"))
            .collect();
        assert_eq!(b_started.len(), 1);

        // C should NOT run (condition "stop" doesn't match).
        let c_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "C"))
            .collect();
        assert_eq!(c_started.len(), 0);
    }

    #[tokio::test]
    async fn test_parallel_diamond() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);

        // Diamond: A -> B, A -> C, B -> D, C -> D
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "diamond-flow".into(),
            name: "Diamond".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "A".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "B".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "C".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "D".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                Edge {
                    id: "e1".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "B".into(),
                    to_port: "b_in".into(),
                    condition: None,
                },
                Edge {
                    id: "e2".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "C".into(),
                    to_port: "c_in".into(),
                    condition: None,
                },
                Edge {
                    id: "e3".into(),
                    from_node: "B".into(),
                    from_port: "output".into(),
                    to_node: "D".into(),
                    to_port: "b_out".into(),
                    condition: None,
                },
                Edge {
                    id: "e4".into(),
                    from_node: "C".into(),
                    from_port: "output".into(),
                    to_node: "D".into(),
                    to_port: "c_out".into(),
                    condition: None,
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({"data": 1}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        // Wait for execution via broadcast, then read from store for seq ordering.
        let _ = drain_events(&mut handle.events).await;

        use crate::traits::RunStore;
        let events = _run_store.events(&handle.run_id).await.unwrap();

        // All 4 nodes should complete.
        assert_eq!(events_of_type(&events, is_node_completed).len(), 4);

        // D should run after both B and C.
        let d_started_seq = events
            .iter()
            .find_map(|e| {
                if let WriteEvent::NodeStarted { node_id, seq, .. } = e {
                    if node_id == "D" {
                        return Some(*seq);
                    }
                }
                None
            })
            .expect("D should have started");

        let b_completed_seq = events
            .iter()
            .find_map(|e| {
                if let WriteEvent::NodeCompleted { node_id, seq, .. } = e {
                    if node_id == "B" {
                        return Some(*seq);
                    }
                }
                None
            })
            .expect("B should have completed");

        let c_completed_seq = events
            .iter()
            .find_map(|e| {
                if let WriteEvent::NodeCompleted { node_id, seq, .. } = e {
                    if node_id == "C" {
                        return Some(*seq);
                    }
                }
                None
            })
            .expect("C should have completed");

        assert!(
            d_started_seq > b_completed_seq,
            "D must start after B completes"
        );
        assert!(
            d_started_seq > c_completed_seq,
            "D must start after C completes"
        );
    }

    #[tokio::test]
    async fn test_timeout() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("slow".into(), Arc::new(SlowNode { delay_ms: 200 }));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "slow")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        let failed = events_of_type(&events, is_node_failed);
        assert_eq!(failed.len(), 1);
        if let WriteEvent::NodeFailed { error, .. } = failed[0] {
            assert!(error.contains("timeout"), "error: {error}");
        }
    }

    #[tokio::test]
    async fn test_retry() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "eventually_succeeds".into(),
            Arc::new(EventuallySucceedsNode {
                fail_count: std::sync::atomic::AtomicU32::new(0),
                succeed_after: 1, // Fail once, succeed on second attempt.
            }),
        );

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "eventually_succeeds")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // Should have 2 NodeStarted (attempt 1 and 2).
        let started = events_of_type(&events, is_node_started);
        assert_eq!(started.len(), 2);

        // First attempt fails with will_retry=true.
        let failed = events_of_type(&events, is_node_failed);
        assert_eq!(failed.len(), 1);
        if let WriteEvent::NodeFailed {
            will_retry,
            attempt,
            ..
        } = failed[0]
        {
            assert!(*will_retry);
            assert_eq!(*attempt, 1);
        }

        // Second attempt succeeds.
        let completed = events_of_type(&events, is_node_completed);
        assert_eq!(completed.len(), 1);

        // Run completes successfully.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("failing".into(), Arc::new(FailingNode { retryable: true }));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "failing")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // Should have 3 NodeStarted (max_attempts=3).
        let started = events_of_type(&events, is_node_started);
        assert_eq!(started.len(), 3);

        // Should have 3 NodeFailed.
        let failed = events_of_type(&events, is_node_failed);
        assert_eq!(failed.len(), 3);

        // Last attempt has will_retry=false.
        if let WriteEvent::NodeFailed {
            will_retry,
            attempt,
            ..
        } = failed[2]
        {
            assert!(!will_retry);
            assert_eq!(*attempt, 3);
        }

        // Run fails.
        let run_completed = events_of_type(&events, is_run_completed);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Failed);
        }
    }

    #[tokio::test]
    async fn test_cancellation() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        // Use a slow node so we have time to cancel.
        registry.insert("slow".into(), Arc::new(SlowNode { delay_ms: 5 }));
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);

        // Build a graph with multiple nodes in sequence.
        let graph = make_graph_linear(vec![
            ("A", "passthrough"),
            ("B", "slow"),
            ("C", "passthrough"),
        ]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        // Cancel immediately.
        let _ = handle.cancel.send(());

        let events = drain_events(&mut handle.events).await;

        // Run should complete with Cancelled status.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Cancelled);
        }
    }

    #[tokio::test]
    async fn test_deadlock_detection() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);

        // Graph where B and C depend on each other (unsatisfiable).
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "deadlock-flow".into(),
            name: "Deadlock".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "A".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "B".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "C".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                Edge {
                    id: "e1".into(),
                    from_node: "B".into(),
                    from_port: "output".into(),
                    to_node: "C".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                Edge {
                    id: "e2".into(),
                    from_node: "C".into(),
                    from_port: "output".into(),
                    to_node: "B".into(),
                    to_port: "input".into(),
                    condition: None,
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // A runs (entry node with no incoming edges).
        // B and C can never become ready (circular dependency).
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Failed);
        }
    }

    #[tokio::test]
    async fn test_llm_node_recording() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("mock_llm".into(), Arc::new(MockLlmNode));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "mock_llm")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        let llm_events = events_of_type(&events, is_llm_invocation);
        assert_eq!(llm_events.len(), 1);

        if let WriteEvent::LlmInvocation {
            request,
            response,
            node_id,
            ..
        } = llm_events[0]
        {
            assert_eq!(node_id, "A");
            assert_eq!(request.model, "gpt-4o");
            assert_eq!(response.finish_reason, "stop");
            assert_eq!(response.input_tokens, Some(10));
        }
    }

    #[tokio::test]
    async fn test_llm_run_summary() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("mock_llm".into(), Arc::new(MockLlmNode));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "mock_llm")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);

        if let WriteEvent::RunCompleted { llm_summary, .. } = run_completed[0] {
            let summary = llm_summary.as_ref().expect("should have LLM summary");
            assert_eq!(summary.total_llm_calls, 1);
            assert_eq!(summary.total_input_tokens, 10);
            assert_eq!(summary.total_output_tokens, 5);
            assert!(summary.total_cost_usd.is_some());
            assert!(summary.models_used.contains(&"gpt-4o".to_string()));
        }
    }

    #[tokio::test]
    async fn test_edge_evaluation_events() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "passthrough"), ("B", "passthrough")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({"x": 1}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        let edge_events = events_of_type(&events, is_edge_evaluated);
        assert_eq!(edge_events.len(), 1); // One edge A->B
        if let WriteEvent::EdgeEvaluated {
            result, condition, ..
        } = edge_events[0]
        {
            assert!(*result); // No condition = true
            assert!(condition.is_none());
        }
    }

    #[tokio::test]
    async fn test_partial_failure() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));
        registry.insert("failing".into(), Arc::new(FailingNode { retryable: false }));

        let (executor, _run_store) = make_executor(registry);

        // A -> B (fails), A -> C (succeeds)
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "partial-fail".into(),
            name: "Partial".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "A".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "B".into(),
                    node_type: "failing".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "C".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                Edge {
                    id: "e1".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "B".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                Edge {
                    id: "e2".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "C".into(),
                    to_port: "input".into(),
                    condition: None,
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // B fails, C succeeds.
        let b_failed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeFailed { node_id, .. } if node_id == "B"))
            .collect();
        assert_eq!(b_failed.len(), 1);

        let c_completed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeCompleted { node_id, .. } if node_id == "C"))
            .collect();
        assert_eq!(c_completed.len(), 1);

        // Run fails because B failed.
        let run_completed = events_of_type(&events, is_run_completed);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Failed);
        }
    }

    #[tokio::test]
    async fn test_seq_ordering() {
        use crate::traits::RunStore;

        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![
            ("A", "passthrough"),
            ("B", "passthrough"),
            ("C", "passthrough"),
        ]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        // Wait for execution to complete.
        let _ = drain_events(&mut handle.events).await;

        // Verify seq ordering from the run store (pipeline assigns real seq numbers).
        let events = run_store.events(&handle.run_id).await.unwrap();
        let mut prev_seq = 0u64;
        for event in &events {
            let seq = event.seq();
            assert!(
                seq > prev_seq,
                "seq {seq} must be > prev_seq {prev_seq} (event: {:?})",
                std::mem::discriminant(event)
            );
            prev_seq = seq;
        }
    }

    #[tokio::test]
    async fn test_node_not_found() {
        let registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "nonexistent")]);

        let result = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await;

        assert!(result.is_err());
        match result.unwrap_err() {
            ExecutorError::NodeNotFound { node_type } => assert_eq!(node_type, "nonexistent"),
            other => panic!("expected NodeNotFound, got: {other}"),
        }
    }

    // -- M8 Chunk 1: New mock handlers --------------------------------------

    /// Returns a configured `Value::Array` as output.
    struct ArrayOutputNode {
        items: Vec<Value>,
    }

    #[async_trait]
    impl NodeHandler for ArrayOutputNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "array_output".into(),
                label: "Array Output".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            Ok(Value::Array(self.items.clone()))
        }
    }

    /// Receives a single element and returns it wrapped. Optionally delays
    /// based on input value for testing fan-in ordering.
    struct IndexedNode;

    #[async_trait]
    impl NodeHandler for IndexedNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "indexed".into(),
                label: "Indexed".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            // Delay proportional to (4 - value) to reverse completion order.
            let index = inputs.as_u64().unwrap_or(0);
            let delay = (4u64.saturating_sub(index)) * 10;
            tokio::time::sleep(Duration::from_millis(delay)).await;
            Ok(json!({"result": inputs, "processed": true}))
        }
    }

    /// Counts invocations via AtomicU32 and returns different outputs
    /// depending on whether max_calls has been reached.
    struct CountingNode {
        count: std::sync::atomic::AtomicU32,
        max_calls: u32,
        output_when_continue: Value,
        output_when_done: Value,
    }

    #[async_trait]
    impl NodeHandler for CountingNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "counting".into(),
                label: "Counting".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            let n = self
                .count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                + 1;
            if n < self.max_calls {
                Ok(self.output_when_continue.clone())
            } else {
                Ok(self.output_when_done.clone())
            }
        }
    }

    /// Node that fails on a specific input value. Used for fan-out
    /// partial failure tests.
    struct SelectiveFailNode {
        fail_on_input: Value,
    }

    #[async_trait]
    impl NodeHandler for SelectiveFailNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "selective_fail".into(),
                label: "Selective Fail".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            if inputs == self.fail_on_input {
                Err(NodeError::Fatal {
                    message: format!("failed on input: {}", self.fail_on_input),
                })
            } else {
                Ok(json!({"result": inputs}))
            }
        }
    }

    // -- M8 Chunk 1: Additional test helpers --------------------------------

    fn make_executor_with_config(
        registry: HashMap<String, Arc<dyn NodeHandler>>,
        config: ExecutorConfig,
    ) -> (Executor, Arc<crate::defaults::InMemoryRunStore>) {
        let run_store = Arc::new(crate::defaults::InMemoryRunStore::new());
        let (run_completed_tx, _) = broadcast::channel::<RunCompletedEvent>(16);
        let executor = Executor::new(
            config,
            registry,
            Arc::new(crate::defaults::EnvSecretsProvider),
            Arc::new(crate::defaults::InMemoryState::new()),
            Arc::new(crate::defaults::InMemoryQueue::new()),
            Arc::new(NoopObservability),
            run_store.clone(),
            Arc::new(crate::defaults::DefaultRedactor),
            WritePipelineConfig::default(),
            Arc::new(HashMap::new()),
            run_completed_tx,
            #[cfg(feature = "mcp")]
            Arc::new(crate::mcp::McpServerRegistry::new()),
        );
        (executor, run_store)
    }

    /// Drain with a longer timeout for cycle tests.
    async fn drain_events_long(rx: &mut broadcast::Receiver<WriteEvent>) -> Vec<WriteEvent> {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let mut events = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        events
    }

    // -- M8 Chunk 1: Tests --------------------------------------------------

    #[tokio::test]
    async fn test_fan_out_basic() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "array_output".into(),
            Arc::new(ArrayOutputNode {
                items: vec![json!(1), json!(2), json!(3)],
            }),
        );
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "array_output"), ("B", "passthrough")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // B should have 3 NodeStarted events (one per fan-out instance).
        let b_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "B"))
            .collect();
        assert_eq!(b_started.len(), 3, "expected 3 fan-out starts for B");

        // Verify each has a fan_out_index.
        let mut indices: Vec<u32> = b_started
            .iter()
            .filter_map(|e| {
                if let WriteEvent::NodeStarted {
                    fan_out_index: Some(foi),
                    ..
                } = e
                {
                    Some(*foi)
                } else {
                    None
                }
            })
            .collect();
        indices.sort();
        assert_eq!(indices, vec![0, 1, 2]);

        // B should have 3 NodeCompleted events.
        let b_completed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeCompleted { node_id, .. } if node_id == "B"))
            .collect();
        assert_eq!(b_completed.len(), 3, "expected 3 fan-out completions for B");

        // Run should complete successfully.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_fan_in_ordering() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "array_output".into(),
            Arc::new(ArrayOutputNode {
                items: vec![json!(1), json!(2), json!(3)],
            }),
        );
        registry.insert("indexed".into(), Arc::new(IndexedNode));
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);

        // A → B (fan-out) → C (receives fan-in result)
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "fan-in-flow".into(),
            name: "Fan-In".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "A".into(),
                    node_type: "array_output".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "B".into(),
                    node_type: "indexed".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "C".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                Edge {
                    id: "e1".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "B".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                Edge {
                    id: "e2".into(),
                    from_node: "B".into(),
                    from_port: "output".into(),
                    to_node: "C".into(),
                    to_port: "input".into(),
                    condition: None,
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events_long(&mut handle.events).await;

        // C should receive the fan-in array sorted by index.
        let c_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "C"))
            .collect();
        assert_eq!(c_started.len(), 1, "C should start exactly once");

        // C's input should be a 3-element array, sorted by fan_out_index.
        if let WriteEvent::NodeStarted { inputs, .. } = c_started[0] {
            let arr = inputs.as_array().expect("C input should be an array");
            assert_eq!(arr.len(), 3);
            // Each element should be {"result": N, "processed": true}
            assert_eq!(arr[0]["result"], json!(1));
            assert_eq!(arr[1]["result"], json!(2));
            assert_eq!(arr[2]["result"], json!(3));
        }
    }

    #[tokio::test]
    async fn test_fan_out_partial_failure() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "array_output".into(),
            Arc::new(ArrayOutputNode {
                items: vec![json!(1), json!(2), json!(3)],
            }),
        );
        registry.insert(
            "selective_fail".into(),
            Arc::new(SelectiveFailNode {
                fail_on_input: json!(2),
            }),
        );

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "array_output"), ("B", "selective_fail")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // B should have 3 NodeStarted.
        let b_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "B"))
            .collect();
        assert_eq!(b_started.len(), 3);

        // 2 NodeCompleted for B (indices 0, 2).
        let b_completed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeCompleted { node_id, .. } if node_id == "B"))
            .collect();
        assert_eq!(b_completed.len(), 2);

        // 1 NodeFailed for B (index 1, input=2).
        let b_failed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeFailed { node_id, .. } if node_id == "B"))
            .collect();
        assert_eq!(b_failed.len(), 1);

        // Run completes (fan-in produces array with error marker at index 1).
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_cycle_basic() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "counting".into(),
            Arc::new(CountingNode {
                count: std::sync::atomic::AtomicU32::new(0),
                max_calls: 3,
                output_when_continue: json!({"should_continue": true}),
                output_when_done: json!({"should_continue": false}),
            }),
        );
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);

        // A (counting) → B (passthrough) → C (passthrough) → A (back-edge)
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "cycle-flow".into(),
            name: "Cycle".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "A".into(),
                    node_type: "counting".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "B".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "C".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                Edge {
                    id: "e1".into(),
                    from_node: "A".into(),
                    from_port: "output".into(),
                    to_node: "B".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                Edge {
                    id: "e2".into(),
                    from_node: "B".into(),
                    from_port: "output".into(),
                    to_node: "C".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                Edge {
                    id: "e3".into(),
                    from_node: "C".into(),
                    from_port: "output".into(),
                    to_node: "A".into(),
                    to_port: "input".into(),
                    condition: Some(EdgeCondition {
                        expression: r#"should_continue == true"#.into(),
                        label: None,
                    }),
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events_long(&mut handle.events).await;

        // A should have run 3 times: calls 1,2 produce continue=true,
        // call 3 produces continue=false.
        let a_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "A"))
            .collect();
        assert_eq!(a_started.len(), 3, "A should start 3 times");

        // Run should complete.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_cycle_termination_max_traversals() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        // Use max_traversals=3.
        let (executor, _run_store) =
            make_executor_with_config(registry, ExecutorConfig { max_traversals: 3 });

        // A → A (self-loop, always-true condition = no condition).
        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "cycle-term-flow".into(),
            name: "Cycle Termination".into(),
            version: "1".into(),
            nodes: vec![NodeInstance {
                instance_id: "A".into(),
                node_type: "passthrough".into(),
                config: json!({}),
                position: None,
            }],
            edges: vec![Edge {
                id: "e1".into(),
                from_node: "A".into(),
                from_port: "output".into(),
                to_node: "A".into(),
                to_port: "input".into(),
                condition: None,
            }],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({"value": 1}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events_long(&mut handle.events).await;

        // With max_traversals=3, the self-loop edge is traversed 3 times
        // (initial + 3 re-executions = 4 total runs for A).
        let a_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "A"))
            .collect();
        assert_eq!(
            a_started.len(),
            4,
            "A should start 1 + max_traversals times"
        );

        // Run should complete (not hang).
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_human_in_loop_infrastructure() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));

        let (executor, _run_store) = make_executor(registry);

        // Test 1: provide_input fails when no sender registered.
        let result = executor.provide_input("run-1", "node-1", json!({"answer": 42}));
        assert!(result.is_err(), "should fail when no sender registered");

        // Test 2: Register a sender manually and verify provide_input works.
        let (tx, rx) = tokio::sync::oneshot::channel::<Value>();
        {
            let mut guard = executor.human_in_loop_senders.try_lock().unwrap();
            guard.insert(("run-1".into(), "node-1".into()), tx);
        }

        let result = executor.provide_input("run-1", "node-1", json!({"answer": 42}));
        assert!(result.is_ok(), "should succeed when sender is registered");

        let received = rx.await.unwrap();
        assert_eq!(received, json!({"answer": 42}));

        // Test 3: After sending, sender is removed from registry.
        let result = executor.provide_input("run-1", "node-1", json!("again"));
        assert!(result.is_err(), "should fail after sender consumed");
    }

    // -- M8 Chunk 3: Integration test mock handlers ---------------------------

    /// Stateful LLM node that returns different responses on successive calls.
    /// Used for agent loop integration tests.
    struct StatefulLlmNode {
        responses: std::sync::Mutex<std::collections::VecDeque<Value>>,
    }

    #[async_trait]
    impl NodeHandler for StatefulLlmNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "stateful_llm".into(),
                label: "Stateful LLM".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            let response_value = {
                let mut guard = self.responses.lock().unwrap();
                guard.pop_front().unwrap_or_else(
                    || json!({"content": "default", "tool_calls": [], "finish_reason": "stop"}),
                )
            };

            // Record an LLM call for each invocation.
            let req = LlmRequest {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                messages: json!([{"role": "user", "content": "test"}]),
                tools: None,
                temperature: Some(0.7),
                top_p: None,
                max_tokens: Some(4096),
                stop_sequences: None,
                response_format: None,
                seed: None,
                extra_params: BTreeMap::new(),
            };
            let tool_calls_for_resp = response_value
                .get("tool_calls")
                .and_then(|v| v.as_array())
                .filter(|arr| !arr.is_empty())
                .map(|arr| {
                    arr.iter()
                        .map(|tc| LlmToolCall {
                            id: tc["id"].as_str().unwrap_or("").into(),
                            tool_name: tc["tool_name"].as_str().unwrap_or("").into(),
                            arguments: tc.get("arguments").cloned().unwrap_or(json!({})),
                        })
                        .collect()
                });
            let resp = LlmResponse {
                content: response_value.get("content").cloned().unwrap_or(json!("")),
                tool_calls: tool_calls_for_resp,
                model_used: "gpt-4o".into(),
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                finish_reason: response_value["finish_reason"]
                    .as_str()
                    .unwrap_or("stop")
                    .into(),
                latency_ms: 50,
                provider_request_id: None,
                cost: Some(LlmCost {
                    input_cost_usd: 0.001,
                    output_cost_usd: 0.0005,
                    total_cost_usd: 0.0015,
                    pricing_source: "config".into(),
                }),
            };
            ctx.record_llm_call(req, resp).await?;

            Ok(response_value)
        }
    }

    /// Simulates executing a tool by name — returns a result based on
    /// the tool_name in the input.
    struct ToolExecutorNode;

    #[async_trait]
    impl NodeHandler for ToolExecutorNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "tool_executor".into(),
                label: "Tool Executor".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            let tool_name = inputs
                .get("tool_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            Ok(json!({"result": format!("result for {tool_name}")}))
        }
    }

    // -- M8 Chunk 3: Integration tests ----------------------------------------

    #[tokio::test]
    async fn test_agent_loop_fan_out_and_cycle() {
        // Test 1: Agent loop pattern — LLM → ToolRouter → fan-out → tools →
        // fan-in → ConditionalNode → back-edge to LLM → repeat until done.
        use crate::nodes::{ConditionalNode, ToolRouterNode};

        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "stateful_llm".into(),
            Arc::new(StatefulLlmNode {
                responses: std::sync::Mutex::new(std::collections::VecDeque::from([
                    // First call: returns one tool call.
                    json!({
                        "content": "",
                        "tool_calls": [
                            {"id": "1", "tool_name": "search", "arguments": {"q": "test"}}
                        ],
                        "finish_reason": "tool_calls"
                    }),
                    // Second call: done.
                    json!({
                        "content": "Final answer",
                        "tool_calls": [],
                        "finish_reason": "stop"
                    }),
                ])),
            }),
        );
        registry.insert("tool_router".into(), Arc::new(ToolRouterNode));
        registry.insert("tool_executor".into(), Arc::new(ToolExecutorNode));
        registry.insert("conditional".into(), Arc::new(ConditionalNode));

        let (executor, _run_store) = make_executor(registry);

        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "agent-loop".into(),
            name: "Agent Loop".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "llm".into(),
                    node_type: "stateful_llm".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "router".into(),
                    node_type: "tool_router".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "tool".into(),
                    node_type: "tool_executor".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "check".into(),
                    node_type: "conditional".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                // llm → check (unconditional — check sees LLM output with finish_reason)
                Edge {
                    id: "e1".into(),
                    from_node: "llm".into(),
                    from_port: "output".into(),
                    to_node: "check".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                // check → router (condition: continue only if LLM wants tool calls)
                Edge {
                    id: "e2".into(),
                    from_node: "check".into(),
                    from_port: "output".into(),
                    to_node: "router".into(),
                    to_port: "input".into(),
                    condition: Some(EdgeCondition {
                        expression: r#"finish_reason == "tool_calls""#.into(),
                        label: Some("continue loop".into()),
                    }),
                },
                // router → tool (unconditional, fan-out on array output)
                Edge {
                    id: "e3".into(),
                    from_node: "router".into(),
                    from_port: "output".into(),
                    to_node: "tool".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                // tool → llm (back-edge, unconditional)
                Edge {
                    id: "e4".into(),
                    from_node: "tool".into(),
                    from_port: "output".into(),
                    to_node: "llm".into(),
                    to_port: "input".into(),
                    condition: None,
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({"messages": [{"role": "user", "content": "Search for Rust tutorials"}]}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events_long(&mut handle.events).await;

        // 1. Execution completes successfully.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed, "run should complete");
        }

        // 2. LLM node executes exactly 2 times (iteration 1 + cycle).
        let llm_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "llm"))
            .collect();
        assert_eq!(llm_started.len(), 2, "llm should start 2 times");

        // 3. Check executes 2 times (once per LLM output).
        let check_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "check"))
            .collect();
        assert_eq!(check_started.len(), 2, "check should start 2 times");

        // 4. Router executes 1 time (only when finish_reason == "tool_calls").
        // In iteration 2, finish_reason == "stop" so check → router edge is false.
        let router_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "router"))
            .collect();
        assert_eq!(router_started.len(), 1, "router should start 1 time");

        // 5. Tool executes 1 time (1 tool call in iteration 1).
        let tool_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "tool"))
            .collect();
        assert_eq!(tool_started.len(), 1, "tool should start 1 time");

        // 6. LLM invocation events recorded for each LLM call.
        let llm_invocations = events_of_type(&events, is_llm_invocation);
        assert_eq!(llm_invocations.len(), 2, "should record 2 LLM invocations");

        // 7. LlmRunSummary in RunCompleted.
        if let WriteEvent::RunCompleted { llm_summary, .. } = run_completed[0] {
            let summary = llm_summary.as_ref().expect("should have LLM summary");
            assert_eq!(summary.total_llm_calls, 2);
            assert!(summary.models_used.contains(&"gpt-4o".to_string()));
        }
    }

    #[tokio::test]
    async fn test_agent_loop_multi_tool_fan_out() {
        // Test 2: Same topology as Test 1, but first LLM call returns 3 tool calls.
        use crate::nodes::{ConditionalNode, ToolRouterNode};

        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert(
            "stateful_llm".into(),
            Arc::new(StatefulLlmNode {
                responses: std::sync::Mutex::new(std::collections::VecDeque::from([
                    // First call: 3 tool calls → fan-out.
                    json!({
                        "content": "",
                        "tool_calls": [
                            {"id": "1", "tool_name": "search", "arguments": {}},
                            {"id": "2", "tool_name": "calculate", "arguments": {}},
                            {"id": "3", "tool_name": "lookup", "arguments": {}}
                        ],
                        "finish_reason": "tool_calls"
                    }),
                    // Second call: done.
                    json!({
                        "content": "All results processed",
                        "tool_calls": [],
                        "finish_reason": "stop"
                    }),
                ])),
            }),
        );
        registry.insert("tool_router".into(), Arc::new(ToolRouterNode));
        registry.insert("tool_executor".into(), Arc::new(ToolExecutorNode));
        registry.insert("conditional".into(), Arc::new(ConditionalNode));

        let (executor, _run_store) = make_executor(registry);

        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "agent-multi-tool".into(),
            name: "Agent Multi Tool".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "llm".into(),
                    node_type: "stateful_llm".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "router".into(),
                    node_type: "tool_router".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "tool".into(),
                    node_type: "tool_executor".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "check".into(),
                    node_type: "conditional".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                // llm → check (unconditional)
                Edge {
                    id: "e1".into(),
                    from_node: "llm".into(),
                    from_port: "output".into(),
                    to_node: "check".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                // check → router (condition: continue loop)
                Edge {
                    id: "e2".into(),
                    from_node: "check".into(),
                    from_port: "output".into(),
                    to_node: "router".into(),
                    to_port: "input".into(),
                    condition: Some(EdgeCondition {
                        expression: r#"finish_reason == "tool_calls""#.into(),
                        label: None,
                    }),
                },
                // router → tool (unconditional, fan-out on array output)
                Edge {
                    id: "e3".into(),
                    from_node: "router".into(),
                    from_port: "output".into(),
                    to_node: "tool".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                // tool → llm (back-edge, unconditional)
                Edge {
                    id: "e4".into(),
                    from_node: "tool".into(),
                    from_port: "output".into(),
                    to_node: "llm".into(),
                    to_port: "input".into(),
                    condition: None,
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events_long(&mut handle.events).await;

        // 1. Tool node executes 3 times (fan-out), each with fan_out_index.
        let tool_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "tool"))
            .collect();
        assert_eq!(tool_started.len(), 3, "tool should start 3 times (fan-out)");

        // Verify fan_out_index values: 0, 1, 2.
        let mut tool_indices: Vec<u32> = tool_started
            .iter()
            .filter_map(|e| {
                if let WriteEvent::NodeStarted {
                    fan_out_index: Some(foi),
                    ..
                } = e
                {
                    Some(*foi)
                } else {
                    None
                }
            })
            .collect();
        tool_indices.sort();
        assert_eq!(tool_indices, vec![0, 1, 2]);

        // 2. All 3 tool completions recorded.
        let tool_completed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeCompleted { node_id, .. } if node_id == "tool"))
            .collect();
        assert_eq!(tool_completed.len(), 3);

        // 3. LLM's second invocation receives the fan-in array from tools.
        let llm_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "llm"))
            .collect();
        assert_eq!(llm_started.len(), 2, "llm should start 2 times");
        if let WriteEvent::NodeStarted { inputs, .. } = llm_started[1] {
            let arr = inputs
                .as_array()
                .expect("llm cycle input should be fan-in array");
            assert_eq!(arr.len(), 3, "fan-in should collect 3 tool results");
        }

        // 4. Router executes 1 time (only in iteration 1).
        let router_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "router"))
            .collect();
        assert_eq!(router_started.len(), 1, "router should start 1 time");

        // 5. Second LLM call returns stop → execution completes.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_human_in_loop_executor_integration() {
        // Test 3: Full flow — start → human_in_loop → end.
        // Human input is provided from a separate task after execution starts.
        use crate::nodes::HumanInLoopNode;

        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));
        registry.insert("human_in_loop".into(), Arc::new(HumanInLoopNode));

        let (executor, _run_store) = make_executor(registry);
        let executor = Arc::new(executor);

        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "human-flow".into(),
            name: "Human Flow".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "start".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "human".into(),
                    node_type: "human_in_loop".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "end".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![
                Edge {
                    id: "e1".into(),
                    from_node: "start".into(),
                    from_port: "output".into(),
                    to_node: "human".into(),
                    to_port: "input".into(),
                    condition: None,
                },
                Edge {
                    id: "e2".into(),
                    from_node: "human".into(),
                    from_port: "output".into(),
                    to_node: "end".into(),
                    to_port: "input".into(),
                    condition: None,
                },
            ],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({"prompt": "approve?"}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let run_id = handle.run_id.clone();
        let executor_clone = Arc::clone(&executor);

        // Spawn a task that waits briefly, then provides human input.
        tokio::spawn(async move {
            // Wait for the human node to register its sender.
            tokio::time::sleep(Duration::from_millis(200)).await;
            let result = executor_clone.provide_input(&run_id, "human", json!({"approved": true}));
            assert!(result.is_ok(), "provide_input should succeed");
        });

        // Wait for execution to complete.
        let events = drain_events_long(&mut handle.events).await;

        // Verify: start completed.
        let start_completed: Vec<&WriteEvent> = events
            .iter()
            .filter(
                |e| matches!(e, WriteEvent::NodeCompleted { node_id, .. } if node_id == "start"),
            )
            .collect();
        assert_eq!(start_completed.len(), 1);

        // Verify: human completed after input.
        let human_completed: Vec<&WriteEvent> = events
            .iter()
            .filter(
                |e| matches!(e, WriteEvent::NodeCompleted { node_id, .. } if node_id == "human"),
            )
            .collect();
        assert_eq!(human_completed.len(), 1, "human should complete");

        // Verify: end received the human output.
        let end_started: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeStarted { node_id, .. } if node_id == "end"))
            .collect();
        assert_eq!(end_started.len(), 1, "end should start");
        if let WriteEvent::NodeStarted { inputs, .. } = end_started[0] {
            assert_eq!(
                *inputs,
                json!({"approved": true}),
                "end should receive human output"
            );
        }

        // Verify: run completes successfully.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }
    }

    #[tokio::test]
    async fn test_human_in_loop_timeout() {
        // Test 4: HumanInLoopNode with a short timeout — no input provided.
        // The executor's timeout machinery should kill the waiting node.
        // Since HumanInLoopNode uses the default ExecutionHints (30s timeout),
        // we need a custom wrapper with a short timeout.
        struct ShortTimeoutHumanNode;

        #[async_trait]
        impl NodeHandler for ShortTimeoutHumanNode {
            fn meta(&self) -> NodeMeta {
                NodeMeta {
                    node_type: "short_timeout_human".into(),
                    label: "Short Timeout Human".into(),
                    category: "test".into(),
                    inputs: vec![],
                    outputs: vec![],
                    config_schema: json!({}),
                    ui: NodeUiHints::default(),
                    execution: ExecutionHints {
                        timeout_ms: 100,
                        ..Default::default()
                    },
                }
            }

            async fn run(
                &self,
                _inputs: Value,
                config: &Value,
                ctx: &NodeCtx,
            ) -> Result<Value, NodeError> {
                let (tx, rx) = tokio::sync::oneshot::channel();
                ctx.register_human_input(tx).await?;
                ctx.emit("human_input_requested", config.clone());
                match rx.await {
                    Ok(value) => Ok(value),
                    Err(_) => Err(NodeError::Fatal {
                        message: "Human input canceled".into(),
                    }),
                }
            }
        }

        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("passthrough".into(), Arc::new(PassthroughNode));
        registry.insert(
            "short_timeout_human".into(),
            Arc::new(ShortTimeoutHumanNode),
        );

        let (executor, _run_store) = make_executor(registry);

        let graph = GraphDef {
            schema_version: GRAPH_SCHEMA_VERSION,
            id: "timeout-human-flow".into(),
            name: "Timeout Human".into(),
            version: "1".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "start".into(),
                    node_type: "passthrough".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "human".into(),
                    node_type: "short_timeout_human".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![Edge {
                id: "e1".into(),
                from_node: "start".into(),
                from_port: "output".into(),
                to_node: "human".into(),
                to_port: "input".into(),
                condition: None,
            }],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        // Don't provide any input — let the timeout expire.
        let events = drain_events_long(&mut handle.events).await;

        // Human node should fail with timeout.
        let human_failed: Vec<&WriteEvent> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::NodeFailed { node_id, .. } if node_id == "human"))
            .collect();
        assert_eq!(human_failed.len(), 1, "human should fail with timeout");
        if let WriteEvent::NodeFailed { error, .. } = human_failed[0] {
            assert!(
                error.contains("timeout"),
                "error should mention timeout, got: {error}"
            );
        }

        // Run should fail.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Failed);
        }
    }

    // -----------------------------------------------------------------------
    // Test: Retry backoff timing scales exponentially
    // -----------------------------------------------------------------------

    /// Node that tracks timestamps of each invocation.
    struct TimingNode {
        timestamps: std::sync::Mutex<Vec<std::time::Instant>>,
        fail_count: std::sync::atomic::AtomicU32,
        succeed_after: u32,
    }

    #[async_trait]
    impl NodeHandler for TimingNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "timing".into(),
                label: "Timing".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints {
                    retry: RetryPolicy {
                        max_attempts: 4,
                        backoff_ms: 50,
                        backoff_multiplier: 2.0,
                        retryable_errors: None,
                    },
                    ..Default::default()
                },
            }
        }

        async fn run(
            &self,
            _inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            self.timestamps
                .lock()
                .unwrap()
                .push(std::time::Instant::now());
            let count = self
                .fail_count
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if count < self.succeed_after {
                Err(NodeError::Retryable {
                    message: "not yet".into(),
                })
            } else {
                Ok(json!({"ok": true}))
            }
        }
    }

    #[tokio::test]
    async fn test_retry_backoff_timing() {
        let timing_node = Arc::new(TimingNode {
            timestamps: std::sync::Mutex::new(Vec::new()),
            fail_count: std::sync::atomic::AtomicU32::new(0),
            succeed_after: 3, // Fail 3 times, succeed on 4th.
        });

        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("timing".into(), timing_node.clone());

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "timing")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events_long(&mut handle.events).await;

        // Run should complete successfully after retries.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Completed);
        }

        // Verify backoff timing: delays should be ~50ms, ~100ms, ~200ms.
        let ts = timing_node.timestamps.lock().unwrap();
        assert_eq!(ts.len(), 4, "expected 4 invocations (3 fails + 1 success)");

        let delay1 = ts[1].duration_since(ts[0]).as_millis();
        let delay2 = ts[2].duration_since(ts[1]).as_millis();
        let delay3 = ts[3].duration_since(ts[2]).as_millis();

        // Each delay should be roughly double the previous (with tolerance).
        assert!(delay1 >= 40, "first backoff too short: {delay1}ms");
        assert!(delay2 >= 80, "second backoff too short: {delay2}ms");
        assert!(delay3 >= 160, "third backoff too short: {delay3}ms");
        assert!(
            delay2 > delay1,
            "second delay ({delay2}ms) should exceed first ({delay1}ms)"
        );
        assert!(
            delay3 > delay2,
            "third delay ({delay3}ms) should exceed second ({delay2}ms)"
        );
    }

    // -----------------------------------------------------------------------
    // Test: Cancellation aborts in-flight nodes promptly
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_cancellation_aborts_in_flight() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        // Use a very slow node so cancellation happens mid-execution.
        registry.insert("slow".into(), Arc::new(SlowNode { delay_ms: 2000 }));

        let (executor, run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "slow")]);

        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let run_id = handle.run_id.clone();

        // Wait briefly for node to start, then cancel.
        tokio::time::sleep(Duration::from_millis(30)).await;
        let _ = handle.cancel.send(());

        // Should resolve quickly (not wait for 2s node).
        let start = std::time::Instant::now();
        let events = drain_events(&mut handle.events).await;
        let elapsed = start.elapsed();
        assert!(
            elapsed.as_millis() < 500,
            "cancellation took too long: {}ms",
            elapsed.as_millis()
        );

        // Run should be Cancelled.
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Cancelled);
        }

        // Verify no lingering state: run store should have the run marked.
        let record = run_store.get_run(&run_id).await.unwrap();
        assert!(
            record.is_some(),
            "run should exist in store after cancellation"
        );
    }

    // -----------------------------------------------------------------------
    // Test: Corrupt/unexpected input shape causes graceful failure
    // -----------------------------------------------------------------------

    /// Node that expects a specific input shape and panics on mismatch.
    struct StrictInputNode;

    #[async_trait]
    impl NodeHandler for StrictInputNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "strict_input".into(),
                label: "Strict Input".into(),
                category: "test".into(),
                inputs: vec![],
                outputs: vec![],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            inputs: Value,
            _config: &Value,
            _ctx: &NodeCtx,
        ) -> Result<Value, NodeError> {
            // Attempt to access a field that may not exist — return Fatal on failure.
            let value = inputs
                .get("required_field")
                .and_then(|v| v.as_str())
                .ok_or_else(|| NodeError::Fatal {
                    message: "missing or invalid 'required_field'".into(),
                })?;
            Ok(json!({"processed": value}))
        }
    }

    #[tokio::test]
    async fn test_corrupt_input_handling() {
        let mut registry: HashMap<String, Arc<dyn NodeHandler>> = HashMap::new();
        registry.insert("strict_input".into(), Arc::new(StrictInputNode));

        let (executor, _run_store) = make_executor(registry);
        let graph = make_graph_linear(vec![("A", "strict_input")]);

        // Pass input that doesn't match expected shape.
        let mut handle = executor
            .execute(
                &graph,
                "v1",
                json!({"wrong_field": 42}),
                TriggerSource::Manual {
                    principal: "test".into(),
                },
                None,
            )
            .await
            .unwrap();

        let events = drain_events(&mut handle.events).await;

        // Node should fail gracefully (not panic).
        let failed = events_of_type(&events, is_node_failed);
        assert_eq!(failed.len(), 1);
        if let WriteEvent::NodeFailed { error, .. } = failed[0] {
            assert!(
                error.contains("required_field"),
                "error should mention missing field, got: {error}"
            );
        }

        // Run should fail (not crash/hang).
        let run_completed = events_of_type(&events, is_run_completed);
        assert_eq!(run_completed.len(), 1);
        if let WriteEvent::RunCompleted { status, .. } = run_completed[0] {
            assert_eq!(*status, RunStatus::Failed);
        }
    }
}

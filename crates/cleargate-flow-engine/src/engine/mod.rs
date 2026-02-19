//! Engine builder and runtime — the single entry point for FlowEngine v2.
//!
//! The [`Engine`] assembles all v2 components (executor, write pipeline,
//! triggers, versioning) into a unified runtime. Construct via
//! [`Engine::builder()`].
//!
//! ```rust,ignore
//! let engine = Engine::builder()
//!     .node(MyCustomNode)
//!     .types(vec![my_typedef()])
//!     .secrets(VaultProvider::new("https://vault.internal"))
//!     .build()
//!     .await?;
//!
//! let handle = engine.execute("my-flow", json!({}), None).await?;
//! ```

mod builder;
pub mod error;

pub use builder::EngineBuilder;
pub use error::EngineError;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use serde_json::Value;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::executor::{ExecutionHandle, Executor};
use crate::traits::{
    FlowLlmProvider, FlowStore, NodeHandler, ObservabilityProvider, QueueProvider, Redactor,
    RunStore, SecretsProvider, StateStore, TagStore,
};
use crate::triggers::{RunCompletedEvent, TriggerRunner};
use crate::types::{GraphDef, NodeMeta, TriggerEvent, TriggerSource, TypeDef};
use crate::versioning::FlowVersioning;

/// The assembled, running FlowEngine v2 runtime.
///
/// Owns all runtime components: executor, write pipeline, triggers, and
/// provider implementations. Constructed via [`Engine::builder()`].
///
/// The engine is `Clone`-friendly — all internals are `Arc`-wrapped.
pub struct Engine {
    pub(super) nodes: Arc<BTreeMap<String, Arc<dyn NodeHandler>>>,
    pub(super) type_defs: Arc<Vec<TypeDef>>,
    pub(super) executor: Arc<Executor>,
    pub(super) versioning: Arc<FlowVersioning>,
    pub(super) flow_store: Arc<dyn FlowStore>,
    pub(super) tag_store: Arc<dyn TagStore>,
    pub(super) run_store: Arc<dyn RunStore>,
    pub(super) secrets: Arc<dyn SecretsProvider>,
    pub(super) state: Arc<dyn StateStore>,
    pub(super) queue: Arc<dyn QueueProvider>,
    pub(super) redactor: Arc<dyn Redactor>,
    #[allow(dead_code)] // Stored for future API layer access.
    pub(super) observability: Arc<dyn ObservabilityProvider>,
    pub(super) llm_providers: Arc<HashMap<String, Arc<dyn FlowLlmProvider>>>,
    pub(super) trigger_runner: Option<TriggerRunner>,
    pub(super) run_completed_tx: broadcast::Sender<RunCompletedEvent>,
    /// Receiver for trigger events (taken once by `start_triggers()`).
    pub(super) trigger_event_rx: tokio::sync::Mutex<Option<mpsc::Receiver<TriggerEvent>>>,
    /// Shutdown sender for triggers.
    pub(super) trigger_shutdown_tx: Option<broadcast::Sender<()>>,
    /// Handles for trigger tasks and dispatch loop (for shutdown).
    pub(super) trigger_handles: tokio::sync::Mutex<Vec<JoinHandle<()>>>,
}

impl Engine {
    /// Create a new [`EngineBuilder`].
    pub fn builder() -> EngineBuilder {
        EngineBuilder::new()
    }

    /// Execute a flow by ID. Resolves the current flow version, builds
    /// the execution context, and starts the run.
    ///
    /// Returns immediately with an [`ExecutionHandle`] for live streaming
    /// and cancellation.
    pub async fn execute(
        &self,
        flow_id: &str,
        inputs: Value,
        trigger_event: Option<TriggerEvent>,
    ) -> Result<ExecutionHandle, EngineError> {
        // Resolve the current flow version.
        let version = self.versioning.get_current(flow_id).await?.ok_or_else(|| {
            EngineError::FlowNotFound {
                flow_id: flow_id.to_string(),
            }
        })?;

        let trigger_source = trigger_event
            .as_ref()
            .map(|e| e.source.clone())
            .unwrap_or_else(|| TriggerSource::Api {
                request_id: uuid::Uuid::new_v4().to_string(),
            });

        let handle = self
            .executor
            .execute(
                &version.graph,
                &version.version_id,
                inputs,
                trigger_source,
                None,
            )
            .await?;

        Ok(handle)
    }

    /// Execute a caller-supplied graph directly (e.g. for replay with modified configs).
    pub async fn execute_graph(
        &self,
        graph: &GraphDef,
        version_id: &str,
        inputs: Value,
        trigger_source: TriggerSource,
        parent_run_id: Option<&str>,
    ) -> Result<ExecutionHandle, EngineError> {
        let handle = self
            .executor
            .execute(graph, version_id, inputs, trigger_source, parent_run_id)
            .await?;
        Ok(handle)
    }

    /// Access the flow versioning service (for the API layer).
    pub fn versioning(&self) -> &FlowVersioning {
        &self.versioning
    }

    /// Access the flow store (for the API layer).
    pub fn flow_store(&self) -> &Arc<dyn FlowStore> {
        &self.flow_store
    }

    /// Access the tag store (for the API layer).
    pub fn tag_store(&self) -> &Arc<dyn TagStore> {
        &self.tag_store
    }

    /// Access the run store (for the API layer).
    pub fn run_store(&self) -> &Arc<dyn RunStore> {
        &self.run_store
    }

    /// Returns metadata for all registered node handlers.
    pub fn node_catalog(&self) -> Vec<NodeMeta> {
        self.nodes.values().map(|h| h.meta()).collect()
    }

    /// Look up a node handler by node type.
    pub fn node_handler(&self, node_type: &str) -> Option<Arc<dyn NodeHandler>> {
        self.nodes.get(node_type).cloned()
    }

    /// Access the secrets provider.
    pub fn secrets(&self) -> &Arc<dyn SecretsProvider> {
        &self.secrets
    }

    /// Access the state store.
    pub fn state(&self) -> &Arc<dyn StateStore> {
        &self.state
    }

    /// Access the queue provider.
    pub fn queue(&self) -> &Arc<dyn QueueProvider> {
        &self.queue
    }

    /// Access the LLM providers.
    pub fn llm_providers(&self) -> &Arc<HashMap<String, Arc<dyn FlowLlmProvider>>> {
        &self.llm_providers
    }

    /// Access the redactor.
    pub fn redactor(&self) -> &Arc<dyn Redactor> {
        &self.redactor
    }

    /// Returns all registered type definitions.
    pub fn type_catalog(&self) -> &[TypeDef] {
        &self.type_defs
    }

    /// Deliver a value to a node that's waiting for human-in-loop input.
    pub async fn provide_input(
        &self,
        run_id: &str,
        node_id: &str,
        input: Value,
    ) -> Result<(), EngineError> {
        self.executor
            .provide_input(run_id, node_id, input)
            .map_err(EngineError::Executor)
    }

    /// Subscribe to live events for a running execution (WebSocket support).
    ///
    /// Returns `None` if the run_id is not currently executing.
    pub async fn subscribe_run(
        &self,
        run_id: &str,
    ) -> Option<broadcast::Receiver<crate::write_event::WriteEvent>> {
        self.executor.subscribe_run(run_id).await
    }

    /// Cancel a running execution.
    pub async fn cancel_run(&self, _run_id: &str) -> Result<(), EngineError> {
        // Cancellation is done via the ExecutionHandle's oneshot sender,
        // which the caller holds. This method is a placeholder for cases
        // where the engine needs to track active runs (future enhancement).
        Ok(())
    }

    /// Start the trigger system: spawns trigger tasks and a dispatch loop
    /// that executes flows in response to trigger events.
    ///
    /// This is a no-op if no triggers were registered or if already started.
    pub async fn start_triggers(self: &Arc<Self>) {
        let mut rx_guard = self.trigger_event_rx.lock().await;
        let event_rx = match rx_guard.take() {
            Some(rx) => rx,
            None => return, // Already started or no triggers.
        };

        // Start trigger runner tasks.
        let mut handles = if let Some(ref runner) = self.trigger_runner {
            runner.start()
        } else {
            return;
        };

        // Spawn dispatch loop that reads trigger events and executes flows.
        let engine = Arc::clone(self);
        let versioning = Arc::clone(&self.versioning);
        let dispatch_handle = tokio::spawn(async move {
            let mut event_rx = event_rx;
            let mut seen_keys: HashSet<String> = HashSet::new();

            while let Some(event) = event_rx.recv().await {
                // Idempotency check.
                if let Some(ref key) = event.idempotency_key {
                    if !seen_keys.insert(key.clone()) {
                        tracing::debug!(key = %key, "dispatch: skipping duplicate trigger event");
                        continue;
                    }
                }

                // Resolve version.
                let version = if let Some(ref vid) = event.version_id {
                    match versioning.get_at_version(vid).await {
                        Ok(Some(v)) => v,
                        _ => {
                            tracing::warn!(flow_id = %event.flow_id, "dispatch: failed to resolve version");
                            continue;
                        }
                    }
                } else {
                    match versioning.get_current(&event.flow_id).await {
                        Ok(Some(v)) => v,
                        _ => {
                            tracing::warn!(flow_id = %event.flow_id, "dispatch: no current version");
                            continue;
                        }
                    }
                };

                tracing::info!(
                    flow_id = %event.flow_id,
                    version_id = %version.version_id,
                    source = ?event.source,
                    "dispatch: executing triggered flow"
                );

                let trigger_event = TriggerEvent {
                    flow_id: event.flow_id.clone(),
                    version_id: Some(version.version_id.clone()),
                    inputs: event.inputs.clone(),
                    source: event.source.clone(),
                    idempotency_key: event.idempotency_key.clone(),
                };

                if let Err(e) = engine
                    .execute(&event.flow_id, event.inputs, Some(trigger_event))
                    .await
                {
                    tracing::error!(
                        flow_id = %event.flow_id,
                        error = %e,
                        "dispatch: failed to execute triggered flow"
                    );
                }
            }
        });

        handles.push(dispatch_handle);
        *self.trigger_handles.lock().await = handles;
    }

    /// Gracefully shut down the engine: stops triggers and dispatch loop.
    pub async fn shutdown(&self) {
        // Signal trigger shutdown — causes trigger tasks to exit.
        if let Some(ref tx) = self.trigger_shutdown_tx {
            let _ = tx.send(());
        }

        // Abort all handles (triggers + dispatch loop).
        // The dispatch loop won't exit on its own because TriggerRunner
        // still holds the event_tx sender. Aborting is safe here since
        // we've already signaled shutdown.
        let handles = std::mem::take(&mut *self.trigger_handles.lock().await);
        for handle in handles {
            handle.abort();
            let _ = handle.await;
        }
    }

    /// Access the broadcast sender for run completion events.
    /// Used by the trigger system.
    pub fn run_completed_tx(&self) -> &broadcast::Sender<RunCompletedEvent> {
        &self.run_completed_tx
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::{FileFlowStore, FileRunStore, InMemoryState};
    use crate::traits::NodeHandler;
    use crate::types::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;

    /// Helper: build an engine with temp directories for stores.
    async fn build_test_engine() -> (Engine, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");

        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .build()
            .await
            .expect("engine build");

        (engine, dir)
    }

    /// A custom test node handler.
    struct EchoNode;

    #[async_trait]
    impl NodeHandler for EchoNode {
        fn meta(&self) -> NodeMeta {
            NodeMeta {
                node_type: "echo".into(),
                label: "Echo".into(),
                category: "test".into(),
                inputs: vec![PortDef {
                    name: "input".into(),
                    port_type: PortType::Json,
                    required: true,
                    default: None,
                    description: Some("Input data".into()),
                    sensitivity: Sensitivity::None,
                }],
                outputs: vec![PortDef {
                    name: "output".into(),
                    port_type: PortType::Json,
                    required: true,
                    default: None,
                    description: Some("Echoed data".into()),
                    sensitivity: Sensitivity::None,
                }],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            }
        }

        async fn run(
            &self,
            inputs: Value,
            _config: &Value,
            _ctx: &crate::node_ctx::NodeCtx,
        ) -> Result<Value, NodeError> {
            Ok(inputs)
        }
    }

    // -----------------------------------------------------------------------
    // Test 1: Default build
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_default_build() {
        let (engine, _dir) = build_test_engine().await;

        // Should have all 6 built-in nodes.
        let catalog = engine.node_catalog();
        assert!(
            catalog.len() >= 6,
            "expected at least 6 built-in nodes, got {}",
            catalog.len()
        );

        // Shutdown cleanly.
        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 2: Custom providers
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_custom_providers() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");
        let custom_state = InMemoryState::new();

        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .state(custom_state)
            .build()
            .await
            .expect("engine build");

        // Engine built successfully with custom state provider.
        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 3: Node registration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_node_registration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");

        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .node(EchoNode)
            .build()
            .await
            .expect("engine build");

        let catalog = engine.node_catalog();
        let echo = catalog.iter().find(|m| m.node_type == "echo");
        assert!(echo.is_some(), "custom EchoNode should be in catalog");

        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 4: Built-in nodes auto-registered
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_builtin_nodes_auto_registered() {
        let (engine, _dir) = build_test_engine().await;

        let catalog = engine.node_catalog();
        let node_types: Vec<&str> = catalog.iter().map(|m| m.node_type.as_str()).collect();

        assert!(node_types.contains(&"llm_call"), "missing llm_call");
        assert!(node_types.contains(&"tool_router"), "missing tool_router");
        assert!(node_types.contains(&"http_call"), "missing http_call");
        assert!(node_types.contains(&"conditional"), "missing conditional");
        assert!(
            node_types.contains(&"human_in_loop"),
            "missing human_in_loop"
        );
        assert!(node_types.contains(&"jq"), "missing jq");

        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 5: Type registration
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_type_registration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");

        let typedef = TypeDef {
            name: "CompanyProfile".into(),
            description: "A company profile".into(),
            fields: vec![TypeField {
                name: "name".into(),
                field_type: PortType::String,
                description: "Company name".into(),
                required: true,
                sensitivity: Sensitivity::None,
                example: Some(json!("Acme Corp")),
            }],
            example: Some(json!({"name": "Acme Corp"})),
        };

        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .types(vec![typedef])
            .build()
            .await
            .expect("engine build");

        let types = engine.type_catalog();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].name, "CompanyProfile");

        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 6: Execute flow (A → B)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_execute_flow() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");

        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .node(EchoNode)
            .build()
            .await
            .expect("engine build");

        // Save a simple flow: echo_a → echo_b.
        let graph = GraphDef {
            schema_version: 1,
            id: "test-flow".into(),
            name: "Test Flow".into(),
            version: "1.0".into(),
            nodes: vec![
                NodeInstance {
                    instance_id: "a".into(),
                    node_type: "echo".into(),
                    config: json!({}),
                    position: None,
                },
                NodeInstance {
                    instance_id: "b".into(),
                    node_type: "echo".into(),
                    config: json!({}),
                    position: None,
                },
            ],
            edges: vec![Edge {
                id: "e1".into(),
                from_node: "a".into(),
                from_port: "output".into(),
                to_node: "b".into(),
                to_port: "input".into(),
                condition: None,
            }],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        engine
            .versioning()
            .save("test-flow", "Test Flow", graph, None, None)
            .await
            .expect("save flow");

        let handle = engine
            .execute("test-flow", json!({"hello": "world"}), None)
            .await
            .expect("execute");

        assert!(!handle.run_id.is_empty(), "run_id should be non-empty");

        // Give the run a moment to complete.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 7: Flow not found
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_flow_not_found() {
        let (engine, _dir) = build_test_engine().await;

        let result = engine.execute("nonexistent-flow", json!({}), None).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, EngineError::FlowNotFound { .. }),
            "expected FlowNotFound, got: {err}"
        );

        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 8: Shutdown
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_shutdown() {
        let (engine, _dir) = build_test_engine().await;

        // Shutdown should not panic.
        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 9: Dylib dir scan (empty directory)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_dylib_dir_scan_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        let dylib_dir = dir.path().join("plugins");
        std::fs::create_dir_all(&dylib_dir).expect("create dylib dir");

        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");

        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .dylib_dir(&dylib_dir)
            .build()
            .await
            .expect("engine build with empty dylib dir");

        // Should still have the built-in nodes.
        assert!(engine.node_catalog().len() >= 6);

        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 10: Duplicate node registration (later registration wins)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_duplicate_node_registration() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");

        // Register EchoNode twice — second should silently override.
        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .node(EchoNode)
            .node(EchoNode)
            .build()
            .await
            .expect("engine build");

        let catalog = engine.node_catalog();
        let echo_count = catalog.iter().filter(|m| m.node_type == "echo").count();
        assert_eq!(echo_count, 1, "duplicate should not create two entries");

        engine.shutdown().await;
    }

    // -----------------------------------------------------------------------
    // Test 11: Shutdown with active run
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_shutdown_with_active_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let flow_store = FileFlowStore::new(dir.path().join("flows")).expect("flow store");
        let run_store = FileRunStore::new(dir.path().join("runs")).expect("run store");

        /// A node that sleeps 500ms.
        struct SlowEchoNode;

        #[async_trait]
        impl NodeHandler for SlowEchoNode {
            fn meta(&self) -> NodeMeta {
                NodeMeta {
                    node_type: "slow_echo".into(),
                    label: "Slow Echo".into(),
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
                _ctx: &crate::node_ctx::NodeCtx,
            ) -> Result<Value, NodeError> {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                Ok(inputs)
            }
        }

        let engine = Engine::builder()
            .flow_store(flow_store)
            .run_store(run_store)
            .node(SlowEchoNode)
            .build()
            .await
            .expect("engine build");

        // Save a flow that uses the slow node.
        let graph = GraphDef {
            schema_version: 1,
            id: "slow-flow".into(),
            name: "Slow Flow".into(),
            version: "1.0".into(),
            nodes: vec![NodeInstance {
                instance_id: "a".into(),
                node_type: "slow_echo".into(),
                config: json!({}),
                position: None,
            }],
            edges: vec![],
            metadata: BTreeMap::new(),
            tool_definitions: BTreeMap::new(),
        };

        engine
            .versioning()
            .save("slow-flow", "Slow Flow", graph, None, None)
            .await
            .expect("save flow");

        // Start execution.
        let _handle = engine
            .execute("slow-flow", json!({"x": 1}), None)
            .await
            .expect("execute");

        // Immediately shutdown — should not panic or hang.
        engine.shutdown().await;
    }
}

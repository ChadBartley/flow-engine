//! Runtime context given to every node invocation.
//!
//! Node authors interact with the engine exclusively through [`NodeCtx`].
//! The engine constructs a `NodeCtx` per invocation — node code never
//! creates one directly.
//!
//! # LLM call recording
//!
//! Nodes that make LLM API calls should use [`NodeCtx::record_llm_call`] to
//! persist the full request and response. This data is authoritative and
//! enables deterministic replay, cost analysis, and model comparison.
//!
//! ```ignore
//! // Inside a NodeHandler::run() implementation:
//! let tools = ctx.available_tools();
//! let llm_req = LlmRequest { /* ... */ };
//! let llm_resp = call_provider(&llm_req).await;
//! ctx.record_llm_call(llm_req, llm_resp).await?;
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::tool_registry::ToolRegistry;
use super::traits::{FlowLlmProvider, QueueProvider, SecretsProvider, StateStore};
use super::types::{
    LlmChunk, LlmInvocationRecord, LlmRequest, LlmResponse, NodeError, Sensitivity, ToolDef,
};

#[cfg(feature = "mcp")]
use super::mcp::McpServerRegistry;
#[cfg(feature = "memory")]
use super::memory::{MemoryManager, TokenCounter};

/// Type alias for the human-in-loop input registry.
/// Keyed by `(run_id, node_instance_id)`.
pub type HumanInputRegistry =
    Arc<tokio::sync::Mutex<HashMap<(String, String), oneshot::Sender<Value>>>>;

// ---------------------------------------------------------------------------
// NodeEvent
// ---------------------------------------------------------------------------

/// A diagnostic event emitted by a node via [`NodeCtx::emit`].
///
/// Events flow through the advisory write channel — they may be dropped
/// under backpressure. Use for logging, metrics, and debugging; never for
/// data required by replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct NodeEvent {
    pub name: String,
    pub data: Value,
    pub sensitivity: Sensitivity,
}

// ---------------------------------------------------------------------------
// NodeCtx
// ---------------------------------------------------------------------------

/// The runtime context given to every node invocation.
///
/// Node authors interact with the engine exclusively through this struct.
/// It provides access to secrets, state, messaging, HTTP, LLM recording,
/// and tool definitions. The engine creates a fresh `NodeCtx` for each
/// node execution — node code never constructs one directly.
pub struct NodeCtx {
    run_id: String,
    node_instance_id: String,
    secrets: Arc<dyn SecretsProvider>,
    state: Arc<dyn StateStore>,
    queue: Arc<dyn QueueProvider>,
    event_tx: mpsc::Sender<NodeEvent>,
    llm_tx: mpsc::Sender<LlmInvocationRecord>,
    http_client: reqwest::Client,
    tool_definitions: Arc<Vec<ToolDef>>,
    human_input_registry: Option<HumanInputRegistry>,
    llm_providers: Arc<HashMap<String, Arc<dyn FlowLlmProvider>>>,
    #[cfg(feature = "mcp")]
    mcp_registry: Arc<McpServerRegistry>,
    #[cfg(feature = "memory")]
    memory_manager: Arc<dyn MemoryManager>,
    #[cfg(feature = "memory")]
    token_counter: Arc<dyn TokenCounter>,
    tool_registry: ToolRegistry,
}

impl NodeCtx {
    /// Construct a `NodeCtx` for a node execution.
    ///
    /// Typically only called by the executor or replay engine — node authors
    /// receive `&NodeCtx` and don't construct instances directly.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        run_id: String,
        node_instance_id: String,
        secrets: Arc<dyn SecretsProvider>,
        state: Arc<dyn StateStore>,
        queue: Arc<dyn QueueProvider>,
        event_tx: mpsc::Sender<NodeEvent>,
        llm_tx: mpsc::Sender<LlmInvocationRecord>,
        http_client: reqwest::Client,
        tool_definitions: Arc<Vec<ToolDef>>,
        human_input_registry: Option<HumanInputRegistry>,
        llm_providers: Arc<HashMap<String, Arc<dyn FlowLlmProvider>>>,
        #[cfg(feature = "mcp")] mcp_registry: Arc<McpServerRegistry>,
        #[cfg(feature = "memory")] memory_manager: Arc<dyn MemoryManager>,
        #[cfg(feature = "memory")] token_counter: Arc<dyn TokenCounter>,
        tool_registry: ToolRegistry,
    ) -> Self {
        Self {
            run_id,
            node_instance_id,
            secrets,
            state,
            queue,
            event_tx,
            llm_tx,
            http_client,
            tool_definitions,
            human_input_registry,
            llm_providers,
            #[cfg(feature = "mcp")]
            mcp_registry,
            #[cfg(feature = "memory")]
            memory_manager,
            #[cfg(feature = "memory")]
            token_counter,
            tool_registry,
        }
    }

    /// The run this node execution belongs to.
    pub fn run_id(&self) -> &str {
        &self.run_id
    }

    /// The specific node instance being executed.
    pub fn node_instance_id(&self) -> &str {
        &self.node_instance_id
    }

    /// Retrieve a secret by key. Returns `NodeError::Fatal` if not found,
    /// because a missing secret typically means the node cannot proceed.
    pub async fn secret(&self, key: &str) -> Result<String, NodeError> {
        self.secrets
            .get(key)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("secrets provider error: {e}"),
            })?
            .ok_or_else(|| NodeError::Fatal {
                message: format!("secret not found: {key}"),
            })
    }

    /// Read a value from the run's state store.
    pub async fn state_get(&self, key: &str) -> Result<Option<Value>, NodeError> {
        self.state
            .get(&self.run_id, key)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("state get error: {e}"),
            })
    }

    /// Write a value to the run's state store with `Sensitivity::None`.
    pub async fn state_set(&self, key: &str, value: Value) -> Result<(), NodeError> {
        self.state
            .set(&self.run_id, key, value, Sensitivity::None)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("state set error: {e}"),
            })
    }

    /// Write a value to the run's state store with `Sensitivity::Secret`.
    pub async fn state_set_secret(&self, key: &str, value: Value) -> Result<(), NodeError> {
        self.state
            .set(&self.run_id, key, value, Sensitivity::Secret)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("state set error: {e}"),
            })
    }

    /// Emit a diagnostic event (advisory channel — may be dropped under
    /// backpressure). Use for logging/metrics, never for replay-critical data.
    pub fn emit(&self, name: &str, data: Value) {
        let _ = self.event_tx.try_send(NodeEvent {
            name: name.to_string(),
            data,
            sensitivity: Sensitivity::None,
        });
    }

    /// Emit a streaming LLM chunk through the advisory channel.
    ///
    /// Stream chunks are ephemeral UI events — the final [`LlmInvocationRecord`]
    /// recorded via [`record_llm_call()`](Self::record_llm_call) is authoritative.
    /// Chunks may be dropped under backpressure.
    pub fn emit_llm_chunk(&self, chunk: LlmChunk) {
        let data = serde_json::to_value(&chunk).unwrap_or_default();
        let _ = self.event_tx.try_send(NodeEvent {
            name: "__llm_chunk".to_string(),
            data,
            sensitivity: Sensitivity::None,
        });
    }

    /// Emit a diagnostic event tagged with a sensitivity level.
    pub fn emit_sensitive(&self, name: &str, data: Value, sensitivity: Sensitivity) {
        let _ = self.event_tx.try_send(NodeEvent {
            name: name.to_string(),
            data,
            sensitivity,
        });
    }

    /// Publish a message to a queue topic.
    pub async fn publish(&self, topic: &str, payload: &[u8]) -> Result<(), NodeError> {
        self.queue
            .publish(topic, payload, None)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("queue publish error: {e}"),
            })
    }

    /// Shared HTTP client for making external requests.
    pub fn http(&self) -> &reqwest::Client {
        &self.http_client
    }

    /// Record a structured LLM invocation through the critical write channel.
    ///
    /// Call this after every LLM API invocation. The engine records the full
    /// request and response for replay, cost analysis, and model comparison.
    /// This data is authoritative — it blocks if the channel is full rather
    /// than dropping.
    pub async fn record_llm_call(
        &self,
        request: LlmRequest,
        response: LlmResponse,
    ) -> Result<(), NodeError> {
        let record = LlmInvocationRecord {
            run_id: self.run_id.clone(),
            node_id: self.node_instance_id.clone(),
            request,
            response,
            timestamp: Utc::now(),
        };
        self.llm_tx
            .send(record)
            .await
            .map_err(|e| NodeError::Fatal {
                message: format!("failed to record LLM invocation: {e}"),
            })
    }

    /// Tool definitions from the flow's graph. Use this to build the `tools`
    /// parameter for your LLM API call.
    pub fn available_tools(&self) -> &[ToolDef] {
        &self.tool_definitions
    }

    /// Access the tool definitions as an `Arc` (for cloning into dylib contexts).
    pub(crate) fn tool_definitions_arc(&self) -> Arc<Vec<ToolDef>> {
        Arc::clone(&self.tool_definitions)
    }

    /// Access the secrets provider (for cloning into dylib contexts).
    pub(crate) fn secrets_provider(&self) -> Arc<dyn SecretsProvider> {
        Arc::clone(&self.secrets)
    }

    /// Access the state store (for cloning into dylib contexts).
    pub(crate) fn state_store(&self) -> Arc<dyn StateStore> {
        Arc::clone(&self.state)
    }

    /// Access the queue provider (for cloning into dylib contexts).
    pub(crate) fn queue_provider(&self) -> Arc<dyn QueueProvider> {
        Arc::clone(&self.queue)
    }

    /// Access the event sender (for cloning into dylib contexts).
    pub(crate) fn event_sender(&self) -> mpsc::Sender<NodeEvent> {
        self.event_tx.clone()
    }

    /// Access the LLM invocation sender (for cloning into dylib contexts).
    pub(crate) fn llm_sender(&self) -> mpsc::Sender<LlmInvocationRecord> {
        self.llm_tx.clone()
    }

    /// Access the LLM providers map (for cloning into dylib contexts).
    pub(crate) fn llm_providers_arc(&self) -> Arc<HashMap<String, Arc<dyn FlowLlmProvider>>> {
        Arc::clone(&self.llm_providers)
    }

    /// Look up a registered LLM provider by name.
    pub fn llm_provider(&self, name: &str) -> Option<Arc<dyn FlowLlmProvider>> {
        self.llm_providers.get(name).cloned()
    }

    /// Look up a registered MCP server by name.
    #[cfg(feature = "mcp")]
    pub fn mcp_server(&self, name: &str) -> Option<Arc<super::mcp::McpServer>> {
        self.mcp_registry.get(name)
    }

    /// Access the MCP server registry (for cloning into dylib contexts).
    #[cfg(feature = "mcp")]
    pub(crate) fn mcp_registry_arc(&self) -> Arc<McpServerRegistry> {
        Arc::clone(&self.mcp_registry)
    }

    /// Access the dynamic tool registry.
    ///
    /// Use this to register or remove tools at runtime. Changes are
    /// immediately visible to all nodes spawned after the modification.
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.tool_registry
    }

    /// Access the memory manager.
    #[cfg(feature = "memory")]
    pub fn memory_manager(&self) -> &Arc<dyn MemoryManager> {
        &self.memory_manager
    }

    /// Access the token counter.
    #[cfg(feature = "memory")]
    pub fn token_counter(&self) -> &Arc<dyn TokenCounter> {
        &self.token_counter
    }

    /// Access the memory manager as an `Arc` (for cloning into dylib contexts).
    #[cfg(feature = "memory")]
    pub(crate) fn memory_manager_arc(&self) -> Arc<dyn MemoryManager> {
        Arc::clone(&self.memory_manager)
    }

    /// Access the token counter as an `Arc` (for cloning into dylib contexts).
    #[cfg(feature = "memory")]
    pub(crate) fn token_counter_arc(&self) -> Arc<dyn TokenCounter> {
        Arc::clone(&self.token_counter)
    }

    /// Create a [`Blackboard`](crate::multi_agent::Blackboard) scoped to this run.
    ///
    /// The blackboard provides namespaced shared memory for cross-agent
    /// communication. It wraps the run's existing [`StateStore`] — no
    /// additional storage is needed.
    #[cfg(feature = "multi-agent")]
    pub fn blackboard(&self) -> crate::multi_agent::Blackboard {
        crate::multi_agent::Blackboard::new(Arc::clone(&self.state), self.run_id.clone())
    }

    /// Register a oneshot sender for human-in-loop input.
    ///
    /// The executor's `provide_input()` method delivers a value through the
    /// channel, unblocking the waiting node.
    pub async fn register_human_input(
        &self,
        sender: oneshot::Sender<Value>,
    ) -> Result<(), NodeError> {
        let registry = self.human_input_registry.as_ref().ok_or(NodeError::Fatal {
            message: "human input registry not available".into(),
        })?;
        let mut guard = registry.lock().await;
        guard.insert((self.run_id.clone(), self.node_instance_id.clone()), sender);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Test support — public so plugin crates can use TestNodeCtx in their tests
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    //! Test utilities for building [`NodeCtx`] instances in node handler tests.
    //!
    //! ```ignore
    //! let (ctx, inspector) = TestNodeCtx::builder()
    //!     .run_id("test-run")
    //!     .node_id("llm-1")
    //!     .secret("API_KEY", "sk-xxx")
    //!     .build();
    //!
    //! my_node.run(inputs, &config, &ctx).await?;
    //!
    //! assert_eq!(inspector.recorded_llm_calls().len(), 1);
    //! ```

    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use parking_lot::RwLock;
    use serde_json::Value;
    use tokio::sync::mpsc;

    use crate::errors::{QueueError, SecretsError, StateError};
    use crate::tool_registry::ToolRegistry;
    use crate::traits::{FlowLlmProvider, QueueProvider, SecretsProvider, StateStore};
    use crate::types::{LlmInvocationRecord, Sensitivity, ToolDef};

    use super::HumanInputRegistry;
    use super::NodeEvent;

    use super::NodeCtx;

    // -- Type aliases for clippy::type_complexity ----------------------------

    type RunState = HashMap<String, HashMap<String, (Value, Sensitivity)>>;
    type MessageLog = Vec<(String, Vec<u8>)>;

    // -- In-memory SecretsProvider ------------------------------------------

    struct MemorySecrets {
        secrets: HashMap<String, String>,
    }

    #[async_trait]
    impl SecretsProvider for MemorySecrets {
        async fn get(&self, key: &str) -> Result<Option<String>, SecretsError> {
            Ok(self.secrets.get(key).cloned())
        }
    }

    // -- In-memory StateStore -----------------------------------------------

    struct MemoryState {
        data: RwLock<RunState>,
    }

    #[async_trait]
    impl StateStore for MemoryState {
        async fn get(&self, run_id: &str, key: &str) -> Result<Option<Value>, StateError> {
            let guard = self.data.read();
            Ok(guard
                .get(run_id)
                .and_then(|m| m.get(key))
                .map(|(v, _)| v.clone()))
        }

        async fn set(
            &self,
            run_id: &str,
            key: &str,
            value: Value,
            sensitivity: Sensitivity,
        ) -> Result<(), StateError> {
            let mut guard = self.data.write();
            guard
                .entry(run_id.to_string())
                .or_default()
                .insert(key.to_string(), (value, sensitivity));
            Ok(())
        }

        async fn snapshot(&self, run_id: &str) -> Result<HashMap<String, Value>, StateError> {
            let guard = self.data.read();
            Ok(guard
                .get(run_id)
                .map(|m| m.iter().map(|(k, (v, _))| (k.clone(), v.clone())).collect())
                .unwrap_or_default())
        }

        async fn restore(
            &self,
            run_id: &str,
            state: HashMap<String, Value>,
        ) -> Result<(), StateError> {
            let mut guard = self.data.write();
            let entry = guard.entry(run_id.to_string()).or_default();
            for (k, v) in state {
                entry.insert(k, (v, Sensitivity::None));
            }
            Ok(())
        }

        async fn cleanup(&self, run_id: &str) -> Result<(), StateError> {
            let mut guard = self.data.write();
            guard.remove(run_id);
            Ok(())
        }
    }

    // -- In-memory QueueProvider --------------------------------------------

    struct MemoryQueue {
        messages: Arc<RwLock<MessageLog>>,
    }

    #[async_trait]
    impl QueueProvider for MemoryQueue {
        async fn publish(
            &self,
            topic: &str,
            payload: &[u8],
            _headers: Option<&HashMap<String, String>>,
        ) -> Result<(), QueueError> {
            let mut guard = self.messages.write();
            guard.push((topic.to_string(), payload.to_vec()));
            Ok(())
        }

        async fn subscribe(
            &self,
            _topic: &str,
        ) -> Result<crate::traits::QueueReceiver, QueueError> {
            let (_, rx) = mpsc::channel(1);
            Ok(crate::traits::QueueReceiver { rx })
        }

        async fn ack(&self, _receipt: &crate::traits::MessageReceipt) -> Result<(), QueueError> {
            Ok(())
        }
    }

    // -- TestNodeCtx builder ------------------------------------------------

    /// Builder for constructing a [`NodeCtx`] in tests.
    pub struct TestNodeCtx {
        run_id: String,
        node_id: String,
        secrets: HashMap<String, String>,
        tools: Vec<ToolDef>,
        human_input_registry: Option<HumanInputRegistry>,
        llm_providers: HashMap<String, Arc<dyn FlowLlmProvider>>,
        #[cfg(feature = "mcp")]
        mcp_registry: Option<Arc<crate::mcp::McpServerRegistry>>,
    }

    impl TestNodeCtx {
        /// Start building a test `NodeCtx`.
        pub fn builder() -> Self {
            Self {
                run_id: "test-run".to_string(),
                node_id: "test-node".to_string(),
                secrets: HashMap::new(),
                tools: Vec::new(),
                human_input_registry: None,
                llm_providers: HashMap::new(),
                #[cfg(feature = "mcp")]
                mcp_registry: None,
            }
        }

        /// Set the run ID.
        pub fn run_id(mut self, run_id: &str) -> Self {
            self.run_id = run_id.to_string();
            self
        }

        /// Set the node instance ID.
        pub fn node_id(mut self, node_id: &str) -> Self {
            self.node_id = node_id.to_string();
            self
        }

        /// Add a secret (can be called multiple times).
        pub fn secret(mut self, key: &str, value: &str) -> Self {
            self.secrets.insert(key.to_string(), value.to_string());
            self
        }

        /// Add a tool definition.
        pub fn tool(mut self, tool: ToolDef) -> Self {
            self.tools.push(tool);
            self
        }

        /// Set the human input registry.
        pub fn with_human_input_registry(mut self, registry: HumanInputRegistry) -> Self {
            self.human_input_registry = Some(registry);
            self
        }

        /// Register an LLM provider for testing.
        pub fn llm_provider(mut self, name: &str, provider: Arc<dyn FlowLlmProvider>) -> Self {
            self.llm_providers.insert(name.to_string(), provider);
            self
        }

        /// Set a custom MCP server registry for testing MCP nodes.
        #[cfg(feature = "mcp")]
        pub fn mcp_registry(mut self, registry: Arc<crate::mcp::McpServerRegistry>) -> Self {
            self.mcp_registry = Some(registry);
            self
        }

        /// Build the `NodeCtx` and an inspector for verifying side effects.
        pub fn build(self) -> (NodeCtx, TestNodeCtxInspector) {
            let (event_tx, event_rx) = mpsc::channel::<NodeEvent>(1000);
            let (llm_tx, llm_rx) = mpsc::channel::<LlmInvocationRecord>(1000);

            let queue_messages: Arc<RwLock<MessageLog>> = Arc::new(RwLock::new(Vec::new()));

            let state = Arc::new(MemoryState {
                data: RwLock::new(HashMap::new()),
            });

            let queue = Arc::new(MemoryQueue {
                messages: Arc::clone(&queue_messages),
            });

            let tool_registry = ToolRegistry::from_tools(self.tools.iter().cloned());

            let ctx = NodeCtx::new(
                self.run_id,
                self.node_id,
                Arc::new(MemorySecrets {
                    secrets: self.secrets,
                }),
                Arc::clone(&state) as Arc<dyn StateStore>,
                Arc::clone(&queue) as Arc<dyn QueueProvider>,
                event_tx,
                llm_tx,
                reqwest::Client::new(),
                Arc::new(self.tools),
                self.human_input_registry,
                Arc::new(self.llm_providers),
                #[cfg(feature = "mcp")]
                self.mcp_registry
                    .unwrap_or_else(|| Arc::new(crate::mcp::McpServerRegistry::new())),
                #[cfg(feature = "memory")]
                Arc::new(crate::memory::InMemoryManager),
                #[cfg(feature = "memory")]
                Arc::new(crate::memory::CharEstimateCounter),
                tool_registry,
            );

            let inspector = TestNodeCtxInspector {
                event_rx: Arc::new(tokio::sync::Mutex::new(event_rx)),
                llm_rx: Arc::new(tokio::sync::Mutex::new(llm_rx)),
                state,
                queue_messages,
            };

            (ctx, inspector)
        }
    }

    /// Inspect side effects produced by a node under test.
    pub struct TestNodeCtxInspector {
        event_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<NodeEvent>>>,
        llm_rx: Arc<tokio::sync::Mutex<mpsc::Receiver<LlmInvocationRecord>>>,
        state: Arc<MemoryState>,
        queue_messages: Arc<RwLock<MessageLog>>,
    }

    impl TestNodeCtxInspector {
        /// Drain all emitted events from the channel.
        pub async fn emitted_events(&self) -> Vec<NodeEvent> {
            let mut rx: tokio::sync::MutexGuard<'_, mpsc::Receiver<NodeEvent>> =
                self.event_rx.lock().await;
            let mut events = Vec::new();
            while let Ok(ev) = rx.try_recv() {
                events.push(ev);
            }
            events
        }

        /// Snapshot the current state for the run.
        pub async fn state_snapshot(&self) -> HashMap<String, Value> {
            // Read all runs — test usually has a single run
            let guard = self.state.data.read();
            guard
                .values()
                .flat_map(|m| m.iter().map(|(k, (v, _))| (k.clone(), v.clone())))
                .collect()
        }

        /// Drain all recorded LLM invocations from the channel.
        pub async fn recorded_llm_calls(&self) -> Vec<LlmInvocationRecord> {
            let mut rx: tokio::sync::MutexGuard<'_, mpsc::Receiver<LlmInvocationRecord>> =
                self.llm_rx.lock().await;
            let mut calls = Vec::new();
            while let Ok(record) = rx.try_recv() {
                calls.push(record);
            }
            calls
        }

        /// Drain all LLM stream chunks emitted via `emit_llm_chunk()`.
        pub async fn recorded_llm_chunks(&self) -> Vec<crate::types::LlmChunk> {
            let events = self.emitted_events().await;
            events
                .into_iter()
                .filter(|e| e.name == "__llm_chunk")
                .filter_map(|e| serde_json::from_value(e.data).ok())
                .collect()
        }

        /// All messages published to the queue.
        pub fn published_messages(&self) -> Vec<(String, Vec<u8>)> {
            self.queue_messages.read().clone()
        }
    }
}

// ---------------------------------------------------------------------------
// Re-export test support types at the module level
// ---------------------------------------------------------------------------
#[cfg(any(test, feature = "test-support"))]
pub use test_support::{TestNodeCtx, TestNodeCtxInspector};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use serde_json::json;

    use super::*;
    use crate::types::{ToolDef, ToolType};

    fn make_llm_request() -> LlmRequest {
        LlmRequest {
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
        }
    }

    fn make_llm_response() -> LlmResponse {
        LlmResponse {
            content: json!("Hello!"),
            tool_calls: None,
            model_used: "gpt-4o".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
            total_tokens: Some(15),
            finish_reason: "stop".into(),
            latency_ms: 200,
            provider_request_id: None,
            cost: None,
        }
    }

    fn make_tool() -> ToolDef {
        ToolDef {
            name: "search".into(),
            description: "Search the web".into(),
            parameters: json!({"type": "object"}),
            tool_type: ToolType::Node {
                target_node_id: "search-node".into(),
            },
            metadata: BTreeMap::new(),
            permissions: BTreeSet::new(),
        }
    }

    #[tokio::test]
    async fn test_secret_found() {
        let (ctx, _inspector) = TestNodeCtx::builder().secret("API_KEY", "sk-xxx").build();

        let val = ctx.secret("API_KEY").await.unwrap();
        assert_eq!(val, "sk-xxx");
    }

    #[tokio::test]
    async fn test_secret_not_found() {
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        let err = ctx.secret("MISSING").await.unwrap_err();
        match err {
            NodeError::Fatal { message } => {
                assert!(message.contains("not found"), "got: {message}");
            }
            other => panic!("expected Fatal, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_state_get_set() {
        let (ctx, inspector) = TestNodeCtx::builder().build();

        ctx.state_set("counter", json!(42)).await.unwrap();
        let val = ctx.state_get("counter").await.unwrap();
        assert_eq!(val, Some(json!(42)));

        let snapshot = inspector.state_snapshot().await;
        assert_eq!(snapshot.get("counter"), Some(&json!(42)));
    }

    #[tokio::test]
    async fn test_state_set_secret() {
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        // state_set_secret should succeed without error
        ctx.state_set_secret("token", json!("sensitive-value"))
            .await
            .unwrap();

        // The value should still be readable
        let val = ctx.state_get("token").await.unwrap();
        assert_eq!(val, Some(json!("sensitive-value")));
    }

    #[tokio::test]
    async fn test_emit_events() {
        let (ctx, inspector) = TestNodeCtx::builder().build();

        ctx.emit("step.started", json!({"step": 1}));
        ctx.emit("step.completed", json!({"step": 1}));
        ctx.emit_sensitive("pii.detected", json!({"field": "email"}), Sensitivity::Pii);

        let events = inspector.emitted_events().await;
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].name, "step.started");
        assert_eq!(events[1].name, "step.completed");
        assert_eq!(events[2].name, "pii.detected");
        assert_eq!(events[2].sensitivity, Sensitivity::Pii);
    }

    #[tokio::test]
    async fn test_emit_does_not_block() {
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        // Channel capacity is 1000 — emit 2000 events and verify no panic.
        // Events beyond capacity are silently dropped (advisory channel).
        for i in 0..2000 {
            ctx.emit("event", json!({"i": i}));
        }
    }

    #[tokio::test]
    async fn test_record_llm_call() {
        let (ctx, inspector) = TestNodeCtx::builder()
            .run_id("run-1")
            .node_id("llm-node")
            .build();

        let req = make_llm_request();
        let resp = make_llm_response();

        ctx.record_llm_call(req, resp).await.unwrap();

        let calls = inspector.recorded_llm_calls().await;
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].run_id, "run-1");
        assert_eq!(calls[0].node_id, "llm-node");
        assert_eq!(calls[0].request.model, "gpt-4o");
        assert_eq!(calls[0].response.finish_reason, "stop");
    }

    #[tokio::test]
    async fn test_available_tools() {
        let tool = make_tool();
        let (ctx, _inspector) = TestNodeCtx::builder().tool(tool.clone()).build();

        let tools = ctx.available_tools();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "search");
    }

    #[tokio::test]
    async fn test_publish() {
        let (ctx, inspector) = TestNodeCtx::builder().build();

        ctx.publish("events", b"hello world").await.unwrap();

        let messages = inspector.published_messages();
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].0, "events");
        assert_eq!(messages[0].1, b"hello world");
    }

    #[tokio::test]
    async fn test_http_client() {
        let (ctx, _inspector) = TestNodeCtx::builder().build();

        // Verify http() returns a valid client without panicking.
        let _client = ctx.http();
    }
}

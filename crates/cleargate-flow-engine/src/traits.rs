//! Plugin trait interfaces for FlowEngine v2.
//!
//! Every pluggable component is defined as an async trait. Default
//! implementations live in `flowengine-defaults` (or `flow/defaults/`).
//! Adding a method to any trait requires a default implementation to
//! preserve backward compatibility.

use std::any::Any;
use std::collections::HashMap;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::Instant;

use super::errors::*;
use super::node_ctx::NodeCtx;
use super::types::*;
use super::write_event::WriteEvent;

// ---------------------------------------------------------------------------
// FlowLlmProvider
// ---------------------------------------------------------------------------

/// LLM provider interface for the flow engine.
///
/// This trait defines the engine's own LLM abstraction, independent of
/// `cleargate_providers::LLMProvider`. This allows the flow engine to be
/// extracted into a standalone crate without server dependencies.
///
/// Implementations should convert the engine's `LlmRequest` to their
/// provider-specific format, make the API call, and convert the response
/// back to `LlmResponse`.
#[async_trait]
pub trait FlowLlmProvider: Send + Sync {
    /// Make an LLM completion request.
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, NodeError>;

    /// Stream an LLM completion token-by-token.
    ///
    /// The default implementation calls [`complete()`](Self::complete) and
    /// wraps the result as a single `LlmChunk::Done`. Override this (and
    /// return `true` from [`supports_streaming()`](Self::supports_streaming))
    /// for real streaming.
    async fn complete_streaming(
        &self,
        request: LlmRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<LlmChunk, NodeError>> + Send>>, NodeError> {
        let resp = self.complete(request).await?;
        let finish_reason = resp.finish_reason.clone();
        Ok(Box::pin(futures::stream::once(async move {
            Ok(LlmChunk::Done { finish_reason })
        })))
    }

    /// Whether this provider supports real streaming.
    fn supports_streaming(&self) -> bool {
        false
    }

    /// Provider name for diagnostics and routing.
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// NodeHandler
// ---------------------------------------------------------------------------

/// Every node type implements this trait. The engine calls `run()` with
/// resolved inputs and a `NodeCtx` providing runtime capabilities.
///
/// # Stability
/// Adding methods requires a default impl. Existing signatures are frozen
/// from v0.1.
#[async_trait]
pub trait NodeHandler: Send + Sync {
    /// Static metadata: ports, config schema, UI hints.
    fn meta(&self) -> NodeMeta;

    /// Execute the node. `inputs` are the resolved upstream values,
    /// `config` is the node instance config from the graph, and `ctx`
    /// provides secrets, state, events, and LLM recording.
    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError>;

    /// Validate node config at graph save time. Returns a list of
    /// validation error messages, or `Ok(())` if valid.
    fn validate_config(&self, _config: &Value) -> Result<(), Vec<String>> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// SecretsProvider
// ---------------------------------------------------------------------------

/// Where credentials come from.
///
/// Implementations might read from environment variables, Vault, AWS Secrets
/// Manager, or a static test map. The engine calls [`get()`](Self::get)
/// during node execution when a node config references `$SECRET_NAME`.
#[async_trait]
pub trait SecretsProvider: Send + Sync {
    /// Retrieve a secret by key. Returns `None` if not found.
    async fn get(&self, key: &str) -> Result<Option<String>, SecretsError>;

    /// List available secret keys. Default: empty list.
    async fn list_keys(&self) -> Result<Vec<String>, SecretsError> {
        Ok(vec![])
    }
}

// ---------------------------------------------------------------------------
// ObservabilityProvider
// ---------------------------------------------------------------------------

/// Where telemetry traces go.
///
/// This is a projection layer (I5) — never authoritative for replay.
/// Span IDs must be derived deterministically from execution IDs.
pub trait ObservabilityProvider: Send + Sync {
    fn start_node_span(
        &self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        meta: &NodeMeta,
    ) -> SpanHandle;

    fn record_io(&self, span: &SpanHandle, inputs: &Value, outputs: &Value);

    fn record_edge(&self, span: &SpanHandle, edge: &Edge, result: bool, data: &Value);

    fn record_event(&self, span: &SpanHandle, name: &str, data: &Value);

    fn record_error(&self, span: &SpanHandle, error: &NodeError);

    /// Record LLM-specific telemetry (model, tokens, cost, finish_reason).
    fn record_llm(&self, span: &SpanHandle, request: &LlmRequest, response: &LlmResponse);

    fn end_span(&self, span: SpanHandle, status: SpanStatus);
}

/// Handle to an active telemetry span.
pub struct SpanHandle {
    /// Derived from run_id.
    pub trace_id: String,
    /// Derived from run_id + node_id + attempt.
    pub span_id: String,
    pub start_time: Instant,
    /// Provider-specific data (e.g. tracing span guard).
    pub inner: Box<dyn Any + Send + Sync>,
}

/// Terminal status of a span.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SpanStatus {
    Ok,
    Error(String),
    Timeout,
    Skipped,
}

// ---------------------------------------------------------------------------
// QueueProvider
// ---------------------------------------------------------------------------

/// Message passing between flows and external systems.
///
/// Used for async communication: a node can publish a message to a topic,
/// and another flow (or external consumer) subscribes. Messages must be
/// acknowledged via [`ack()`](Self::ack) after processing.
#[async_trait]
pub trait QueueProvider: Send + Sync {
    async fn publish(
        &self,
        topic: &str,
        payload: &[u8],
        headers: Option<&HashMap<String, String>>,
    ) -> Result<(), QueueError>;

    async fn subscribe(&self, topic: &str) -> Result<QueueReceiver, QueueError>;

    async fn ack(&self, receipt: &MessageReceipt) -> Result<(), QueueError>;
}

/// Receiver end of a queue subscription.
pub struct QueueReceiver {
    pub rx: tokio::sync::mpsc::Receiver<QueueMessage>,
}

/// A message received from a queue.
#[derive(Debug, Clone)]
pub struct QueueMessage {
    pub payload: Vec<u8>,
    pub headers: HashMap<String, String>,
    pub receipt: MessageReceipt,
}

/// Opaque receipt for acknowledging a queue message.
#[derive(Debug, Clone)]
pub struct MessageReceipt {
    pub id: String,
}

// ---------------------------------------------------------------------------
// StateStore
// ---------------------------------------------------------------------------

/// Execution scratchpad and checkpointing.
///
/// Provides per-run key-value storage for nodes to share state during
/// execution. The [`snapshot()`](Self::snapshot) / [`restore()`](Self::restore)
/// pair enables checkpoint-based crash recovery.
#[async_trait]
pub trait StateStore: Send + Sync {
    async fn get(&self, run_id: &str, key: &str) -> Result<Option<Value>, StateError>;

    async fn set(
        &self,
        run_id: &str,
        key: &str,
        value: Value,
        sensitivity: Sensitivity,
    ) -> Result<(), StateError>;

    async fn snapshot(&self, run_id: &str) -> Result<HashMap<String, Value>, StateError>;

    async fn restore(&self, run_id: &str, state: HashMap<String, Value>) -> Result<(), StateError>;

    async fn cleanup(&self, run_id: &str) -> Result<(), StateError>;
}

// ---------------------------------------------------------------------------
// Redactor
// ---------------------------------------------------------------------------

/// Data scrubbing, applied inside the write pipeline as the single
/// enforcement point.
pub trait Redactor: Send + Sync {
    /// Redact a single field value based on its sensitivity.
    fn redact(&self, field: &str, sensitivity: &Sensitivity, value: &Value) -> Value;

    /// Redact LLM message content. Applied to `LlmRequest.messages` and
    /// `LlmResponse.content` before persisting. Default: delegates to
    /// field-level redaction using port sensitivity metadata.
    fn redact_llm_messages(&self, messages: &Value, sensitivity: &Sensitivity) -> Value {
        self.redact("llm_messages", sensitivity, messages)
    }
}

// ---------------------------------------------------------------------------
// FlowStore
// ---------------------------------------------------------------------------

/// Persistence for flow definitions and versions.
///
/// The flow store is content-addressed: each version is identified by a
/// SHA-256 hash of its canonical JSON. The "head" pointer tracks which
/// version is current for each flow_id.
#[async_trait]
pub trait FlowStore: Send + Sync {
    async fn put_version(&self, version: &FlowVersion) -> Result<(), FlowStoreError>;

    async fn put_head(&self, head: &FlowHead) -> Result<(), FlowStoreError>;

    async fn get_head(&self, flow_id: &str) -> Result<Option<FlowHead>, FlowStoreError>;

    async fn get_version(&self, version_id: &str) -> Result<Option<FlowVersion>, FlowStoreError>;

    /// List versions for a flow, sorted by `created_at` descending
    /// with `version_id` as tiebreaker.
    async fn list_versions(
        &self,
        flow_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<FlowVersion>, FlowStoreError>;

    async fn list_flows(&self) -> Result<Vec<FlowHead>, FlowStoreError>;

    async fn get_versions_for_diff(
        &self,
        a: &str,
        b: &str,
    ) -> Result<(FlowVersion, FlowVersion), FlowStoreError>;
}

// ---------------------------------------------------------------------------
// TagStore
// ---------------------------------------------------------------------------

/// Mutable tag mappings for flow versions.
///
/// Tags are separate from the immutable version graph. A tag is a
/// human-readable alias (e.g. "prod", "staging") pointing at a version_id.
#[async_trait]
pub trait TagStore: Send + Sync {
    /// Set (create or update) a tag for a flow version.
    async fn set_tag(
        &self,
        flow_id: &str,
        tag: &str,
        version_id: &str,
    ) -> Result<(), FlowStoreError>;

    /// Get the version_id for a tag. Returns `None` if tag doesn't exist.
    async fn get_tag(&self, flow_id: &str, tag: &str) -> Result<Option<String>, FlowStoreError>;

    /// List all tags for a flow.
    async fn list_tags(&self, flow_id: &str) -> Result<Vec<FlowTag>, FlowStoreError>;

    /// Delete a tag. Returns `true` if the tag existed.
    async fn delete_tag(&self, flow_id: &str, tag: &str) -> Result<bool, FlowStoreError>;

    /// Find all tags pointing at a specific version.
    async fn tags_for_version(&self, version_id: &str) -> Result<Vec<String>, FlowStoreError>;
}

// ---------------------------------------------------------------------------
// RunStore
// ---------------------------------------------------------------------------

/// Persistence for execution records. The authoritative source for replay.
///
/// `write_batch` must be all-or-nothing. Events carry `seq` numbers;
/// implementations should deduplicate on `(run_id, seq)`.
#[async_trait]
pub trait RunStore: Send + Sync {
    /// Write a batch of events. Must be all-or-nothing.
    async fn write_batch(&self, events: &[WriteEvent]) -> Result<(), RunStoreError>;

    /// Return the run summary (metadata, status, timing, aggregate LLM stats).
    /// Does **not** include the detailed event log — use [`events()`](Self::events)
    /// for that.
    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, RunStoreError>;

    async fn list_runs(&self, filter: &RunFilter) -> Result<RunPage, RunStoreError>;

    async fn get_node_results(&self, run_id: &str) -> Result<Vec<NodeRunResult>, RunStoreError>;

    async fn get_edge_results(&self, run_id: &str) -> Result<Vec<EdgeRunResult>, RunStoreError>;

    async fn get_node_detail(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeRunDetail>, RunStoreError>;

    async fn get_custom_events(
        &self,
        run_id: &str,
    ) -> Result<Vec<CustomEventRecord>, RunStoreError>;

    /// Retrieve all LLM invocations for a run, ordered by seq number.
    async fn get_llm_invocations(
        &self,
        run_id: &str,
    ) -> Result<Vec<LlmInvocationRecord>, RunStoreError>;

    /// Retrieve LLM invocations for a specific node.
    async fn get_llm_invocations_for_node(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Vec<LlmInvocationRecord>, RunStoreError>;

    /// Return the full event log for a run, ordered by seq.
    ///
    /// Each event is a [`WriteEvent`] — LLM calls, tool invocations, steps,
    /// topology hints, etc. Together with [`get_run()`](Self::get_run) this
    /// gives the complete picture needed by DiffEngine and ReplayEngine.
    ///
    /// Default: returns empty vec (override for stores that support it).
    async fn events(&self, _run_id: &str) -> Result<Vec<WriteEvent>, RunStoreError> {
        Ok(vec![])
    }

    /// Count runs grouped by version_id for a given flow.
    /// Default: returns empty map (override for stores that support it).
    async fn count_runs_by_version(
        &self,
        _flow_id: &str,
    ) -> Result<std::collections::BTreeMap<String, usize>, RunStoreError> {
        Ok(std::collections::BTreeMap::new())
    }

    /// Mark a run as interrupted (crash recovery).
    /// Writes a synthetic RunCompleted event with status Interrupted.
    async fn mark_interrupted(&self, run_id: &str) -> Result<(), RunStoreError> {
        let event = WriteEvent::RunCompleted {
            seq: u64::MAX,
            schema_version: super::types::WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.to_string(),
            status: RunStatus::Interrupted,
            duration_ms: 0,
            llm_summary: None,
            timestamp: chrono::Utc::now(),
        };
        self.write_batch(&[event]).await
    }
}

/// Filter criteria for listing runs.
#[derive(Debug, Clone, Default)]
pub struct RunFilter {
    pub flow_id: Option<String>,
    pub status: Option<RunStatus>,
    pub version_id: Option<String>,
    pub parent_run_id: Option<String>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

/// Paginated run listing result.
#[derive(Debug, Clone, Serialize)]
pub struct RunPage {
    pub runs: Vec<RunRecord>,
    pub total: usize,
}

/// Summary of a single node's execution within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct NodeRunResult {
    pub node_id: String,
    pub status: String,
    pub inputs: Value,
    pub outputs: Option<Value>,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
    pub attempts: u32,
}

/// Summary of a single edge evaluation within a run.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct EdgeRunResult {
    pub edge_id: String,
    pub condition: Option<String>,
    pub result: bool,
    pub data_summary: Value,
}

/// Detailed information about a specific node execution, including all
/// attempts and fan-out instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct NodeRunDetail {
    pub node_id: String,
    pub attempts: Vec<NodeAttemptDetail>,
}

/// A single execution attempt for a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct NodeAttemptDetail {
    pub attempt: u32,
    pub fan_out_index: Option<u32>,
    pub inputs: Value,
    pub outputs: Option<Value>,
    pub error: Option<String>,
    pub duration_ms: Option<u64>,
}

/// A custom event emitted by a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub struct CustomEventRecord {
    pub run_id: String,
    pub node_id: Option<String>,
    pub name: String,
    pub data: Value,
    pub seq: u64,
}

// ---------------------------------------------------------------------------
// Trigger
// ---------------------------------------------------------------------------

/// A source of flow invocations (HTTP webhook, cron schedule, flow chain).
///
/// Triggers run as long-lived tasks that emit [`TriggerEvent`]s when a flow
/// should be executed. The engine manages their lifecycle through
/// [`TriggerRunner`](crate::triggers::TriggerRunner).
#[async_trait]
pub trait Trigger: Send + Sync {
    fn trigger_type(&self) -> &str;
    fn description(&self) -> &str;
    fn config_schema(&self) -> Value;

    /// Start the trigger. Sends events to `tx` until `shutdown` fires.
    async fn start(
        &self,
        config: Value,
        tx: tokio::sync::mpsc::Sender<TriggerEvent>,
        shutdown: tokio::sync::broadcast::Receiver<()>,
    ) -> Result<(), TriggerError>;

    fn validate_config(&self, _config: &Value) -> Result<(), Vec<String>> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Noop ObservabilityProvider (useful for tests)
// ---------------------------------------------------------------------------

/// No-op observability provider that discards all telemetry.
pub struct NoopObservability;

impl ObservabilityProvider for NoopObservability {
    fn start_node_span(
        &self,
        run_id: &str,
        node_id: &str,
        attempt: u32,
        _meta: &NodeMeta,
    ) -> SpanHandle {
        SpanHandle {
            trace_id: run_id.to_string(),
            span_id: format!("{run_id}:{node_id}:{attempt}"),
            start_time: Instant::now(),
            inner: Box::new(()),
        }
    }

    fn record_io(&self, _span: &SpanHandle, _inputs: &Value, _outputs: &Value) {}
    fn record_edge(&self, _span: &SpanHandle, _edge: &Edge, _result: bool, _data: &Value) {}
    fn record_event(&self, _span: &SpanHandle, _name: &str, _data: &Value) {}
    fn record_error(&self, _span: &SpanHandle, _error: &NodeError) {}
    fn record_llm(&self, _span: &SpanHandle, _req: &LlmRequest, _resp: &LlmResponse) {}
    fn end_span(&self, _span: SpanHandle, _status: SpanStatus) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Verify NoopObservability satisfies the trait without panicking.
    #[test]
    fn noop_observability_works() {
        let noop = NoopObservability;
        let meta = NodeMeta {
            node_type: "test".into(),
            label: "Test".into(),
            category: "test".into(),
            inputs: vec![],
            outputs: vec![],
            config_schema: json!({}),
            ui: NodeUiHints::default(),
            execution: ExecutionHints::default(),
        };
        let span = noop.start_node_span("run-1", "node-1", 1, &meta);
        assert!(!span.trace_id.is_empty());
        assert!(!span.span_id.is_empty());
        noop.end_span(span, SpanStatus::Ok);
    }

    #[test]
    fn run_filter_defaults() {
        let f = RunFilter::default();
        assert!(f.flow_id.is_none());
        assert!(f.status.is_none());
        assert!(f.limit.is_none());
    }
}

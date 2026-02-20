//! Single-node execution with retry and timeout logic.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;
use tokio::sync::{broadcast, mpsc};

use super::edges::resolve_inputs;
use super::run::RunContext;
use crate::node_ctx::{HumanInputRegistry, NodeCtx, NodeEvent};
use crate::tool_registry::ToolRegistry;
use crate::traits::{
    FlowLlmProvider, NodeHandler, ObservabilityProvider, QueueProvider, SecretsProvider,
    SpanStatus, StateStore,
};
use crate::types::*;
use crate::write_event::WriteEvent;
use crate::write_handle::{WriteHandle, WriteHandleError};

// ---------------------------------------------------------------------------
// Node execution result
// ---------------------------------------------------------------------------

pub(super) struct NodeResult {
    pub node_id: String,
    pub fan_out_index: Option<u32>,
    pub outcome: NodeOutcome,
    pub llm_records: Vec<LlmInvocationRecord>,
    pub events: Vec<NodeEvent>,
}

pub(super) enum NodeOutcome {
    Success(Value),
    Failed(String),
}

// ---------------------------------------------------------------------------
// Spawn a single node execution as a tokio task
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_node(
    node_id: &str,
    ctx: &RunContext,
    node_outputs: &HashMap<String, Value>,
    incoming: &HashMap<String, Vec<&Edge>>,
    tool_registry: &ToolRegistry,
    fan_out_index: Option<u32>,
    override_inputs: Option<Value>,
) -> Option<tokio::task::JoinHandle<NodeResult>> {
    let node_instance = ctx.graph.nodes.iter().find(|n| n.instance_id == node_id)?;

    let handler = ctx.node_registry.get(&node_instance.node_type)?.clone();
    let meta = handler.meta();

    // Resolve inputs from upstream edges, or use override for fan-out.
    let inputs = override_inputs.unwrap_or_else(|| resolve_inputs(node_id, node_outputs, incoming));

    let run_id = ctx.run_id.clone();
    let node_id_owned = node_id.to_string();
    let config = node_instance.config.clone();
    let write_handle = ctx.session.write_handle().clone();
    let event_tx = ctx.session.event_sender().clone();
    let secrets = Arc::clone(&ctx.secrets);
    let state = Arc::clone(&ctx.state);
    let queue = Arc::clone(&ctx.queue);
    let observability = Arc::clone(&ctx.observability);
    let llm_providers = Arc::clone(&ctx.llm_providers);
    let http_client = ctx.http_client.clone();
    // Produce per-node tool snapshot, applying allowlist/permission filtering
    // when the `dynamic-tools` feature is enabled.
    let tool_defs = {
        #[cfg(feature = "dynamic-tools")]
        {
            let snap = if let Some(ref access) = node_instance.tool_access {
                tool_registry
                    .snapshot_filtered(access.allowed_tools.as_ref(), &access.granted_permissions)
            } else {
                tool_registry.snapshot()
            };
            Arc::new(snap)
        }
        #[cfg(not(feature = "dynamic-tools"))]
        {
            Arc::new(tool_registry.snapshot())
        }
    };
    let tool_registry = tool_registry.clone();
    let human_input_registry = Arc::clone(&ctx.human_input_registry);
    #[cfg(feature = "mcp")]
    let mcp_registry = Arc::clone(&ctx.mcp_registry);
    #[cfg(feature = "memory")]
    let memory_manager = Arc::clone(&ctx.memory_manager);
    #[cfg(feature = "memory")]
    let token_counter = Arc::clone(&ctx.token_counter);
    #[cfg(feature = "subflow")]
    let subflow_registry = Arc::clone(&ctx.subflow_registry);
    #[cfg(feature = "subflow")]
    let executor = ctx.executor.clone();

    let timeout_ms = meta.execution.timeout_ms;
    let retry = meta.execution.retry.clone();

    Some(tokio::spawn(async move {
        execute_node(
            run_id,
            node_id_owned,
            handler,
            meta,
            inputs,
            config,
            write_handle,
            event_tx,
            secrets,
            state,
            queue,
            observability,
            llm_providers,
            http_client,
            tool_defs,
            tool_registry,
            timeout_ms,
            retry,
            fan_out_index,
            human_input_registry,
            #[cfg(feature = "mcp")]
            mcp_registry,
            #[cfg(feature = "memory")]
            memory_manager,
            #[cfg(feature = "memory")]
            token_counter,
            #[cfg(feature = "subflow")]
            subflow_registry,
            #[cfg(feature = "subflow")]
            executor,
        )
        .await
    }))
}

/// Execute a single node with retry and timeout logic.
#[allow(clippy::too_many_arguments)]
async fn execute_node(
    run_id: String,
    node_id: String,
    handler: Arc<dyn NodeHandler>,
    meta: NodeMeta,
    inputs: Value,
    config: Value,
    write_handle: WriteHandle,
    event_tx: broadcast::Sender<WriteEvent>,
    secrets: Arc<dyn SecretsProvider>,
    state: Arc<dyn StateStore>,
    queue: Arc<dyn QueueProvider>,
    observability: Arc<dyn ObservabilityProvider>,
    llm_providers: Arc<HashMap<String, Arc<dyn FlowLlmProvider>>>,
    http_client: reqwest::Client,
    tool_defs: Arc<Vec<ToolDef>>,
    tool_registry: ToolRegistry,
    timeout_ms: u64,
    retry: RetryPolicy,
    fan_out_index: Option<u32>,
    human_input_registry: HumanInputRegistry,
    #[cfg(feature = "mcp")] mcp_registry: Arc<crate::mcp::McpServerRegistry>,
    #[cfg(feature = "memory")] memory_manager: Arc<dyn crate::memory::MemoryManager>,
    #[cfg(feature = "memory")] token_counter: Arc<dyn crate::memory::TokenCounter>,
    #[cfg(feature = "subflow")] subflow_registry: Arc<crate::subflow_registry::SubflowRegistry>,
    #[cfg(feature = "subflow")] executor: Option<Arc<super::Executor>>,
) -> NodeResult {
    let max_attempts = retry.max_attempts.max(1);
    let mut last_error = String::new();
    let mut all_llm_records = Vec::new();
    let mut all_events = Vec::new();

    for attempt in 1..=max_attempts {
        // Write NodeStarted (critical).
        let started = WriteEvent::NodeStarted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.clone(),
            node_id: node_id.clone(),
            inputs: inputs.clone(),
            fan_out_index,
            attempt,
            timestamp: Utc::now(),
        };
        let _ = emit_critical(&write_handle, &event_tx, started).await;

        // Start OTel span.
        let span = observability.start_node_span(&run_id, &node_id, attempt, &meta);

        // Create per-invocation channels.
        let (node_event_tx, mut node_event_rx) = mpsc::channel::<NodeEvent>(1000);
        let (llm_tx, mut llm_rx) = mpsc::channel::<LlmInvocationRecord>(1000);

        let node_ctx = NodeCtx::new(
            run_id.clone(),
            node_id.clone(),
            Arc::clone(&secrets),
            Arc::clone(&state),
            Arc::clone(&queue),
            node_event_tx,
            llm_tx,
            http_client.clone(),
            Arc::clone(&tool_defs),
            Some(Arc::clone(&human_input_registry)),
            Arc::clone(&llm_providers),
            #[cfg(feature = "mcp")]
            Arc::clone(&mcp_registry),
            #[cfg(feature = "memory")]
            Arc::clone(&memory_manager),
            #[cfg(feature = "memory")]
            Arc::clone(&token_counter),
            tool_registry.clone(),
            #[cfg(feature = "subflow")]
            Arc::clone(&subflow_registry),
            #[cfg(feature = "subflow")]
            executor.clone(),
        );

        let node_start = std::time::Instant::now();

        // Execute with timeout (E5).
        let result = tokio::time::timeout(
            Duration::from_millis(timeout_ms),
            handler.run(inputs.clone(), &config, &node_ctx),
        )
        .await;

        let duration_ms = node_start.elapsed().as_millis() as u64;

        // Drain LLM records and events.
        let mut llm_records = Vec::new();
        while let Ok(record) = llm_rx.try_recv() {
            llm_records.push(record);
        }
        all_llm_records.extend(llm_records.clone());

        let mut events = Vec::new();
        while let Ok(ev) = node_event_rx.try_recv() {
            events.push(ev);
        }
        all_events.extend(events.clone());

        match result {
            Ok(Ok(outputs)) => {
                observability.record_io(&span, &inputs, &outputs);
                observability.end_span(span, SpanStatus::Ok);

                // Write NodeCompleted (critical).
                let completed = WriteEvent::NodeCompleted {
                    seq: 0,
                    schema_version: WRITE_EVENT_SCHEMA_VERSION,
                    run_id: run_id.clone(),
                    node_id: node_id.clone(),
                    outputs: outputs.clone(),
                    fan_out_index,
                    duration_ms,
                    timestamp: Utc::now(),
                };
                let _ = emit_critical(&write_handle, &event_tx, completed).await;

                // Write Checkpoint (critical, E4).
                let checkpoint = WriteEvent::Checkpoint {
                    seq: 0,
                    schema_version: WRITE_EVENT_SCHEMA_VERSION,
                    run_id: run_id.clone(),
                    node_id: node_id.clone(),
                    state_snapshot: outputs.clone(),
                    timestamp: Utc::now(),
                };
                let _ = emit_critical(&write_handle, &event_tx, checkpoint).await;

                return NodeResult {
                    node_id,
                    fan_out_index,
                    outcome: NodeOutcome::Success(outputs),
                    llm_records: all_llm_records,
                    events: all_events,
                };
            }
            Ok(Err(node_error)) => {
                let will_retry =
                    matches!(&node_error, NodeError::Retryable { .. }) && attempt < max_attempts;
                let error_msg = node_error.to_string();

                observability.record_error(&span, &node_error);
                observability.end_span(span, SpanStatus::Error(error_msg.clone()));

                // Write NodeFailed (critical).
                let failed_event = WriteEvent::NodeFailed {
                    seq: 0,
                    schema_version: WRITE_EVENT_SCHEMA_VERSION,
                    run_id: run_id.clone(),
                    node_id: node_id.clone(),
                    error: error_msg.clone(),
                    will_retry,
                    attempt,
                    timestamp: Utc::now(),
                };
                let _ = emit_critical(&write_handle, &event_tx, failed_event).await;

                last_error = error_msg;

                if will_retry {
                    // Exponential backoff.
                    let backoff = retry.backoff_ms as f64
                        * retry.backoff_multiplier.powi((attempt - 1) as i32);
                    tokio::time::sleep(Duration::from_millis(backoff as u64)).await;
                    continue;
                }

                return NodeResult {
                    node_id,
                    fan_out_index,
                    outcome: NodeOutcome::Failed(last_error),
                    llm_records: all_llm_records,
                    events: all_events,
                };
            }
            Err(_elapsed) => {
                let error = NodeError::Timeout {
                    elapsed_ms: timeout_ms,
                };
                let error_msg = error.to_string();

                observability.record_error(&span, &error);
                observability.end_span(span, SpanStatus::Timeout);

                let failed_event = WriteEvent::NodeFailed {
                    seq: 0,
                    schema_version: WRITE_EVENT_SCHEMA_VERSION,
                    run_id: run_id.clone(),
                    node_id: node_id.clone(),
                    error: error_msg.clone(),
                    will_retry: false,
                    attempt,
                    timestamp: Utc::now(),
                };
                let _ = emit_critical(&write_handle, &event_tx, failed_event).await;

                return NodeResult {
                    node_id,
                    fan_out_index,
                    outcome: NodeOutcome::Failed(error_msg),
                    llm_records: all_llm_records,
                    events: all_events,
                };
            }
        }
    }

    NodeResult {
        node_id,
        fan_out_index,
        outcome: NodeOutcome::Failed(last_error),
        llm_records: all_llm_records,
        events: all_events,
    }
}

// ---------------------------------------------------------------------------
// Event emission helpers
// ---------------------------------------------------------------------------

pub(super) async fn emit_critical(
    write_handle: &WriteHandle,
    broadcast: &broadcast::Sender<WriteEvent>,
    event: WriteEvent,
) -> Result<(), WriteHandleError> {
    let _ = broadcast.send(event.clone());
    write_handle.write(event).await
}

//! Core execution loop — runs a flow graph as a DAG of tokio tasks.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::oneshot;

use super::edges::{
    all_reachable_done, build_incoming_edges, build_outgoing_edges, evaluate_edges_and_find_ready,
    find_back_edges, find_entry_nodes, find_ready_after_failure,
};
use super::fanout::{handle_fan_out_completion, spawn_ready_nodes, FanOutState};
use super::node::{spawn_node, NodeOutcome, NodeResult};
use crate::node_ctx::HumanInputRegistry;
use crate::runtime::ExecutionSession;
use crate::tool_registry::ToolRegistry;
use crate::traits::{
    FlowLlmProvider, NodeHandler, ObservabilityProvider, QueueProvider, SecretsProvider, StateStore,
};
use crate::types::*;
use crate::write_event::WriteEvent;

// ---------------------------------------------------------------------------
// Internal execution context
// ---------------------------------------------------------------------------

pub(super) struct RunContext {
    pub run_id: String,
    pub graph: GraphDef,
    pub inputs: Value,
    pub session: ExecutionSession,
    pub cancel_rx: oneshot::Receiver<()>,
    pub node_registry: HashMap<String, Arc<dyn NodeHandler>>,
    pub secrets: Arc<dyn SecretsProvider>,
    pub state: Arc<dyn StateStore>,
    pub queue: Arc<dyn QueueProvider>,
    pub observability: Arc<dyn ObservabilityProvider>,
    pub llm_providers: Arc<HashMap<String, Arc<dyn FlowLlmProvider>>>,
    pub http_client: reqwest::Client,
    pub max_traversals: u32,
    pub human_input_registry: HumanInputRegistry,
    #[cfg(feature = "mcp")]
    pub mcp_registry: Arc<crate::mcp::McpServerRegistry>,
}

// ---------------------------------------------------------------------------
// Core execution loop
// ---------------------------------------------------------------------------

pub(super) async fn execute_run(mut ctx: RunContext) -> (String, RunStatus) {
    let run_id = ctx.run_id.clone();
    // Session already emitted RunStarted in start().

    // Build adjacency data structures.
    let back_edge_ids = find_back_edges(&ctx.graph);
    let entry_nodes = find_entry_nodes(&ctx.graph, &back_edge_ids);
    let outgoing = build_outgoing_edges(&ctx.graph);
    let incoming = build_incoming_edges(&ctx.graph);

    // Execution state.
    let mut completed: HashSet<String> = HashSet::new();
    let mut failed: HashSet<String> = HashSet::new();
    let mut node_outputs: HashMap<String, Value> = HashMap::new();
    let mut edge_traversal_counts: HashMap<String, u32> = HashMap::new();
    let mut llm_records: Vec<LlmInvocationRecord> = Vec::new();
    let mut fan_out_states: HashMap<String, FanOutState> = HashMap::new();
    let mut cancelled = false;

    // Tool registry — single source of truth for available tools.
    // Populated from graph-defined tools and MCP-discovered tools.
    #[allow(unused_mut)]
    let mut all_tools: Vec<ToolDef> = ctx.graph.tool_definitions.values().cloned().collect();

    #[cfg(feature = "mcp")]
    {
        if !ctx.mcp_registry.is_empty() {
            match ctx.mcp_registry.discover_all_tools().await {
                Ok(mcp_tools) => all_tools.extend(mcp_tools),
                Err(e) => {
                    tracing::warn!(error = %e, "failed to discover MCP tools");
                }
            }
        }
    }

    let tool_registry = ToolRegistry::from_tools(all_tools);

    // Seed entry nodes with the trigger inputs.
    for entry in &entry_nodes {
        node_outputs.insert(entry.clone(), ctx.inputs.clone());
    }

    // Parallel execution via FuturesUnordered.
    let mut running: FuturesUnordered<tokio::task::JoinHandle<NodeResult>> =
        FuturesUnordered::new();

    // Spawn initially ready nodes with trigger inputs directly (bypass
    // resolve_inputs which would see back-edges with no upstream output yet).
    for node_id in &entry_nodes {
        if let Some(task) = spawn_node(
            node_id,
            &ctx,
            &node_outputs,
            &incoming,
            &tool_registry,
            None,
            Some(ctx.inputs.clone()),
        ) {
            running.push(task);
        }
    }

    let mut running_set: HashSet<String> = entry_nodes.iter().cloned().collect();
    let all_node_ids: HashSet<String> = ctx
        .graph
        .nodes
        .iter()
        .map(|n| n.instance_id.clone())
        .collect();

    loop {
        // Check cancellation.
        if check_cancel(&mut ctx.cancel_rx) {
            cancelled = true;
            break;
        }

        if running.is_empty() {
            break;
        }

        tokio::select! {
            Some(join_result) = running.next() => {
                let result = match join_result {
                    Ok(r) => r,
                    Err(e) => {
                        // Task panicked — treat as fatal.
                        NodeResult {
                            node_id: "unknown".into(),
                            fan_out_index: None,
                            outcome: NodeOutcome::Failed(format!("task panic: {e}")),
                            llm_records: vec![],
                            events: vec![],
                        }
                    }
                };

                // Collect LLM records.
                llm_records.extend(result.llm_records.clone());

                // Write LLM invocation events (critical).
                for record in &result.llm_records {
                    let event = WriteEvent::LlmInvocation {
                        seq: 0,
                        schema_version: WRITE_EVENT_SCHEMA_VERSION,
                        run_id: ctx.run_id.clone(),
                        node_id: record.node_id.clone(),
                        request: Box::new(record.request.clone()),
                        response: Box::new(record.response.clone()),
                        timestamp: record.timestamp,
                    };
                    let _ = ctx.session.record_event(event).await;
                }

                // Write custom events and stream chunks (advisory).
                for event in &result.events {
                    if event.name == "__llm_chunk" {
                        // Convert sentinel NodeEvent to WriteEvent::LlmStreamChunk.
                        if let Ok(chunk) =
                            serde_json::from_value::<LlmChunk>(event.data.clone())
                        {
                            let we = WriteEvent::LlmStreamChunk {
                                seq: 0,
                                schema_version: WRITE_EVENT_SCHEMA_VERSION,
                                run_id: ctx.run_id.clone(),
                                node_id: result.node_id.clone(),
                                chunk,
                                timestamp: Utc::now(),
                            };
                            let _ = ctx.session.record_advisory(we);
                        }
                    } else {
                        let we = WriteEvent::Custom {
                            seq: 0,
                            schema_version: WRITE_EVENT_SCHEMA_VERSION,
                            run_id: ctx.run_id.clone(),
                            node_id: Some(result.node_id.clone()),
                            name: event.name.clone(),
                            data: event.data.clone(),
                            timestamp: Utc::now(),
                        };
                        let _ = ctx.session.record_advisory(we);
                    }
                }

                // Determine if downstream evaluation should happen.
                // The bool indicates whether the output came from fan-in
                // (to prevent cascading fan-out on the merged array).
                let evaluate_downstream: Option<(String, Value, bool)> =
                    if let Some(foi) = result.fan_out_index {
                        // Fan-out instance completed.
                        handle_fan_out_completion(
                            &result.node_id,
                            foi,
                            &result.outcome,
                            &mut fan_out_states,
                            &mut completed,
                            &mut node_outputs,
                        )
                        .map(|(id, val)| (id, val, true))
                    } else {
                        // Normal node completed.
                        running_set.remove(&result.node_id);
                        match &result.outcome {
                            NodeOutcome::Success(outputs) => {
                                completed.insert(result.node_id.clone());
                                node_outputs.insert(result.node_id.clone(), outputs.clone());
                                Some((result.node_id.clone(), outputs.clone(), false))
                            }
                            NodeOutcome::Failed(_) => {
                                failed.insert(result.node_id.clone());
                                let newly_ready = find_ready_after_failure(
                                    &result.node_id,
                                    &ctx,
                                    &outgoing,
                                    &incoming,
                                    &completed,
                                    &failed,
                                    &running_set,
                                );
                                for nid in newly_ready {
                                    if let Some(task) = spawn_node(
                                        &nid, &ctx, &node_outputs, &incoming,
                                        &tool_registry, None, None,
                                    ) {
                                        running_set.insert(nid);
                                        running.push(task);
                                    }
                                }
                                None
                            }
                        }
                    };

                // Process and strip runtime tool changes from node output.
                #[cfg(feature = "dynamic-tools")]
                let evaluate_downstream = evaluate_downstream.map(|(id, mut outputs, fan_in)| {
                    process_tool_changes(&mut outputs, &tool_registry);
                    // Update stored output to exclude tool change keys.
                    node_outputs.insert(id.clone(), outputs.clone());
                    (id, outputs, fan_in)
                });

                if let Some((completed_node_id, outputs, from_fan_in)) = evaluate_downstream {
                    let newly_ready = evaluate_edges_and_find_ready(
                        &completed_node_id,
                        &outputs,
                        &ctx,
                        &outgoing,
                        &incoming,
                        &mut completed,
                        &failed,
                        &running_set,
                        &mut edge_traversal_counts,
                        &node_outputs,
                        &back_edge_ids,
                        &fan_out_states,
                        from_fan_in,
                    )
                    .await;

                    spawn_ready_nodes(
                        newly_ready,
                        &ctx,
                        &node_outputs,
                        &incoming,
                        &tool_registry,
                        &mut running,
                        &mut running_set,
                        &mut completed,
                        &mut fan_out_states,
                    );
                }
            }
        }
    }

    // Determine final status.
    let done_count = completed.len() + failed.len();
    let status = if cancelled {
        RunStatus::Cancelled
    } else if done_count < all_node_ids.len()
        && running_set.is_empty()
        && !all_reachable_done(
            &all_node_ids,
            &completed,
            &failed,
            &incoming,
            &outgoing,
            &edge_traversal_counts,
            ctx.max_traversals,
        )
    {
        // Deadlock: no running, no ready, not all done.
        RunStatus::Failed
    } else if failed.is_empty() {
        RunStatus::Completed
    } else {
        RunStatus::Failed
    };

    let llm_summary = compute_llm_summary(&llm_records);
    let summary = if llm_records.is_empty() {
        None
    } else {
        Some(llm_summary)
    };

    let _ = ctx.session.finish(status.clone(), summary).await;
    (run_id, status)
}

// ---------------------------------------------------------------------------
// Runtime tool changes (dynamic-tools feature)
// ---------------------------------------------------------------------------

/// Process `_tool_changes` from node output and apply to the registry.
///
/// Expected format:
/// ```json
/// { "_tool_changes": { "add": [ToolDef...], "remove": ["tool_name"] } }
/// ```
///
/// The `_tool_changes` key is stripped from the output after processing.
#[cfg(feature = "dynamic-tools")]
fn process_tool_changes(outputs: &mut Value, registry: &ToolRegistry) {
    let changes = match outputs
        .as_object_mut()
        .and_then(|o| o.remove("_tool_changes"))
    {
        Some(v) => v,
        None => return,
    };

    // Process additions.
    if let Some(adds) = changes.get("add").and_then(|v| v.as_array()) {
        for tool_json in adds {
            match serde_json::from_value::<ToolDef>(tool_json.clone()) {
                Ok(tool_def) => {
                    let perms = tool_def.permissions.clone();
                    registry.register(tool_def, perms);
                }
                Err(e) => {
                    tracing::warn!(error = %e, "invalid tool in _tool_changes.add, skipping");
                }
            }
        }
    }

    // Process removals.
    if let Some(removes) = changes.get("remove").and_then(|v| v.as_array()) {
        for name in removes {
            if let Some(name_str) = name.as_str() {
                if !registry.remove(name_str) {
                    tracing::debug!(tool = name_str, "tool not found for removal");
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cancellation
// ---------------------------------------------------------------------------

fn check_cancel(cancel_rx: &mut oneshot::Receiver<()>) -> bool {
    matches!(
        cancel_rx.try_recv(),
        Ok(()) | Err(oneshot::error::TryRecvError::Closed)
    )
}

// ---------------------------------------------------------------------------
// LLM summary computation
// ---------------------------------------------------------------------------

fn compute_llm_summary(records: &[LlmInvocationRecord]) -> LlmRunSummary {
    let mut total_input_tokens: u64 = 0;
    let mut total_output_tokens: u64 = 0;
    let mut total_cost: f64 = 0.0;
    let mut has_cost = false;
    let mut models: HashSet<String> = HashSet::new();
    let mut tools: HashSet<String> = HashSet::new();

    for record in records {
        if let Some(t) = record.response.input_tokens {
            total_input_tokens += u64::from(t);
        }
        if let Some(t) = record.response.output_tokens {
            total_output_tokens += u64::from(t);
        }
        if let Some(ref cost) = record.response.cost {
            total_cost += cost.total_cost_usd;
            has_cost = true;
        }
        models.insert(record.request.model.clone());
        if let Some(ref tool_calls) = record.response.tool_calls {
            for tc in tool_calls {
                tools.insert(tc.tool_name.clone());
            }
        }
    }

    let mut models_used: Vec<String> = models.into_iter().collect();
    models_used.sort();
    let mut tools_invoked: Vec<String> = tools.into_iter().collect();
    tools_invoked.sort();

    LlmRunSummary {
        total_llm_calls: records.len() as u32,
        total_input_tokens,
        total_output_tokens,
        total_cost_usd: if has_cost { Some(total_cost) } else { None },
        models_used,
        tools_invoked,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    fn make_tool(name: &str) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: format!("Tool {name}"),
            parameters: serde_json::json!({"type": "object"}),
            tool_type: ToolType::Node {
                target_node_id: format!("{name}-node"),
            },
            metadata: BTreeMap::new(),
            permissions: BTreeSet::new(),
        }
    }

    #[cfg(feature = "dynamic-tools")]
    #[test]
    fn tool_changes_add_and_remove() {
        let registry = ToolRegistry::from_tools(vec![make_tool("existing")]);
        assert_eq!(registry.len(), 1);

        let mut outputs = serde_json::json!({
            "result": "ok",
            "_tool_changes": {
                "add": [
                    {
                        "name": "new_tool",
                        "description": "A new tool",
                        "parameters": {"type": "object"},
                        "tool_type": {"kind": "node", "target_node_id": "n1"}
                    }
                ],
                "remove": ["existing"]
            }
        });

        process_tool_changes(&mut outputs, &registry);

        // _tool_changes should be stripped from output.
        assert!(outputs.get("_tool_changes").is_none());
        assert_eq!(outputs.get("result").unwrap(), "ok");

        // Registry should reflect changes.
        assert_eq!(registry.len(), 1);
        assert!(registry.get("new_tool").is_some());
        assert!(registry.get("existing").is_none());
    }

    #[cfg(feature = "dynamic-tools")]
    #[test]
    fn tool_changes_no_op_when_absent() {
        let registry = ToolRegistry::from_tools(vec![make_tool("search")]);
        let mut outputs = serde_json::json!({"result": "ok"});

        process_tool_changes(&mut outputs, &registry);

        assert_eq!(registry.len(), 1);
        assert_eq!(outputs, serde_json::json!({"result": "ok"}));
    }

    #[cfg(feature = "dynamic-tools")]
    #[test]
    fn tool_changes_invalid_tool_skipped() {
        let registry = ToolRegistry::new();
        let mut outputs = serde_json::json!({
            "_tool_changes": {
                "add": [{"invalid": true}]
            }
        });

        process_tool_changes(&mut outputs, &registry);
        assert!(registry.is_empty());
    }

    #[cfg(feature = "dynamic-tools")]
    #[test]
    fn tool_changes_remove_nonexistent_no_panic() {
        let registry = ToolRegistry::new();
        let mut outputs = serde_json::json!({
            "_tool_changes": {
                "remove": ["ghost"]
            }
        });

        process_tool_changes(&mut outputs, &registry);
        assert!(registry.is_empty());
    }
}

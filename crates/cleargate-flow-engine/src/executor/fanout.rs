//! Fan-out / fan-in helpers and ready-node spawning.

use std::collections::{HashMap, HashSet};

use futures::stream::FuturesUnordered;
use serde_json::Value;

use super::node::{spawn_node, NodeOutcome, NodeResult};
use super::run::RunContext;
use crate::tool_registry::ToolRegistry;
use crate::types::*;

/// Tracks the state of fan-out instances for a single node.
pub(super) struct FanOutState {
    pub expected: u32,
    pub results: Vec<Option<Value>>,
    pub completed: u32,
}

/// A node that's ready to be spawned, possibly with fan-out.
pub(super) struct ReadyNode {
    pub node_id: String,
    pub fan_out_items: Option<Vec<Value>>,
    pub is_cycle: bool,
}

/// Handle a completed fan-out instance. Returns `Some((node_id, merged_output))`
/// when all instances are done (fan-in complete).
pub(super) fn handle_fan_out_completion(
    node_id: &str,
    fan_out_index: u32,
    outcome: &NodeOutcome,
    fan_out_states: &mut HashMap<String, FanOutState>,
    completed: &mut HashSet<String>,
    node_outputs: &mut HashMap<String, Value>,
) -> Option<(String, Value)> {
    let state = fan_out_states.get_mut(node_id)?;
    let idx = fan_out_index as usize;

    match outcome {
        NodeOutcome::Success(val) => {
            state.results[idx] = Some(val.clone());
        }
        NodeOutcome::Failed(err) => {
            state.results[idx] = Some(serde_json::json!({
                "error": err,
                "fan_out_index": fan_out_index,
            }));
        }
    }
    state.completed += 1;

    if state.completed == state.expected {
        // Fan-in: collect all results sorted by index.
        let merged: Vec<Value> = state
            .results
            .iter()
            .map(|r| r.clone().unwrap_or(Value::Null))
            .collect();
        let merged_output = Value::Array(merged);

        completed.insert(node_id.to_string());
        node_outputs.insert(node_id.to_string(), merged_output.clone());
        fan_out_states.remove(node_id);

        Some((node_id.to_string(), merged_output))
    } else {
        None
    }
}

/// Spawn all ready nodes, handling fan-out and cycle re-execution.
#[allow(clippy::too_many_arguments)]
pub(super) fn spawn_ready_nodes(
    ready_nodes: Vec<ReadyNode>,
    ctx: &RunContext,
    node_outputs: &HashMap<String, Value>,
    incoming: &HashMap<String, Vec<&Edge>>,
    tool_registry: &ToolRegistry,
    running: &mut FuturesUnordered<tokio::task::JoinHandle<NodeResult>>,
    running_set: &mut HashSet<String>,
    completed: &mut HashSet<String>,
    fan_out_states: &mut HashMap<String, FanOutState>,
) {
    for ready in ready_nodes {
        if ready.is_cycle {
            completed.remove(&ready.node_id);
        }

        if let Some(items) = ready.fan_out_items {
            // Fan-out: spawn N instances.
            let count = items.len() as u32;
            fan_out_states.insert(
                ready.node_id.clone(),
                FanOutState {
                    expected: count,
                    results: vec![None; count as usize],
                    completed: 0,
                },
            );
            for (i, item) in items.into_iter().enumerate() {
                if let Some(task) = spawn_node(
                    &ready.node_id,
                    ctx,
                    node_outputs,
                    incoming,
                    tool_registry,
                    Some(i as u32),
                    Some(item),
                ) {
                    running.push(task);
                }
            }
        } else {
            // Normal spawn.
            if let Some(task) = spawn_node(
                &ready.node_id,
                ctx,
                node_outputs,
                incoming,
                tool_registry,
                None,
                None,
            ) {
                running_set.insert(ready.node_id);
                running.push(task);
            }
        }
    }
}

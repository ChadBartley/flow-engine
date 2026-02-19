//! Graph analysis helpers and edge evaluation.

use std::collections::{HashMap, HashSet};

use chrono::Utc;
use serde_json::Value;

use super::run::RunContext;
use crate::expression;
use crate::types::*;
use crate::write_event::WriteEvent;

use super::fanout::{FanOutState, ReadyNode};

// ---------------------------------------------------------------------------
// Back-edge detection (cycle support)
// ---------------------------------------------------------------------------

/// Find back-edges via DFS. A back-edge points to a node already on the
/// current recursion stack, identifying exactly one edge per cycle.
pub(super) fn find_back_edges(graph: &GraphDef) -> HashSet<String> {
    let mut back_edges = HashSet::new();
    let mut visited = HashSet::new();
    let mut in_stack = HashSet::new();

    // Build adjacency: node → outgoing edges.
    let mut outgoing: HashMap<&str, Vec<&Edge>> = HashMap::new();
    for edge in &graph.edges {
        outgoing
            .entry(edge.from_node.as_str())
            .or_default()
            .push(edge);
    }

    // True entry nodes: no incoming edges at all.
    let nodes_with_incoming: HashSet<&str> =
        graph.edges.iter().map(|e| e.to_node.as_str()).collect();
    let start_nodes: Vec<&str> = graph
        .nodes
        .iter()
        .filter(|n| !nodes_with_incoming.contains(n.instance_id.as_str()))
        .map(|n| n.instance_id.as_str())
        .collect();

    if start_nodes.is_empty() {
        // No true entry nodes — pure cycle. Visit all nodes to find back-edges.
        for node in &graph.nodes {
            if !visited.contains(node.instance_id.as_str()) {
                dfs_back_edges(
                    &node.instance_id,
                    &outgoing,
                    &mut visited,
                    &mut in_stack,
                    &mut back_edges,
                );
            }
        }
    } else {
        // DFS only from true entry nodes. Unreachable subgraphs stay
        // unvisited and produce no back-edges, preserving deadlock detection.
        for start in &start_nodes {
            dfs_back_edges(
                start,
                &outgoing,
                &mut visited,
                &mut in_stack,
                &mut back_edges,
            );
        }
    }

    back_edges
}

fn dfs_back_edges(
    node: &str,
    outgoing: &HashMap<&str, Vec<&Edge>>,
    visited: &mut HashSet<String>,
    in_stack: &mut HashSet<String>,
    back_edges: &mut HashSet<String>,
) {
    if visited.contains(node) {
        return;
    }
    visited.insert(node.to_string());
    in_stack.insert(node.to_string());

    if let Some(edges) = outgoing.get(node) {
        for edge in edges {
            if in_stack.contains(edge.to_node.as_str()) {
                back_edges.insert(edge.id.clone());
            } else if !visited.contains(edge.to_node.as_str()) {
                dfs_back_edges(&edge.to_node, outgoing, visited, in_stack, back_edges);
            }
        }
    }

    in_stack.remove(node);
}

// ---------------------------------------------------------------------------
// Graph analysis helpers
// ---------------------------------------------------------------------------

/// Find nodes with no incoming non-back-edge edges (entry points).
pub(super) fn find_entry_nodes(graph: &GraphDef, back_edge_ids: &HashSet<String>) -> Vec<String> {
    let nodes_with_incoming: HashSet<&str> = graph
        .edges
        .iter()
        .filter(|e| !back_edge_ids.contains(&e.id))
        .map(|e| e.to_node.as_str())
        .collect();
    graph
        .nodes
        .iter()
        .filter(|n| !nodes_with_incoming.contains(n.instance_id.as_str()))
        .map(|n| n.instance_id.clone())
        .collect()
}

/// Build a map: from_node -> vec of outgoing edges.
pub(super) fn build_outgoing_edges(graph: &GraphDef) -> HashMap<String, Vec<&Edge>> {
    let mut map: HashMap<String, Vec<&Edge>> = HashMap::new();
    for edge in &graph.edges {
        map.entry(edge.from_node.clone()).or_default().push(edge);
    }
    map
}

/// Build a map: to_node -> vec of incoming edges.
pub(super) fn build_incoming_edges(graph: &GraphDef) -> HashMap<String, Vec<&Edge>> {
    let mut map: HashMap<String, Vec<&Edge>> = HashMap::new();
    for edge in &graph.edges {
        map.entry(edge.to_node.clone()).or_default().push(edge);
    }
    map
}

/// Resolve inputs for a node from upstream node outputs.
pub(super) fn resolve_inputs(
    node_id: &str,
    node_outputs: &HashMap<String, Value>,
    incoming: &HashMap<String, Vec<&Edge>>,
) -> Value {
    let edges = match incoming.get(node_id) {
        Some(e) => e,
        None => return Value::Object(serde_json::Map::new()),
    };

    let mut merged = serde_json::Map::new();
    for edge in edges {
        if let Some(upstream_output) = node_outputs.get(&edge.from_node) {
            // Use the to_port name as the key.
            let value = if let Some(port_val) = upstream_output.get(&edge.from_port) {
                port_val.clone()
            } else {
                upstream_output.clone()
            };
            merged.insert(edge.to_port.clone(), value);
        }
    }

    if merged.len() == 1 {
        // Single input — unwrap for convenience.
        merged
            .into_iter()
            .next()
            .map(|(_, v)| v)
            .unwrap_or(Value::Null)
    } else if merged.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        Value::Object(merged)
    }
}

// ---------------------------------------------------------------------------
// Edge evaluation
// ---------------------------------------------------------------------------

/// Resolve the value flowing through an edge from node output.
pub(super) fn resolve_edge_value(outputs: &Value, from_port: &str) -> Value {
    if let Some(port_val) = outputs.get(from_port) {
        port_val.clone()
    } else {
        outputs.clone()
    }
}

/// Evaluate outgoing edges from a completed node and find newly ready nodes.
/// Returns `ReadyNode` with fan-out and cycle information.
///
/// `from_fan_in`: when `true`, array outputs are passed through as-is
/// instead of triggering another fan-out.
#[allow(clippy::too_many_arguments)]
pub(super) async fn evaluate_edges_and_find_ready<'a>(
    completed_node: &str,
    outputs: &Value,
    ctx: &RunContext,
    outgoing: &HashMap<String, Vec<&'a Edge>>,
    incoming: &HashMap<String, Vec<&'a Edge>>,
    completed: &mut HashSet<String>,
    failed: &HashSet<String>,
    running: &HashSet<String>,
    edge_traversal_counts: &mut HashMap<String, u32>,
    node_outputs: &HashMap<String, Value>,
    back_edge_ids: &HashSet<String>,
    fan_out_states: &HashMap<String, FanOutState>,
    from_fan_in: bool,
) -> Vec<ReadyNode> {
    let mut ready = Vec::new();
    let mut seen = HashSet::new();

    let edges = match outgoing.get(completed_node) {
        Some(e) => e,
        None => return ready,
    };

    for edge in edges {
        // Evaluate edge condition.
        let result = evaluate_edge_condition(edge, outputs);

        // Check traversal limit for cycles (E1).
        let count = edge_traversal_counts.entry(edge.id.clone()).or_insert(0);
        let within_limit = *count < ctx.max_traversals;

        let passed = result && within_limit;

        if passed {
            *count += 1;
        }

        // Write EdgeEvaluated event (critical).
        let edge_event = WriteEvent::EdgeEvaluated {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: ctx.run_id.clone(),
            edge_id: edge.id.clone(),
            condition: edge.condition.as_ref().map(|c| c.expression.clone()),
            result: passed,
            data_summary: outputs.clone(),
            timestamp: Utc::now(),
        };
        let _ = ctx.session.record_event(edge_event).await;

        if passed {
            let target = &edge.to_node;

            // Prevent duplicate spawns for the same target.
            if !seen.insert(target.clone()) {
                continue;
            }

            let is_fan_out_running = fan_out_states
                .get(target.as_str())
                .is_some_and(|s| s.completed < s.expected);

            if is_fan_out_running || running.contains(target) {
                continue;
            }

            if completed.contains(target) {
                // Back-edge: target already completed → cycle re-execution.
                // Fan-out still applies on cycles (e.g. multi-tool calls
                // on the second+ iteration of an agent loop).
                let fan_out_items = if from_fan_in {
                    None
                } else {
                    match resolve_edge_value(outputs, &edge.from_port) {
                        Value::Array(items) if !items.is_empty() => Some(items),
                        _ => None,
                    }
                };
                ready.push(ReadyNode {
                    node_id: target.clone(),
                    fan_out_items,
                    is_cycle: true,
                });
            } else if !failed.contains(target)
                && is_node_ready(
                    target,
                    incoming,
                    completed,
                    failed,
                    node_outputs,
                    outgoing,
                    edge_traversal_counts,
                    ctx.max_traversals,
                    back_edge_ids,
                )
            {
                // Fan-out: trigger when edge value is an array, but NOT
                // when the output came from fan-in (avoid cascading).
                let fan_out_items = if from_fan_in {
                    None
                } else {
                    match resolve_edge_value(outputs, &edge.from_port) {
                        Value::Array(items) if !items.is_empty() => Some(items),
                        _ => None,
                    }
                };
                ready.push(ReadyNode {
                    node_id: target.clone(),
                    fan_out_items,
                    is_cycle: false,
                });
            }
        }
    }

    ready
}

/// Check if a node is ready to execute: all upstream sources (excluding
/// back-edge sources) have completed (or failed), and at least one
/// incoming edge's condition evaluated to true.
#[allow(clippy::too_many_arguments)]
pub(super) fn is_node_ready(
    node_id: &str,
    incoming: &HashMap<String, Vec<&Edge>>,
    completed: &HashSet<String>,
    failed: &HashSet<String>,
    node_outputs: &HashMap<String, Value>,
    _outgoing: &HashMap<String, Vec<&Edge>>,
    edge_traversal_counts: &HashMap<String, u32>,
    max_traversals: u32,
    back_edge_ids: &HashSet<String>,
) -> bool {
    let edges = match incoming.get(node_id) {
        Some(e) => e,
        None => return true, // Entry node — always ready.
    };

    if edges.is_empty() {
        return true;
    }

    // A node is ready when all its non-back-edge incoming edge sources
    // have completed (or failed) AND at least one edge's condition
    // evaluated to true.
    let mut has_passing_edge = false;

    for edge in edges {
        // Skip back-edges — they don't block initial execution.
        if back_edge_ids.contains(&edge.id) {
            continue;
        }

        let source_done = completed.contains(&edge.from_node) || failed.contains(&edge.from_node);
        if !source_done {
            return false; // Upstream not done yet.
        }

        // Check if this edge has been traversed (condition passed).
        if completed.contains(&edge.from_node) {
            if let Some(outputs) = node_outputs.get(&edge.from_node) {
                let cond_result = evaluate_edge_condition(edge, outputs);
                let count = edge_traversal_counts.get(&edge.id).copied().unwrap_or(0);
                if cond_result && count <= max_traversals {
                    has_passing_edge = true;
                }
            }
        }
    }

    has_passing_edge
}

/// Evaluate an edge's condition against node output data.
fn evaluate_edge_condition(edge: &Edge, data: &Value) -> bool {
    match &edge.condition {
        None => true,
        Some(cond) => expression::evaluate(&cond.expression, data).unwrap_or(false),
    }
}

/// After a node fails, check if any downstream nodes can still proceed
/// (E6: partial failure = continue).
pub(super) fn find_ready_after_failure<'a>(
    failed_node: &str,
    ctx: &RunContext,
    outgoing: &HashMap<String, Vec<&'a Edge>>,
    incoming: &HashMap<String, Vec<&'a Edge>>,
    completed: &HashSet<String>,
    failed: &HashSet<String>,
    running: &HashSet<String>,
) -> Vec<String> {
    let mut ready = Vec::new();
    let edges = match outgoing.get(failed_node) {
        Some(e) => e,
        None => return ready,
    };

    for edge in edges {
        let target = &edge.to_node;
        if completed.contains(target) || failed.contains(target) || running.contains(target) {
            continue;
        }

        // Check if the target has other incoming edges from completed nodes
        // that could satisfy it. For now, skip nodes that depend solely on
        // the failed node — they cannot run.
        if let Some(target_incoming) = incoming.get(target.as_str()) {
            let all_sources_done = target_incoming
                .iter()
                .all(|e| completed.contains(&e.from_node) || failed.contains(&e.from_node));
            let has_completed_source = target_incoming
                .iter()
                .any(|e| completed.contains(&e.from_node));
            if all_sources_done && has_completed_source {
                ready.push(target.clone());
            }
        }
    }

    let _ = ctx; // used for run_id in future expansions
    ready
}

// ---------------------------------------------------------------------------
// Deadlock / completion detection
// ---------------------------------------------------------------------------

/// Check if all reachable nodes from entry points have been processed.
pub(super) fn all_reachable_done(
    all_nodes: &HashSet<String>,
    completed: &HashSet<String>,
    failed: &HashSet<String>,
    _incoming: &HashMap<String, Vec<&Edge>>,
    _outgoing: &HashMap<String, Vec<&Edge>>,
    _edge_counts: &HashMap<String, u32>,
    _max_traversals: u32,
) -> bool {
    // Simple check: every node is either completed or failed.
    all_nodes
        .iter()
        .all(|n| completed.contains(n) || failed.contains(n))
}

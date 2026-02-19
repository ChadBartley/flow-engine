//! Shared reconstruction logic for building RunStore query results from events.
//!
//! Both `FileRunStore` and `SeaOrmRunStore` use event sourcing — this module
//! provides the common fold logic to reconstruct `RunRecord`, `NodeRunResult`,
//! etc. from a `Vec<WriteEvent>`.

use std::collections::HashMap;

use serde_json::Value;

use crate::traits::{
    CustomEventRecord, EdgeRunResult, NodeAttemptDetail, NodeRunDetail, NodeRunResult,
};
use crate::types::{LlmInvocationRecord, RunRecord, RunStatus};
use crate::write_event::WriteEvent;

/// Reconstruct a `RunRecord` from ordered events.
/// Returns `None` if events is empty.
pub fn reconstruct_run(run_id: &str, events: &[WriteEvent]) -> Option<RunRecord> {
    if events.is_empty() {
        return None;
    }

    let mut record = RunRecord {
        run_id: run_id.to_string(),
        ..RunRecord::default()
    };

    for event in events {
        match event {
            WriteEvent::RunStarted {
                flow_id,
                version_id,
                trigger_inputs,
                triggered_by,
                config_hash,
                parent_run_id,
                timestamp,
                ..
            } => {
                record.flow_id = flow_id.clone();
                record.version_id = version_id.clone();
                record.inputs = trigger_inputs.clone();
                record.triggered_by = triggered_by.clone();
                record.config_hash = config_hash.clone();
                record.parent_run_id = parent_run_id.clone();
                record.started_at = Some(*timestamp);
                record.status = RunStatus::Running;
            }
            WriteEvent::RunCompleted {
                status,
                duration_ms,
                llm_summary,
                timestamp,
                ..
            } => {
                record.status = status.clone();
                record.duration_ms = Some(*duration_ms);
                record.llm_summary = llm_summary.clone();
                record.completed_at = Some(*timestamp);
            }
            _ => {}
        }
    }

    Some(record)
}

/// Extract node results from events.
pub fn extract_node_results(events: &[WriteEvent]) -> Vec<NodeRunResult> {
    let mut nodes: HashMap<String, NodeRunResult> = HashMap::new();

    for event in events {
        match event {
            WriteEvent::NodeStarted {
                node_id,
                inputs,
                attempt,
                ..
            } => {
                let entry = nodes
                    .entry(node_id.clone())
                    .or_insert_with(|| NodeRunResult {
                        node_id: node_id.clone(),
                        status: "running".into(),
                        inputs: Value::Null,
                        outputs: None,
                        error: None,
                        duration_ms: None,
                        attempts: 0,
                    });
                entry.inputs = inputs.clone();
                entry.attempts = entry.attempts.max(*attempt);
            }
            WriteEvent::NodeCompleted {
                node_id,
                outputs,
                duration_ms,
                ..
            } => {
                if let Some(entry) = nodes.get_mut(node_id) {
                    entry.status = "completed".into();
                    entry.outputs = Some(outputs.clone());
                    entry.duration_ms = Some(*duration_ms);
                }
            }
            WriteEvent::NodeFailed {
                node_id,
                error,
                will_retry,
                ..
            } => {
                if let Some(entry) = nodes.get_mut(node_id) {
                    if !will_retry {
                        entry.status = "failed".into();
                        entry.error = Some(error.clone());
                    }
                }
            }
            _ => {}
        }
    }

    nodes.into_values().collect()
}

/// Extract edge results from events.
pub fn extract_edge_results(events: &[WriteEvent]) -> Vec<EdgeRunResult> {
    let mut results = Vec::new();

    for event in events {
        if let WriteEvent::EdgeEvaluated {
            edge_id,
            condition,
            result,
            data_summary,
            ..
        } = event
        {
            results.push(EdgeRunResult {
                edge_id: edge_id.clone(),
                condition: condition.clone(),
                result: *result,
                data_summary: data_summary.clone(),
            });
        }
    }

    results
}

/// Extract node detail (all attempts) for a specific node.
pub fn extract_node_detail(events: &[WriteEvent], node_id: &str) -> Option<NodeRunDetail> {
    // Collect attempts in order. A node can run multiple times in cycles
    // (same attempt number, same fan_out_index), so we track each
    // NodeStarted → NodeCompleted/NodeFailed pair sequentially.
    let mut attempts: Vec<NodeAttemptDetail> = Vec::new();

    for event in events {
        match event {
            WriteEvent::NodeStarted {
                node_id: nid,
                inputs,
                fan_out_index,
                attempt,
                ..
            } if nid == node_id => {
                attempts.push(NodeAttemptDetail {
                    attempt: *attempt,
                    fan_out_index: *fan_out_index,
                    inputs: inputs.clone(),
                    outputs: None,
                    error: None,
                    duration_ms: None,
                });
            }
            WriteEvent::NodeCompleted {
                node_id: nid,
                outputs,
                fan_out_index,
                duration_ms,
                ..
            } if nid == node_id => {
                // Match the most recent unfinished attempt with this fan_out_index.
                if let Some(detail) = attempts.iter_mut().rev().find(|d| {
                    d.fan_out_index == *fan_out_index && d.outputs.is_none() && d.error.is_none()
                }) {
                    detail.outputs = Some(outputs.clone());
                    detail.duration_ms = Some(*duration_ms);
                }
            }
            WriteEvent::NodeFailed {
                node_id: nid,
                error,
                ..
            } if nid == node_id => {
                // Match the most recent unfinished attempt.
                if let Some(detail) = attempts
                    .iter_mut()
                    .rev()
                    .find(|d| d.outputs.is_none() && d.error.is_none())
                {
                    detail.error = Some(error.clone());
                }
            }
            _ => {}
        }
    }

    if attempts.is_empty() {
        return None;
    }

    Some(NodeRunDetail {
        node_id: node_id.to_string(),
        attempts,
    })
}

/// Extract custom events.
pub fn extract_custom_events(events: &[WriteEvent]) -> Vec<CustomEventRecord> {
    let mut records = Vec::new();

    for event in events {
        if let WriteEvent::Custom {
            run_id,
            node_id,
            name,
            data,
            seq,
            ..
        } = event
        {
            records.push(CustomEventRecord {
                run_id: run_id.clone(),
                node_id: node_id.clone(),
                name: name.clone(),
                data: data.clone(),
                seq: *seq,
            });
        }
    }

    records
}

/// Extract LLM invocations.
pub fn extract_llm_invocations(events: &[WriteEvent]) -> Vec<LlmInvocationRecord> {
    let mut records = Vec::new();

    for event in events {
        if let WriteEvent::LlmInvocation {
            run_id,
            node_id,
            request,
            response,
            timestamp,
            ..
        } = event
        {
            records.push(LlmInvocationRecord {
                run_id: run_id.clone(),
                node_id: node_id.clone(),
                request: *request.clone(),
                response: *response.clone(),
                timestamp: *timestamp,
            });
        }
    }

    records
}

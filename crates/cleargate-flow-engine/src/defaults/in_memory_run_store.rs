//! In-memory run store for testing and lightweight usage.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::RwLock;

use crate::errors::RunStoreError;
use crate::traits::{
    CustomEventRecord, EdgeRunResult, NodeAttemptDetail, NodeRunDetail, NodeRunResult, RunFilter,
    RunPage, RunStore,
};
use crate::types::{LlmInvocationRecord, RunRecord, RunStatus};
use crate::write_event::WriteEvent;

/// In-memory implementation of [`RunStore`].
///
/// Uses `BTreeMap` for deterministic iteration order (project convention).
/// Suitable for tests and short-lived processes.
pub struct InMemoryRunStore {
    runs: Arc<RwLock<BTreeMap<String, RunRecord>>>,
    events: Arc<RwLock<BTreeMap<String, Vec<WriteEvent>>>>,
}

impl InMemoryRunStore {
    pub fn new() -> Self {
        Self {
            runs: Arc::new(RwLock::new(BTreeMap::new())),
            events: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }
}

impl Default for InMemoryRunStore {
    fn default() -> Self {
        Self::new()
    }
}

fn reconstruct_run(run_id: &str, events: &[WriteEvent]) -> RunRecord {
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

    record
}

#[async_trait]
impl RunStore for InMemoryRunStore {
    async fn write_batch(&self, new_events: &[WriteEvent]) -> Result<(), RunStoreError> {
        let mut by_run: HashMap<String, Vec<&WriteEvent>> = HashMap::new();
        for event in new_events {
            by_run
                .entry(event.run_id().to_string())
                .or_default()
                .push(event);
        }

        let mut events_map = self.events.write().await;
        let mut runs_map = self.runs.write().await;

        for (run_id, batch) in by_run {
            let stored = events_map.entry(run_id.clone()).or_default();
            let existing_seqs: HashSet<u64> = stored.iter().map(|e| e.seq()).collect();

            for ev in batch {
                if !existing_seqs.contains(&ev.seq()) {
                    stored.push(ev.clone());
                }
            }
            stored.sort_by_key(|e| e.seq());

            let record = reconstruct_run(&run_id, stored);
            runs_map.insert(run_id, record);
        }

        Ok(())
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, RunStoreError> {
        Ok(self.runs.read().await.get(run_id).cloned())
    }

    async fn list_runs(&self, filter: &RunFilter) -> Result<RunPage, RunStoreError> {
        let runs_map = self.runs.read().await;
        let mut runs: Vec<RunRecord> = runs_map
            .values()
            .filter(|r| {
                if let Some(ref fid) = filter.flow_id {
                    if r.flow_id != *fid {
                        return false;
                    }
                }
                if let Some(ref status) = filter.status {
                    if r.status != *status {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();

        let total = runs.len();
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(runs.len());
        runs = runs.into_iter().skip(offset).take(limit).collect();

        Ok(RunPage { runs, total })
    }

    async fn get_node_results(&self, run_id: &str) -> Result<Vec<NodeRunResult>, RunStoreError> {
        let events_map = self.events.read().await;
        let events = match events_map.get(run_id) {
            Some(e) => e,
            None => return Ok(vec![]),
        };

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

        Ok(nodes.into_values().collect())
    }

    async fn get_edge_results(&self, run_id: &str) -> Result<Vec<EdgeRunResult>, RunStoreError> {
        let events_map = self.events.read().await;
        let events = match events_map.get(run_id) {
            Some(e) => e,
            None => return Ok(vec![]),
        };

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

        Ok(results)
    }

    async fn get_node_detail(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeRunDetail>, RunStoreError> {
        let events_map = self.events.read().await;
        let events = match events_map.get(run_id) {
            Some(e) => e,
            None => return Ok(None),
        };

        let mut attempts: HashMap<(u32, Option<u32>), NodeAttemptDetail> = HashMap::new();

        for event in events {
            match event {
                WriteEvent::NodeStarted {
                    node_id: nid,
                    inputs,
                    fan_out_index,
                    attempt,
                    ..
                } if nid == node_id => {
                    let key = (*attempt, *fan_out_index);
                    attempts.entry(key).or_insert_with(|| NodeAttemptDetail {
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
                    for detail in attempts.values_mut() {
                        if detail.fan_out_index == *fan_out_index {
                            detail.outputs = Some(outputs.clone());
                            detail.duration_ms = Some(*duration_ms);
                        }
                    }
                }
                WriteEvent::NodeFailed {
                    node_id: nid,
                    error,
                    attempt,
                    ..
                } if nid == node_id => {
                    if let Some(detail) = attempts.get_mut(&(*attempt, None)) {
                        detail.error = Some(error.clone());
                    }
                }
                _ => {}
            }
        }

        if attempts.is_empty() {
            return Ok(None);
        }

        let mut attempt_list: Vec<NodeAttemptDetail> = attempts.into_values().collect();
        attempt_list.sort_by_key(|a| (a.attempt, a.fan_out_index));

        Ok(Some(NodeRunDetail {
            node_id: node_id.to_string(),
            attempts: attempt_list,
        }))
    }

    async fn get_custom_events(
        &self,
        run_id: &str,
    ) -> Result<Vec<CustomEventRecord>, RunStoreError> {
        let events_map = self.events.read().await;
        let events = match events_map.get(run_id) {
            Some(e) => e,
            None => return Ok(vec![]),
        };

        let mut records = Vec::new();
        for event in events {
            if let WriteEvent::Custom {
                run_id: rid,
                node_id,
                name,
                data,
                seq,
                ..
            } = event
            {
                records.push(CustomEventRecord {
                    run_id: rid.clone(),
                    node_id: node_id.clone(),
                    name: name.clone(),
                    data: data.clone(),
                    seq: *seq,
                });
            }
        }

        Ok(records)
    }

    async fn get_llm_invocations(
        &self,
        run_id: &str,
    ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
        let events_map = self.events.read().await;
        let events = match events_map.get(run_id) {
            Some(e) => e,
            None => return Ok(vec![]),
        };

        let mut records = Vec::new();
        for event in events {
            if let WriteEvent::LlmInvocation {
                run_id: rid,
                node_id,
                request,
                response,
                timestamp,
                ..
            } = event
            {
                records.push(LlmInvocationRecord {
                    run_id: rid.clone(),
                    node_id: node_id.clone(),
                    request: *request.clone(),
                    response: *response.clone(),
                    timestamp: *timestamp,
                });
            }
        }

        Ok(records)
    }

    async fn get_llm_invocations_for_node(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
        let all = self.get_llm_invocations(run_id).await?;
        Ok(all.into_iter().filter(|r| r.node_id == node_id).collect())
    }

    async fn events(&self, run_id: &str) -> Result<Vec<WriteEvent>, RunStoreError> {
        let events_map = self.events.read().await;
        Ok(events_map.get(run_id).cloned().unwrap_or_default())
    }
}

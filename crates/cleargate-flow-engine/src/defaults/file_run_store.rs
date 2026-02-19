//! File-system backed run store using JSONL event files.
//!
//! Layout:
//! ```text
//! {base_dir}/runs/{run_id}/events.jsonl
//! ```
//!
//! Each line is a JSON-serialized [`WriteEvent`]. Writes are all-or-nothing
//! via temp-file-then-rename, with deduplication on `(run_id, seq)`.

use std::collections::{HashMap, HashSet};
use std::io::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;

use super::reconstruct;
use crate::errors::RunStoreError;
use crate::traits::{
    CustomEventRecord, EdgeRunResult, NodeRunDetail, NodeRunResult, RunFilter, RunPage, RunStore,
};
use crate::types::{LlmInvocationRecord, RunRecord};
use crate::write_event::WriteEvent;

/// File-system backed store for execution records.
///
/// Events are stored as JSONL (one JSON object per line) in
/// `{base_dir}/runs/{run_id}/events.jsonl`. Writes are all-or-nothing
/// via a temp-file-then-rename pattern with fsync.
pub struct FileRunStore {
    base_dir: PathBuf,
}

impl FileRunStore {
    /// Create a new `FileRunStore` rooted at `base_dir`.
    ///
    /// Creates `{base_dir}/runs/` if it doesn't exist.
    pub fn new(base_dir: PathBuf) -> Result<Self, RunStoreError> {
        let runs_dir = base_dir.join("runs");
        std::fs::create_dir_all(&runs_dir).map_err(|e| RunStoreError::Store {
            message: format!("failed to create runs directory: {e}"),
        })?;
        Ok(Self { base_dir })
    }

    fn events_path(&self, run_id: &str) -> PathBuf {
        self.base_dir.join("runs").join(run_id).join("events.jsonl")
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.base_dir.join("runs").join(run_id)
    }

    /// Read all events for a run, ordered by seq.
    fn read_events(&self, run_id: &str) -> Result<Vec<WriteEvent>, RunStoreError> {
        let path = self.events_path(run_id);
        if !path.exists() {
            return Ok(vec![]);
        }
        let content = std::fs::read_to_string(&path).map_err(|e| RunStoreError::Store {
            message: format!("failed to read events file: {e}"),
        })?;
        let mut events = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let event: WriteEvent =
                serde_json::from_str(line).map_err(|e| RunStoreError::Store {
                    message: format!("failed to deserialize event: {e}"),
                })?;
            events.push(event);
        }
        events.sort_by_key(|e| e.seq());
        Ok(events)
    }

    /// Reconstruct a `RunRecord` from the events of a run.
    fn reconstruct_run(&self, run_id: &str) -> Result<Option<RunRecord>, RunStoreError> {
        let events = self.read_events(run_id)?;
        Ok(reconstruct::reconstruct_run(run_id, &events))
    }
}

#[async_trait]
impl RunStore for FileRunStore {
    async fn write_batch(&self, events: &[WriteEvent]) -> Result<(), RunStoreError> {
        // Group incoming events by run_id.
        let mut by_run: HashMap<String, Vec<&WriteEvent>> = HashMap::new();
        for event in events {
            by_run
                .entry(event.run_id().to_string())
                .or_default()
                .push(event);
        }

        for (run_id, new_events) in by_run {
            let run_dir = self.run_dir(&run_id);
            std::fs::create_dir_all(&run_dir).map_err(|e| RunStoreError::Store {
                message: format!("failed to create run directory: {e}"),
            })?;

            let events_path = self.events_path(&run_id);

            // Read existing events for dedup.
            let existing = self.read_events(&run_id)?;
            let existing_seqs: HashSet<u64> = existing.iter().map(|e| e.seq()).collect();

            // Filter out duplicates.
            let deduped: Vec<&WriteEvent> = new_events
                .into_iter()
                .filter(|e| !existing_seqs.contains(&e.seq()))
                .collect();

            if deduped.is_empty() {
                continue;
            }

            // All-or-nothing: write existing + new to temp file, then rename.
            let temp_path = events_path.with_extension("jsonl.tmp");
            let mut file = std::fs::File::create(&temp_path).map_err(|e| RunStoreError::Store {
                message: format!("failed to create temp file: {e}"),
            })?;

            // Write existing events.
            for event in &existing {
                let line = serde_json::to_string(event).map_err(|e| RunStoreError::Store {
                    message: format!("failed to serialize event: {e}"),
                })?;
                writeln!(file, "{line}").map_err(|e| RunStoreError::Store {
                    message: format!("failed to write event: {e}"),
                })?;
            }

            // Write new events.
            for event in &deduped {
                let line = serde_json::to_string(event).map_err(|e| RunStoreError::Store {
                    message: format!("failed to serialize event: {e}"),
                })?;
                writeln!(file, "{line}").map_err(|e| RunStoreError::Store {
                    message: format!("failed to write event: {e}"),
                })?;
            }

            file.sync_all().map_err(|e| RunStoreError::Store {
                message: format!("failed to fsync: {e}"),
            })?;
            drop(file);

            std::fs::rename(&temp_path, &events_path).map_err(|e| RunStoreError::Store {
                message: format!("failed to rename temp file: {e}"),
            })?;
        }

        Ok(())
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, RunStoreError> {
        self.reconstruct_run(run_id)
    }

    async fn list_runs(&self, filter: &RunFilter) -> Result<RunPage, RunStoreError> {
        let runs_dir = self.base_dir.join("runs");
        if !runs_dir.exists() {
            return Ok(RunPage {
                runs: vec![],
                total: 0,
            });
        }

        let entries = std::fs::read_dir(&runs_dir).map_err(|e| RunStoreError::Store {
            message: format!("failed to read runs directory: {e}"),
        })?;

        let mut runs = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| RunStoreError::Store {
                message: format!("failed to read dir entry: {e}"),
            })?;
            if !entry.path().is_dir() {
                continue;
            }
            let run_id = entry.file_name().to_string_lossy().to_string();
            if let Some(record) = self.reconstruct_run(&run_id)? {
                // Apply filters.
                if let Some(ref fid) = filter.flow_id {
                    if record.flow_id != *fid {
                        continue;
                    }
                }
                if let Some(ref status) = filter.status {
                    if record.status != *status {
                        continue;
                    }
                }
                if let Some(ref vid) = filter.version_id {
                    if record.version_id != *vid {
                        continue;
                    }
                }
                if let Some(ref pid) = filter.parent_run_id {
                    if record.parent_run_id.as_ref() != Some(pid) {
                        continue;
                    }
                }
                runs.push(record);
            }
        }

        let total = runs.len();
        let offset = filter.offset.unwrap_or(0);
        let limit = filter.limit.unwrap_or(runs.len());
        let page = runs.into_iter().skip(offset).take(limit).collect();

        Ok(RunPage { runs: page, total })
    }

    async fn get_node_results(&self, run_id: &str) -> Result<Vec<NodeRunResult>, RunStoreError> {
        let events = self.read_events(run_id)?;
        Ok(reconstruct::extract_node_results(&events))
    }

    async fn get_edge_results(&self, run_id: &str) -> Result<Vec<EdgeRunResult>, RunStoreError> {
        let events = self.read_events(run_id)?;
        Ok(reconstruct::extract_edge_results(&events))
    }

    async fn get_node_detail(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeRunDetail>, RunStoreError> {
        let events = self.read_events(run_id)?;
        Ok(reconstruct::extract_node_detail(&events, node_id))
    }

    async fn get_custom_events(
        &self,
        run_id: &str,
    ) -> Result<Vec<CustomEventRecord>, RunStoreError> {
        let events = self.read_events(run_id)?;
        Ok(reconstruct::extract_custom_events(&events))
    }

    async fn get_llm_invocations(
        &self,
        run_id: &str,
    ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
        let events = self.read_events(run_id)?;
        Ok(reconstruct::extract_llm_invocations(&events))
    }

    async fn get_llm_invocations_for_node(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
        let all = self.get_llm_invocations(run_id).await?;
        Ok(all.into_iter().filter(|r| r.node_id == node_id).collect())
    }

    async fn count_runs_by_version(
        &self,
        flow_id: &str,
    ) -> Result<std::collections::BTreeMap<String, usize>, RunStoreError> {
        let runs_dir = self.base_dir.join("runs");
        if !runs_dir.exists() {
            return Ok(std::collections::BTreeMap::new());
        }

        let mut counts = std::collections::BTreeMap::new();
        let entries = std::fs::read_dir(&runs_dir).map_err(|e| RunStoreError::Store {
            message: format!("failed to read runs directory: {e}"),
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| RunStoreError::Store {
                message: format!("failed to read dir entry: {e}"),
            })?;
            if !entry.path().is_dir() {
                continue;
            }
            let run_id = entry.file_name().to_string_lossy().to_string();
            if let Some(record) = self.reconstruct_run(&run_id)? {
                if record.flow_id == flow_id {
                    *counts.entry(record.version_id).or_insert(0) += 1;
                }
            }
        }
        Ok(counts)
    }

    async fn events(&self, run_id: &str) -> Result<Vec<WriteEvent>, RunStoreError> {
        self.read_events(run_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        LlmRequest, LlmResponse, RunStatus, TriggerSource, WRITE_EVENT_SCHEMA_VERSION,
    };
    use chrono::Utc;
    use serde_json::json;
    use std::collections::BTreeMap;

    fn make_run_started(run_id: &str, seq: u64) -> WriteEvent {
        WriteEvent::RunStarted {
            seq,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.into(),
            flow_id: "flow-1".into(),
            version_id: "v1".into(),
            trigger_inputs: json!({"input": "data"}),
            triggered_by: TriggerSource::Manual {
                principal: "test".into(),
            },
            config_hash: Some("hash123".into()),
            parent_run_id: None,
            timestamp: Utc::now(),
        }
    }

    fn make_run_completed(run_id: &str, seq: u64) -> WriteEvent {
        WriteEvent::RunCompleted {
            seq,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.into(),
            status: RunStatus::Completed,
            duration_ms: 500,
            llm_summary: None,
            timestamp: Utc::now(),
        }
    }

    fn make_node_started(run_id: &str, node_id: &str, seq: u64) -> WriteEvent {
        WriteEvent::NodeStarted {
            seq,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.into(),
            node_id: node_id.into(),
            inputs: json!({"x": 1}),
            fan_out_index: None,
            attempt: 1,
            timestamp: Utc::now(),
        }
    }

    fn make_node_completed(run_id: &str, node_id: &str, seq: u64) -> WriteEvent {
        WriteEvent::NodeCompleted {
            seq,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.into(),
            node_id: node_id.into(),
            outputs: json!({"y": 2}),
            fan_out_index: None,
            duration_ms: 100,
            timestamp: Utc::now(),
        }
    }

    fn make_edge_evaluated(run_id: &str, edge_id: &str, seq: u64) -> WriteEvent {
        WriteEvent::EdgeEvaluated {
            seq,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.into(),
            edge_id: edge_id.into(),
            condition: Some("finish_reason == 'stop'".into()),
            result: true,
            data_summary: json!({}),
            timestamp: Utc::now(),
        }
    }

    fn make_llm_invocation(run_id: &str, node_id: &str, seq: u64) -> WriteEvent {
        WriteEvent::LlmInvocation {
            seq,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.into(),
            node_id: node_id.into(),
            request: Box::new(LlmRequest {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                messages: json!([{"role": "user", "content": "hi"}]),
                tools: None,
                temperature: Some(0.7),
                top_p: None,
                max_tokens: None,
                stop_sequences: None,
                response_format: None,
                seed: None,
                extra_params: BTreeMap::new(),
            }),
            response: Box::new(LlmResponse {
                content: json!("hello"),
                tool_calls: None,
                model_used: "gpt-4o".into(),
                input_tokens: Some(10),
                output_tokens: Some(5),
                total_tokens: Some(15),
                finish_reason: "stop".into(),
                latency_ms: 200,
                provider_request_id: None,
                cost: None,
            }),
            timestamp: Utc::now(),
        }
    }

    fn make_custom_event(run_id: &str, seq: u64) -> WriteEvent {
        WriteEvent::Custom {
            seq,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: run_id.into(),
            node_id: Some("node-1".into()),
            name: "my_event".into(),
            data: json!({"key": "value"}),
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_write_batch_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        let events = vec![
            make_run_started("run-1", 1),
            make_node_started("run-1", "node-a", 2),
            make_node_completed("run-1", "node-a", 3),
            make_run_completed("run-1", 4),
        ];
        store.write_batch(&events).await.unwrap();

        let record = store.get_run("run-1").await.unwrap().unwrap();
        assert_eq!(record.run_id, "run-1");
        assert_eq!(record.flow_id, "flow-1");
        assert_eq!(record.status, RunStatus::Completed);
        assert!(record.config_hash.is_some());
    }

    #[tokio::test]
    async fn test_idempotent_write() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        let events = vec![make_run_started("run-1", 1), make_run_completed("run-1", 2)];
        store.write_batch(&events).await.unwrap();
        store.write_batch(&events).await.unwrap(); // duplicate

        // Verify no duplicate events.
        let all_events = store.read_events("run-1").unwrap();
        assert_eq!(all_events.len(), 2);
    }

    #[tokio::test]
    async fn test_get_node_results() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        let events = vec![
            make_run_started("run-1", 1),
            make_node_started("run-1", "node-a", 2),
            make_node_completed("run-1", "node-a", 3),
            make_node_started("run-1", "node-b", 4),
            WriteEvent::NodeFailed {
                seq: 5,
                schema_version: WRITE_EVENT_SCHEMA_VERSION,
                run_id: "run-1".into(),
                node_id: "node-b".into(),
                error: "something broke".into(),
                will_retry: false,
                attempt: 1,
                timestamp: Utc::now(),
            },
        ];
        store.write_batch(&events).await.unwrap();

        let results = store.get_node_results("run-1").await.unwrap();
        assert_eq!(results.len(), 2);

        let a = results.iter().find(|r| r.node_id == "node-a").unwrap();
        assert_eq!(a.status, "completed");
        assert!(a.outputs.is_some());

        let b = results.iter().find(|r| r.node_id == "node-b").unwrap();
        assert_eq!(b.status, "failed");
        assert_eq!(b.error.as_deref(), Some("something broke"));
    }

    #[tokio::test]
    async fn test_get_edge_results() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        let events = vec![
            make_run_started("run-1", 1),
            make_edge_evaluated("run-1", "edge-1", 2),
            make_edge_evaluated("run-1", "edge-2", 3),
        ];
        store.write_batch(&events).await.unwrap();

        let results = store.get_edge_results("run-1").await.unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|r| r.edge_id == "edge-1"));
        assert!(results.iter().any(|r| r.edge_id == "edge-2"));
    }

    #[tokio::test]
    async fn test_get_llm_invocations() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        let events = vec![
            make_run_started("run-1", 1),
            make_llm_invocation("run-1", "llm-1", 2),
            make_llm_invocation("run-1", "llm-2", 3),
            make_llm_invocation("run-1", "llm-1", 4),
        ];
        store.write_batch(&events).await.unwrap();

        let invocations = store.get_llm_invocations("run-1").await.unwrap();
        assert_eq!(invocations.len(), 3);
        // Ordered by seq.
        assert_eq!(invocations[0].node_id, "llm-1");
        assert_eq!(invocations[1].node_id, "llm-2");
        assert_eq!(invocations[2].node_id, "llm-1");
    }

    #[tokio::test]
    async fn test_get_llm_invocations_for_node() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        let events = vec![
            make_run_started("run-1", 1),
            make_llm_invocation("run-1", "llm-1", 2),
            make_llm_invocation("run-1", "llm-2", 3),
            make_llm_invocation("run-1", "llm-1", 4),
        ];
        store.write_batch(&events).await.unwrap();

        let invocations = store
            .get_llm_invocations_for_node("run-1", "llm-1")
            .await
            .unwrap();
        assert_eq!(invocations.len(), 2);
        assert!(invocations.iter().all(|r| r.node_id == "llm-1"));
    }

    #[tokio::test]
    async fn test_get_custom_events() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        let events = vec![make_run_started("run-1", 1), make_custom_event("run-1", 2)];
        store.write_batch(&events).await.unwrap();

        let customs = store.get_custom_events("run-1").await.unwrap();
        assert_eq!(customs.len(), 1);
        assert_eq!(customs[0].name, "my_event");
        assert_eq!(customs[0].seq, 2);
    }

    #[tokio::test]
    async fn test_missing_run_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();
        assert!(store.get_run("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_corrupted_jsonl_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileRunStore::new(dir.path().to_path_buf()).unwrap();

        // Write valid events first.
        let events = vec![make_run_started("run-corrupt", 1)];
        store.write_batch(&events).await.unwrap();

        // Manually inject a corrupted line into the events file.
        let events_path = store.events_path("run-corrupt");
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&events_path)
            .unwrap();
        writeln!(file, "{{this is not valid json}}").unwrap();

        // Reading should return an error (not silently skip).
        let result = store.get_run("run-corrupt").await;
        assert!(
            result.is_err(),
            "corrupted JSONL should produce an error, got: {result:?}"
        );
    }

    #[tokio::test]
    async fn test_concurrent_event_writes() {
        let dir = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(FileRunStore::new(dir.path().to_path_buf()).unwrap());

        // First, create the run.
        store
            .write_batch(&[make_run_started("run-conc", 1)])
            .await
            .unwrap();

        // 10 tasks appending node events concurrently with unique seq numbers.
        let mut handles = Vec::new();
        for i in 0..10u32 {
            let store = store.clone();
            handles.push(tokio::spawn(async move {
                let node_id = format!("node-{i}");
                let seq = (i as u64) + 10; // Unique seqs starting at 10.
                let events = vec![
                    make_node_started("run-conc", &node_id, seq * 2),
                    make_node_completed("run-conc", &node_id, seq * 2 + 1),
                ];
                store.write_batch(&events).await.unwrap();
            }));
        }

        for h in handles {
            h.await.unwrap();
        }

        // All events should be present (1 RunStarted + 10*2 node events = 21).
        let all_events = store.read_events("run-conc").unwrap();
        assert_eq!(
            all_events.len(),
            21,
            "expected 21 events (1 start + 20 node), got {}",
            all_events.len()
        );

        // All 10 nodes should have results.
        let results = store.get_node_results("run-conc").await.unwrap();
        assert_eq!(results.len(), 10, "all 10 nodes should have results");
    }
}

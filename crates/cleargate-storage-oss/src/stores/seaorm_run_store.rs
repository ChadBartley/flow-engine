//! SeaORM-backed run store using event sourcing.
//!
//! Events are stored in the `run_event` table as JSON payloads.
//! Reconstruction uses the shared `reconstruct` module from `cleargate-flow-engine`.

use async_trait::async_trait;
use sea_orm::entity::prelude::*;
use sea_orm::{ActiveValue, DatabaseConnection, QueryOrder, QuerySelect};
use std::sync::Arc;

use crate::models::run_event;

use cleargate_flow_engine::defaults::reconstruct;
use cleargate_flow_engine::errors::RunStoreError;
use cleargate_flow_engine::traits::{
    CustomEventRecord, EdgeRunResult, NodeRunDetail, NodeRunResult, RunFilter, RunPage, RunStore,
};
use cleargate_flow_engine::types::{LlmInvocationRecord, RunRecord};
use cleargate_flow_engine::write_event::WriteEvent;

/// Database-backed store for execution records using event sourcing.
pub struct SeaOrmRunStore {
    db: Arc<DatabaseConnection>,
}

impl SeaOrmRunStore {
    pub fn new(db: Arc<DatabaseConnection>) -> Self {
        Self { db }
    }
}

fn to_store_err(e: impl std::fmt::Display) -> RunStoreError {
    RunStoreError::Store {
        message: e.to_string(),
    }
}

impl SeaOrmRunStore {
    /// Read all events for a run, ordered by seq.
    async fn read_events(&self, run_id: &str) -> Result<Vec<WriteEvent>, RunStoreError> {
        let models = run_event::Entity::find()
            .filter(run_event::Column::RunId.eq(run_id))
            .order_by_asc(run_event::Column::Seq)
            .all(self.db.as_ref())
            .await
            .map_err(to_store_err)?;

        let mut events = Vec::with_capacity(models.len());
        for m in models {
            let event: WriteEvent = serde_json::from_value(m.payload).map_err(to_store_err)?;
            events.push(event);
        }
        Ok(events)
    }
}

#[async_trait]
impl RunStore for SeaOrmRunStore {
    async fn write_batch(&self, events: &[WriteEvent]) -> Result<(), RunStoreError> {
        for event in events {
            let payload = serde_json::to_value(event).map_err(to_store_err)?;
            #[allow(unreachable_patterns)]
            let event_type = match event {
                WriteEvent::RunStarted { .. } => "run_started",
                WriteEvent::RunCompleted { .. } => "run_completed",
                WriteEvent::NodeStarted { .. } => "node_started",
                WriteEvent::NodeCompleted { .. } => "node_completed",
                WriteEvent::NodeFailed { .. } => "node_failed",
                WriteEvent::EdgeEvaluated { .. } => "edge_evaluated",
                WriteEvent::Checkpoint { .. } => "checkpoint",
                WriteEvent::LlmInvocation { .. } => "llm_invocation",
                WriteEvent::LlmStreamChunk { .. } => "llm_stream_chunk",
                WriteEvent::ToolInvocation { .. } => "tool_invocation",
                WriteEvent::StepRecorded { .. } => "step_recorded",
                WriteEvent::Custom { .. } => "custom",
                WriteEvent::TopologyHint { .. } => "topology_hint",
                _ => "unknown",
            };

            let model = run_event::ActiveModel {
                id: ActiveValue::NotSet,
                run_id: ActiveValue::Set(event.run_id().to_string()),
                seq: ActiveValue::Set(event.seq() as i64),
                event_type: ActiveValue::Set(event_type.to_string()),
                payload: ActiveValue::Set(payload),
                created_at: ActiveValue::Set(chrono::Utc::now().naive_utc()),
            };

            run_event::Entity::insert(model)
                .on_conflict(
                    sea_orm::sea_query::OnConflict::columns([
                        run_event::Column::RunId,
                        run_event::Column::Seq,
                    ])
                    .do_nothing()
                    .to_owned(),
                )
                .do_nothing()
                .exec(self.db.as_ref())
                .await
                .map_err(to_store_err)?;
        }
        Ok(())
    }

    async fn get_run(&self, run_id: &str) -> Result<Option<RunRecord>, RunStoreError> {
        let events = self.read_events(run_id).await?;
        Ok(reconstruct::reconstruct_run(run_id, &events))
    }

    async fn list_runs(&self, filter: &RunFilter) -> Result<RunPage, RunStoreError> {
        let query = run_event::Entity::find()
            .select_only()
            .column(run_event::Column::RunId)
            .group_by(run_event::Column::RunId);

        let models = query
            .into_model::<RunIdOnly>()
            .all(self.db.as_ref())
            .await
            .map_err(to_store_err)?;

        let mut runs = Vec::new();
        for m in &models {
            if let Some(record) = self.get_run(&m.run_id).await? {
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
        let events = self.read_events(run_id).await?;
        Ok(reconstruct::extract_node_results(&events))
    }

    async fn get_edge_results(&self, run_id: &str) -> Result<Vec<EdgeRunResult>, RunStoreError> {
        let events = self.read_events(run_id).await?;
        Ok(reconstruct::extract_edge_results(&events))
    }

    async fn get_node_detail(
        &self,
        run_id: &str,
        node_id: &str,
    ) -> Result<Option<NodeRunDetail>, RunStoreError> {
        let events = self.read_events(run_id).await?;
        Ok(reconstruct::extract_node_detail(&events, node_id))
    }

    async fn get_custom_events(
        &self,
        run_id: &str,
    ) -> Result<Vec<CustomEventRecord>, RunStoreError> {
        let events = self.read_events(run_id).await?;
        Ok(reconstruct::extract_custom_events(&events))
    }

    async fn get_llm_invocations(
        &self,
        run_id: &str,
    ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
        let events = self.read_events(run_id).await?;
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

    async fn events(&self, run_id: &str) -> Result<Vec<WriteEvent>, RunStoreError> {
        self.read_events(run_id).await
    }
}

/// Helper model for selecting distinct run_ids.
#[derive(Debug, Clone, sea_orm::FromQueryResult)]
struct RunIdOnly {
    run_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use cleargate_flow_engine::types::{
        LlmRequest, LlmResponse, RunStatus, TriggerSource, WRITE_EVENT_SCHEMA_VERSION,
    };
    use sea_orm::Database;
    use sea_orm_migration::MigratorTrait;
    use serde_json::json;
    use std::collections::BTreeMap;

    async fn setup_db() -> DatabaseConnection {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        crate::migrations::Migrator::up(&db, None).await.unwrap();
        db
    }

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

    #[tokio::test]
    async fn test_write_batch_and_read() {
        let db = setup_db().await;
        let store = SeaOrmRunStore::new(Arc::new(db));

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
    }

    #[tokio::test]
    async fn test_idempotent_write() {
        let db = setup_db().await;
        let store = SeaOrmRunStore::new(Arc::new(db));

        let events = vec![make_run_started("run-1", 1), make_run_completed("run-1", 2)];
        store.write_batch(&events).await.unwrap();
        store.write_batch(&events).await.unwrap();

        let all_events = store.events("run-1").await.unwrap();
        assert_eq!(all_events.len(), 2);
    }

    #[tokio::test]
    async fn test_get_node_results() {
        let db = setup_db().await;
        let store = SeaOrmRunStore::new(Arc::new(db));

        let events = vec![
            make_run_started("run-1", 1),
            make_node_started("run-1", "node-a", 2),
            make_node_completed("run-1", "node-a", 3),
        ];
        store.write_batch(&events).await.unwrap();

        let results = store.get_node_results("run-1").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].node_id, "node-a");
        assert_eq!(results[0].status, "completed");
    }

    #[tokio::test]
    async fn test_get_llm_invocations() {
        let db = setup_db().await;
        let store = SeaOrmRunStore::new(Arc::new(db));

        let events = vec![
            make_run_started("run-1", 1),
            make_llm_invocation("run-1", "llm-1", 2),
            make_llm_invocation("run-1", "llm-2", 3),
        ];
        store.write_batch(&events).await.unwrap();

        let invocations = store.get_llm_invocations("run-1").await.unwrap();
        assert_eq!(invocations.len(), 2);
    }

    #[tokio::test]
    async fn test_missing_run_returns_none() {
        let db = setup_db().await;
        let store = SeaOrmRunStore::new(Arc::new(db));
        assert!(store.get_run("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_list_runs_with_filter() {
        let db = setup_db().await;
        let store = SeaOrmRunStore::new(Arc::new(db));

        store
            .write_batch(&[make_run_started("run-1", 1), make_run_completed("run-1", 2)])
            .await
            .unwrap();
        store
            .write_batch(&[make_run_started("run-2", 1)])
            .await
            .unwrap();

        let page = store.list_runs(&RunFilter::default()).await.unwrap();
        assert_eq!(page.total, 2);

        let page = store
            .list_runs(&RunFilter {
                status: Some(RunStatus::Completed),
                ..Default::default()
            })
            .await
            .unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.runs[0].run_id, "run-1");
    }
}

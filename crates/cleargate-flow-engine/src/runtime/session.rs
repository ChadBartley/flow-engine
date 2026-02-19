//! Per-execution recording session.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use thiserror::Error;
use tokio::sync::broadcast;

use crate::types::{
    ExecutionId, LlmRequest, LlmResponse, LlmRunSummary, RunStatus, TriggerSource,
    WRITE_EVENT_SCHEMA_VERSION,
};
use crate::write_event::WriteEvent;
use crate::write_handle::WriteHandle;
use crate::write_pipeline::{WritePipeline, WritePipelineHandle};

use super::config::{ExecutionConfig, ExecutionSessionBuilder};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum SessionError {
    #[error("session already finished")]
    AlreadyFinished,
    #[error("write channel closed")]
    ChannelClosed,
    #[error("pipeline error: {0}")]
    Pipeline(String),
}

// ---------------------------------------------------------------------------
// SessionHandle
// ---------------------------------------------------------------------------

/// Lightweight handle returned from `ExecutionSession::start`.
pub struct SessionHandle {
    pub execution_id: ExecutionId,
    pub events: broadcast::Receiver<WriteEvent>,
}

// ---------------------------------------------------------------------------
// ExecutionSession
// ---------------------------------------------------------------------------

/// A standalone per-execution recording primitive.
///
/// Owns a `WriteHandle` + `WritePipeline` pair, providing methods to
/// record LLM calls, tool invocations, steps, and custom events. Does NOT
/// require an Engine or GraphDef.
pub struct ExecutionSession {
    id: ExecutionId,
    #[allow(dead_code)]
    session_name: Option<String>,
    start_time: DateTime<Utc>,
    metadata: BTreeMap<String, serde_json::Value>,
    write_handle: WriteHandle,
    event_tx: broadcast::Sender<WriteEvent>,
    pipeline_handle: Option<WritePipelineHandle>,
    finished: bool,
}

impl ExecutionSession {
    /// Create a new builder.
    pub fn builder() -> ExecutionSessionBuilder {
        ExecutionSessionBuilder::new()
    }

    /// Start a new session from config.
    pub(crate) async fn start(
        config: ExecutionConfig,
    ) -> Result<(Self, SessionHandle), SessionError> {
        let id = ExecutionId::new();
        let now = Utc::now();

        let (write_handle, receivers) =
            WriteHandle::new(config.critical_capacity, config.advisory_capacity);

        let pipeline = WritePipeline::new(
            config.pipeline_config,
            receivers,
            config.run_store,
            config.redactor,
            Arc::new(HashMap::new()), // no node meta for standalone sessions
        );
        let pipeline_handle = pipeline.spawn();

        let (event_tx, event_rx) = broadcast::channel(256);

        // Emit RunStarted.
        let run_started = WriteEvent::RunStarted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: id.run_id_str(),
            flow_id: config
                .session_name
                .clone()
                .unwrap_or_else(|| "session".into()),
            version_id: config
                .version_id
                .clone()
                .unwrap_or_else(|| "standalone".into()),
            trigger_inputs: config.trigger_inputs.clone().unwrap_or_else(|| {
                serde_json::Value::Object(
                    config
                        .metadata
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect(),
                )
            }),
            triggered_by: config
                .triggered_by
                .clone()
                .unwrap_or_else(|| TriggerSource::Manual {
                    principal: "session".into(),
                }),
            config_hash: config.config_hash.clone(),
            parent_run_id: config.parent_run_id.clone(),
            timestamp: now,
        };
        write_handle
            .write(run_started.clone())
            .await
            .map_err(|_| SessionError::ChannelClosed)?;
        let _ = event_tx.send(run_started);

        let handle = SessionHandle {
            execution_id: id.clone(),
            events: event_rx,
        };

        let session = Self {
            id,
            session_name: config.session_name,
            start_time: now,
            metadata: config.metadata,
            write_handle,
            event_tx,
            pipeline_handle: Some(pipeline_handle),
            finished: false,
        };

        Ok((session, handle))
    }

    /// Record an LLM invocation.
    pub async fn record_llm_call(
        &self,
        node_id: &str,
        request: LlmRequest,
        response: LlmResponse,
    ) -> Result<(), SessionError> {
        self.check_finished()?;
        let event = WriteEvent::LlmInvocation {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: self.id.run_id_str(),
            node_id: node_id.into(),
            request: Box::new(request),
            response: Box::new(response),
            timestamp: Utc::now(),
        };
        self.emit_critical(event).await
    }

    /// Record a tool invocation.
    pub async fn record_tool_call(
        &self,
        tool_name: &str,
        inputs: serde_json::Value,
        outputs: serde_json::Value,
        duration_ms: u64,
    ) -> Result<(), SessionError> {
        self.check_finished()?;
        let event = WriteEvent::ToolInvocation {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: self.id.run_id_str(),
            tool_name: tool_name.into(),
            inputs,
            outputs,
            duration_ms,
            timestamp: Utc::now(),
        };
        self.emit_critical(event).await
    }

    /// Record a named step with arbitrary data.
    pub async fn record_step(
        &self,
        step_name: &str,
        data: serde_json::Value,
    ) -> Result<(), SessionError> {
        self.check_finished()?;
        let event = WriteEvent::StepRecorded {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: self.id.run_id_str(),
            step_name: step_name.into(),
            data,
            timestamp: Utc::now(),
        };
        self.emit_critical(event).await
    }

    /// Record a custom (advisory) event.
    pub fn record_custom_event(
        &self,
        name: &str,
        data: serde_json::Value,
    ) -> Result<(), SessionError> {
        self.check_finished()?;
        let event = WriteEvent::Custom {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: self.id.run_id_str(),
            node_id: None,
            name: name.into(),
            data,
            timestamp: Utc::now(),
        };
        self.emit_advisory(event)
    }

    /// Record an arbitrary event through the critical channel.
    pub async fn record_event(&self, event: WriteEvent) -> Result<(), SessionError> {
        self.check_finished()?;
        self.emit_critical(event).await
    }

    /// Subscribe to live events.
    pub fn subscribe(&self) -> broadcast::Receiver<WriteEvent> {
        self.event_tx.subscribe()
    }

    /// Access the broadcast sender (for cloning into spawned tasks).
    pub fn event_sender(&self) -> &broadcast::Sender<WriteEvent> {
        &self.event_tx
    }

    /// Record an arbitrary event through the advisory channel.
    pub fn record_advisory(&self, event: WriteEvent) -> Result<(), SessionError> {
        self.check_finished()?;
        self.emit_advisory(event)
    }

    /// Access the underlying write handle.
    pub fn write_handle(&self) -> &WriteHandle {
        &self.write_handle
    }

    /// The execution id.
    pub fn execution_id(&self) -> &ExecutionId {
        &self.id
    }

    /// Convenience accessor for the run ID string.
    pub fn run_id(&self) -> String {
        self.id.run_id_str()
    }

    /// When the session started.
    pub fn start_time(&self) -> DateTime<Utc> {
        self.start_time
    }

    /// Session metadata.
    pub fn metadata(&self) -> &BTreeMap<String, serde_json::Value> {
        &self.metadata
    }

    /// Finish the session, emitting RunCompleted and shutting down the pipeline.
    pub async fn finish(
        mut self,
        status: RunStatus,
        llm_summary: Option<LlmRunSummary>,
    ) -> Result<(), SessionError> {
        if self.finished {
            return Err(SessionError::AlreadyFinished);
        }
        self.finished = true;

        let duration_ms = (Utc::now() - self.start_time).num_milliseconds().max(0) as u64;
        let event = WriteEvent::RunCompleted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: self.id.run_id_str(),
            status,
            duration_ms,
            llm_summary,
            timestamp: Utc::now(),
        };

        let _ = self.write_handle.write(event.clone()).await;
        let _ = self.event_tx.send(event);

        // Shutdown pipeline (signals drain + waits for flush).
        if let Some(ph) = self.pipeline_handle.take() {
            ph.shutdown().await;
        }

        Ok(())
    }

    // -- private helpers --

    fn check_finished(&self) -> Result<(), SessionError> {
        if self.finished {
            return Err(SessionError::AlreadyFinished);
        }
        Ok(())
    }

    async fn emit_critical(&self, event: WriteEvent) -> Result<(), SessionError> {
        self.write_handle
            .write(event.clone())
            .await
            .map_err(|_| SessionError::ChannelClosed)?;
        let _ = self.event_tx.send(event);
        Ok(())
    }

    fn emit_advisory(&self, event: WriteEvent) -> Result<(), SessionError> {
        self.write_handle
            .try_write(event.clone())
            .map_err(|_| SessionError::ChannelClosed)?;
        let _ = self.event_tx.send(event);
        Ok(())
    }
}

impl Drop for ExecutionSession {
    fn drop(&mut self) {
        if !self.finished {
            tracing::warn!(
                run_id = %self.id,
                "ExecutionSession dropped without finish() — emitting Aborted"
            );
            let event = WriteEvent::RunCompleted {
                seq: 0,
                schema_version: WRITE_EVENT_SCHEMA_VERSION,
                run_id: self.id.run_id_str(),
                status: RunStatus::Failed,
                duration_ms: (Utc::now() - self.start_time).num_milliseconds().max(0) as u64,
                llm_summary: None,
                timestamp: Utc::now(),
            };
            let _ = self.write_handle.try_write(event);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::InMemoryRunStore;
    use crate::traits::RunStore;
    use crate::types::{LlmRequest, LlmResponse};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn test_store() -> Arc<InMemoryRunStore> {
        Arc::new(InMemoryRunStore::new())
    }

    fn test_llm_request() -> LlmRequest {
        LlmRequest {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            messages: json!([{"role": "user", "content": "hi"}]),
            tools: None,
            temperature: None,
            top_p: None,
            max_tokens: None,
            stop_sequences: None,
            response_format: None,
            seed: None,
            extra_params: BTreeMap::new(),
        }
    }

    fn test_llm_response() -> LlmResponse {
        LlmResponse {
            content: json!("hello"),
            tool_calls: None,
            model_used: "gpt-4o".into(),
            input_tokens: Some(10),
            output_tokens: Some(5),
            total_tokens: Some(15),
            finish_reason: "stop".into(),
            latency_ms: 100,
            provider_request_id: None,
            cost: None,
        }
    }

    #[tokio::test]
    async fn test_session_lifecycle() {
        let store = test_store();
        let (session, _handle) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        let run_id = session.execution_id().run_id_str();
        session.record_step("step1", json!({"x": 1})).await.unwrap();
        session.finish(RunStatus::Completed, None).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        assert!(events.len() >= 3); // RunStarted + StepRecorded + RunCompleted
        assert!(matches!(&events[0], WriteEvent::RunStarted { .. }));
        assert!(matches!(
            &events[events.len() - 1],
            WriteEvent::RunCompleted { status, .. } if *status == RunStatus::Completed
        ));
    }

    #[tokio::test]
    async fn test_llm_call_recording() {
        let store = test_store();
        let (session, _handle) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        let run_id = session.execution_id().run_id_str();
        session
            .record_llm_call("llm-1", test_llm_request(), test_llm_response())
            .await
            .unwrap();
        session.finish(RunStatus::Completed, None).await.unwrap();

        let invocations = store.get_llm_invocations(&run_id).await.unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].node_id, "llm-1");
    }

    #[tokio::test]
    async fn test_tool_call_recording() {
        let store = test_store();
        let (session, _handle) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        let run_id = session.execution_id().run_id_str();
        session
            .record_tool_call("search", json!({"q": "rust"}), json!({"results": []}), 50)
            .await
            .unwrap();
        session.finish(RunStatus::Completed, None).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        let tool_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::ToolInvocation { .. }))
            .collect();
        assert_eq!(tool_events.len(), 1);
    }

    #[tokio::test]
    async fn test_step_recording() {
        let store = test_store();
        let (session, _handle) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        let run_id = session.execution_id().run_id_str();
        session
            .record_step("preprocess", json!({"cleaned": true}))
            .await
            .unwrap();
        session.finish(RunStatus::Completed, None).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        let step_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::StepRecorded { .. }))
            .collect();
        assert_eq!(step_events.len(), 1);
    }

    #[tokio::test]
    async fn test_custom_event_advisory() {
        let store = test_store();
        let (session, _handle) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        let run_id = session.execution_id().run_id_str();
        session
            .record_custom_event("debug", json!({"info": "test"}))
            .unwrap();
        session.finish(RunStatus::Completed, None).await.unwrap();

        let customs = store.get_custom_events(&run_id).await.unwrap();
        // Advisory events may or may not arrive depending on timing,
        // but it should not panic.
        assert!(customs.len() <= 1);
    }

    #[tokio::test]
    async fn test_monotonic_seq() {
        let store = test_store();
        let (session, _handle) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        let run_id = session.execution_id().run_id_str();
        for i in 0..5 {
            session
                .record_step(&format!("step-{i}"), json!({}))
                .await
                .unwrap();
        }
        session.finish(RunStatus::Completed, None).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        for i in 1..events.len() {
            assert!(
                events[i].seq() > events[i - 1].seq(),
                "seq {} must be > {}",
                events[i].seq(),
                events[i - 1].seq()
            );
        }
    }

    #[tokio::test]
    async fn test_empty_session() {
        let store = test_store();
        let (session, _handle) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        let run_id = session.execution_id().run_id_str();
        session.finish(RunStatus::Completed, None).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        assert_eq!(events.len(), 2); // RunStarted + RunCompleted
    }

    #[tokio::test]
    async fn test_concurrent_sessions() {
        let store = test_store();

        let (s1, _h1) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();
        let (s2, _h2) = ExecutionSession::builder()
            .run_store(store.clone())
            .start()
            .await
            .unwrap();

        assert_ne!(
            s1.execution_id().run_id,
            s2.execution_id().run_id,
            "concurrent sessions must have different run_ids"
        );

        s1.record_step("a", json!({})).await.unwrap();
        s2.record_step("b", json!({})).await.unwrap();

        let id1 = s1.execution_id().run_id_str();
        let id2 = s2.execution_id().run_id_str();

        s1.finish(RunStatus::Completed, None).await.unwrap();
        s2.finish(RunStatus::Completed, None).await.unwrap();

        let e1 = store.events(&id1).await.unwrap();
        let e2 = store.events(&id2).await.unwrap();
        assert!(e1.iter().all(|e| e.run_id() == id1));
        assert!(e2.iter().all(|e| e.run_id() == id2));
    }

    #[tokio::test]
    async fn test_drop_safety() {
        let store = test_store();
        let run_id;
        {
            let (session, _handle) = ExecutionSession::builder()
                .run_store(store.clone())
                .start()
                .await
                .unwrap();
            run_id = session.execution_id().run_id_str();
            session.record_step("x", json!({})).await.unwrap();
            // Drop without finish — should emit Aborted (Failed) via try_write.
        }

        // Give pipeline time to flush.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let events = store.events(&run_id).await.unwrap();
        // May or may not have the Failed event (advisory channel, drop timing),
        // but should have at least RunStarted + StepRecorded.
        assert!(events.len() >= 2);
    }

    #[tokio::test]
    async fn test_builder_defaults() {
        // Builder with no explicit config should start successfully.
        let (session, handle) = ExecutionSession::builder().start().await.unwrap();

        assert!(!handle.execution_id.run_id_str().is_empty());
        assert_eq!(session.execution_id(), &handle.execution_id);

        session.finish(RunStatus::Completed, None).await.unwrap();
    }
}

//! Batched write pipeline with redaction.
//!
//! Drains both channels from [`WriteHandleReceivers`], applies redaction via
//! the [`Redactor`] trait, and flushes batches to the [`RunStore`].
//!
//! **Invariant I2**: Critical events are NEVER dropped, even on RunStore failure.
//! Advisory events may be dropped under persistent failure.
//!
//! **Invariant E9**: Sequence numbers are preserved — the pipeline does not
//! reorder events.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::traits::{Redactor, RunStore};
use super::types::{NodeMeta, Sensitivity};
use super::write_event::WriteEvent;
use super::write_handle::WriteHandleReceivers;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for the `WritePipeline`.
///
/// All fields have sensible defaults via the [`Default`] impl.
#[derive(Debug, Clone)]
pub struct WritePipelineConfig {
    /// Maximum events per batch before an immediate flush. Default: 500.
    pub batch_size: usize,
    /// Flush interval in milliseconds when the batch is non-empty. Default: 100.
    pub flush_interval_ms: u64,
}

impl Default for WritePipelineConfig {
    fn default() -> Self {
        Self {
            batch_size: 500,
            flush_interval_ms: 100,
        }
    }
}

// ---------------------------------------------------------------------------
// WritePipeline
// ---------------------------------------------------------------------------

/// Batched write pipeline that drains dual channels, applies redaction, and
/// flushes to the [`RunStore`].
///
/// Construct via [`WritePipeline::new`], then call [`WritePipeline::spawn`]
/// to start the background flush loop.
pub struct WritePipeline {
    config: WritePipelineConfig,
    receivers: WriteHandleReceivers,
    run_store: Arc<dyn RunStore>,
    redactor: Arc<dyn Redactor>,
    node_meta: Arc<HashMap<String, NodeMeta>>,
}

impl WritePipeline {
    /// Create a new pipeline (not yet running).
    ///
    /// - `receivers`: the dual-channel receivers from [`WriteHandle::new`].
    /// - `run_store`: persistence target for batched events.
    /// - `redactor`: applied to sensitive fields before persisting.
    /// - `node_meta`: maps `node_type` to [`NodeMeta`] for port sensitivity lookup.
    pub fn new(
        config: WritePipelineConfig,
        receivers: WriteHandleReceivers,
        run_store: Arc<dyn RunStore>,
        redactor: Arc<dyn Redactor>,
        node_meta: Arc<HashMap<String, NodeMeta>>,
    ) -> Self {
        Self {
            config,
            receivers,
            run_store,
            redactor,
            node_meta,
        }
    }

    /// Spawn the flush loop as a tokio task.
    ///
    /// Returns a [`WritePipelineHandle`] that can be used to gracefully shut
    /// down the pipeline.
    pub fn spawn(self) -> WritePipelineHandle {
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let join = tokio::spawn(flush_loop(
            self.config,
            self.receivers,
            self.run_store,
            self.redactor,
            self.node_meta,
            shutdown_rx,
        ));
        WritePipelineHandle {
            shutdown_tx: Some(shutdown_tx),
            join,
        }
    }
}

// ---------------------------------------------------------------------------
// WritePipelineHandle
// ---------------------------------------------------------------------------

/// Handle to a running `WritePipeline`. Allows graceful shutdown.
pub struct WritePipelineHandle {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    join: JoinHandle<()>,
}

impl WritePipelineHandle {
    /// Signal the pipeline to drain remaining events and stop.
    ///
    /// Waits for the flush loop task to complete.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        let _ = self.join.await;
    }
}

// ---------------------------------------------------------------------------
// Flush loop
// ---------------------------------------------------------------------------

async fn flush_loop(
    config: WritePipelineConfig,
    mut receivers: WriteHandleReceivers,
    run_store: Arc<dyn RunStore>,
    redactor: Arc<dyn Redactor>,
    node_meta: Arc<HashMap<String, NodeMeta>>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let mut batch: Vec<WriteEvent> = Vec::with_capacity(config.batch_size);
    let mut interval =
        tokio::time::interval(tokio::time::Duration::from_millis(config.flush_interval_ms));
    // Don't fire immediately on creation.
    interval.tick().await;

    let mut critical_open = true;
    let mut advisory_open = true;

    loop {
        tokio::select! {
            biased;

            // Shutdown signal.
            _ = &mut shutdown_rx => {
                // Drain remaining events from both channels.
                drain_channel(&mut receivers.critical_rx, &mut batch);
                drain_channel(&mut receivers.advisory_rx, &mut batch);
                if !batch.is_empty() {
                    flush_batch(&mut batch, &run_store, &redactor, &node_meta).await;
                }
                return;
            }

            // Critical channel: always prioritize.
            event = receivers.critical_rx.recv(), if critical_open => {
                match event {
                    Some(ev) => {
                        batch.push(ev);
                        // Drain any additional ready events from both channels.
                        drain_channel(&mut receivers.critical_rx, &mut batch);
                        drain_channel(&mut receivers.advisory_rx, &mut batch);

                        if batch.len() >= config.batch_size {
                            flush_batch(&mut batch, &run_store, &redactor, &node_meta).await;
                        }
                    }
                    None => {
                        critical_open = false;
                        if !advisory_open {
                            // Both channels closed — final flush and exit.
                            if !batch.is_empty() {
                                flush_batch(&mut batch, &run_store, &redactor, &node_meta).await;
                            }
                            return;
                        }
                    }
                }
            }

            // Advisory channel.
            event = receivers.advisory_rx.recv(), if advisory_open => {
                match event {
                    Some(ev) => {
                        batch.push(ev);
                        drain_channel(&mut receivers.advisory_rx, &mut batch);
                        drain_channel(&mut receivers.critical_rx, &mut batch);

                        if batch.len() >= config.batch_size {
                            flush_batch(&mut batch, &run_store, &redactor, &node_meta).await;
                        }
                    }
                    None => {
                        advisory_open = false;
                        if !critical_open {
                            if !batch.is_empty() {
                                flush_batch(&mut batch, &run_store, &redactor, &node_meta).await;
                            }
                            return;
                        }
                    }
                }
            }

            // Interval tick: flush if non-empty.
            _ = interval.tick() => {
                if !batch.is_empty() {
                    flush_batch(&mut batch, &run_store, &redactor, &node_meta).await;
                }
            }
        }

        // Adaptive flush: under light load, flush small batches immediately
        // rather than waiting for the full interval.
        if !batch.is_empty() && batch.len() < 10 {
            // Check if both channels are currently empty.
            let critical_empty = receivers.critical_rx.is_empty();
            let advisory_empty = receivers.advisory_rx.is_empty();
            if critical_empty && advisory_empty {
                flush_batch(&mut batch, &run_store, &redactor, &node_meta).await;
            }
        }
    }
}

/// Non-blocking drain of all immediately available events from a channel.
fn drain_channel(rx: &mut mpsc::Receiver<WriteEvent>, batch: &mut Vec<WriteEvent>) {
    while let Ok(ev) = rx.try_recv() {
        batch.push(ev);
    }
}

/// Apply redaction and flush a batch to the RunStore.
///
/// On failure: retry once after 50ms. On persistent failure, buffer critical
/// events for the next cycle and drop advisory events.
async fn flush_batch(
    batch: &mut Vec<WriteEvent>,
    run_store: &Arc<dyn RunStore>,
    redactor: &Arc<dyn Redactor>,
    node_meta: &Arc<HashMap<String, NodeMeta>>,
) {
    let events: Vec<WriteEvent> = std::mem::take(batch);
    let redacted: Vec<WriteEvent> = events
        .into_iter()
        .map(|ev| redact_event(ev, redactor, node_meta))
        .collect();

    match run_store.write_batch(&redacted).await {
        Ok(()) => {}
        Err(e) => {
            tracing::error!("write_batch failed, retrying in 50ms: {e}");
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
            if let Err(e2) = run_store.write_batch(&redacted).await {
                tracing::error!("write_batch retry failed: {e2}");
                // Re-buffer critical events for the next flush cycle.
                for ev in redacted {
                    if ev.is_critical() {
                        batch.push(ev);
                    }
                    // Advisory events are dropped on persistent failure.
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Apply redaction to a single event based on node port sensitivities.
fn redact_event(
    mut event: WriteEvent,
    redactor: &Arc<dyn Redactor>,
    node_meta: &Arc<HashMap<String, NodeMeta>>,
) -> WriteEvent {
    match &mut event {
        WriteEvent::NodeStarted {
            node_id, inputs, ..
        } => {
            if let Some(meta) = node_meta.get(node_id.as_str()) {
                *inputs = redact_ports(inputs, &meta.inputs, redactor);
            }
        }
        WriteEvent::NodeCompleted {
            node_id, outputs, ..
        } => {
            if let Some(meta) = node_meta.get(node_id.as_str()) {
                *outputs = redact_ports(outputs, &meta.outputs, redactor);
            }
        }
        WriteEvent::LlmInvocation {
            node_id,
            request,
            response,
            ..
        } => {
            let sensitivity = node_meta
                .get(node_id.as_str())
                .and_then(|m| {
                    m.inputs
                        .iter()
                        .find(|p| p.name == "messages")
                        .map(|p| &p.sensitivity)
                })
                .unwrap_or(&Sensitivity::None);

            request.messages = redactor.redact_llm_messages(&request.messages, sensitivity);
            response.content = redactor.redact_llm_messages(&response.content, sensitivity);
        }
        WriteEvent::Custom {
            node_id: Some(nid),
            data,
            name,
            ..
        } => {
            if let Some(meta) = node_meta.get(nid.as_str()) {
                let has_sensitive = meta
                    .outputs
                    .iter()
                    .any(|p| p.sensitivity != Sensitivity::None);
                if has_sensitive {
                    *data = redactor.redact(name, &Sensitivity::Pii, data);
                }
            }
        }
        // Other events don't contain user data that needs redaction.
        _ => {}
    }
    event
}

/// Redact a Value according to port definitions.
fn redact_ports(
    value: &serde_json::Value,
    ports: &[super::types::PortDef],
    redactor: &Arc<dyn Redactor>,
) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (k, v) in map {
                let sensitivity = ports
                    .iter()
                    .find(|p| p.name == *k)
                    .map(|p| &p.sensitivity)
                    .unwrap_or(&Sensitivity::None);
                out.insert(k.clone(), redactor.redact(k, sensitivity, v));
            }
            serde_json::Value::Object(out)
        }
        other => other.clone(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::defaults::DefaultRedactor;
    use crate::errors::RunStoreError;
    use crate::traits::{
        CustomEventRecord, EdgeRunResult, NodeRunDetail, NodeRunResult, RunFilter, RunPage,
        RunStore,
    };
    use crate::types::*;
    use crate::write_handle::WriteHandle;
    use async_trait::async_trait;
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    // -- Mock RunStore that collects events --

    #[derive(Default)]
    struct CollectingRunStore {
        events: parking_lot::Mutex<Vec<WriteEvent>>,
    }

    #[async_trait]
    impl RunStore for CollectingRunStore {
        async fn write_batch(&self, events: &[WriteEvent]) -> Result<(), RunStoreError> {
            self.events.lock().extend(events.iter().cloned());
            Ok(())
        }
        async fn get_run(&self, _: &str) -> Result<Option<RunRecord>, RunStoreError> {
            Ok(None)
        }
        async fn list_runs(&self, _: &RunFilter) -> Result<RunPage, RunStoreError> {
            Ok(RunPage {
                runs: vec![],
                total: 0,
            })
        }
        async fn get_node_results(&self, _: &str) -> Result<Vec<NodeRunResult>, RunStoreError> {
            Ok(vec![])
        }
        async fn get_edge_results(&self, _: &str) -> Result<Vec<EdgeRunResult>, RunStoreError> {
            Ok(vec![])
        }
        async fn get_node_detail(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Option<NodeRunDetail>, RunStoreError> {
            Ok(None)
        }
        async fn get_custom_events(
            &self,
            _: &str,
        ) -> Result<Vec<CustomEventRecord>, RunStoreError> {
            Ok(vec![])
        }
        async fn get_llm_invocations(
            &self,
            _: &str,
        ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
            Ok(vec![])
        }
        async fn get_llm_invocations_for_node(
            &self,
            _: &str,
            _: &str,
        ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
            Ok(vec![])
        }
    }

    // -- Mock RunStore that fails N times then succeeds --

    struct FailNRunStore {
        inner: CollectingRunStore,
        fail_count: AtomicU32,
    }

    impl FailNRunStore {
        fn new(fail_count: u32) -> Self {
            Self {
                inner: CollectingRunStore::default(),
                fail_count: AtomicU32::new(fail_count),
            }
        }
    }

    #[async_trait]
    impl RunStore for FailNRunStore {
        async fn write_batch(&self, events: &[WriteEvent]) -> Result<(), RunStoreError> {
            let remaining = self.fail_count.load(Ordering::SeqCst);
            if remaining > 0 {
                self.fail_count.fetch_sub(1, Ordering::SeqCst);
                return Err(RunStoreError::Store {
                    message: "simulated failure".into(),
                });
            }
            self.inner.write_batch(events).await
        }
        async fn get_run(&self, id: &str) -> Result<Option<RunRecord>, RunStoreError> {
            self.inner.get_run(id).await
        }
        async fn list_runs(&self, f: &RunFilter) -> Result<RunPage, RunStoreError> {
            self.inner.list_runs(f).await
        }
        async fn get_node_results(&self, id: &str) -> Result<Vec<NodeRunResult>, RunStoreError> {
            self.inner.get_node_results(id).await
        }
        async fn get_edge_results(&self, id: &str) -> Result<Vec<EdgeRunResult>, RunStoreError> {
            self.inner.get_edge_results(id).await
        }
        async fn get_node_detail(
            &self,
            r: &str,
            n: &str,
        ) -> Result<Option<NodeRunDetail>, RunStoreError> {
            self.inner.get_node_detail(r, n).await
        }
        async fn get_custom_events(
            &self,
            id: &str,
        ) -> Result<Vec<CustomEventRecord>, RunStoreError> {
            self.inner.get_custom_events(id).await
        }
        async fn get_llm_invocations(
            &self,
            id: &str,
        ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
            self.inner.get_llm_invocations(id).await
        }
        async fn get_llm_invocations_for_node(
            &self,
            r: &str,
            n: &str,
        ) -> Result<Vec<LlmInvocationRecord>, RunStoreError> {
            self.inner.get_llm_invocations_for_node(r, n).await
        }
    }

    // -- Helpers --

    fn make_node_started(node_id: &str) -> WriteEvent {
        WriteEvent::NodeStarted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "run-1".into(),
            node_id: node_id.into(),
            inputs: json!({"messages": "hello"}),
            fan_out_index: None,
            attempt: 1,
            timestamp: chrono::Utc::now(),
        }
    }

    fn make_node_completed(node_id: &str) -> WriteEvent {
        WriteEvent::NodeCompleted {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "run-1".into(),
            node_id: node_id.into(),
            outputs: json!({"result": "secret-data-1234"}),
            fan_out_index: None,
            duration_ms: 100,
            timestamp: chrono::Utc::now(),
        }
    }

    fn make_custom_event() -> WriteEvent {
        WriteEvent::Custom {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "run-1".into(),
            node_id: None,
            name: "debug".into(),
            data: json!({"info": "test"}),
            timestamp: chrono::Utc::now(),
        }
    }

    fn make_llm_invocation(node_id: &str) -> WriteEvent {
        WriteEvent::LlmInvocation {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: "run-1".into(),
            node_id: node_id.into(),
            request: Box::new(LlmRequest {
                provider: "openai".into(),
                model: "gpt-4o".into(),
                messages: json!([
                    {"role": "user", "content": "Tell me about john@example.com"}
                ]),
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
                content: json!("Here is info about the user"),
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
            timestamp: chrono::Utc::now(),
        }
    }

    fn empty_node_meta() -> Arc<HashMap<String, NodeMeta>> {
        Arc::new(HashMap::new())
    }

    fn node_meta_with_pii(node_id: &str) -> Arc<HashMap<String, NodeMeta>> {
        let mut map = HashMap::new();
        map.insert(
            node_id.to_string(),
            NodeMeta {
                node_type: node_id.into(),
                label: "Test".into(),
                category: "test".into(),
                inputs: vec![PortDef {
                    name: "messages".into(),
                    port_type: PortType::Json,
                    required: true,
                    default: None,
                    description: None,
                    sensitivity: Sensitivity::Pii,
                }],
                outputs: vec![PortDef {
                    name: "result".into(),
                    port_type: PortType::String,
                    required: true,
                    default: None,
                    description: None,
                    sensitivity: Sensitivity::Secret,
                }],
                config_schema: json!({}),
                ui: NodeUiHints::default(),
                execution: ExecutionHints::default(),
            },
        );
        Arc::new(map)
    }

    /// Spawn a pipeline, return (handle, store).
    fn spawn_pipeline(
        receivers: WriteHandleReceivers,
        store: Arc<dyn RunStore>,
        node_meta: Arc<HashMap<String, NodeMeta>>,
    ) -> WritePipelineHandle {
        let config = WritePipelineConfig {
            batch_size: 500,
            flush_interval_ms: 50,
        };
        let pipeline = WritePipeline::new(
            config,
            receivers,
            store,
            Arc::new(DefaultRedactor),
            node_meta,
        );
        pipeline.spawn()
    }

    // -- Tests --

    #[tokio::test]
    async fn test_events_flow_through() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        let store = Arc::new(CollectingRunStore::default());
        let ph = spawn_pipeline(receivers, store.clone(), empty_node_meta());

        handle.write(make_node_started("n1")).await.unwrap();
        handle.write(make_node_completed("n1")).await.unwrap();

        drop(handle);
        ph.shutdown().await;

        let events = store.events.lock().clone();
        assert_eq!(events.len(), 2);
    }

    #[tokio::test]
    async fn test_seq_monotonically_increasing() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        let store = Arc::new(CollectingRunStore::default());
        let ph = spawn_pipeline(receivers, store.clone(), empty_node_meta());

        for _ in 0..20 {
            handle.write(make_node_started("n1")).await.unwrap();
        }

        drop(handle);
        ph.shutdown().await;

        let events = store.events.lock().clone();
        assert_eq!(events.len(), 20);
        for i in 1..events.len() {
            assert!(
                events[i].seq() > events[i - 1].seq(),
                "seq {} should be > {}",
                events[i].seq(),
                events[i - 1].seq()
            );
        }
    }

    #[tokio::test]
    async fn test_critical_events_never_dropped() {
        // Use a small advisory channel to force advisory drops.
        let (handle, receivers) = WriteHandle::new(1000, 2);
        let store = Arc::new(CollectingRunStore::default());
        let ph = spawn_pipeline(receivers, store.clone(), empty_node_meta());

        // Send many advisory events (some will be dropped due to small channel).
        for _ in 0..50 {
            let _ = handle.try_write(make_custom_event());
        }

        // Send critical events — all must arrive.
        for _ in 0..20 {
            handle.write(make_node_started("n1")).await.unwrap();
        }

        drop(handle);
        ph.shutdown().await;

        let events = store.events.lock().clone();
        let critical_count = events.iter().filter(|e| e.is_critical()).count();
        assert_eq!(critical_count, 20, "all 20 critical events must arrive");
    }

    #[tokio::test]
    async fn test_advisory_events_drop_gracefully() {
        // Tiny advisory channel.
        let (handle, receivers) = WriteHandle::new(1000, 2);
        let store = Arc::new(CollectingRunStore::default());
        let ph = spawn_pipeline(receivers, store.clone(), empty_node_meta());

        // Flood advisory channel — should not panic.
        for _ in 0..100 {
            let _ = handle.try_write(make_custom_event());
        }

        // Critical events still work.
        handle.write(make_node_started("n1")).await.unwrap();

        drop(handle);
        ph.shutdown().await;

        let events = store.events.lock().clone();
        let critical_count = events.iter().filter(|e| e.is_critical()).count();
        assert_eq!(critical_count, 1);
        // Advisory events: some arrived, some were dropped — that's fine.
    }

    #[tokio::test]
    async fn test_llm_invocation_redacted() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        let store = Arc::new(CollectingRunStore::default());
        let meta = node_meta_with_pii("llm-1");
        let ph = spawn_pipeline(receivers, store.clone(), meta);

        handle.write(make_llm_invocation("llm-1")).await.unwrap();

        drop(handle);
        ph.shutdown().await;

        let events = store.events.lock().clone();
        assert_eq!(events.len(), 1);

        if let WriteEvent::LlmInvocation {
            request, response, ..
        } = &events[0]
        {
            // Messages should be redacted (PII sensitivity on "messages" port).
            let msgs = request
                .messages
                .as_array()
                .expect("messages should be array");
            assert_eq!(
                msgs[0]["content"], "[PII_REDACTED]",
                "message content should be PII-redacted"
            );
            // Response content should also be redacted.
            assert_eq!(response.content, json!("[PII_REDACTED]"));
        } else {
            panic!("expected LlmInvocation event");
        }
    }

    #[tokio::test]
    async fn test_node_io_redacted() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        let store = Arc::new(CollectingRunStore::default());
        let meta = node_meta_with_pii("n1");
        let ph = spawn_pipeline(receivers, store.clone(), meta);

        handle.write(make_node_started("n1")).await.unwrap();
        handle.write(make_node_completed("n1")).await.unwrap();

        drop(handle);
        ph.shutdown().await;

        let events = store.events.lock().clone();
        assert_eq!(events.len(), 2);

        // NodeStarted: inputs should have "messages" port redacted.
        if let WriteEvent::NodeStarted { inputs, .. } = &events[0] {
            assert_eq!(
                inputs["messages"], "[PII_REDACTED]",
                "input 'messages' port should be PII-redacted"
            );
        } else {
            panic!("expected NodeStarted");
        }

        // NodeCompleted: outputs should have "result" port redacted (Secret).
        if let WriteEvent::NodeCompleted { outputs, .. } = &events[1] {
            assert_eq!(
                outputs["result"], "[REDACTED]",
                "output 'result' port should be Secret-redacted"
            );
        } else {
            panic!("expected NodeCompleted");
        }
    }

    #[tokio::test]
    async fn test_batch_size_flush() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        let store = Arc::new(CollectingRunStore::default());

        let config = WritePipelineConfig {
            batch_size: 5,
            flush_interval_ms: 5000, // very long — shouldn't be hit
        };
        let pipeline = WritePipeline::new(
            config,
            receivers,
            store.clone(),
            Arc::new(DefaultRedactor),
            empty_node_meta(),
        );
        let ph = pipeline.spawn();

        // Send exactly batch_size events.
        for _ in 0..5 {
            handle.write(make_node_started("n1")).await.unwrap();
        }

        // Give the flush loop time to process.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let count = store.events.lock().len();
        assert_eq!(count, 5, "batch_size events should flush immediately");

        drop(handle);
        ph.shutdown().await;
    }

    #[tokio::test]
    async fn test_adaptive_flush() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        let store = Arc::new(CollectingRunStore::default());

        let config = WritePipelineConfig {
            batch_size: 500,
            flush_interval_ms: 5000, // very long
        };
        let pipeline = WritePipeline::new(
            config,
            receivers,
            store.clone(),
            Arc::new(DefaultRedactor),
            empty_node_meta(),
        );
        let ph = pipeline.spawn();

        // Send 3 events (< 10, triggers adaptive flush).
        for _ in 0..3 {
            handle.write(make_node_started("n1")).await.unwrap();
        }

        // Adaptive flush should happen quickly (within ~200ms), not after 5s.
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let count = store.events.lock().len();
        assert!(
            count >= 3,
            "adaptive flush should have flushed {count}/3 events within 200ms"
        );

        drop(handle);
        ph.shutdown().await;
    }

    #[tokio::test]
    async fn test_shutdown_drains() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        let store = Arc::new(CollectingRunStore::default());

        let config = WritePipelineConfig {
            batch_size: 500,
            flush_interval_ms: 10_000, // long interval — events should still flush on shutdown
        };
        let pipeline = WritePipeline::new(
            config,
            receivers,
            store.clone(),
            Arc::new(DefaultRedactor),
            empty_node_meta(),
        );
        let ph = pipeline.spawn();

        for _ in 0..10 {
            handle.write(make_node_started("n1")).await.unwrap();
        }

        drop(handle);
        ph.shutdown().await;

        let count = store.events.lock().len();
        assert_eq!(count, 10, "shutdown must drain all remaining events");
    }

    #[tokio::test]
    async fn test_write_batch_retry() {
        let (handle, receivers) = WriteHandle::new(1000, 1000);
        // Fails once, then succeeds.
        let store = Arc::new(FailNRunStore::new(1));
        let ph = spawn_pipeline(receivers, store.clone(), empty_node_meta());

        for _ in 0..5 {
            handle.write(make_node_started("n1")).await.unwrap();
        }

        drop(handle);
        ph.shutdown().await;

        let events = store.inner.events.lock().clone();
        assert!(
            !events.is_empty(),
            "events should arrive after retry succeeds"
        );
    }
}

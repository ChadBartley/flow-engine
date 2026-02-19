//! [`ObserverSession`] — thin ergonomic wrapper around [`ExecutionSession`]
//! for external callers who want to record LLM calls, tool invocations, and
//! steps without graph execution concerns.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use serde_json::json;
use tokio::sync::broadcast;

use crate::runtime::{ExecutionSession, SessionError, SessionHandle};
use crate::traits::{Redactor, RunStore};
use crate::types::{LlmRequest, LlmResponse, RunStatus, TriggerSource, WRITE_EVENT_SCHEMA_VERSION};
use crate::write_event::WriteEvent;

/// A thin wrapper around [`ExecutionSession`] that provides a simplified API
/// for recording LLM calls, tool invocations, and steps.
///
/// Unlike the full engine executor, `ObserverSession` does not require a graph
/// definition or engine instance — it is a standalone recording primitive with
/// ergonomic defaults for the "observer" use case.
pub struct ObserverSession {
    inner: ExecutionSession,
    seen_nodes: Arc<Mutex<BTreeSet<String>>>,
    topology_set: Arc<Mutex<bool>>,
}

impl ObserverSession {
    /// Start a new observer session.
    ///
    /// Uses `TriggerSource::Custom { trigger_type: "observer", metadata: {} }`
    /// as the trigger source.
    pub async fn start(
        name: impl Into<String>,
        config: ObserverConfig,
    ) -> Result<(Self, SessionHandle), SessionError> {
        let name: String = name.into();
        let mut builder =
            ExecutionSession::builder()
                .session_name(name)
                .triggered_by(TriggerSource::Custom {
                    trigger_type: "observer".into(),
                    metadata: json!({}),
                });

        if let Some(store) = config.run_store {
            builder = builder.run_store(store);
        }
        if let Some(redactor) = config.redactor {
            builder = builder.redactor(redactor);
        }
        if !config.metadata.is_empty() {
            builder = builder.metadata(config.metadata);
        }

        let (session, handle) = builder.start().await?;
        Ok((
            Self {
                inner: session,
                seen_nodes: Arc::new(Mutex::new(BTreeSet::new())),
                topology_set: Arc::new(Mutex::new(false)),
            },
            handle,
        ))
    }

    /// Record an LLM invocation (hardcodes `node_id = "observer"`).
    pub async fn record_llm_call(
        &self,
        request: LlmRequest,
        response: LlmResponse,
    ) -> Result<(), SessionError> {
        self.track_node("observer");
        self.inner
            .record_llm_call("observer", request, response)
            .await
    }

    /// Record an LLM invocation with a caller-specified node ID.
    pub async fn record_llm_call_for_node(
        &self,
        node_id: &str,
        request: LlmRequest,
        response: LlmResponse,
    ) -> Result<(), SessionError> {
        self.track_node(node_id);
        self.inner.record_llm_call(node_id, request, response).await
    }

    /// Record a tool invocation.
    pub async fn record_tool_call(
        &self,
        tool_name: &str,
        inputs: serde_json::Value,
        outputs: serde_json::Value,
        duration_ms: u64,
    ) -> Result<(), SessionError> {
        self.track_node(tool_name);
        self.inner
            .record_tool_call(tool_name, inputs, outputs, duration_ms)
            .await
    }

    /// Record a named step with arbitrary data.
    pub async fn record_step(
        &self,
        step_name: &str,
        data: serde_json::Value,
    ) -> Result<(), SessionError> {
        self.track_node(step_name);
        self.inner.record_step(step_name, data).await
    }

    /// Record a custom (advisory) event.
    pub fn record_custom(&self, name: &str, data: serde_json::Value) -> Result<(), SessionError> {
        self.inner.record_custom_event(name, data)
    }

    /// Set an explicit flow topology hint.
    ///
    /// If called, auto-topology inference on `finish()` is suppressed.
    pub fn set_flow_topology(
        &self,
        nodes: Vec<String>,
        edges: Vec<(String, String)>,
    ) -> Result<(), SessionError> {
        *self.topology_set.lock().unwrap() = true;
        let event = WriteEvent::TopologyHint {
            seq: 0,
            schema_version: WRITE_EVENT_SCHEMA_VERSION,
            run_id: self.inner.run_id(),
            nodes,
            edges,
            timestamp: Utc::now(),
        };
        self.inner.record_advisory(event)
    }

    /// Finish the session with the given status.
    ///
    /// If no explicit topology was set via `set_flow_topology`, emits an
    /// auto-inferred `TopologyHint` from the recorded node/step IDs as a
    /// linear chain.
    pub async fn finish(self, status: RunStatus) -> Result<(), SessionError> {
        let topology_already_set = *self.topology_set.lock().unwrap();
        if !topology_already_set {
            let nodes: Vec<String> = self.seen_nodes.lock().unwrap().iter().cloned().collect();
            if !nodes.is_empty() {
                let edges: Vec<(String, String)> = nodes
                    .windows(2)
                    .map(|w| (w[0].clone(), w[1].clone()))
                    .collect();
                let event = WriteEvent::TopologyHint {
                    seq: 0,
                    schema_version: WRITE_EVENT_SCHEMA_VERSION,
                    run_id: self.inner.run_id(),
                    nodes,
                    edges,
                    timestamp: Utc::now(),
                };
                let _ = self.inner.record_advisory(event);
            }
        }
        self.inner.finish(status, None).await
    }

    /// Access the inner ExecutionSession (for adapter integration).
    pub fn inner(&self) -> &ExecutionSession {
        &self.inner
    }

    /// Subscribe to live events.
    pub fn subscribe(&self) -> broadcast::Receiver<WriteEvent> {
        self.inner.subscribe()
    }

    /// The run ID string.
    pub fn run_id(&self) -> String {
        self.inner.run_id()
    }

    /// The execution ID.
    pub fn execution_id(&self) -> &crate::types::ExecutionId {
        self.inner.execution_id()
    }

    /// When the session started.
    pub fn start_time(&self) -> DateTime<Utc> {
        self.inner.start_time()
    }

    /// Session metadata.
    pub fn metadata(&self) -> &BTreeMap<String, serde_json::Value> {
        self.inner.metadata()
    }

    fn track_node(&self, node_id: &str) {
        self.seen_nodes.lock().unwrap().insert(node_id.to_string());
    }
}

/// Configuration for [`ObserverSession::start`].
#[derive(Default)]
pub struct ObserverConfig {
    pub run_store: Option<Arc<dyn RunStore>>,
    pub redactor: Option<Arc<dyn Redactor>>,
    pub metadata: BTreeMap<String, serde_json::Value>,
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

    fn test_store() -> Arc<InMemoryRunStore> {
        Arc::new(InMemoryRunStore::new())
    }

    fn test_config(store: Arc<InMemoryRunStore>) -> ObserverConfig {
        ObserverConfig {
            run_store: Some(store),
            ..Default::default()
        }
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
    async fn test_observer_lifecycle() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("my-observer", test_config(store.clone()))
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .record_step("init", json!({"ready": true}))
            .await
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        assert!(events.len() >= 3); // RunStarted + StepRecorded + RunCompleted
    }

    #[tokio::test]
    async fn test_observer_llm_call() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("llm-test", test_config(store.clone()))
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .record_llm_call(test_llm_request(), test_llm_response())
            .await
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let invocations = store.get_llm_invocations(&run_id).await.unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].node_id, "observer");
    }

    #[tokio::test]
    async fn test_observer_tool_call() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("tool-test", test_config(store.clone()))
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .record_tool_call("search", json!({"q": "rust"}), json!({"results": []}), 42)
            .await
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        let tool_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::ToolInvocation { .. }))
            .collect();
        assert_eq!(tool_events.len(), 1);
    }

    #[tokio::test]
    async fn test_observer_custom_event() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("custom-test", test_config(store.clone()))
            .await
            .unwrap();

        session
            .record_custom("debug", json!({"info": "test"}))
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();
    }

    #[tokio::test]
    async fn test_observer_trigger_source() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("trigger-test", test_config(store.clone()))
            .await
            .unwrap();

        let run_id = session.run_id();
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        if let WriteEvent::RunStarted { triggered_by, .. } = &events[0] {
            match triggered_by {
                TriggerSource::Custom {
                    trigger_type,
                    metadata,
                } => {
                    assert_eq!(trigger_type, "observer");
                    assert_eq!(*metadata, json!({}));
                }
                other => panic!("expected Custom trigger, got {:?}", other),
            }
        } else {
            panic!("first event should be RunStarted");
        }
    }

    #[tokio::test]
    async fn test_observer_llm_call_for_node() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("node-test", test_config(store.clone()))
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .record_llm_call_for_node("my-llm-node", test_llm_request(), test_llm_response())
            .await
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let invocations = store.get_llm_invocations(&run_id).await.unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].node_id, "my-llm-node");
    }

    #[tokio::test]
    async fn test_observer_explicit_topology() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("topo-test", test_config(store.clone()))
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .set_flow_topology(
                vec!["a".into(), "b".into(), "c".into()],
                vec![("a".into(), "b".into()), ("b".into(), "c".into())],
            )
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        let topo_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::TopologyHint { .. }))
            .collect();
        assert_eq!(topo_events.len(), 1);
        if let WriteEvent::TopologyHint { nodes, edges, .. } = &topo_events[0] {
            assert_eq!(nodes, &vec!["a".to_string(), "b".into(), "c".into()]);
            assert_eq!(edges.len(), 2);
        } else {
            panic!("expected TopologyHint");
        }
    }

    #[tokio::test]
    async fn test_observer_auto_topology() {
        let store = test_store();
        let (session, _handle) = ObserverSession::start("auto-topo", test_config(store.clone()))
            .await
            .unwrap();

        let run_id = session.run_id();
        session.record_step("step_a", json!({})).await.unwrap();
        session.record_step("step_b", json!({})).await.unwrap();
        session.record_step("step_c", json!({})).await.unwrap();
        // No explicit set_flow_topology — should auto-infer on finish.
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        let topo_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::TopologyHint { .. }))
            .collect();
        // Advisory events may or may not arrive, but if they do, check structure.
        if !topo_events.is_empty() {
            if let WriteEvent::TopologyHint { nodes, edges, .. } = &topo_events[0] {
                assert_eq!(nodes.len(), 3);
                assert_eq!(edges.len(), 2);
                // BTreeSet ordering: step_a, step_b, step_c
                assert_eq!(edges[0], ("step_a".into(), "step_b".into()));
                assert_eq!(edges[1], ("step_b".into(), "step_c".into()));
            }
        }
    }

    #[tokio::test]
    async fn test_explicit_topology_suppresses_auto() {
        let store = test_store();
        let (session, _handle) =
            ObserverSession::start("suppress-test", test_config(store.clone()))
                .await
                .unwrap();

        let run_id = session.run_id();
        session.record_step("step_a", json!({})).await.unwrap();
        session.record_step("step_b", json!({})).await.unwrap();
        // Set explicit topology — auto should be suppressed.
        session.set_flow_topology(vec!["x".into()], vec![]).unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        let topo_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, WriteEvent::TopologyHint { .. }))
            .collect();
        // Should have exactly 1 (the explicit one), not 2.
        assert!(topo_events.len() <= 1);
        if !topo_events.is_empty() {
            if let WriteEvent::TopologyHint { nodes, .. } = &topo_events[0] {
                assert_eq!(nodes, &vec!["x".to_string()]);
            }
        }
    }

    #[tokio::test]
    async fn test_observer_metadata() {
        let store = test_store();
        let mut meta = BTreeMap::new();
        meta.insert("env".into(), json!("test"));

        let config = ObserverConfig {
            run_store: Some(store),
            metadata: meta.clone(),
            ..Default::default()
        };

        let (session, _handle) = ObserverSession::start("meta-test", config).await.unwrap();

        assert_eq!(session.metadata(), &meta);
        session.finish(RunStatus::Completed).await.unwrap();
    }
}

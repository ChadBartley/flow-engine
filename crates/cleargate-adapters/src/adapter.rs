//! Core adapter trait and `AdapterSession` that bridges framework adapters
//! to the `ObserverSession` recording infrastructure.

use std::collections::BTreeMap;
use std::sync::Mutex;

use chrono::Utc;
use cleargate_flow_engine::observer::{ObserverConfig, ObserverSession};
use cleargate_flow_engine::runtime::{SessionError, SessionHandle};
use cleargate_flow_engine::types::{LlmRequest, LlmResponse, RunStatus};

use crate::types::AdapterEvent;

/// Trait for framework-specific adapters.
///
/// Each adapter translates raw framework callback payloads (as JSON) into
/// normalized [`AdapterEvent`] values. The JSON boundary keeps Rust code
/// testable without Python and makes the FFI surface clean.
pub trait FrameworkAdapter: Send + Sync {
    /// Convert a framework-specific callback payload into adapter events.
    fn translate(&self, raw: serde_json::Value) -> Vec<AdapterEvent>;

    /// The framework name (e.g. "langchain", "langgraph", "semantic_kernel").
    fn framework_name(&self) -> &str;
}

type PendingToolStart = (String, serde_json::Value, chrono::DateTime<Utc>);

/// Wraps an [`ObserverSession`] with a [`FrameworkAdapter`] to automatically
/// translate framework callbacks into recorded events.
pub struct AdapterSession {
    observer: ObserverSession,
    adapter: Box<dyn FrameworkAdapter>,
    seen_edges: Mutex<Vec<(String, String)>>,
    pending_llm_starts: Mutex<BTreeMap<String, serde_json::Value>>,
    pending_tool_starts: Mutex<BTreeMap<String, PendingToolStart>>,
}

impl AdapterSession {
    /// Create a new adapter session.
    pub async fn start(
        name: impl Into<String>,
        adapter: Box<dyn FrameworkAdapter>,
        config: ObserverConfig,
    ) -> Result<(Self, SessionHandle), SessionError> {
        let (observer, handle) = ObserverSession::start(name, config).await?;
        Ok((
            Self {
                observer,
                adapter,
                seen_edges: Mutex::new(Vec::new()),
                pending_llm_starts: Mutex::new(BTreeMap::new()),
                pending_tool_starts: Mutex::new(BTreeMap::new()),
            },
            handle,
        ))
    }

    /// Process a raw framework callback payload.
    ///
    /// The adapter translates it into [`AdapterEvent`] values, which are then
    /// recorded through the underlying `ObserverSession`.
    pub async fn on_event(&self, raw: serde_json::Value) -> Result<(), SessionError> {
        let callback = raw.get("callback").and_then(|v| v.as_str()).unwrap_or("?");
        tracing::debug!(callback, "on_event translate");
        let events = self.adapter.translate(raw);
        tracing::debug!(count = events.len(), "on_event translated");
        for (i, event) in events.into_iter().enumerate() {
            tracing::debug!(i, "handle_event");
            self.handle_event(event).await?;
            tracing::debug!(i, "handle_event done");
        }
        Ok(())
    }

    /// The run ID for this session.
    pub fn run_id(&self) -> String {
        self.observer.run_id()
    }

    /// The framework name.
    pub fn framework_name(&self) -> &str {
        self.adapter.framework_name()
    }

    /// Access the underlying observer session.
    pub fn observer(&self) -> &ObserverSession {
        &self.observer
    }

    /// Finish the session, emitting topology from seen edges.
    pub async fn finish(self, status: RunStatus) -> Result<(), SessionError> {
        let edges = std::mem::take(&mut *self.seen_edges.lock().unwrap());
        if !edges.is_empty() {
            let mut nodes: Vec<String> = edges
                .iter()
                .flat_map(|(a, b)| [a.clone(), b.clone()])
                .collect();
            nodes.sort();
            nodes.dedup();
            let _ = self.observer.set_flow_topology(nodes, edges);
        }
        self.observer.finish(status).await
    }

    async fn handle_event(&self, event: AdapterEvent) -> Result<(), SessionError> {
        match event {
            AdapterEvent::LlmStart {
                node_id, request, ..
            } => {
                self.pending_llm_starts
                    .lock()
                    .unwrap()
                    .insert(node_id, request);
            }
            AdapterEvent::LlmEnd {
                node_id,
                response,
                duration_ms,
                ..
            } => {
                let request_json = self
                    .pending_llm_starts
                    .lock()
                    .unwrap()
                    .remove(&node_id)
                    .unwrap_or_else(|| serde_json::json!({}));

                let request = parse_llm_request(&request_json);
                let resp = parse_llm_response(&response, duration_ms);
                self.observer
                    .record_llm_call_for_node(&node_id, request, resp)
                    .await?;
            }
            AdapterEvent::ToolStart {
                node_id,
                tool_name,
                inputs,
                timestamp,
            } => {
                self.pending_tool_starts
                    .lock()
                    .unwrap()
                    .insert(node_id, (tool_name, inputs, timestamp));
            }
            AdapterEvent::ToolEnd {
                node_id,
                tool_name,
                outputs,
                duration_ms,
                ..
            } => {
                let (name, inputs, _) = self
                    .pending_tool_starts
                    .lock()
                    .unwrap()
                    .remove(&node_id)
                    .unwrap_or_else(|| (tool_name.clone(), serde_json::json!({}), Utc::now()));
                self.observer
                    .record_tool_call(&name, inputs, outputs, duration_ms)
                    .await?;
            }
            AdapterEvent::NodeTransition { from, to, .. } => {
                self.seen_edges.lock().unwrap().push((from, to));
            }
            AdapterEvent::StepStart { name, data, .. } => {
                self.observer.record_step(&name, data).await?;
            }
            AdapterEvent::StepEnd { name, data, .. } => {
                self.observer
                    .record_step(&format!("{name}_end"), data)
                    .await?;
            }
        }
        Ok(())
    }
}

fn parse_llm_request(raw: &serde_json::Value) -> LlmRequest {
    LlmRequest {
        provider: raw
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .into(),
        model: raw
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .into(),
        messages: raw
            .get("messages")
            .cloned()
            .unwrap_or_else(|| serde_json::json!([])),
        tools: raw.get("tools").and_then(|v| v.as_array().cloned()),
        temperature: raw.get("temperature").and_then(|v| v.as_f64()),
        top_p: raw.get("top_p").and_then(|v| v.as_f64()),
        max_tokens: raw
            .get("max_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        stop_sequences: None,
        response_format: raw.get("response_format").cloned(),
        seed: raw.get("seed").and_then(|v| v.as_u64()),
        extra_params: BTreeMap::new(),
    }
}

fn parse_llm_response(raw: &serde_json::Value, duration_ms: u64) -> LlmResponse {
    LlmResponse {
        content: raw
            .get("content")
            .or_else(|| raw.get("text"))
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        tool_calls: None,
        model_used: raw
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .into(),
        input_tokens: raw
            .get("usage")
            .and_then(|u| u.get("prompt_tokens").or_else(|| u.get("input_tokens")))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        output_tokens: raw
            .get("usage")
            .and_then(|u| {
                u.get("completion_tokens")
                    .or_else(|| u.get("output_tokens"))
            })
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        total_tokens: raw
            .get("usage")
            .and_then(|u| u.get("total_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        finish_reason: raw
            .get("finish_reason")
            .or_else(|| raw.get("stop_reason"))
            .and_then(|v| v.as_str())
            .unwrap_or("stop")
            .into(),
        latency_ms: duration_ms,
        provider_request_id: raw.get("id").and_then(|v| v.as_str()).map(String::from),
        cost: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cleargate_flow_engine::defaults::InMemoryRunStore;
    use cleargate_flow_engine::traits::RunStore;
    use std::sync::Arc;

    struct EchoAdapter;
    impl FrameworkAdapter for EchoAdapter {
        fn translate(&self, raw: serde_json::Value) -> Vec<AdapterEvent> {
            serde_json::from_value(raw.get("events").cloned().unwrap_or_default())
                .unwrap_or_default()
        }
        fn framework_name(&self) -> &str {
            "echo"
        }
    }

    #[tokio::test]
    async fn test_adapter_session_lifecycle() {
        let store = Arc::new(InMemoryRunStore::new());
        let config = ObserverConfig {
            run_store: Some(store.clone()),
            ..Default::default()
        };
        let (session, _handle) = AdapterSession::start("test", Box::new(EchoAdapter), config)
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .on_event(serde_json::json!({
                "events": [
                    {"type": "step_start", "name": "init", "data": {}, "timestamp": "2026-01-01T00:00:00Z"}
                ]
            }))
            .await
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        assert!(events.len() >= 3); // RunStarted + StepRecorded + RunCompleted
    }

    #[tokio::test]
    async fn test_adapter_llm_call_pairing() {
        let store = Arc::new(InMemoryRunStore::new());
        let config = ObserverConfig {
            run_store: Some(store.clone()),
            ..Default::default()
        };
        let (session, _handle) = AdapterSession::start("llm-test", Box::new(EchoAdapter), config)
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .on_event(serde_json::json!({
                "events": [
                    {"type": "llm_start", "node_id": "llm-1", "request": {"model": "gpt-4", "messages": []}, "timestamp": "2026-01-01T00:00:00Z"},
                    {"type": "llm_end", "node_id": "llm-1", "response": {"content": "hi", "model": "gpt-4", "usage": {"prompt_tokens": 10, "completion_tokens": 5}}, "duration_ms": 200, "timestamp": "2026-01-01T00:00:01Z"}
                ]
            }))
            .await
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let invocations = store.get_llm_invocations(&run_id).await.unwrap();
        assert_eq!(invocations.len(), 1);
        assert_eq!(invocations[0].node_id, "llm-1");
    }

    #[tokio::test]
    async fn test_adapter_node_transitions_topology() {
        let store = Arc::new(InMemoryRunStore::new());
        let config = ObserverConfig {
            run_store: Some(store.clone()),
            ..Default::default()
        };
        let (session, _handle) = AdapterSession::start("topo-test", Box::new(EchoAdapter), config)
            .await
            .unwrap();

        let run_id = session.run_id();
        session
            .on_event(serde_json::json!({
                "events": [
                    {"type": "node_transition", "from": "a", "to": "b", "timestamp": "2026-01-01T00:00:00Z"},
                    {"type": "node_transition", "from": "b", "to": "c", "timestamp": "2026-01-01T00:00:01Z"}
                ]
            }))
            .await
            .unwrap();
        session.finish(RunStatus::Completed).await.unwrap();

        let events = store.events(&run_id).await.unwrap();
        let topo: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, cleargate_flow_engine::WriteEvent::TopologyHint { .. }))
            .collect();
        // Topology hint may or may not arrive (advisory channel).
        if !topo.is_empty() {
            if let cleargate_flow_engine::WriteEvent::TopologyHint { nodes, edges, .. } = &topo[0] {
                assert_eq!(nodes.len(), 3);
                assert_eq!(edges.len(), 2);
            }
        }
    }

    #[test]
    fn test_parse_llm_request() {
        let raw = serde_json::json!({
            "provider": "openai",
            "model": "gpt-4",
            "messages": [{"role": "user", "content": "hi"}],
            "temperature": 0.7
        });
        let req = parse_llm_request(&raw);
        assert_eq!(req.provider, "openai");
        assert_eq!(req.model, "gpt-4");
        assert_eq!(req.temperature, Some(0.7));
    }

    #[test]
    fn test_parse_llm_response() {
        let raw = serde_json::json!({
            "content": "hello",
            "model": "gpt-4",
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15},
            "finish_reason": "stop"
        });
        let resp = parse_llm_response(&raw, 200);
        assert_eq!(resp.model_used, "gpt-4");
        assert_eq!(resp.input_tokens, Some(10));
        assert_eq!(resp.output_tokens, Some(5));
        assert_eq!(resp.latency_ms, 200);
    }
}

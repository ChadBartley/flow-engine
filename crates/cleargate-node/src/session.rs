//! napi-rs wrapper for ObserverSession.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi_derive::napi;

use cleargate_flow_engine::defaults::InMemoryRunStore;
use cleargate_flow_engine::observer::{ObserverConfig, ObserverSession};
use cleargate_flow_engine::traits::RunStore;
use cleargate_flow_engine::types::{LlmRequest, LlmResponse, RunStatus};

use crate::runtime::block_on;

#[napi]
pub struct NapiObserverSession {
    inner: Arc<Mutex<Option<ObserverSession>>>,
    finished: AtomicBool,
    run_id_value: String,
    store: Arc<dyn RunStore>,
}

#[napi]
impl NapiObserverSession {
    /// Start a new observer session.
    #[napi(factory)]
    pub fn start(name: String, store_url: Option<String>) -> Result<NapiObserverSession> {
        let store: Arc<dyn RunStore> = if let Some(url) = store_url.as_deref() {
            let db = block_on(cleargate_storage_oss::connect(url))
                .map_err(|e| Error::from_reason(format!("DB connect failed: {e}")))?;
            block_on(cleargate_storage_oss::run_migrations(&db))
                .map_err(|e| Error::from_reason(format!("DB migration failed: {e}")))?;
            Arc::new(cleargate_storage_oss::SeaOrmRunStore::new(Arc::new(db)))
        } else {
            Arc::new(InMemoryRunStore::new())
        };

        let config = ObserverConfig {
            run_store: Some(store.clone()),
            ..Default::default()
        };

        let (session, _handle) = block_on(ObserverSession::start(&name, config))
            .map_err(|e| Error::from_reason(format!("failed to start session: {e}")))?;

        let run_id_value = session.run_id();
        Ok(Self {
            inner: Arc::new(Mutex::new(Some(session))),
            finished: AtomicBool::new(false),
            run_id_value,
            store,
        })
    }

    /// The run ID.
    #[napi(getter)]
    pub fn run_id(&self) -> String {
        self.run_id_value.clone()
    }

    /// Record an LLM call.
    #[napi]
    pub fn record_llm_call(
        &self,
        node_id: String,
        request: serde_json::Value,
        response: serde_json::Value,
    ) -> Result<()> {
        let session = self.get_session()?;
        let req = json_to_llm_request(&request);
        let resp = json_to_llm_response(&response);

        block_on(session.record_llm_call_for_node(&node_id, req, resp))
            .map_err(|e| Error::from_reason(format!("record failed: {e}")))
    }

    /// Record a tool call.
    #[napi]
    pub fn record_tool_call(
        &self,
        tool_name: String,
        inputs: serde_json::Value,
        outputs: serde_json::Value,
        duration_ms: u32,
    ) -> Result<()> {
        let session = self.get_session()?;

        block_on(session.record_tool_call(&tool_name, inputs, outputs, duration_ms as u64))
            .map_err(|e| Error::from_reason(format!("record failed: {e}")))
    }

    /// Record a named step.
    #[napi]
    pub fn record_step(&self, step_name: String, data: serde_json::Value) -> Result<()> {
        let session = self.get_session()?;

        block_on(session.record_step(&step_name, data))
            .map_err(|e| Error::from_reason(format!("record failed: {e}")))
    }

    /// Finish the session.
    #[napi]
    pub fn finish(&self, status: Option<String>) -> Result<()> {
        let run_status = parse_status(status.as_deref());

        self.finished.store(true, Ordering::Release);

        let session = self
            .inner
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Error::from_reason("session already finished"))?;

        block_on(session.finish(run_status))
            .map_err(|e| Error::from_reason(format!("finish failed: {e}")))
    }

    /// Return the run summary (metadata, status, timing, aggregate LLM stats).
    /// Does not include the detailed event log — use `getEvents()` for that.
    /// Together they provide the complete picture for DiffEngine and ReplayEngine.
    #[napi]
    pub fn get_run_data(&self) -> Result<serde_json::Value> {
        let record = block_on(self.store.get_run(&self.run_id_value))
            .map_err(|e| Error::from_reason(format!("get_run failed: {e}")))?
            .ok_or_else(|| Error::from_reason("run not found"))?;

        serde_json::to_value(&record)
            .map_err(|e| Error::from_reason(format!("serialize failed: {e}")))
    }

    /// Return the full event log as a JSON array.
    ///
    /// Each event is a WriteEvent — LLM calls, tool invocations, steps,
    /// topology hints, etc. This is the detailed record consumed by
    /// DiffEngine and ReplayEngine.
    #[napi]
    pub fn get_events(&self) -> Result<serde_json::Value> {
        let events = block_on(self.store.events(&self.run_id_value))
            .map_err(|e| Error::from_reason(format!("events failed: {e}")))?;

        serde_json::to_value(&events)
            .map_err(|e| Error::from_reason(format!("serialize failed: {e}")))
    }
}

impl NapiObserverSession {
    /// Get a reference to the inner session, checking liveness.
    ///
    /// Unlike the Python bindings, Node.js is single-threaded so we do not
    /// need the raw-pointer trick to avoid reentrant mutex deadlocks. We can
    /// simply lock, get a pointer, and drop the guard before calling block_on.
    fn get_session(&self) -> Result<&ObserverSession> {
        if self.finished.load(Ordering::Acquire) {
            return Err(Error::from_reason("session already finished"));
        }
        let guard = self.inner.lock().unwrap();
        match guard.as_ref() {
            Some(s) => {
                // SAFETY: The session lives in Arc<Mutex<Option<_>>> and is only
                // removed by finish(), which sets the AtomicBool first. Node.js
                // callbacks are single-threaded, so no concurrent finish().
                let ptr = s as *const ObserverSession;
                drop(guard);
                Ok(unsafe { &*ptr })
            }
            None => Err(Error::from_reason("session already finished")),
        }
    }
}

fn parse_status(status: Option<&str>) -> RunStatus {
    match status.unwrap_or("completed") {
        "failed" => RunStatus::Failed,
        "cancelled" => RunStatus::Cancelled,
        _ => RunStatus::Completed,
    }
}

fn json_to_llm_request(raw: &serde_json::Value) -> LlmRequest {
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

fn json_to_llm_response(raw: &serde_json::Value) -> LlmResponse {
    LlmResponse {
        content: raw
            .get("content")
            .cloned()
            .unwrap_or_else(|| serde_json::json!("")),
        tool_calls: None,
        model_used: raw
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .into(),
        input_tokens: raw
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        output_tokens: raw
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        total_tokens: raw
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
        finish_reason: raw
            .get("finish_reason")
            .and_then(|v| v.as_str())
            .unwrap_or("stop")
            .into(),
        latency_ms: raw.get("latency_ms").and_then(|v| v.as_u64()).unwrap_or(0),
        provider_request_id: None,
        cost: None,
    }
}

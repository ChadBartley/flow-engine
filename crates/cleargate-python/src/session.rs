//! Python wrapper for ObserverSession.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use cleargate_flow_engine::defaults::InMemoryRunStore;
use cleargate_flow_engine::observer::{ObserverConfig, ObserverSession};
use cleargate_flow_engine::traits::RunStore;
use cleargate_flow_engine::types::{LlmRequest, LlmResponse, RunStatus};

use crate::runtime::block_on;

/// Obtain a raw pointer to the session, dropping the mutex guard immediately.
///
/// SAFETY: see [`PyObserverSession::record_llm_call`] for the invariants.
macro_rules! session_ptr {
    ($this:expr) => {{
        if $this.finished.load(Ordering::Acquire) {
            return Err(PyRuntimeError::new_err("session already finished"));
        }
        let guard = $this.inner.lock().unwrap();
        match guard.as_ref() {
            Some(s) => {
                let ptr = s as *const ObserverSession as usize;
                drop(guard);
                ptr
            }
            None => return Err(PyRuntimeError::new_err("session already finished")),
        }
    }};
}

/// Python-facing observer session for recording LLM calls and steps.
///
/// Uses the same pointer-based approach as [`PyAdapterSession`] to avoid
/// reentrant mutex deadlocks from LangChain's nested callbacks.
#[pyclass(name = "ObserverSession")]
pub struct PyObserverSession {
    inner: Arc<Mutex<Option<ObserverSession>>>,
    finished: AtomicBool,
    run_id: String,
    store: Arc<dyn RunStore>,
}

fn py_to_json(py: Python<'_>, obj: &PyObject) -> PyResult<serde_json::Value> {
    let json_mod = py.import("json")?;
    let json_str: String = json_mod.call_method1("dumps", (obj,))?.extract()?;
    serde_json::from_str(&json_str)
        .map_err(|e| PyRuntimeError::new_err(format!("JSON parse error: {e}")))
}

#[pymethods]
impl PyObserverSession {
    /// Start a new observer session.
    #[staticmethod]
    #[pyo3(signature = (name, store_path=None))]
    fn start(name: &str, store_path: Option<&str>) -> PyResult<Self> {
        let store: Arc<dyn RunStore> = if let Some(url) = store_path {
            let db = block_on(cleargate_storage_oss::connect(url))
                .map_err(|e| PyRuntimeError::new_err(format!("DB connect failed: {e}")))?;
            block_on(cleargate_storage_oss::run_migrations(&db))
                .map_err(|e| PyRuntimeError::new_err(format!("DB migration failed: {e}")))?;
            Arc::new(cleargate_storage_oss::SeaOrmRunStore::new(Arc::new(db)))
        } else {
            Arc::new(InMemoryRunStore::new())
        };

        let config = ObserverConfig {
            run_store: Some(store.clone()),
            ..Default::default()
        };

        let (session, _handle) = block_on(ObserverSession::start(name, config))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to start session: {e}")))?;

        let run_id = session.run_id();
        Ok(Self {
            inner: Arc::new(Mutex::new(Some(session))),
            finished: AtomicBool::new(false),
            run_id,
            store,
        })
    }

    /// Record an LLM call.
    ///
    /// Does NOT hold the mutex across `block_on` to avoid deadlocks from
    /// reentrant Python callbacks.
    ///
    /// SAFETY of the raw pointer:
    /// 1. The session lives in `Arc<Mutex<Option<_>>>` and is only removed
    ///    by `finish()`, which sets the `AtomicBool` first.
    /// 2. Python callbacks are single-threaded (GIL).
    /// 3. All recording methods take `&self` — no mutation.
    #[pyo3(signature = (node_id, request, response))]
    fn record_llm_call(
        &self,
        py: Python<'_>,
        node_id: &str,
        request: PyObject,
        response: PyObject,
    ) -> PyResult<()> {
        let req_json = py_to_json(py, &request)?;
        let resp_json = py_to_json(py, &response)?;

        let req = json_to_llm_request(&req_json);
        let resp = json_to_llm_response(&resp_json);

        let addr = session_ptr!(self);
        py.allow_threads(|| {
            let session = unsafe { &*(addr as *const ObserverSession) };
            block_on(session.record_llm_call_for_node(node_id, req, resp))
                .map_err(|e| PyRuntimeError::new_err(format!("record failed: {e}")))
        })
    }

    /// Record a tool call.
    #[pyo3(signature = (tool_name, inputs, outputs, duration_ms))]
    fn record_tool_call(
        &self,
        py: Python<'_>,
        tool_name: &str,
        inputs: PyObject,
        outputs: PyObject,
        duration_ms: u64,
    ) -> PyResult<()> {
        let inputs_json = py_to_json(py, &inputs)?;
        let outputs_json = py_to_json(py, &outputs)?;

        let addr = session_ptr!(self);
        py.allow_threads(|| {
            let session = unsafe { &*(addr as *const ObserverSession) };
            block_on(session.record_tool_call(tool_name, inputs_json, outputs_json, duration_ms))
                .map_err(|e| PyRuntimeError::new_err(format!("record failed: {e}")))
        })
    }

    /// Record a named step.
    #[pyo3(signature = (step_name, data))]
    fn record_step(&self, py: Python<'_>, step_name: &str, data: PyObject) -> PyResult<()> {
        let data_json = py_to_json(py, &data)?;

        let addr = session_ptr!(self);
        py.allow_threads(|| {
            let session = unsafe { &*(addr as *const ObserverSession) };
            block_on(session.record_step(step_name, data_json))
                .map_err(|e| PyRuntimeError::new_err(format!("record failed: {e}")))
        })
    }

    /// Finish the session.
    #[pyo3(signature = (status="completed"))]
    fn finish(&self, py: Python<'_>, status: &str) -> PyResult<()> {
        let run_status = match status {
            "completed" => RunStatus::Completed,
            "failed" => RunStatus::Failed,
            "cancelled" => RunStatus::Cancelled,
            _ => RunStatus::Completed,
        };

        self.finished.store(true, Ordering::Release);

        let session = self
            .inner
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("session already finished"))?;

        py.allow_threads(|| {
            block_on(session.finish(run_status))
                .map_err(|e| PyRuntimeError::new_err(format!("finish failed: {e}")))
        })
    }

    /// The run ID.
    #[getter]
    fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Return the run summary (metadata, status, timing, LLM token stats)
    /// as a JSON-serializable Python dict. Does not include the detailed
    /// event log — use ``get_events()`` for that. Together they give the
    /// full picture needed by DiffEngine and ReplayEngine.
    fn get_run_data(&self, py: Python<'_>) -> PyResult<PyObject> {
        let record = block_on(self.store.get_run(&self.run_id))
            .map_err(|e| PyRuntimeError::new_err(format!("get_run failed: {e}")))?
            .ok_or_else(|| PyRuntimeError::new_err("run not found"))?;

        let json_str = serde_json::to_string(&record)
            .map_err(|e| PyRuntimeError::new_err(format!("serialize failed: {e}")))?;

        let json_mod = py.import("json")?;
        json_mod.call_method1("loads", (json_str,))?.extract()
    }

    /// Return the full event log as a list of JSON-serializable Python dicts.
    ///
    /// Each event is a ``WriteEvent`` — LLM calls, tool invocations, steps,
    /// topology hints, etc. This is the detailed record consumed by
    /// DiffEngine and ReplayEngine.
    fn get_events(&self, py: Python<'_>) -> PyResult<PyObject> {
        let events = block_on(self.store.events(&self.run_id))
            .map_err(|e| PyRuntimeError::new_err(format!("events failed: {e}")))?;

        let json_str = serde_json::to_string(&events)
            .map_err(|e| PyRuntimeError::new_err(format!("serialize failed: {e}")))?;

        let json_mod = py.import("json")?;
        json_mod.call_method1("loads", (json_str,))?.extract()
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    #[pyo3(signature = (_ty=None, _val=None, _tb=None))]
    fn __exit__(
        &self,
        py: Python<'_>,
        _ty: Option<PyObject>,
        _val: Option<PyObject>,
        _tb: Option<PyObject>,
    ) -> PyResult<bool> {
        let has_exception = _ty.is_some();
        let status = if has_exception { "failed" } else { "completed" };
        self.finish(py, status)?;
        Ok(false)
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

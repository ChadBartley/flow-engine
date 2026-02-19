//! PyO3 wrappers for framework adapters.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use cleargate_adapters::{
    AdapterSession, LangChainAdapter, LangGraphAdapter, SemanticKernelAdapter,
};
use cleargate_flow_engine::defaults::InMemoryRunStore;
use cleargate_flow_engine::observer::ObserverConfig;
use cleargate_flow_engine::traits::RunStore;
use cleargate_flow_engine::types::RunStatus;

use crate::runtime::block_on;

/// Python-facing adapter session wrapping a framework adapter + observer.
///
/// LangChain fires callbacks **reentrantly** — e.g. `on_chain_start` inside
/// another `on_chain_start` — on the same thread. A `std::sync::Mutex` held
/// across `block_on` will deadlock because the same thread tries to lock it
/// again when the nested callback fires.
///
/// To avoid this, `on_event()` uses an `AtomicBool` to check liveness and
/// accesses the session through a raw pointer. This is safe because:
///   1. `on_event` takes `&self` on `AdapterSession` (no mutation).
///   2. Python callbacks are single-threaded (GIL serializes them).
///   3. `finish()` sets the flag before consuming the session.
#[pyclass(name = "AdapterSession")]
pub struct PyAdapterSession {
    inner: Arc<Mutex<Option<AdapterSession>>>,
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
impl PyAdapterSession {
    /// Start a new adapter session for a given framework.
    #[staticmethod]
    #[pyo3(signature = (framework, name=None, store_path=None))]
    fn start(framework: &str, name: Option<&str>, store_path: Option<&str>) -> PyResult<Self> {
        let adapter: Box<dyn cleargate_adapters::FrameworkAdapter> = match framework {
            "langchain" => Box::new(LangChainAdapter),
            "langgraph" => Box::new(LangGraphAdapter),
            "semantic_kernel" => Box::new(SemanticKernelAdapter),
            other => {
                return Err(PyRuntimeError::new_err(format!(
                    "unknown framework: {other}. Supported: langchain, langgraph, semantic_kernel"
                )))
            }
        };

        let session_name = name.unwrap_or(framework);

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

        let (session, _handle) = block_on(AdapterSession::start(session_name, adapter, config))
            .map_err(|e| PyRuntimeError::new_err(format!("failed to start: {e}")))?;

        let run_id = session.run_id();
        Ok(Self {
            inner: Arc::new(Mutex::new(Some(session))),
            finished: AtomicBool::new(false),
            run_id,
            store,
        })
    }

    /// Process a framework callback event (as a dict).
    ///
    /// Does NOT hold the mutex across `block_on` to avoid deadlocks from
    /// LangChain's reentrant callbacks. Uses a raw pointer to the session
    /// which is safe because `on_event` is `&self` and Python callbacks are
    /// single-threaded.
    fn on_event(&self, py: Python<'_>, event: PyObject) -> PyResult<()> {
        if self.finished.load(Ordering::Acquire) {
            return Err(PyRuntimeError::new_err("session already finished"));
        }

        let json = py_to_json(py, &event)?;

        // Get a pointer to the session without holding the lock across block_on.
        let session_ptr = {
            let guard = self.inner.lock().unwrap();
            match guard.as_ref() {
                Some(session) => session as *const AdapterSession as usize,
                None => {
                    return Err(PyRuntimeError::new_err("session already finished"));
                }
            }
        };
        // MutexGuard is dropped here.

        // SAFETY: The pointer is valid because:
        // 1. The session lives in Arc<Mutex<Option<_>>> and is only removed by finish().
        // 2. finish() sets the AtomicBool first, so we won't reach here after finish.
        // 3. Python callbacks are single-threaded (GIL), so no concurrent finish().
        // 4. on_event takes &self, no mutation occurs.
        let session_addr = session_ptr;
        py.allow_threads(|| {
            let session = unsafe { &*(session_addr as *const AdapterSession) };
            block_on(session.on_event(json))
                .map_err(|e| PyRuntimeError::new_err(format!("event processing failed: {e}")))
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

        // Set the flag first so any concurrent on_event call sees it.
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

    #[getter]
    fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Return the run summary (metadata, status, timing, LLM token stats)
    /// as a JSON-serializable Python dict. Does not include the detailed
    /// event log — use ``get_events()`` for that.
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

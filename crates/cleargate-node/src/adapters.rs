//! napi-rs wrappers for framework adapters.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use napi::bindgen_prelude::*;
use napi_derive::napi;

use cleargate_adapters::{
    AdapterSession, LangChainAdapter, LangGraphAdapter, SemanticKernelAdapter,
};
use cleargate_flow_engine::defaults::InMemoryRunStore;
use cleargate_flow_engine::observer::ObserverConfig;
use cleargate_flow_engine::traits::RunStore;
use cleargate_flow_engine::types::RunStatus;

use crate::runtime::block_on;

#[napi]
pub struct NapiAdapterSession {
    inner: Arc<Mutex<Option<AdapterSession>>>,
    finished: AtomicBool,
    run_id_value: String,
    store: Arc<dyn RunStore>,
}

#[napi]
impl NapiAdapterSession {
    /// Start a new adapter session for a given framework.
    #[napi(factory)]
    pub fn start(framework: String, name: Option<String>) -> Result<NapiAdapterSession> {
        let adapter: Box<dyn cleargate_adapters::FrameworkAdapter> = match framework.as_str() {
            "langchain" => Box::new(LangChainAdapter),
            "langgraph" => Box::new(LangGraphAdapter),
            "semantic_kernel" => Box::new(SemanticKernelAdapter),
            other => {
                return Err(Error::from_reason(format!(
                    "unknown framework: {other}. Supported: langchain, langgraph, semantic_kernel"
                )));
            }
        };

        let session_name = name.as_deref().unwrap_or(&framework);

        let store: Arc<dyn RunStore> = Arc::new(InMemoryRunStore::new());

        let config = ObserverConfig {
            run_store: Some(store.clone()),
            ..Default::default()
        };

        let (session, _handle) = block_on(AdapterSession::start(session_name, adapter, config))
            .map_err(|e| Error::from_reason(format!("failed to start: {e}")))?;

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

    /// Process a framework callback event.
    #[napi]
    pub fn on_event(&self, event: serde_json::Value) -> Result<()> {
        if self.finished.load(Ordering::Acquire) {
            return Err(Error::from_reason("session already finished"));
        }

        // Get a pointer without holding the lock across block_on.
        let session_ptr = {
            let guard = self.inner.lock().unwrap();
            match guard.as_ref() {
                Some(session) => session as *const AdapterSession as usize,
                None => {
                    return Err(Error::from_reason("session already finished"));
                }
            }
        };

        // SAFETY: The pointer is valid because:
        // 1. The session lives in Arc<Mutex<Option<_>>> and is only removed by finish().
        // 2. finish() sets the AtomicBool first, so we won't reach here after finish.
        // 3. Node.js is single-threaded, so no concurrent finish().
        // 4. on_event takes &self, no mutation occurs.
        let session = unsafe { &*(session_ptr as *const AdapterSession) };
        block_on(session.on_event(event))
            .map_err(|e| Error::from_reason(format!("event processing failed: {e}")))
    }

    /// Finish the session.
    #[napi]
    pub fn finish(&self, status: Option<String>) -> Result<()> {
        let run_status = match status.as_deref().unwrap_or("completed") {
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
            .ok_or_else(|| Error::from_reason("session already finished"))?;

        block_on(session.finish(run_status))
            .map_err(|e| Error::from_reason(format!("finish failed: {e}")))
    }

    /// Return the run summary (metadata, status, timing, aggregate LLM stats).
    /// Does not include the detailed event log — use `getEvents()` for that.
    #[napi]
    pub fn get_run_data(&self) -> Result<serde_json::Value> {
        let record = block_on(self.store.get_run(&self.run_id_value))
            .map_err(|e| Error::from_reason(format!("get_run failed: {e}")))?
            .ok_or_else(|| Error::from_reason("run not found"))?;

        serde_json::to_value(&record)
            .map_err(|e| Error::from_reason(format!("serialize failed: {e}")))
    }

    /// Return the full event log as a JSON array (LLM calls, tool invocations,
    /// steps, topology hints, etc.). This is the detailed record consumed by
    /// DiffEngine and ReplayEngine.
    #[napi]
    pub fn get_events(&self) -> Result<serde_json::Value> {
        let events = block_on(self.store.events(&self.run_id_value))
            .map_err(|e| Error::from_reason(format!("events failed: {e}")))?;

        serde_json::to_value(&events)
            .map_err(|e| Error::from_reason(format!("serialize failed: {e}")))
    }
}

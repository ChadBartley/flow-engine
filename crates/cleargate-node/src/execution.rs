//! Node.js wrapper for `ExecutionHandle`.
//!
//! Provides `nextEvent()` for polling and `cancel()` / `wait()`.

use std::sync::Mutex;

use napi::bindgen_prelude::*;
use napi_derive::napi;

use cleargate_flow_engine::executor::ExecutionHandle;
use cleargate_flow_engine::write_event::WriteEvent;
use tokio::sync::{broadcast, oneshot};

use crate::runtime::block_on;

/// Node.js-facing execution handle.
#[napi]
pub struct NapiExecutionHandle {
    run_id: String,
    events: Mutex<broadcast::Receiver<WriteEvent>>,
    cancel: Mutex<Option<oneshot::Sender<()>>>,
}

impl NapiExecutionHandle {
    pub fn new(handle: ExecutionHandle) -> Self {
        Self {
            run_id: handle.run_id,
            events: Mutex::new(handle.events),
            cancel: Mutex::new(Some(handle.cancel)),
        }
    }
}

#[napi]
impl NapiExecutionHandle {
    /// The unique run ID for this execution.
    #[napi(getter)]
    pub fn run_id(&self) -> String {
        self.run_id.clone()
    }

    /// Poll for the next event. Returns a JSON object or `null` if the stream is closed.
    #[napi]
    pub fn next_event(&self) -> Result<Option<serde_json::Value>> {
        let mut rx = self.events.lock().unwrap();
        loop {
            match block_on(rx.recv()) {
                Ok(event) => {
                    return serde_json::to_value(&event)
                        .map(Some)
                        .map_err(|e| Error::from_reason(format!("serialize failed: {e}")));
                }
                Err(broadcast::error::RecvError::Closed) => return Ok(None),
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(skipped = n, "Node.js event receiver lagged");
                    continue;
                }
            }
        }
    }

    /// Cancel the running execution.
    #[napi]
    pub fn cancel(&self) -> Result<()> {
        let sender = self
            .cancel
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| Error::from_reason("already cancelled"))?;
        let _ = sender.send(());
        Ok(())
    }

    /// Block until the execution completes. Returns all events as a JSON array.
    #[napi]
    pub fn wait_for_completion(&self) -> Result<serde_json::Value> {
        let mut all_events: Vec<serde_json::Value> = Vec::new();
        while let Some(event) = self.next_event()? {
            all_events.push(event);
        }
        Ok(serde_json::Value::Array(all_events))
    }
}

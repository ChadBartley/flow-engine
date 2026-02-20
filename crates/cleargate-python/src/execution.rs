//! Python wrapper for `ExecutionHandle`.
//!
//! Provides event streaming via `next_event()` polling and `wait()` for
//! blocking until completion.

use std::sync::Mutex;

use pyo3::exceptions::{PyRuntimeError, PyStopIteration};
use pyo3::prelude::*;

use cleargate_flow_engine::executor::ExecutionHandle;
use cleargate_flow_engine::write_event::WriteEvent;
use tokio::sync::{broadcast, oneshot};

use crate::runtime::block_on;

/// Python-facing execution handle for a running flow.
///
/// Supports event polling via `next_event()` and cancellation via `cancel()`.
/// Also implements the iterator protocol for `for event in handle:` loops.
#[pyclass(name = "ExecutionHandle")]
pub struct PyExecutionHandle {
    run_id: String,
    events: Mutex<broadcast::Receiver<WriteEvent>>,
    cancel: Mutex<Option<oneshot::Sender<()>>>,
}

impl PyExecutionHandle {
    pub fn new(handle: ExecutionHandle) -> Self {
        Self {
            run_id: handle.run_id,
            events: Mutex::new(handle.events),
            cancel: Mutex::new(Some(handle.cancel)),
        }
    }
}

/// Receive the next event from the broadcast channel.
///
/// This is extracted as a free function so we can call it inside
/// `py.allow_threads` without holding the `MutexGuard` across the
/// GIL boundary.
fn recv_next(rx: &Mutex<broadcast::Receiver<WriteEvent>>) -> Option<WriteEvent> {
    // We use spawn_blocking + oneshot to receive without holding the MutexGuard
    // across the await point. Instead, we lock briefly, clone the receiver state
    // via resubscribe if needed, and recv.
    //
    // Simpler approach: lock, take a reference, block_on recv, drop lock.
    // This works because block_on runs on the current thread (not a tokio worker).
    let mut guard = rx.lock().unwrap();
    loop {
        match block_on(guard.recv()) {
            Ok(event) => return Some(event),
            Err(broadcast::error::RecvError::Closed) => return None,
            Err(broadcast::error::RecvError::Lagged(n)) => {
                tracing::warn!(skipped = n, "Python event receiver lagged");
                continue;
            }
        }
    }
}

fn event_to_py(py: Python<'_>, event: &WriteEvent) -> PyResult<PyObject> {
    let json_str = serde_json::to_string(event)
        .map_err(|e| PyRuntimeError::new_err(format!("serialize failed: {e}")))?;
    let json_mod = py.import("json")?;
    json_mod.call_method1("loads", (json_str,))?.extract()
}

#[pymethods]
impl PyExecutionHandle {
    /// The unique run ID for this execution.
    #[getter]
    fn run_id(&self) -> &str {
        &self.run_id
    }

    /// Poll for the next event. Returns a dict or `None` if the stream is closed.
    ///
    /// Blocks the calling thread until an event is available.
    /// Releases the GIL while waiting.
    fn next_event(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        // Release the GIL while blocking on the event channel.
        let event = py.allow_threads(|| recv_next(&self.events));
        match event {
            Some(ev) => Ok(Some(event_to_py(py, &ev)?)),
            None => Ok(None),
        }
    }

    /// Cancel the running execution.
    fn cancel(&self) -> PyResult<()> {
        let sender = self
            .cancel
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("already cancelled"))?;
        let _ = sender.send(());
        Ok(())
    }

    /// Block until the execution completes. Returns all events as a list of dicts.
    fn wait(&self, py: Python<'_>) -> PyResult<PyObject> {
        let mut events = Vec::new();
        loop {
            let event = py.allow_threads(|| recv_next(&self.events));
            match event {
                Some(ev) => events.push(event_to_py(py, &ev)?),
                None => break,
            }
        }
        Ok(pyo3::types::PyList::new(py, events)?.into_any().unbind())
    }

    fn __iter__(slf: PyRef<'_, Self>) -> PyRef<'_, Self> {
        slf
    }

    fn __next__(&self, py: Python<'_>) -> PyResult<Option<PyObject>> {
        let event = py.allow_threads(|| recv_next(&self.events));
        match event {
            Some(ev) => Ok(Some(event_to_py(py, &ev)?)),
            None => Err(PyStopIteration::new_err("")),
        }
    }
}

//! Python wrapper for `EngineBuilder` and `Engine`.
//!
//! Exposes the builder → engine → execution pipeline to Python.
//! All cross-language types are marshaled as JSON dicts via `serde_json::Value`.

use std::path::PathBuf;
use std::sync::Arc;

use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;

use cleargate_flow_engine::engine::{Engine, EngineBuilder};
use cleargate_flow_engine::executor::ExecutorConfig;
use cleargate_flow_engine::traits::FlowLlmProvider;
use cleargate_flow_engine::types::GraphDef;

use crate::execution::PyExecutionHandle;
use crate::llm_provider::PyLlmProviderBridge;
use crate::node_handler::PyNodeHandlerBridge;
use crate::runtime::block_on;

/// Python-facing engine builder.
///
/// Mirrors `EngineBuilder` with a mutable-option pattern: methods consume
/// the inner builder and replace it. `build()` takes it and returns a `PyEngine`.
#[pyclass(name = "EngineBuilder")]
pub struct PyEngineBuilder {
    inner: Option<EngineBuilder>,
}

#[pymethods]
impl PyEngineBuilder {
    /// Create a new engine builder with default settings.
    #[new]
    fn new() -> Self {
        Self {
            inner: Some(Engine::builder()),
        }
    }

    /// Set the directory for flow store (JSON flow definitions).
    fn flow_store_path(&mut self, path: &str) -> PyResult<()> {
        let builder = self.take_builder()?;
        let store = cleargate_flow_engine::defaults::FileFlowStore::new(PathBuf::from(path))
            .map_err(|e| PyRuntimeError::new_err(format!("flow store error: {e}")))?;
        self.inner = Some(builder.flow_store(store));
        Ok(())
    }

    /// Set the directory for run store (execution records).
    fn run_store_path(&mut self, path: &str) -> PyResult<()> {
        let builder = self.take_builder()?;
        let store = cleargate_flow_engine::defaults::FileRunStore::new(PathBuf::from(path))
            .map_err(|e| PyRuntimeError::new_err(format!("run store error: {e}")))?;
        self.inner = Some(builder.run_store(store));
        Ok(())
    }

    /// Set a database URL for the run store (SQLite or PostgreSQL).
    fn run_store_url(&mut self, url: &str) -> PyResult<()> {
        let builder = self.take_builder()?;
        let db = block_on(cleargate_storage_oss::connect(url))
            .map_err(|e| PyRuntimeError::new_err(format!("DB connect failed: {e}")))?;
        block_on(cleargate_storage_oss::run_migrations(&db))
            .map_err(|e| PyRuntimeError::new_err(format!("DB migration failed: {e}")))?;
        let store = cleargate_storage_oss::SeaOrmRunStore::new(Arc::new(db));
        self.inner = Some(builder.run_store(store));
        Ok(())
    }

    /// Register a Python LLM provider by name.
    ///
    /// The provider must be an object with a `complete(request_dict) -> response_dict`
    /// method and a `name` property.
    fn llm_provider(&mut self, name: &str, provider: PyObject) -> PyResult<()> {
        let builder = self.take_builder()?;
        let bridge = Arc::new(PyLlmProviderBridge::new(provider)) as Arc<dyn FlowLlmProvider>;
        self.inner = Some(builder.llm_provider(name, bridge));
        Ok(())
    }

    /// Register a Python custom node handler.
    ///
    /// The handler must be an object with `meta() -> dict` and
    /// `run(inputs, config, context) -> dict` methods.
    fn node(&mut self, handler: PyObject) -> PyResult<()> {
        let builder = self.take_builder()?;
        let bridge = PyNodeHandlerBridge::new(handler)?;
        self.inner = Some(builder.node(bridge));
        Ok(())
    }

    /// Set the maximum edge traversals for cycle detection. Default: 50.
    fn max_traversals(&mut self, value: u32) -> PyResult<()> {
        let builder = self.take_builder()?;
        self.inner = Some(builder.executor_config(ExecutorConfig {
            max_traversals: value,
        }));
        Ok(())
    }

    /// Disable crash recovery (marking orphaned runs as interrupted).
    fn crash_recovery(&mut self, enabled: bool) -> PyResult<()> {
        let builder = self.take_builder()?;
        self.inner = Some(builder.crash_recovery(enabled));
        Ok(())
    }

    /// Build the engine. Consumes the builder.
    fn build(&mut self, py: Python<'_>) -> PyResult<PyEngine> {
        let builder = self.take_builder()?;
        let engine = py.allow_threads(|| {
            block_on(builder.build())
                .map_err(|e| PyRuntimeError::new_err(format!("engine build failed: {e}")))
        })?;
        Ok(PyEngine {
            inner: Arc::new(engine),
        })
    }
}

impl PyEngineBuilder {
    fn take_builder(&mut self) -> PyResult<EngineBuilder> {
        self.inner
            .take()
            .ok_or_else(|| PyRuntimeError::new_err("builder already consumed by build()"))
    }
}

/// Python-facing engine — the assembled runtime.
///
/// Wraps `Arc<Engine>` so it can be shared across Python references.
#[pyclass(name = "Engine")]
pub struct PyEngine {
    inner: Arc<Engine>,
}

#[pymethods]
impl PyEngine {
    /// Execute a flow by ID with the given inputs (a dict).
    ///
    /// Returns an `ExecutionHandle` for streaming events and cancellation.
    #[pyo3(signature = (flow_id, inputs=None))]
    fn execute(
        &self,
        py: Python<'_>,
        flow_id: &str,
        inputs: Option<PyObject>,
    ) -> PyResult<PyExecutionHandle> {
        let inputs_val = match inputs {
            Some(obj) => py_to_json(py, &obj)?,
            None => serde_json::json!({}),
        };
        let engine = Arc::clone(&self.inner);
        let flow_id = flow_id.to_string();

        let handle = py.allow_threads(|| {
            block_on(engine.execute(&flow_id, inputs_val, None))
                .map_err(|e| PyRuntimeError::new_err(format!("execute failed: {e}")))
        })?;

        Ok(PyExecutionHandle::new(handle))
    }

    /// Execute a graph definition directly (for ad-hoc graphs not stored in the flow store).
    ///
    /// `graph_json` is a dict matching the `GraphDef` schema.
    #[pyo3(signature = (graph_json, inputs=None))]
    fn execute_graph(
        &self,
        py: Python<'_>,
        graph_json: PyObject,
        inputs: Option<PyObject>,
    ) -> PyResult<PyExecutionHandle> {
        let graph_val = py_to_json(py, &graph_json)?;
        let graph: GraphDef = serde_json::from_value(graph_val)
            .map_err(|e| PyRuntimeError::new_err(format!("invalid graph JSON: {e}")))?;

        let inputs_val = match inputs {
            Some(obj) => py_to_json(py, &obj)?,
            None => serde_json::json!({}),
        };

        let engine = Arc::clone(&self.inner);
        let handle = py.allow_threads(|| {
            block_on(engine.execute_graph(
                &graph,
                "python-adhoc",
                inputs_val,
                cleargate_flow_engine::types::TriggerSource::Api {
                    request_id: uuid::Uuid::new_v4().to_string(),
                },
                None,
            ))
            .map_err(|e| PyRuntimeError::new_err(format!("execute_graph failed: {e}")))
        })?;

        Ok(PyExecutionHandle::new(handle))
    }

    /// Deliver input to a node waiting for human-in-the-loop.
    fn provide_input(
        &self,
        py: Python<'_>,
        run_id: &str,
        node_id: &str,
        input: PyObject,
    ) -> PyResult<()> {
        let input_val = py_to_json(py, &input)?;
        let engine = Arc::clone(&self.inner);
        let run_id = run_id.to_string();
        let node_id = node_id.to_string();

        py.allow_threads(|| {
            block_on(engine.provide_input(&run_id, &node_id, input_val))
                .map_err(|e| PyRuntimeError::new_err(format!("provide_input failed: {e}")))
        })
    }

    /// Return metadata for all registered node handlers.
    fn node_catalog(&self, py: Python<'_>) -> PyResult<PyObject> {
        let catalog = self.inner.node_catalog();
        let json_str = serde_json::to_string(&catalog)
            .map_err(|e| PyRuntimeError::new_err(format!("serialize failed: {e}")))?;
        let json_mod = py.import("json")?;
        json_mod.call_method1("loads", (json_str,))?.extract()
    }

    /// Gracefully shut down the engine.
    fn shutdown(&self, py: Python<'_>) -> PyResult<()> {
        let engine = Arc::clone(&self.inner);
        py.allow_threads(|| {
            block_on(engine.shutdown());
            Ok(())
        })
    }
}

fn py_to_json(py: Python<'_>, obj: &PyObject) -> PyResult<serde_json::Value> {
    let json_mod = py.import("json")?;
    let json_str: String = json_mod.call_method1("dumps", (obj,))?.extract()?;
    serde_json::from_str(&json_str)
        .map_err(|e| PyRuntimeError::new_err(format!("JSON parse error: {e}")))
}

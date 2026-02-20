//! Bridge between a Python node handler class and the Rust `NodeHandler` trait.
//!
//! A Python node handler must implement:
//! - `meta() -> dict` returning node metadata
//! - `run(inputs: dict, config: dict, context: dict) -> dict` executing the node

use async_trait::async_trait;
use pyo3::prelude::*;
use serde_json::Value;

use cleargate_flow_engine::node_ctx::NodeCtx;
use cleargate_flow_engine::traits::NodeHandler;
use cleargate_flow_engine::types::{NodeError, NodeMeta};

/// Rust-side bridge that holds a Python object and implements `NodeHandler`.
pub struct PyNodeHandlerBridge {
    py_handler: PyObject,
    cached_meta: NodeMeta,
}

// SAFETY: PyObject is Send+Sync when accessed only via Python::with_gil.
unsafe impl Send for PyNodeHandlerBridge {}
unsafe impl Sync for PyNodeHandlerBridge {}

impl PyNodeHandlerBridge {
    /// Create from a Python object. Calls `meta()` eagerly to cache metadata
    /// and validate the interface.
    pub fn new(py_handler: PyObject) -> PyResult<Self> {
        let meta = Python::with_gil(|py| {
            let meta_dict = py_handler.call_method0(py, "meta")?;
            let json_mod = py.import("json")?;
            let json_str: String = json_mod.call_method1("dumps", (meta_dict,))?.extract()?;
            let meta: NodeMeta = serde_json::from_str(&json_str).map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("invalid node meta JSON: {e}"))
            })?;
            Ok::<_, PyErr>(meta)
        })?;

        Ok(Self {
            py_handler,
            cached_meta: meta,
        })
    }
}

#[async_trait]
impl NodeHandler for PyNodeHandlerBridge {
    fn meta(&self) -> NodeMeta {
        self.cached_meta.clone()
    }

    async fn run(&self, inputs: Value, config: &Value, ctx: &NodeCtx) -> Result<Value, NodeError> {
        let context = serde_json::json!({
            "run_id": ctx.run_id(),
            "node_instance_id": ctx.node_instance_id(),
        });

        let py_handler = Python::with_gil(|py| self.py_handler.clone_ref(py));
        let config = config.clone();

        tokio::task::spawn_blocking(move || {
            Python::with_gil(|py| {
                let json_mod = py.import("json").map_err(node_err)?;

                let inputs_str = serde_json::to_string(&inputs).map_err(|e| NodeError::Fatal {
                    message: format!("serialize inputs: {e}"),
                })?;
                let config_str = serde_json::to_string(&config).map_err(|e| NodeError::Fatal {
                    message: format!("serialize config: {e}"),
                })?;
                let context_str =
                    serde_json::to_string(&context).map_err(|e| NodeError::Fatal {
                        message: format!("serialize context: {e}"),
                    })?;

                let inputs_obj = json_mod
                    .call_method1("loads", (inputs_str,))
                    .map_err(node_err)?;
                let config_obj = json_mod
                    .call_method1("loads", (config_str,))
                    .map_err(node_err)?;
                let context_obj = json_mod
                    .call_method1("loads", (context_str,))
                    .map_err(node_err)?;

                let result = py_handler
                    .call_method1(py, "run", (inputs_obj, config_obj, context_obj))
                    .map_err(node_err)?;

                let result_str: String = json_mod
                    .call_method1("dumps", (result,))
                    .map_err(node_err)?
                    .extract()
                    .map_err(node_err)?;

                serde_json::from_str(&result_str).map_err(|e| NodeError::Fatal {
                    message: format!("parse Python result: {e}"),
                })
            })
        })
        .await
        .map_err(|e| NodeError::Fatal {
            message: format!("Python handler task panicked: {e}"),
        })?
    }
}

fn node_err(e: PyErr) -> NodeError {
    NodeError::Fatal {
        message: format!("Python handler error: {e}"),
    }
}

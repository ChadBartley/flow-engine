//! Bridge between a Python LLM provider and the Rust `FlowLlmProvider` trait.
//!
//! A Python LLM provider must implement:
//! - `complete(request: dict) -> dict` performing the LLM call
//! - `name` property returning the provider name

use async_trait::async_trait;
use pyo3::prelude::*;

use cleargate_flow_engine::traits::FlowLlmProvider;
use cleargate_flow_engine::types::{LlmRequest, LlmResponse, NodeError};

/// Rust-side bridge that holds a Python LLM provider object.
pub struct PyLlmProviderBridge {
    py_provider: PyObject,
    cached_name: String,
}

// SAFETY: PyObject is Send+Sync when accessed only via Python::with_gil.
unsafe impl Send for PyLlmProviderBridge {}
unsafe impl Sync for PyLlmProviderBridge {}

impl PyLlmProviderBridge {
    pub fn new(py_provider: PyObject) -> Self {
        let name = Python::with_gil(|py| {
            py_provider
                .getattr(py, "name")
                .and_then(|n| n.extract::<String>(py))
                .unwrap_or_else(|_| "python-provider".to_string())
        });
        Self {
            py_provider,
            cached_name: name,
        }
    }
}

#[async_trait]
impl FlowLlmProvider for PyLlmProviderBridge {
    async fn complete(&self, request: LlmRequest) -> Result<LlmResponse, NodeError> {
        let py_provider = Python::with_gil(|py| self.py_provider.clone_ref(py));

        tokio::task::spawn_blocking(move || {
            Python::with_gil(|py| {
                let json_mod = py.import("json").map_err(llm_err)?;

                let req_json = serde_json::to_string(&request).map_err(|e| NodeError::Fatal {
                    message: format!("serialize LLM request: {e}"),
                })?;
                let req_obj = json_mod
                    .call_method1("loads", (req_json,))
                    .map_err(llm_err)?;

                let result = py_provider
                    .call_method1(py, "complete", (req_obj,))
                    .map_err(llm_err)?;

                let result_str: String = json_mod
                    .call_method1("dumps", (result,))
                    .map_err(llm_err)?
                    .extract()
                    .map_err(llm_err)?;

                serde_json::from_str(&result_str).map_err(|e| NodeError::Fatal {
                    message: format!("parse Python LLM response: {e}"),
                })
            })
        })
        .await
        .map_err(|e| NodeError::Fatal {
            message: format!("Python LLM provider task panicked: {e}"),
        })?
    }

    fn name(&self) -> &str {
        &self.cached_name
    }
}

fn llm_err(e: PyErr) -> NodeError {
    NodeError::Fatal {
        message: format!("Python LLM provider error: {e}"),
    }
}

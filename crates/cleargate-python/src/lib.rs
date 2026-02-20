//! Cleargate Python bindings via PyO3.

use pyo3::prelude::*;

mod adapters;
#[cfg(feature = "engine")]
mod engine;
#[cfg(feature = "engine")]
mod execution;
#[cfg(feature = "engine")]
mod llm_provider;
#[cfg(feature = "engine")]
mod node_handler;
mod runtime;
mod session;

/// The native Rust module exposed to Python as `cleargate._native`.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<session::PyObserverSession>()?;
    m.add_class::<adapters::PyAdapterSession>()?;
    #[cfg(feature = "engine")]
    {
        m.add_class::<engine::PyEngineBuilder>()?;
        m.add_class::<engine::PyEngine>()?;
        m.add_class::<execution::PyExecutionHandle>()?;
    }
    Ok(())
}

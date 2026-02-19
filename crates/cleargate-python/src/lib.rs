//! Cleargate Python bindings via PyO3.

use pyo3::prelude::*;

mod adapters;
mod runtime;
mod session;

/// The native Rust module exposed to Python as `cleargate._native`.
#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<session::PyObserverSession>()?;
    m.add_class::<adapters::PyAdapterSession>()?;
    Ok(())
}

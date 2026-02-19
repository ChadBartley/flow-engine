//! Shared tokio runtime for bridging async Rust ↔ sync Python.
//!
//! Uses `Runtime::block_on` for async → sync bridging. This is safe because
//! Python calls always originate from a non-tokio thread (the Python main
//! thread or a Python worker thread), never from a tokio worker thread.

use std::sync::OnceLock;
use tokio::runtime::Runtime;

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub fn get_runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("failed to create tokio runtime for cleargate Python bindings")
    })
}

pub fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    get_runtime().block_on(fut)
}

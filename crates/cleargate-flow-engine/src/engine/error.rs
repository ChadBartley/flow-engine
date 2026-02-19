//! Engine error types.

use thiserror::Error;

use crate::dylib::DylibError;
use crate::executor::ExecutorError;

/// Errors from [`Engine`](super::Engine) operations.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EngineError {
    /// The requested flow was not found.
    #[error("flow not found: {flow_id}")]
    FlowNotFound { flow_id: String },
    /// An executor error occurred.
    #[error("executor error: {0}")]
    Executor(#[from] ExecutorError),
    /// A flow store error occurred.
    #[error("flow store error: {0}")]
    FlowStore(#[from] crate::errors::FlowStoreError),
    /// A run store error occurred.
    #[error("run store error: {0}")]
    RunStore(#[from] crate::errors::RunStoreError),
    /// An error during engine construction.
    #[error("build error: {message}")]
    Build { message: String },
    /// A dylib loading error occurred.
    #[error("dylib error: {0}")]
    Dylib(#[from] DylibError),
    /// A resource was not found.
    #[error("not found: {message}")]
    NotFound { message: String },
}

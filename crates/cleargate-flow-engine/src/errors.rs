//! Error types for all FlowEngine v2 trait operations.

use thiserror::Error;

/// Errors from [`SecretsProvider`](super::traits::SecretsProvider).
#[derive(Debug, Error)]
pub enum SecretsError {
    #[error("secret not found: {key}")]
    NotFound { key: String },
    #[error("secrets provider error: {message}")]
    Provider { message: String },
}

/// Errors from [`StateStore`](super::traits::StateStore).
#[derive(Debug, Error)]
pub enum StateError {
    #[error("state store error: {message}")]
    Store { message: String },
}

/// Errors from [`QueueProvider`](super::traits::QueueProvider).
#[derive(Debug, Error)]
pub enum QueueError {
    #[error("queue error: {message}")]
    Queue { message: String },
}

/// Errors from [`FlowStore`](super::traits::FlowStore).
#[derive(Debug, Error)]
pub enum FlowStoreError {
    #[error("flow not found: {id}")]
    NotFound { id: String },
    #[error("flow store error: {message}")]
    Store { message: String },
}

/// Errors from [`RunStore`](super::traits::RunStore).
#[derive(Debug, Error)]
pub enum RunStoreError {
    #[error("run not found: {id}")]
    NotFound { id: String },
    #[error("run store error: {message}")]
    Store { message: String },
}

/// Errors from [`Trigger`](super::traits::Trigger) implementations.
#[derive(Debug, Error)]
pub enum TriggerError {
    #[error("trigger config error: {message}")]
    Config { message: String },
    #[error("trigger runtime error: {message}")]
    Runtime { message: String },
}

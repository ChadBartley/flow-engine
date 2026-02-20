//! Cleargate Node.js bindings via napi-rs.

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

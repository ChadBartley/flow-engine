//! Bridge between JS LLM provider and Rust `FlowLlmProvider` trait.
//!
//! Direct JS object bridging requires `ThreadsafeFunction` for cross-thread
//! callbacks. This module provides the types; full bridging is a future enhancement.

// Placeholder — JS LLM provider bridging via ThreadsafeFunction will be added
// when the napi-rs ThreadsafeFunction API stabilizes for this use case.

#[allow(dead_code)] // Reserved for ThreadsafeFunction-based JS LLM provider bridging.
pub struct JsLlmProviderBridge;

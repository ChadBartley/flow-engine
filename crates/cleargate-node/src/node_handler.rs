//! Bridge between JS node handler and Rust `NodeHandler` trait.
//!
//! Direct JS object bridging requires `ThreadsafeFunction` for cross-thread
//! callbacks. This module provides the types; full bridging is a future enhancement.

// Placeholder — JS node handler bridging via ThreadsafeFunction will be added
// when the napi-rs ThreadsafeFunction API stabilizes for this use case.

#[allow(dead_code)] // Reserved for ThreadsafeFunction-based JS node bridging.
pub struct JsNodeHandlerBridge;

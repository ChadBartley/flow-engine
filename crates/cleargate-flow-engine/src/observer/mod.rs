//! Observer session — ergonomic wrapper for recording LLM calls, tool
//! invocations, and steps without graph execution concerns.

mod session;

pub use session::{ObserverConfig, ObserverSession};

//! Framework adapters for recording external orchestrator executions
//! through the Cleargate ObserverSession.

pub mod adapter;
pub mod langchain;
pub mod langgraph;
pub mod semantic_kernel;
pub mod types;

pub use adapter::{AdapterSession, FrameworkAdapter};
pub use langchain::LangChainAdapter;
pub use langgraph::LangGraphAdapter;
pub use semantic_kernel::SemanticKernelAdapter;
pub use types::AdapterEvent;

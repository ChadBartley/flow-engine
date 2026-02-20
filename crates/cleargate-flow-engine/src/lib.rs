//! FlowEngine — deterministic, replayable workflow execution.
//!
//! This crate provides the core types, traits, and runtime for executing
//! graph-based workflows with full determinism and replay capability. All
//! execution data is structured and authoritative, enabling replay from the
//! RunStore alone.
//!
//! The engine is designed to be embedded in other applications and has zero
//! dependencies on web servers, databases, or other application-level concerns.
#[cfg(feature = "schemars")]
pub mod schema;

pub mod defaults;
pub mod dylib;
pub mod engine;
pub mod errors;
pub mod executor;
pub(crate) mod expression;
#[cfg(feature = "mcp")]
pub mod mcp;
pub mod node_ctx;
pub mod nodes;
pub mod observer;
pub mod runtime;
pub mod traits;
pub mod triggers;
pub mod types;
pub mod validate;
pub mod versioning;
pub mod write_event;
pub(crate) mod write_handle;
pub(crate) mod write_pipeline;

// Re-export public types at the crate level.

// defaults
pub use defaults::{
    DefaultRedactor, EnvSecretsProvider, FileFlowStore, FileRunStore, FileTagStore, InMemoryQueue,
    InMemoryRunStore, InMemoryState, OtelProvider,
};

// dylib
pub use dylib::{DylibError, DylibNodeHandler, DylibNodeLoader, NodeCallbacks};

// engine
pub use engine::{Engine, EngineBuilder, EngineError};

// errors
pub use errors::{
    FlowStoreError, QueueError, RunStoreError, SecretsError, StateError, TriggerError,
};

// executor
pub use executor::{ExecutionHandle, ExecutorConfig, ExecutorError};

// node_ctx
#[cfg(any(test, feature = "test-support"))]
pub use node_ctx::test_support::{TestNodeCtx, TestNodeCtxInspector};
pub use node_ctx::{NodeCtx, NodeEvent};

// nodes
#[cfg(feature = "mcp")]
pub use nodes::McpCallNode;
pub use nodes::{
    ConditionalNode, CsvToolNode, HttpNode, HumanInLoopNode, JqNode, JsonLookupNode, LlmCallNode,
    ToolRouterNode,
};

// observer
pub use observer::ObserverSession;

// traits
pub use traits::{
    CustomEventRecord, EdgeRunResult, FlowLlmProvider, FlowStore, MessageReceipt,
    NodeAttemptDetail, NodeHandler, NodeRunDetail, NodeRunResult, NoopObservability,
    ObservabilityProvider, QueueMessage, QueueProvider, QueueReceiver, Redactor, RunFilter,
    RunPage, RunStore, SecretsProvider, SpanHandle, SpanStatus, StateStore, TagStore, Trigger,
};

// triggers
pub use triggers::{
    CronTrigger, DispatchedRun, FlowTrigger, HttpTrigger, HttpTriggerRoute, RunCompletedEvent,
    TriggerDispatcher, TriggerInstance, TriggerRunner,
};

// types
pub use types::{
    Edge, EdgeCondition, ExecutionHints, ExecutionId, FlowBundle, FlowHead, FlowTag, FlowVersion,
    GraphDef, LlmChunk, LlmCost, LlmInvocationRecord, LlmRequest, LlmResponse, LlmRunSummary,
    LlmToolCall, NodeError, NodeInstance, NodeMeta, NodeUiHints, PortDef, PortType,
    ReplaySubstitutions, RetryPolicy, RunRecord, RunStatus, Sensitivity, ToolDef, ToolType,
    TriggerEvent, TriggerSource, TypeDef, TypeField, GRAPH_SCHEMA_VERSION,
    WRITE_EVENT_SCHEMA_VERSION,
};

// validate
pub use validate::validate_graph;

// versioning
pub use versioning::{compute_config_hash, compute_version_id, FlowVersioning};

// write_event / write_pipeline
pub use write_event::WriteEvent;
pub use write_pipeline::WritePipelineConfig;

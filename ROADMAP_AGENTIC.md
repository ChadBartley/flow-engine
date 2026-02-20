# ClearGate FlowEngine — Orchestration Framework Maturity Roadmap

> Side quest alongside [ROADMAP.md](./ROADMAP.md) (replay/logging/observer/diff workstream).
> This roadmap covers orchestration capabilities that make FlowEngine a production-grade agentic runtime.

## Context

FlowEngine v2 (M1–M12) is complete: 350+ tests, zero server dependencies. The engine works as a graph-based orchestrator with full recording, versioning, and streaming. Framework adapters for Python (LangChain/LangGraph), Node.js, and .NET (Semantic Kernel) ship in the `cleargate-adapters` crate.

This roadmap tracks the remaining orchestration features. Core engine features land in `cleargate-flow-engine`. Capabilities that require external dependencies or optional storage follow an **extension crate** pattern (separate crate, opt-in dependency) to keep the core engine lightweight and dependency-free.

**Storage principle**: The core engine never depends on a specific database. Extension crates (e.g., `cleargate-storage-oss`) provide persistence via SeaORM. Consumers choose file-based defaults, bring-your-own trait impls, or extension crate storage.

---

## O1: MCP Tool Integration ✅ COMPLETE

**Impact**: High — MCP is becoming the standard tool protocol. Enables massive tool ecosystem.
**Status**: Complete — 35 new tests, all quality gates pass.

### What Was Built

New module: `crates/cleargate-flow-engine/src/mcp/`

- **MCP client** (stdio + SSE transports) — `client.rs`, `transport.rs`
- **Protocol types** — `types.rs` (JSON-RPC 2.0, initialize, tools/list, tools/call)
- **Tool discovery** — `discovery.rs` converts `McpTool` → `ToolDef` with `ToolType::Mcp`
- **Server registry** — `server.rs` with `McpServerConfig`, `McpServer` (lazy connect), `McpServerRegistry`
- **Error mapping** — `McpError` with `From<McpError> for NodeError` (Connection→Retryable, Protocol→Fatal, Timeout→Timeout)
- **`McpCallNode`** — new built-in node dispatching tool calls to MCP servers
- **`EngineBuilder::mcp_server(name, config)`** — registration API
- **MCP tools merge into `ctx.available_tools()`** at run start via `discover_all_tools()`
- **`TestNodeCtx::builder().mcp_registry()`** — test support for injecting mock MCP servers

### Architecture Decision

Dedicated `McpCallNode` instead of modifying `ToolRouterNode`. ToolRouterNode stays as a pure fan-out passthrough; MCP dispatch is a separate node, testable independently.

### Key Details for Next Agent

- `mcp` feature gate: `Cargo.toml` feature `mcp = []` (no extra deps — `reqwest` is now always-on since the core executor uses it unconditionally)
- `reqwest`, `libloading`, `cron`, `jq-rs` were all changed from `optional = true` to required because they're used unconditionally in core modules. Feature flags for `dylib`, `jq`, `cron`, `http-node`, `mcp` are now empty `[]` — they exist for semantic opt-in but don't gate dependencies.
- `McpServer::with_client()` and `McpServerRegistry::register_server()` are `#[cfg(any(test, feature = "test-support"))]` for injecting mock transports in tests
- MCP tool discovery happens once at run start in `executor/run.rs` — not per-node. Failed servers are logged and skipped (partial discovery).
- The `#[cfg(feature = "mcp")]` pattern threads through: NodeCtx field + constructor param, execute_node(), spawn_node(), RunContext, Executor, EngineBuilder, Engine, dylib.rs callback. Use `#[allow(unused_mut)]` where `let mut` is only mutated inside a cfg block.
- Protocol version: `2024-11-05`. Sends `notifications/initialized` after handshake.
- Pagination supported for `tools/list` (follows `nextCursor`).

### Files

- **Created:** `src/mcp/mod.rs`, `client.rs`, `transport.rs`, `types.rs`, `discovery.rs`, `server.rs`, `src/nodes/mcp_call.rs`
- **Modified:** `Cargo.toml`, `src/lib.rs`, `src/nodes/mod.rs`, `src/engine/builder.rs`, `src/engine/mod.rs`, `src/executor/mod.rs`, `src/executor/run.rs`, `src/executor/node.rs`, `src/node_ctx.rs`, `src/dylib.rs`

---

## O2: Structured Output ✅ COMPLETE

**Impact**: High — directly improves agent reliability.
**Status**: Complete — 9 new tests (20 total in llm_call), all quality gates pass.

### What Was Built

- **`StructuredOutputConfig`** type in `types/llm.rs` — `schema`, `max_retries` (default 2), `strict` (default true)
- **JSON schema validation** on LLM responses using `jsonschema` crate (feature-gated `structured-output`)
- **Auto-retry loop** — on validation failure, appends the failed response + error feedback as conversation messages and re-calls the provider; respects `max_retries`
- **`output_schema` config field** on `LlmCallNode` — when set, validates response content against the schema
- **`strict` mode** — when `true`, auto-sets `response_format` to `{"type": "json_schema", "json_schema": {"schema": ...}}` (OpenAI format) so providers with native structured output support enforce the schema at generation time
- **All attempts recorded** — every LLM call (including retries) is recorded via `record_llm_call` for replay and cost tracking
- **Works with both streaming and non-streaming** paths

### Key Details for Next Agent

- `structured-output` feature gate: `Cargo.toml` feature `structured-output = ["dep:jsonschema"]`, included in `default` features
- `jsonschema = "0.28"` added to workspace `Cargo.toml`
- `validate_structured_output()` helper is `#[cfg(feature = "structured-output")]` — parses string content as JSON, then validates against schema
- `#[allow(unused_mut)]` on `response_format`, `attempts`, `last_validation_error`, `current_messages` — only mutated inside `#[cfg(feature = "structured-output")]` blocks
- The retry loop rebuilds `LlmRequest` with updated messages each iteration (includes prior failed response + error feedback)
- `StructuredOutputConfig` uses `#[serde(default = "...")]` for `max_retries` and `strict` fields

### Files

- **Modified:** `Cargo.toml` (workspace), `crates/cleargate-flow-engine/Cargo.toml`, `src/types/llm.rs`, `src/nodes/llm_call.rs`

---

## O3: Dynamic Tool Registration (Complete)

**Impact**: Medium-High — enables context-dependent tooling and runtime flexibility.
**Scope**: Small-Medium (~1-2 weeks)

### What Was Built

- **`ToolRegistry`** — thread-safe registry (`parking_lot::RwLock` + `Arc`) managing tool lifecycle; replaces static `Arc<Vec<ToolDef>>` as single source of truth
- **Runtime tool add/remove** — nodes output `_tool_changes: { add: [ToolDef...], remove: ["name"] }` to register/unregister tools between executions
- **`NodeToolAccess`** — per-node tool filtering via `allowed_tools` allowlist and `granted_permissions` set on `NodeInstance`
- **`ToolDef.permissions`** — permission labels on tools; `ToolRegistry.snapshot_filtered()` enforces intersection with caller permissions
- **Feature gate** — `dynamic-tools` feature (in default features, no extra deps); controls whether filtering is applied in `spawn_node`

### Key Decisions

- `ToolRegistry` is the single source of truth for available tools at execution time
- Tools can be added/removed between node executions via `_tool_changes` output key (declarative)
- `NodeToolAccess` and `ToolDef.permissions` are always compiled (not struct-level `#[cfg]`) to avoid touching 40+ construction sites; the `dynamic-tools` feature controls filtering logic only
- MCP tools (O1) integrate through the same registry; runtime MCP server discovery via `_mcp_servers` output key deferred (infrastructure ready)

### Files

- **Created:** `crates/cleargate-flow-engine/src/tool_registry.rs`
- **Modified:** `Cargo.toml`, `lib.rs`, `types/graph.rs`, `executor/run.rs`, `executor/node.rs`, `executor/fanout.rs`, `node_ctx.rs`, `dylib.rs`

---

## O4: Memory & Context Management ✅ COMPLETE

**Impact**: High — enables multi-turn agents.
**Status**: Complete — ~40 new tests, all quality gates pass.

### What Was Built

New module: `crates/cleargate-flow-engine/src/memory/`

- **`TokenCounter` trait** — `token_counter.rs` with `count_tokens(&str)` and `count_messages(&[Value])`; `CharEstimateCounter` default (chars/4, no external deps)
- **`MemoryStrategy` enum** — `strategy.rs` with three variants: `MessageCount`, `TokenBudget`, `Summarize`; serde-tagged for JSON config
- **`MemoryConfig`** — `config.rs` with flattened strategy deserialization from node config JSON
- **`MemoryManager` trait** — `mod.rs` with `prepare_messages()` async method; `InMemoryManager` default implementation
- **`ContextManagementNode`** — standalone built-in node for explicit context management in graph pipelines
- **LlmCallNode integration** — automatic memory management via `memory` config key; separates trimmed messages (sent to LLM) from full history (persisted to state)

### Architecture Decision

Integrated into core engine behind `memory` feature gate (semantic opt-in, no extra deps, enabled by default). Token counting uses char-estimate in core; accurate tiktoken-rs counter planned for `cleargate-providers`. Summarization calls LLM via the existing provider infrastructure — configurable per-node or falls back to the node's own provider/model.

### Key Details for Next Agent

- `MemoryManager` + `TokenCounter` propagate through the full chain: `EngineBuilder` → `Engine` → `Executor` → `RunContext` → `execute_node` → `NodeCtx`
- System messages are always preserved across all strategies
- `Summarize` strategy calls LLM to condense older messages, keeps recent N verbatim
- Full conversation history is always persisted to state (not the trimmed version) so future summarizations have complete context

### Files

- **Created**: `memory/mod.rs`, `memory/strategy.rs`, `memory/config.rs`, `memory/token_counter.rs`, `nodes/context_management.rs`
- **Modified**: `Cargo.toml`, `lib.rs`, `node_ctx.rs`, `executor/mod.rs`, `executor/run.rs`, `executor/node.rs`, `engine/builder.rs`, `dylib.rs`, `nodes/mod.rs`, `nodes/llm_call.rs`

---

## O5: Multi-Agent Patterns ✅ COMPLETE

**Impact**: Medium — enables complex agent architectures.
**Status**: Complete — all quality gates pass.

### What Was Built

New module: `crates/cleargate-flow-engine/src/multi_agent/`

- **`Blackboard`** — namespaced shared memory wrapper over `StateStore` (`blackboard.rs`)
  - `read(namespace, key)`, `write(namespace, key, value)`, `read_all(namespace)`
  - Keys stored as `"{namespace}:{key}"` — agents write to their namespace, supervisors read across
- **`AgentHandoff`** — serde-serializable context transfer struct (`handoff.rs`)
  - `from_node_output()` extracts from `_handoff` key; `to_node_input()` wraps for downstream
  - Fields: `from_agent`, `to_agent`, `task`, `context`, `constraints`
- **`AgentRole`** enum — `Supervisor` / `Worker` for introspection and observability
- **`SupervisorNode`** (`nodes/supervisor.rs`) — LLM-powered delegation node
  - Builds a prompt listing available workers, calls LLM to decide delegation
  - Outputs `AgentHandoff`, `target_worker` (for edge-condition routing), `done`, `final_result`
  - Persists delegation history to blackboard under `supervisor:delegations`
  - Validates worker names against config; rejects unknown workers
- **`WorkerNode`** (`nodes/worker.rs`) — task execution wrapper
  - Accepts `AgentHandoff` (via `_handoff` or `handoff` key), calls LLM, writes result to blackboard
  - Outputs `result` and `worker_name` for supervisor consumption
- **`NodeCtx::blackboard()`** method — constructs a `Blackboard` from existing state + run_id
- **Feature gate**: `multi-agent = []` (no extra deps, included in `default`)
- **Registration**: `SupervisorNode` and `WorkerNode` in `engine/builder.rs` builtins

### Architecture Decision

Pattern library on top of existing executor — no new runtime or execution primitives. Blackboard wraps `StateStore` with namespaced keys. Handoff travels as serialized edge data. Supervisor routing uses `target_worker` output field matched by downstream conditional edges.

### Key Details for Next Agent

- `multi-agent` feature gate: empty `[]` like `mcp`, `memory` — semantic opt-in, no extra deps
- `Blackboard` constructed from `Arc<dyn StateStore>` + `run_id` — no new NodeCtx fields needed
- `NodeCtx::blackboard()` is `#[cfg(feature = "multi-agent")]`
- Supervisor uses `response_format: json_object` for reliable JSON decisions
- Worker accepts handoff from `_handoff` key (from `to_node_input()`), `handoff` key (from supervisor output), or treats entire input as task (fallback)

### Files

- `src/multi_agent/mod.rs` — module exports, `AgentRole` enum
- `src/multi_agent/blackboard.rs` — `Blackboard` wrapper
- `src/multi_agent/handoff.rs` — `AgentHandoff` types
- `src/nodes/supervisor.rs` — `SupervisorNode`
- `src/nodes/worker.rs` — `WorkerNode`
- Modified: `lib.rs`, `node_ctx.rs`, `nodes/mod.rs`, `engine/builder.rs`, `Cargo.toml`

---

## O6: Graph Composition ✅

**Impact**: Medium — enables reusable flow components.
**Status**: Complete

### What Was Built

- **`SubflowNode`** — built-in node that executes a sub-`GraphDef` as a child run
- **`SubflowRegistry`** — thread-safe registry for named sub-flow definitions
- **Two resolution modes**: named (`"subflow": "name"`) and inline (`"subflow_inline": {...}`)
- **Input/output mapping** — optional field renaming between parent and child
- **`EngineBuilder::register_subflow(name, graph_def)`** — registration API
- **Recursive composition** — sub-flows can contain other sub-flows (nested execution)
- **Failure propagation** — child run failures map to parent `NodeError::Fatal`
- Feature-gated behind `subflow` (default-enabled)

### Files

- **Created:** `src/subflow_registry.rs`, `src/nodes/subflow.rs`
- **Modified:** `Cargo.toml`, `src/lib.rs`, `src/nodes/mod.rs`, `src/executor/mod.rs`, `src/executor/run.rs`, `src/executor/node.rs`, `src/node_ctx.rs`, `src/engine/builder.rs`, `src/engine/mod.rs`, `src/dylib.rs`

---

## O7: Language Bindings — Orchestration Mode ✅ COMPLETE

**Impact**: High — exposes full engine to Python, Node.js, and .NET developers.
**Status**: Complete — all quality gates pass.
**Depends on**: O1-O6 stabilized
**Cross-ref**: [ROADMAP.md](./ROADMAP.md) covers observer-mode bindings

### What Was Built

Orchestration-mode bindings for all three language crates, feature-gated behind `engine` (default-enabled):

**Python (`cleargate-python`):**

- `PyEngineBuilder` — fluent builder wrapping `EngineBuilder` with mutable-option ownership pattern
- `PyEngine` — wraps `Arc<Engine>`, exposes `execute()`, `execute_graph()`, `provide_input()`, `node_catalog()`, `shutdown()`
- `PyExecutionHandle` — wraps `broadcast::Receiver<WriteEvent>`, implements `__iter__`/`__next__` for event streaming
- `PyNodeHandlerBridge` — bridges Python class (`meta()` + `run()`) to Rust `NodeHandler` trait via `spawn_blocking` + `Python::with_gil`
- `PyLlmProviderBridge` — bridges Python class to `FlowLlmProvider` trait, JSON marshaling for `LlmRequest`/`LlmResponse`

**Node.js (`cleargate-node`):**

- `NapiEngineBuilder` / `NapiEngine` — NAPI classes mirroring Rust builder
- `NapiExecutionHandle` — `nextEvent()`, `waitForCompletion()`, `cancel()` methods
- TypeScript SDK types (`sdks/node/src/engine.ts`) — `WriteEvent` discriminated union, `streamEvents()` async generator, `NodeHandler`/`LlmProvider` interfaces
- Node handler and LLM provider bridging via ThreadsafeFunction deferred (placeholder types created)

**.NET (`cleargate-dotnet`):**

- Handle-based C FFI layer (`engine_ffi.rs`) — `cleargate_engine_builder_*`, `cleargate_engine_*`, `cleargate_execution_*` functions
- Managed C# wrappers (`sdks/dotnet/Cleargate/`) — `EngineBuilder` (IDisposable, fluent API), `Engine` (IDisposable), `ExecutionHandle` (IDisposable with `Events()` IEnumerable)
- P/Invoke declarations in `Native.cs`

**Examples (`examples/orchestration/`):**

- Shared flow definitions: `echo-flow.json`, `simple-responder.json`
- Python: 5 examples (basic_flow, streaming_events, custom_node, custom_llm_provider, execute_graph)
- Node.js: 3 examples (basic_flow, streaming_events, execute_graph)
- .NET: BasicFlow, ExecuteGraph with Program.cs runner

### Architecture Decisions

- **Feature gate is `engine`** — code-only gate since `cleargate-flow-engine` dep already exists in all binding crates
- **JSON marshaling** for all cross-language types via `serde_json::Value` ↔ language dict/object
- **Mutable-option builder pattern** (`Option<EngineBuilder>`) to work around ownership transfer in binding languages
- **`unsafe impl Send/Sync`** for `PyObject` bridges — safe when accessed only via `Python::with_gil`
- **Handle-based C ABI** for .NET — `HashMap<u64, Wrapper>` behind `Mutex` with thread-local errors

### Key Details

- Python `recv_next()` extracted as free function to avoid holding `MutexGuard` across GIL boundary
- `broadcast::Receiver<WriteEvent>` used for event streaming (from `ExecutionHandle`)
- All three crates compile with `--no-default-features` (engine feature disabled)
- Node.js ThreadsafeFunction-based JS node/LLM bridging deferred — stub types with clear docs

### Files

**Python:**

- **Created:** `src/engine.rs`, `src/execution.rs`, `src/llm_provider.rs` (new), `src/node_handler.rs` (new)
- **Modified:** `Cargo.toml`, `src/lib.rs`, `python/cleargate/__init__.py`

**Node.js:**

- **Created:** `src/engine.rs`, `src/execution.rs`, `src/node_handler.rs`, `src/llm_provider.rs`, `sdks/node/src/engine.ts`
- **Modified:** `Cargo.toml`, `src/lib.rs`, `sdks/node/src/index.ts`

**.NET:**

- **Created:** `src/engine_ffi.rs`, `sdks/dotnet/Cleargate/Engine.cs`
- **Modified:** `Cargo.toml`, `src/lib.rs`, `sdks/dotnet/Cleargate/Native.cs`

**Examples:**

- **Created:** `examples/orchestration/flows/`, `examples/orchestration/python/`, `examples/orchestration/node/`, `examples/orchestration/dotnet/`

---

## Additional Items (demand-driven)

| Feature           | Notes                                               |
| ----------------- | --------------------------------------------------- |
| Schema Generation | OpenAPI/JSON Schema for flow definitions (schemars) |
| WASM Target       | Constrained (no dylib, no fs, single-threaded)      |

---

## Dependency Graph

```
O1 (MCP)                ─┐
O2 (Structured Output)  ─┤── independent, can parallelize
O3 (Dynamic Tools)      ─┘

O4 (Memory)             ─── O5 (Multi-Agent)

O6 (Graph Composition)  ─── standalone

O1-O6 stabilized        ─── O7 (Language Bindings)
```

**Suggested execution**: O1 → O2 → O3 → O4 → O5 / O6 (parallel) → O7

---

## Verification

Each milestone verified by:

- `cargo test --workspace` — all existing tests still pass
- `cargo clippy --workspace` — 0 warnings
- New integration tests per milestone
- O7: language-specific test suites (`pytest`, `npm test`, `dotnet test`)

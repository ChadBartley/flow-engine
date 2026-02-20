# ClearGate FlowEngine — Orchestration Framework Maturity Roadmap

> Side quest alongside [ROADMAP.md](./ROADMAP.md) (replay/logging/observer/diff workstream).
> This roadmap covers orchestration capabilities that make FlowEngine a production-grade agentic runtime.

## Context

FlowEngine v2 (M1–M12) is complete: 350+ tests, zero server dependencies. The engine works as a graph-based orchestrator with full recording, versioning, and streaming. Framework adapters for Python (LangChain/LangGraph), Node.js, and .NET (Semantic Kernel) ship in the `cleargate-adapters` crate.

This roadmap tracks the remaining orchestration features. Core engine features land in `cleargate-flow-engine`. Capabilities that require external dependencies or optional storage follow an **extension crate** pattern (separate crate, opt-in dependency) to keep the core engine lightweight and dependency-free.

**Storage principle**: The core engine never depends on a specific database. Extension crates (e.g., `cleargate-storage-oss`) provide persistence via SeaORM. Consumers choose file-based defaults, bring-your-own trait impls, or extension crate storage.

---

## O1: MCP Tool Integration

**Impact**: High — MCP is becoming the standard tool protocol. Enables massive tool ecosystem.
**Scope**: Medium (~2-3 weeks)
**Depends on**: Phase 0 only

Wire the existing `ToolType::Mcp` stub into a working MCP client.

### What Gets Built

New module: `crates/cleargate-flow-engine/src/flow/mcp/`

- **MCP client** (stdio + SSE transports) — `client.rs`, `transport.rs`
- **Protocol types** — `types.rs` (initialize, tools/list, tools/call, resources)
- **Tool discovery** — `discovery.rs` converts MCP tool schemas → `ToolDef`
- **ToolRouterNode** routes `ToolType::Mcp` calls through MCP client
- **`EngineBuilder::mcp_server(name, config)`** registration

### Key Decisions

- Feature-gated: `mcp` feature (default on)
- MCP servers registered at engine build time, connections established lazily
- Tool discovery on first use or explicit refresh
- MCP tools appear in `ctx.available_tools()` alongside graph-defined tools
- **Static server list only** — O1 wires the protocol client and tool discovery; runtime add/remove of MCP servers (e.g., discover from a URL mid-run) is O3 territory

### Files

- **Create:** `flow/mcp/mod.rs`, `client.rs`, `transport.rs`, `types.rs`, `discovery.rs`
- **Modify:** `flow/nodes/tool_router.rs`, `flow/engine.rs`, `flow/types.rs`, `Cargo.toml`

---

## O2: Structured Output

**Impact**: High — directly improves agent reliability.
**Scope**: Small (~1 week)

### What Gets Built

- JSON schema validation on LLM responses (via `serde_json` + `jsonschema` crate)
- Auto-retry on parse failure (configurable max retries)
- `LlmCallNode` `"output_schema"` config field — when set, validates response and retries if invalid
- `response_format` passthrough to providers that support native structured output (config field already exists in `LlmCallNode`)

### Files

- **Modify:** `flow/nodes/llm_call.rs`, `flow/types.rs`
- **Add dep:** `jsonschema` (feature-gated)

---

## O3: Dynamic Tool Registration

**Impact**: Medium-High — enables context-dependent tooling and runtime flexibility.
**Scope**: Small-Medium (~1-2 weeks)

### What Gets Built

- **`ToolRegistry`** — manages tool lifecycle, replaces static tool list in executor; single source of truth for available tools at execution time
- **Runtime tool add/remove** — executor API to register and unregister tools (including MCP servers) during a flow run
- **Runtime MCP server discovery** — add MCP servers by URL or config mid-run; O1's client handles the connection, O3 manages the lifecycle
- **Context-dependent tool availability** — per-node or per-agent tool filtering via config
- **Node-scoped tool sets** — nodes declare which tools they can access; executor enforces boundaries
- **Tool permission labels** — tools declare capability labels (e.g., `read`, `write`, `admin`, `pii-access`, or custom); agents/nodes declare a permission set; `ToolRegistry` enforces the intersection so agents only see tools whose labels match their permissions

### Key Decisions

- `ToolRegistry` is the single source of truth for available tools at execution time
- Tools can be added/removed between node executions, not mid-node
- MCP tools (O1) integrate through the same registry — O1 provides the protocol client, O3 manages when and where those tools are available
- Tool availability is declarative (node config) — no imperative tool manipulation inside node handlers
- **Permission enforcement is mandatory** — every tool has labels, every agent/node has a permission set; the registry filters on the intersection. Agents cannot escalate their own permissions at runtime

### Files

- **Create:** `flow/tool_registry.rs`
- **Modify:** `flow/executor.rs`, `flow/engine.rs`, `flow/types.rs`, `flow/nodes/tool_router.rs`

---

## O4: Memory & Context Management

**Impact**: High — enables multi-turn agents.
**Scope**: Medium (~2 weeks)
**Pattern**: Extension crate (`cleargate-memory` or integrated into `cleargate-storage-oss`)

### What Gets Built

- **Conversation history windowing** — sliding window by message count or token count
- **Summarization hooks** — trait for pluggable summarization (LLM-based or extractive)
- **Token counting** — trait `TokenCounter` with tiktoken-rs default impl
- **`MemoryManager`** — manages history per flow run, backed by `cleargate-storage-oss` SeaORM models or in-memory fallback

### Key Decisions

- Core engine defines the `MemoryManager` trait and in-memory default
- Persistent memory lives in an extension crate, leveraging `cleargate-storage-oss` and SeaORM for SQLite/PostgreSQL storage
- Token counting is trait-based — users can plug in provider-specific tokenizers
- Summarization is optional — flows work with simple windowing by default

---

## O5: Multi-Agent Patterns

**Impact**: Medium — enables complex agent architectures.
**Scope**: Medium (~2-3 weeks)
**Depends on**: O4 (Memory)

### What Gets Built

- **Supervisor/worker topology** — supervisor node delegates to worker sub-graphs
- **Agent handoff** — structured transfer of context between agent nodes
- **Shared blackboard state** — `StateStore`-backed shared memory across agents in a run
- Patterns implemented as graph topologies + conventions, not new execution primitives

### Key Decisions

- Multi-agent is a pattern library on top of existing executor, not a new runtime
- Blackboard uses existing `StateStore` with namespaced keys
- Handoff is modeled as edge data between nodes

---

## O6: Graph Composition

**Impact**: Medium — enables reusable flow components.
**Scope**: Medium (~2 weeks)

### What Gets Built

- **Sub-flow node** — embed one `GraphDef` as a single node in another graph
- Sub-flow gets its own `NodeCtx` scope but shares the parent run's `StateStore`
- Input/output mapping between parent graph edges and sub-flow entry/exit nodes
- `EngineBuilder::register_subflow(name, graph_def)` API

### Files

- **Create:** `flow/nodes/subflow_node.rs`
- **Modify:** `flow/executor.rs`, `flow/engine.rs`, `flow/types.rs`

---

## O7: Language Bindings — Orchestration Mode

**Impact**: High — exposes full engine to Python, Node.js, and .NET developers.
**Scope**: Large (~3-4 weeks per language, parallelizable)
**Depends on**: O1-O6 stabilized
**Cross-ref**: [ROADMAP.md](./ROADMAP.md) covers observer-mode bindings

### What Gets Built

Orchestration-mode bindings for all three language crates, feature-gated so users opt in:

| Crate | Package | Feature gate |
|---|---|---|
| `cleargate-python` | `pip install cleargate[engine]` | `engine` feature |
| `cleargate-node` | `npm install cleargate` (engine entry point) | `engine` feature |
| `cleargate-dotnet` | `ClearGate.Engine` NuGet package | `Engine` feature |

**Capabilities exposed per language:**

- `EngineBuilder` — configure flow/run stores, register providers, build engine
- `Engine.execute(flow_name, inputs)` — run a flow, receive streaming events
- **Custom node handlers** — register language-native functions as node implementations
- **Custom LLM providers** — plug in language-native LLM clients

**Python example:**

```python
from cleargate import Engine, EngineBuilder

engine = await EngineBuilder() \
    .flow_store_path("./flows") \
    .run_store_path("./runs") \
    .llm_provider("openai", my_provider) \
    .build()

handle = await engine.execute("agent-flow", {"query": "..."})
async for event in handle.events():
    print(event)
```

### Note

Observer-mode bindings (ObserverSession, replay, diff, inspect) already ship in each crate. This milestone adds orchestrator-mode bindings behind feature gates so both modes coexist in a single package per language.

---

## Additional Items (demand-driven)

| Feature | Notes |
|---|---|
| Schema Generation | OpenAPI/JSON Schema for flow definitions (schemars) |
| WASM Target | Constrained (no dylib, no fs, single-threaded) |

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

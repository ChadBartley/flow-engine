# ClearGate FlowEngine — Orchestration Framework Maturity Roadmap

> Side quest alongside [ROADMAP.md](./ROADMAP.md) (replay/logging/observer/diff workstream).
> This roadmap covers orchestration capabilities that make FlowEngine a production-grade agentic runtime.

## Context

FlowEngine v2 (M1–M12) is complete: 348 tests, zero server dependencies. The engine works as a graph-based orchestrator with full recording, versioning, and streaming. This roadmap tracks the remaining orchestration features needed to reach parity with (and surpass) frameworks like LangGraph.

**Storage principle**: The crate never depends on a specific database. Consumers choose file-based defaults, bring-your-own trait impls, or a future SaaS storage layer.

---

## O1: MCP Tool Integration

**Impact**: High — MCP is becoming the standard tool protocol. Enables massive tool ecosystem.
**Scope**: Medium (~2-3 weeks)
**Depends on**: Phase 0 only

Wire the existing `ToolType::Mcp` stub into a working MCP client.

### What Gets Built

New module: `crates/cleargate-core/src/flow/mcp/`

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
- Optional `response_format` passthrough to providers that support native structured output

### Files

- **Modify:** `flow/nodes/llm_call.rs`, `flow/types.rs`
- **Add dep:** `jsonschema` (feature-gated)

---

## O3: Memory & Context Management

**Impact**: High — enables multi-turn agents.
**Scope**: Medium (~2 weeks)

### What Gets Built

New module: `flow/memory/`

- **Conversation history windowing** — sliding window by message count or token count
- **Summarization hooks** — trait for pluggable summarization (LLM-based or extractive)
- **Token counting** — trait `TokenCounter` with tiktoken-rs default impl
- **`MemoryManager`** — manages history per flow run, integrates with `StateStore`

### Key Decisions

- Memory is per-run state managed through existing `StateStore` trait
- Token counting is trait-based — users can plug in provider-specific tokenizers
- Summarization is optional — flows work with simple windowing by default

---

## O4: Rate Limiting & Caching

**Impact**: Medium — cost savings, production stability.
**Scope**: Small (~1-2 weeks)

### What Gets Built

- **Per-provider rate limiting** — token bucket in executor, configurable tokens/min and requests/min
- **LLM response caching** — exact-match (request hash → cached response)
- Cache trait (`FlowCache`) for pluggable backends (in-memory default)
- Semantic cache as future extension (not in scope here)

### Files

- **Create:** `flow/cache.rs`
- **Modify:** `flow/executor.rs`, `flow/engine.rs`, `flow/traits.rs`

---

## O5: Guardrails

**Impact**: Medium — safety layer for production agents.
**Scope**: Medium (~2 weeks)

### What Gets Built

- **Input/output validation nodes** — configurable validation rules (regex, schema, blocklist)
- **Content filtering hooks** — trait `ContentFilter` with pre/post LLM call hooks
- **`GuardrailNode`** built-in node type — runs validation, can reject/modify/pass-through
- Integration with `LlmCallNode` — optional pre/post guardrail config

### Key Decisions

- Guardrails are composable nodes, not a separate system
- Filtering is trait-based — users plug in their own classifiers
- Rejection produces a `NodeOutcome::Failed` with structured reason

---

## O6: Multi-Agent Patterns

**Impact**: Medium — enables complex agent architectures.
**Scope**: Medium (~2-3 weeks)
**Depends on**: O3 (Memory)

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

## O7: Graph Composition

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

## O8: Resumable Execution

**Impact**: Medium — critical for long-running workflows.
**Scope**: Small-Medium (~1-2 weeks)

### What Gets Built

Infrastructure exists (StateStore, crash recovery marks interrupted runs). Wire it up:

- **Checkpoint** — snapshot executor state to `StateStore` after each node completes
- **Resume** — `Engine::resume(run_id)` loads checkpoint, re-enters executor at last completed node
- **`cleargate resume <run_id>`** CLI command
- Human-in-the-loop resume already works — this generalizes it to any interruption

### Files

- **Modify:** `flow/executor.rs`, `flow/engine.rs`
- **Add CLI:** `cli/resume.rs`

---

## O9: Python Bindings (PyO3)

**Impact**: High — largest AI developer audience.
**Scope**: Large (~3-4 weeks)
**Depends on**: O1-O8 stabilized
**Cross-ref**: [ROADMAP.md](./ROADMAP.md) M-E covers observer-mode Python bindings

### What Gets Built

New crate: `crates/cleargate-py/` (PyO3 + maturin → `pip install cleargate`)

**Orchestrator mode exposed to Python:**

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

**Custom nodes and LLM providers via decorators:**

```python
@cleargate.node(node_type="my_transform", inputs=[...], outputs=[...])
async def my_transform(inputs, config, ctx):
    return {"output": transformed}
```

### Files

- `crates/cleargate-py/Cargo.toml`, `src/lib.rs`, `src/engine.rs`, `src/node.rs`, `src/llm.rs`, `src/types.rs`
- `crates/cleargate-py/pyproject.toml`, `python/cleargate/__init__.py`

### Note

Observer-mode Python bindings (ObserverSession, replay, diff, inspect) live in ROADMAP.md M-E. This milestone covers orchestrator-mode bindings. The crate is shared — both modes ship in `pip install cleargate`.

---

## O10: Framework Adapters

**Impact**: Medium — adoption hook for existing framework users.
**Scope**: Small per adapter (~1-2 weeks each)
**Depends on**: O9 (Python bindings)
**Cross-ref**: [ROADMAP.md](./ROADMAP.md) M-E covers observer adapters

### What Gets Built

Thin Python packages that let existing framework users leverage ClearGate orchestration features:

- **`cleargate-langchain`** — LangChain callback handler → ObserverSession + orchestrator bridge
- **`cleargate-crewai`** — CrewAI callback pattern
- **`cleargate-semantic-kernel`** — SK middleware filters
- **`cleargate-autogen`** — Message interceptor

Each adapter is ~100-200 LOC. The hard work is in O1-O9.

---

## Additional Items (demand-driven)

| Feature | Source | Notes |
|---|---|---|
| SeaORM Persistence | CONSOLIDATED_PLAN F2 | Separate crate `cleargate-flow-storage/`, keeps engine dep-free |
| Testing Utilities | CONSOLIDATED_PLAN F8 | Mock providers, deterministic replay, snapshot testing |
| Schema Generation | ROBUST_AGENTS M16.9 | OpenAPI/JSON Schema for flow definitions (schemars) |
| C ABI Surface | CONSOLIDATED_PLAN E1 | Handle-based C API for Go/C#/Ruby/Swift bindings |
| Node.js Bindings | CONSOLIDATED_PLAN E2 | napi-rs, Promise-based async |
| WASM Target | CONSOLIDATED_PLAN E3 | Constrained (no dylib, no fs, single-threaded) |

---

## Dependency Graph

```
O1 (MCP)              ─┐
O2 (Structured Output) ─┤
O4 (Rate Limit/Cache)  ─┤─── independent, can parallelize
O5 (Guardrails)        ─┤
O8 (Resumable)         ─┘

O3 (Memory)            ─── O6 (Multi-Agent)

O7 (Graph Composition) ─── standalone

O1-O8 stabilized       ─── O9 (Python) ─── O10 (Adapters)
```

**Suggested execution**: O1 → O2 → O3 → O4/O5 (parallel) → O6 → O7 → O8 → O9 → O10

---

## Verification

Each milestone verified by:

- `cargo test --workspace` — all existing tests still pass
- `cargo clippy --workspace` — 0 warnings
- New integration tests per milestone
- O9-O10: `maturin develop && pytest`

# ClearGate Flow Engine — Project Context

## What This Is

An open-source flow engine for orchestrating AI workflows. Provides deterministic, replayable DAG execution with multi-language SDK support (Python, Node.js, .NET) and observability for LLM-powered agent pipelines.

## Architecture

```
crates/
  cleargate-flow-engine   Core DAG engine (pure library, no DB/web deps)
  cleargate-storage-oss   SeaORM persistence (SQLite, PostgreSQL)
  cleargate-adapters      Framework adapters (LangChain, LangGraph, Semantic Kernel)
  cleargate-providers     LLM providers (OpenAI, Anthropic, Gemini, Ollama)
  cleargate-python        Python native bindings (PyO3/Maturin)
  cleargate-node          Node.js native bindings (NAPI-RS)
  cleargate-dotnet        .NET native bindings (C FFI / P/Invoke)
  test-echo-node          Test fixture
sdks/
  dotnet/                 Managed C# SDK (ObserverSession, SemanticKernelFilter)
  node/                   TypeScript SDK (ObserverSession, LangChain/LangGraph handlers)
examples/
  flow-engine-agents/     Flow configuration examples (JSON)
  observability/          SDK examples (Python, Node.js, .NET)
```

## Build Commands

```bash
# Rust workspace
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo doc --workspace --no-deps
cargo fmt --all -- --check

# Auto-format everything
./fmt.sh

# Python SDK (development build)
cd crates/cleargate-python && maturin develop --release

# Node.js SDK
cd crates/cleargate-node && npm run build

# .NET SDK
cargo build --release -p cleargate-dotnet
```

## Quality Gates (must pass before completing any task)

- `cargo clippy --workspace` — zero warnings
- `cargo doc --no-deps --workspace` — zero warnings
- `cargo test --workspace` — all tests pass
- `cargo fmt --all -- --check` — no formatting issues

## Conventions

- **Commits**: Conventional Commits format (`feat:`, `fix:`, `refactor:`, etc.)
- **File size**: Split files over ~500 lines into module directories
- **Visibility**: No glob re-exports in lib.rs; use explicit `pub use`
- **Tests**: Feature-gate test utilities behind `#[cfg(any(test, feature = "test-support"))]`
- **Docs**: Escape angle brackets in doc comments with backticks
- **Formatting**: Run `./fmt.sh` before committing

## Key Patterns

- **ObserverSession**: Core recording primitive across all SDKs — records LLM calls, tool calls, steps
- **AdapterSession**: Framework-specific session that translates adapter events to WriteEvents
- **WriteEvent**: Immutable event log entry — enables deterministic replay
- **NodeHandler trait**: Extension point for custom flow engine nodes
- **LLMProvider trait**: Unified interface to LLM providers with streaming support

## License

Apache-2.0

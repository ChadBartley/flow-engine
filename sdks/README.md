# ClearGate Language SDKs

Managed SDKs that provide idiomatic access to ClearGate's observability and adapter sessions across multiple languages.

## Architecture

All SDKs follow the same two-layer pattern:

1. **Native layer** (Rust) -- High-performance core compiled per-platform
2. **Managed layer** (language-idiomatic) -- Ergonomic API wrapping the native bindings

## SDKs

| Language | Native Binding | Managed SDK | Location |
|----------|---------------|-------------|----------|
| **Python** | PyO3 | Built-in (single package) | `crates/cleargate-python/` |
| **Node.js** | NAPI-RS | TypeScript wrapper | `crates/cleargate-node/` + `sdks/node/` |
| **.NET** | C FFI / P/Invoke | C# wrapper | `crates/cleargate-dotnet/` + `sdks/dotnet/` |

## Shared Concepts

All SDKs expose the same core abstractions:

- **ObserverSession** -- Record LLM calls, tool calls, and workflow steps into a deterministic replay session
- **AdapterSession** -- Framework-specific wrapper that translates LangChain, LangGraph, or Semantic Kernel events into the unified session model

## Getting Started

See each SDK's README for installation and usage instructions:

- [Node.js SDK](node/README.md)
- [.NET SDK](dotnet/README.md)
- Python SDK: see `crates/cleargate-python/`

## License

Apache-2.0

# ClearGate Flow Engine

An open-source flow engine for orchestrating AI workflows, with LLM provider integrations and multi-language SDK support.

## Crates

| Crate | Description |
|-------|-------------|
| `cleargate-flow-engine` | Core flow engine: DAG execution, built-in nodes, triggers |
| `cleargate-storage-oss` | SeaORM-backed persistence for flows and executions |
| `cleargate-adapters` | Framework adapters (LangChain, etc.) |
| `cleargate-providers` | LLM provider integrations (OpenAI, Anthropic, etc.) |
| `cleargate-python` | Python bindings (PyO3) |
| `cleargate-node` | Node.js bindings (NAPI-RS) |
| `cleargate-dotnet` | .NET native bindings |

## SDKs

- **Python**: `crates/cleargate-python/` (native) + PyPI package
- **Node.js**: `crates/cleargate-node/` (native) + `packages/cleargate/` (TypeScript SDK)
- **C#/.NET**: `crates/cleargate-dotnet/` (native) + `dotnet/Cleargate/` (managed SDK)

## Building

```bash
cargo build --workspace
cargo test --workspace
cargo clippy --workspace -- -D warnings
```

## License

Apache-2.0 — see [LICENSE](LICENSE) for details.

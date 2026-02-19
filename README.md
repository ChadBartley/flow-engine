# ClearGate Flow Engine

An open-source flow engine for orchestrating AI workflows, with LLM provider integrations, framework adapters, and multi-language SDK support.

## What It Does

- **Flow Engine** — Deterministic, replayable DAG execution with built-in nodes for HTTP calls, LLM completions, conditional branching, JQ transforms, CSV tools, human-in-the-loop, and tool routing
- **Observability** — Record every LLM call, tool invocation, and execution step into structured sessions for debugging, replay, and cost analysis
- **Framework Adapters** — Drop-in integration with LangChain, LangGraph, and Semantic Kernel
- **LLM Providers** — Unified interface to OpenAI, Anthropic, Gemini, Ollama, and any OpenAI-compatible endpoint
- **Multi-Language SDKs** — Native bindings for Python, Node.js, and .NET with idiomatic managed SDKs

## Directory Structure

| Directory | Description |
|-----------|-------------|
| `crates/cleargate-flow-engine` | Core DAG execution engine (pure Rust library) |
| `crates/cleargate-storage-oss` | SeaORM-backed persistence (SQLite, PostgreSQL) |
| `crates/cleargate-adapters` | Framework adapters (LangChain, LangGraph, Semantic Kernel) |
| `crates/cleargate-providers` | LLM provider integrations |
| `crates/cleargate-python` | Python native bindings (PyO3) |
| `crates/cleargate-node` | Node.js native bindings (NAPI-RS) |
| `crates/cleargate-dotnet` | .NET native bindings (C FFI) |
| `sdks/dotnet/` | Managed C# SDK |
| `sdks/node/` | TypeScript SDK |
| `examples/flow-engine-agents/` | Flow configuration examples |
| `examples/observability/` | SDK observability examples |

## Quick Start

### Rust

```bash
cargo build --workspace
cargo test --workspace
```

### Python

```bash
cd crates/cleargate-python
maturin develop --release
```

```python
from cleargate import observe

with observe("my-session") as session:
    session.record_llm_call("node-id", request_json, response_json)
    session.record_tool_call("tool-name", inputs_json, outputs_json, duration_ms)
```

### Node.js

```bash
cd crates/cleargate-node && npm run build
cd sdks/node && npm install
```

```typescript
import { observe } from "cleargate";

const session = observe("my-session");
session.recordLlmCall("node-id", requestJson, responseJson);
session.recordToolCall("tool-name", inputsJson, outputsJson, durationMs);
session.finish();
```

### .NET

```bash
cargo build --release -p cleargate-dotnet
```

```csharp
using Cleargate;

using var session = ObserverSession.Start("my-session");
session.RecordLlmCall("node-id", requestJson, responseJson);
session.RecordToolCall("tool-name", inputsJson, outputsJson, 5);
```

## Examples

See [`examples/`](examples/) for runnable examples:

- **[Flow Engine Agents](examples/flow-engine-agents/)** — Flow configuration files for math, data analysis, inventory, and story-writing agents
- **[Python Observability](examples/observability/python/)** — ObserverSession, LangChain, LangGraph, and Semantic Kernel
- **[Node.js Observability](examples/observability/node/)** — ObserverSession, LangChain, and LangGraph
- **[.NET Observability](examples/observability/dotnet/)** — ObserverSession and Semantic Kernel

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup and guidelines.

## License

Apache-2.0 — see [LICENSE](LICENSE) for details.

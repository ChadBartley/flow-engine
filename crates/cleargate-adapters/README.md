# cleargate-adapters

Framework adapter layer that bridges popular AI orchestration frameworks to ClearGate's unified observability model.

## Overview

Each adapter translates framework-specific events (LLM calls, tool invocations, chain steps) into ClearGate's `ObserverSession` format, enabling consistent recording and replay regardless of the source framework.

## Exports

| Adapter | Framework | Description |
|---------|-----------|-------------|
| `LangChainAdapter` | LangChain | Captures chain runs, LLM calls, and tool calls |
| `LangGraphAdapter` | LangGraph | Captures graph node executions and state transitions |
| `SemanticKernelAdapter` | Semantic Kernel | Captures kernel function invocations and planner steps |

## How It Works

1. An `AdapterSession` wraps an `ObserverSession` with framework-specific event translation
2. The adapter receives callbacks or events from the framework runtime
3. Events are normalized into ClearGate's unified schema (LLM calls, tool calls, steps)
4. The underlying `ObserverSession` records everything for replay and analysis

## Usage

```rust
use cleargate_adapters::LangChainAdapter;

let session = ObserverSession::new(config);
let adapter = LangChainAdapter::new(session);
// Pass adapter as callback handler to LangChain
```

## License

Apache-2.0

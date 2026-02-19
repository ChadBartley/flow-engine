# Observability Examples

Demonstrations of ClearGate's observability SDKs across Python, Node.js, and .NET. Each example records LLM interactions and framework events into ClearGate sessions for deterministic replay and analysis.

## Examples by Language

### Python

Uses the `cleargate` PyO3 package directly. Demonstrates:

- `ObserverSession` for recording raw LLM calls
- LangChain adapter integration
- LangGraph adapter integration

### Node.js

Uses the `@cleargate/sdk` TypeScript package. Demonstrates:

- `ObserverSession` for recording raw LLM calls
- LangChain adapter integration

### .NET

Uses the `ClearGate` NuGet package. Demonstrates:

- `ObserverSession` for recording raw LLM calls
- `SemanticKernelFilter` for automatic Semantic Kernel instrumentation
- `InstrumentedChatCompletionService` wrapper

## Prerequisites

All examples expect a local Ollama instance with a model available:

```bash
ollama pull llama3.2
```

## Running

Each language directory contains its own setup instructions. Generally:

```bash
# Python
cd python && pip install -e . && python main.py

# Node.js
cd node && npm install && npx tsx main.ts

# .NET
cd dotnet && dotnet run
```

## License

Apache-2.0

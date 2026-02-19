# cleargate

LLM observability, replay, and testing for AI agent pipelines.

## Installation

```bash
npm install cleargate
```

## Quick Start

### Observer Session

```ts
import { observe } from "cleargate";

const session = observe("my-flow");

session.recordLlmCall("llm-1", {
  provider: "openai",
  model: "gpt-4o",
  messages: [{ role: "user", content: "Hello" }],
}, {
  content: "Hi there!",
  model: "gpt-4o",
  input_tokens: 10,
  output_tokens: 5,
  total_tokens: 15,
  finish_reason: "stop",
  latency_ms: 200,
});

session.recordToolCall("weather", { city: "tokyo" }, { result: "80F" }, 15);
session.recordStep("final-answer", { answer: "It's 80F in Tokyo" });

session.finish("completed");

// Run summary — metadata, status, timing, LLM token stats
const runData = session.getRunData();

// Full event log — every LLM call, tool invocation, step
// This is the detailed record used by DiffEngine and ReplayEngine
const events = session.getEvents();
```

### LangChain Integration

```ts
import { CleargateLangChainHandler } from "cleargate";

const handler = new CleargateLangChainHandler("my-agent");

const result = await agent.invoke(
  { messages: [["user", "What's the weather?"]] },
  { callbacks: [handler] },
);

handler.finish();

const runData = handler.getRunData();
const events = handler.getEvents();
```

### LangGraph Integration

```ts
import { CleargateLangGraphHandler } from "cleargate";

const handler = new CleargateLangGraphHandler("my-graph");

const app = graph.compile();
const result = await app.invoke(
  { messages: [["user", "hello"]] },
  { callbacks: [handler] },
);

handler.finish();
```

### Semantic Kernel Integration

```ts
import { CleargateSemanticKernelHandler } from "cleargate";

const handler = new CleargateSemanticKernelHandler("my-kernel");

handler.onFunctionInvoking("WeatherPlugin", "GetWeather", { city: "tokyo" });
handler.onFunctionInvoked("WeatherPlugin", "GetWeather", "80F, humid");

handler.finish();
```

## API

### ObserverSession

| Method | Description |
|--------|-------------|
| `ObserverSession.start(name)` | Create a new session |
| `recordLlmCall(nodeId, request, response)` | Record an LLM call |
| `recordToolCall(toolName, inputs, outputs, durationMs)` | Record a tool invocation |
| `recordStep(stepName, data)` | Record a named step |
| `finish(status?)` | Finish the session |
| `getRunData()` | Run summary as object |
| `getEvents()` | Full event log as array |
| `runId` | The run ID |

### AdapterSession

| Method | Description |
|--------|-------------|
| `AdapterSession.start(framework, name?)` | Create session |
| `onEvent(eventData)` | Process a framework callback |
| `finish(status?)` | Finish the session |
| `getRunData()` | Run summary |
| `getEvents()` | Full event log |

## Native Library

This package requires the `cleargate-node` native binary (built from `crates/cleargate-node/`):

```bash
cargo build -p cleargate-node --release
```

## Requirements

- Node.js 18+
- `cleargate-node` native binary

## License

Apache-2.0

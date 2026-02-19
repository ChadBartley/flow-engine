# cleargate-flow-engine

Core DAG-based workflow execution engine for ClearGate. This is a pure library crate with no database or web server dependencies.

## Overview

The flow engine executes directed acyclic graphs (DAGs) of nodes, where each node performs a discrete unit of work. Flows are defined declaratively and executed with built-in retry policies, cost tracking, and expression evaluation.

## Built-in Nodes

| Node | Description |
|------|-------------|
| `HttpNode` | Make HTTP requests with configurable method, headers, and body |
| `LlmCallNode` | Invoke LLM providers with prompt templates and model selection |
| `ConditionalNode` | Branch execution based on expression evaluation |
| `JqNode` | Transform data using JQ expressions |
| `CsvToolNode` | Parse and query CSV data |
| `ToolRouterNode` | Route tool calls to registered handlers |
| `HumanInLoopNode` | Pause execution for human review and approval |
| `JsonLookupNode` | Extract values from JSON payloads |

## Triggers

- **HTTP** -- Start a flow via an HTTP request
- **Cron** -- Schedule flows on a recurring basis
- **Flow** -- Chain flows together by triggering one from another

## Key Features

- **Expression Evaluation**: Reference outputs from upstream nodes using expression syntax
- **Retry Policies**: Configurable retry with backoff for transient failures
- **Cost Tracking**: Accumulate token usage and cost across LLM calls in a run
- **Deterministic Replay**: Flow runs produce a complete event log for replay and debugging

## Usage

```rust
use cleargate_flow_engine::{FlowEngine, FlowDefinition};

let definition = FlowDefinition::from_file("my-flow.yaml")?;
let engine = FlowEngine::new(providers);
let result = engine.execute(definition, input).await?;
```

## License

Apache-2.0

# Flow Engine Agent Examples

Declarative flow configurations that demonstrate the ClearGate flow engine's capabilities.

## Agents

| Flow | Description |
|------|-------------|
| `math-agent` | Solves math problems using LLM with tool-calling for calculations |
| `data-analyst` | Analyzes CSV data using JQ transformations and LLM summarization |
| `inventory-agent` | Manages inventory queries with conditional routing and HTTP lookups |
| `story-writer` | Multi-step creative writing with human-in-loop review |

## Test Configurations

Additional flow configs used by the integration test suite. These exercise specific node types and edge cases such as retry policies, conditional branching, and expression evaluation.

## Flow Structure

Each flow is defined as a YAML file containing:

- **Nodes** -- The processing steps (LLM calls, HTTP requests, conditionals, etc.)
- **Edges** -- Connections between nodes defining execution order
- **Triggers** -- How the flow is started (HTTP, Cron, or chained from another flow)
- **Variables** -- Input parameters and expression references

## Running

Flows are executed by the ClearGate flow engine. See the main project README for setup instructions.

## License

Apache-2.0

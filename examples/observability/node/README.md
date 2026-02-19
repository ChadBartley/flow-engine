# ClearGate Node.js Observability Examples

Examples demonstrating ClearGate's observability features with Node.js AI frameworks.

## Prerequisites

- Node.js 18+
- [Ollama](https://ollama.ai/) running locally
- Pull a model: `ollama pull qwen3:4b`

## Setup

1. Build the ClearGate native library (from repo root):

   ```bash
   cd crates/cleargate-node
   npm run build
   ```

2. Install dependencies:

   ```bash
   npm install
   ```

## Running Examples

```bash
# Basic ObserverSession usage
npm run basic

# LangChain with tool-calling agent
npm run langchain

# LangGraph state graph
npm run langgraph
```

## Persistent Storage

All examples persist run data to a local SQLite database (`cleargate_runs.db`) using the `storeUrl` parameter. The `?mode=rwc` query parameter tells SQLite to create the file if it doesn't exist. Data accumulates across subsequent runs.

## What Gets Captured

Each example records LLM calls, tool invocations, and execution steps into a ClearGate session. After running, you can inspect:

- **Run data**: Metadata, timing, token usage, cost estimates
- **Events**: Full sequence of LLM calls, tool calls, and state transitions

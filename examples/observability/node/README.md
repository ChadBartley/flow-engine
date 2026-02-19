# ClearGate Node.js Observability Examples

Examples demonstrating ClearGate's observability features with Node.js AI frameworks.

## Prerequisites

- Node.js 18+
- [Ollama](https://ollama.ai/) running locally
- Pull the required models:
  ```bash
  ollama pull qwen3:4b        # used by langchain_example
  ollama pull ministral-3:3b  # used by advanced_agent_example
  ```

## Setup

Run the setup script from this directory:

```bash
./setup.sh
```

This builds the Rust native addon, compiles the TypeScript SDK, and installs all example dependencies in one step.

### Manual Setup

If you prefer to run the steps individually:

1. Build the native Rust addon (from repo root):

   ```bash
   cargo build --release -p cleargate-node
   cp target/release/libcleargate_node.so sdks/node/cleargate.node   # .dylib on macOS
   ```

2. Build the TypeScript SDK:

   ```bash
   cd sdks/node
   npm install
   npm run build
   ```

3. Install example dependencies (from this directory):

   ```bash
   npm install --install-links
   ```

## Running Examples

```bash
# Basic ObserverSession usage
npm run basic

# LangChain with tool-calling agent
npm run langchain

# LangGraph state graph
npm run langgraph

# Advanced multi-tool agent with full event capture
npm run advanced
```

## Persistent Storage

All examples persist run data to a local SQLite database (`cleargate_runs.db`) using the `storeUrl` parameter. The `?mode=rwc` query parameter tells SQLite to create the file if it doesn't exist. Data accumulates across subsequent runs.

## What Gets Captured

Each example records LLM calls, tool invocations, and execution steps into a ClearGate session. After running, you can inspect:

- **Run data**: Metadata, timing, token usage, cost estimates
- **Events**: Full sequence of LLM calls, tool calls, and state transitions

The advanced example (`npm run advanced`) demonstrates the complete capture pattern: multiple tools, full event summary, and detailed run data output — matching the Python `advanced_agent_example.py`.

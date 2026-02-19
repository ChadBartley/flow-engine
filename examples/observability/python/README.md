# ClearGate Python Observability Examples

Examples demonstrating ClearGate's observability features with Python AI frameworks.

## Prerequisites

- Python 3.9+
- [Ollama](https://ollama.ai/) running locally
- Pull a model: `ollama pull qwen3:4b`

## Setup

1. Create a virtual environment and install dependencies:

   ```bash
   cd examples/observability/python
   uv venv
   source .venv/bin/activate
   uv pip install langchain langchain-ollama langgraph "semantic-kernel>=1.0.0"
   ```

2. Build and install the ClearGate native library into the active venv:

   ```bash
   cd ../../../crates/cleargate-python
   maturin develop --release
   ```

3. Return to the examples directory:

   ```bash
   cd ../../examples/observability/python
   ```

## Running Examples

```bash
# Basic ObserverSession usage
python basic_observe.py

# LangChain with tool-calling agent
python langchain_example.py

# LangGraph state graph
python langgraph_example.py

# Semantic Kernel basic
python semantic_kernel_example.py

# Semantic Kernel with tools and full event capture
python semantic_kernel_full_example.py

# Advanced multi-tool agent with full event dump
python advanced_agent_example.py
```

## Persistent Storage

All examples persist run data to a local SQLite database (`cleargate_runs.db`) using the `store_path` parameter. The `?mode=rwc` query parameter tells SQLite to create the file if it doesn't exist. Data accumulates across subsequent runs.

## What Gets Captured

Each example records LLM calls, tool invocations, and execution steps into a ClearGate session. After running, you can inspect:

- **Run data**: Metadata, timing, token usage, cost estimates
- **Events**: Full sequence of LLM calls, tool calls, and state transitions

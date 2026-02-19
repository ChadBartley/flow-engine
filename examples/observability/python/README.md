# ClearGate Python Observability Examples

Examples demonstrating ClearGate's observability features with Python AI frameworks.

## Prerequisites

- Python 3.9+
- [Ollama](https://ollama.ai/) running locally
- Pull a model: `ollama pull qwen3:4b`

## Setup

1. Build the ClearGate native library (from repo root):

   ```bash
   cd crates/cleargate-python
   maturin develop --release
   ```

2. Install dependencies:

   ```bash
   uv sync
   ```

## Running Examples

```bash
# Basic ObserverSession usage
uv run python basic_observe.py

# LangChain with tool-calling agent
uv run python langchain_example.py

# LangGraph state graph
uv run python langgraph_example.py

# Semantic Kernel
uv run python semantic_kernel_example.py
```

## What Gets Captured

Each example records LLM calls, tool invocations, and execution steps into a ClearGate session. After running, you can inspect:

- **Run data**: Metadata, timing, token usage, cost estimates
- **Events**: Full sequence of LLM calls, tool calls, and state transitions

# Cleargate Orchestration — Python Examples

Run flow graphs through the Cleargate engine from Python.

## Prerequisites

1. Build the Python native module:
   ```bash
   cd crates/cleargate-python && maturin develop --release
   ```

2. (Optional) Install Ollama for LLM-based examples:
   ```bash
   ollama pull llama3.2:1b
   ```

## Examples

| # | Script | Description |
|---|--------|-------------|
| 01 | `01_basic_flow.py` | Load and execute a simple echo flow, print events |
| 02 | `02_streaming_events.py` | Stream events in real-time with filtering |
| 03 | `03_custom_node.py` | Register a Python-native custom node handler |
| 04 | `04_custom_llm_provider.py` | Wrap an HTTP API as a custom LLM provider |
| 05 | `05_execute_graph.py` | Execute an ad-hoc graph without storing it |

## Running

```bash
python 01_basic_flow.py
```

# ClearGate .NET Observability Examples

Examples demonstrating ClearGate's observability features with .NET and Semantic Kernel.

## Prerequisites

- .NET 8.0 SDK
- [Ollama](https://ollama.ai/) running locally
- Pull a model: `ollama pull qwen3:4b`

## Setup

1. Build the ClearGate native library (from repo root):

   ```bash
   cargo build --release -p cleargate-dotnet
   ```

2. Ensure the native library is discoverable (copy to output or set `LD_LIBRARY_PATH`/`DYLD_LIBRARY_PATH`).

## Running Examples

```bash
# Basic ObserverSession usage
dotnet run -- basic

# Semantic Kernel with auto-capture
dotnet run -- semantic-kernel
```

## What Gets Captured

Each example records LLM calls, tool invocations, and execution steps into a ClearGate session. After running, you can inspect:

- **Run data**: Metadata, timing, token usage, cost estimates
- **Events**: Full sequence of LLM calls, tool calls, and state transitions

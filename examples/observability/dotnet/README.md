# ClearGate .NET Observability Examples

Examples demonstrating ClearGate's observability features with .NET and Semantic Kernel.

## Prerequisites

- .NET 8.0 SDK
- Rust toolchain (for building the native library)
- [Ollama](https://ollama.ai/) running locally
- Pull a model: `ollama pull qwen3:4b`

## Setup

The quickest way to get started is the setup script, which builds the native library, restores .NET dependencies, and copies the shared library to the output directory:

```bash
chmod +x setup.sh
./setup.sh
```

Or do it manually:

1. Build the ClearGate native library (from repo root):

   ```bash
   cargo build --release -p cleargate-dotnet
   ```

2. Copy the native library to the output directory or set `LD_LIBRARY_PATH` (Linux) / `DYLD_LIBRARY_PATH` (macOS):

   ```bash
   cp ../../../target/release/libcleargate_dotnet.so bin/Debug/net8.0/
   ```

## Running Examples

```bash
# Basic ObserverSession usage — manual LLM call recording
dotnet run -- basic

# Semantic Kernel with auto-capture — filter + instrumented chat service
dotnet run -- semantic-kernel

# Tool calling — multiple SK plugins with auto-captured tool invocations
dotnet run -- tool-call
```

## Examples

| Example | Description |
|---------|-------------|
| `basic` | Manual `ObserverSession` usage: records an LLM call, tool call, and step explicitly |
| `semantic-kernel` | `SemanticKernelFilter` with `Instrument()` for automatic LLM capture |
| `tool-call` | Multiple SK plugins (weather, math) with tool-use enabled — verifies all tool invocations and LLM round-trips are captured |

## Persistent Storage

All examples persist run data to a local SQLite database (`cleargate_runs.db`) using the `storePath` parameter. The `?mode=rwc` query parameter tells SQLite to create the file if it doesn't exist. Data accumulates across subsequent runs.

## What Gets Captured

Each example records LLM calls, tool invocations, and execution steps into a ClearGate session. After running, you can inspect:

- **Run data**: Metadata, timing, token usage, cost estimates
- **Events**: Full sequence of LLM calls, tool calls, and state transitions

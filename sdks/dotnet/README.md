# ClearGate .NET SDK

Managed C# SDK for ClearGate observability and framework adapters.

## Overview

This SDK provides idiomatic C# classes that wrap the native `cleargate-dotnet` library via P/Invoke, handling memory management and marshalling automatically.

## Key Classes

| Class | Description |
|-------|-------------|
| `ObserverSession` | Record LLM calls, tool calls, and steps for replay |
| `AdapterSession` | Framework-specific event translation |
| `SemanticKernelFilter` | Drop-in `IFunctionInvocationFilter` / `IPromptRenderFilter` for automatic Semantic Kernel instrumentation |
| `InstrumentedChatCompletionService` | Wraps any `IChatCompletionService` with automatic recording (internal, created via `SemanticKernelFilter.Instrument`) |

## Usage

### Observer Session

```csharp
using Cleargate;

using var session = ObserverSession.Start("my-flow", storePath: "sqlite://runs.db?mode=rwc");

session.RecordLlmCall("llm-1", new
{
    provider = "openai",
    model = "gpt-4o",
    messages = new[] { new { role = "user", content = "Hello" } },
}, new
{
    content = "Hi there!",
    model = "gpt-4o",
    input_tokens = 10,
    output_tokens = 5,
    total_tokens = 15,
    finish_reason = "stop",
    latency_ms = 200,
});

session.RecordToolCall("weather", new { city = "tokyo" }, new { result = "80F" }, durationMs: 15);
session.RecordStep("final-answer", new { answer = "It's 80F in Tokyo" });

session.Finish("completed");

var runData = session.GetRunData();  // Run summary as JSON
var events = session.GetEvents();    // Full event log as JSON array
```

### Semantic Kernel Integration

```csharp
using Cleargate;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

using var filter = new SemanticKernelFilter("my-kernel", storePath: "sqlite://runs.db?mode=rwc");

// Register filters for automatic function/prompt capture
kernel.FunctionInvocationFilters.Add(filter);
kernel.PromptRenderFilters.Add(filter);

// Instrument the chat service for LLM call capture
// Pass model/provider explicitly for reliable metadata, or omit to auto-detect from service attributes
var chatService = filter.Instrument(
    kernel.GetRequiredService<IChatCompletionService>(),
    model: "gpt-4o",
    provider: "openai");

var history = new ChatHistory();
history.AddUserMessage("What's the weather in Tokyo?");
var response = await chatService.GetChatMessageContentsAsync(history);

filter.Finish();
var runData = filter.GetRunData();  // summary with token stats
var events = filter.GetEvents();    // full event log for replay/diff
```

## Prerequisites

The native `cleargate-dotnet` shared library must be available at runtime. Build it from the repo root:

```bash
cargo build --release -p cleargate-dotnet
```

Then place it in the application's output directory or set `LD_LIBRARY_PATH` (Linux) / `DYLD_LIBRARY_PATH` (macOS).

See [`examples/observability/dotnet/`](../../examples/observability/dotnet/) for runnable examples with a setup script.

## License

Apache-2.0

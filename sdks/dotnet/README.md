# ClearGate .NET SDK

Managed C# SDK for ClearGate observability and framework adapters.

## Overview

This SDK provides idiomatic C# classes that wrap the native `cleargate-dotnet` library via P/Invoke, handling memory management and marshalling automatically.

## Key Classes

| Class | Description |
|-------|-------------|
| `ObserverSession` | Record LLM calls, tool calls, and steps for replay |
| `AdapterSession` | Framework-specific event translation |
| `SemanticKernelFilter` | Drop-in `IFunctionFilter` for automatic Semantic Kernel instrumentation |
| `InstrumentedChatCompletionService` | Wraps any `IChatCompletionService` with automatic recording |

## Usage

```csharp
using ClearGate;

await using var session = new ObserverSession(config);

session.RecordLlmCall(new LlmCallRecord
{
    Provider = "openai",
    Model = "gpt-4o",
    InputTokens = 150,
    OutputTokens = 42
});

await session.EndAsync();
```

### Semantic Kernel Integration

```csharp
var kernel = Kernel.CreateBuilder()
    .AddOpenAIChatCompletion("gpt-4o", apiKey)
    .Build();

var session = new AdapterSession(config, Framework.SemanticKernel);
kernel.FunctionFilters.Add(new SemanticKernelFilter(session));
```

## Prerequisites

The native `cleargate-dotnet` shared library must be available at runtime. Place it in the application's output directory or in a system library path.

## License

Apache-2.0

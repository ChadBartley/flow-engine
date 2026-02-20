# Cleargate

LLM observability, replay, and testing for AI agent pipelines.

## Installation

```bash
dotnet add package Cleargate
```

## Quick Start

### Observer Session

```csharp
using Cleargate;

using var session = ObserverSession.Start("my-flow");

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

// Run summary — metadata, status, timing, LLM token stats
var runData = session.GetRunData();

// Full event log — every LLM call, tool invocation, step
// This is the detailed record used by DiffEngine and ReplayEngine
var events = session.GetEvents();
```

### Semantic Kernel Integration (Auto-Capture)

```csharp
using Cleargate;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

var filter = new SemanticKernelFilter("my-kernel");

// Register filters — captures function invocations and prompt renders
kernel.FunctionInvocationFilters.Add(filter);
kernel.PromptRenderFilters.Add(filter);

// Instrument the chat service — captures every LLM call with full request/response
// Pass model/provider explicitly for reliable metadata, or omit to auto-detect
var chatService = filter.Instrument(
    kernel.GetRequiredService<IChatCompletionService>(),
    model: "gpt-4o",
    provider: "openai");

// kernel.InvokeAsync triggers filters automatically
var result = await kernel.InvokeAsync(promptFn);

// Direct chat service calls are also captured
var history = new ChatHistory();
history.AddUserMessage("What's the weather in Tokyo?");
var response = await chatService.GetChatMessageContentsAsync(history);

filter.Finish();
var runData = filter.GetRunData();  // summary with token stats
var events = filter.GetEvents();    // full event log for replay/diff
```

### Adapter Session

For LangChain, LangGraph, or Semantic Kernel event streams:

```csharp
using Cleargate;

using var session = AdapterSession.Start("semantic_kernel", "my-session");

session.OnEvent(new
{
    callback = "function_invoking",
    plugin_name = "MathPlugin",
    function_name = "Calculate",
    arguments = new { expression = "25 * 4" },
});

session.OnEvent(new
{
    callback = "function_invoked",
    plugin_name = "MathPlugin",
    function_name = "Calculate",
    result = "100",
    duration_ms = 5,
});

session.Finish();
```

## API

### ObserverSession

| Method | Description |
|--------|-------------|
| `Start(name, storePath?)` | Create a new session |
| `RecordLlmCall(nodeId, request, response)` | Record an LLM call |
| `RecordToolCall(toolName, inputs, outputs, durationMs)` | Record a tool invocation |
| `RecordStep(stepName, data)` | Record a named step |
| `Finish(status?)` | Finish the session (`"completed"`, `"failed"`, `"cancelled"`) |
| `GetRunData()` | Run summary as JSON |
| `GetEvents()` | Full event log as JSON array |
| `RunId` | The run ID |
| `Dispose()` | Finishes if needed, then cleans up |

### AdapterSession

| Method | Description |
|--------|-------------|
| `Start(framework, name?)` | Create session (`"langchain"`, `"langgraph"`, `"semantic_kernel"`) |
| `OnEvent(eventData)` | Process a framework callback event |
| `Finish(status?)` | Finish the session |
| `GetRunData()` | Run summary as JSON |
| `GetEvents()` | Full event log as JSON array |

### SemanticKernelFilter

Implements `IFunctionInvocationFilter` and `IPromptRenderFilter` — register with a kernel for automatic capture:

```csharp
kernel.FunctionInvocationFilters.Add(filter);
kernel.PromptRenderFilters.Add(filter);
```

| Method / Interface | Description |
|--------|-------------|
| `IFunctionInvocationFilter` | Auto-captures function invocations (args, result, duration) |
| `IPromptRenderFilter` | Auto-captures prompt rendering (template → rendered text) |
| `Instrument(service, model?, provider?)` | Wraps a chat service to capture every LLM request/response. Optional `model`/`provider` override auto-detection. |
| `OnFunctionInvoking(plugin, function, args?)` | Manual: record function start |
| `OnFunctionInvoked(plugin, function, result?, durationMs?)` | Manual: record function completion |
| `OnPromptRendering(template)` | Manual: record prompt template |
| `OnPromptRendered(renderedPrompt)` | Manual: record rendered prompt |
| `Finish(status?)` | Finish the session |
| `GetRunData()` / `GetEvents()` | Query results |

## Native Library

This package requires the `cleargate_dotnet` native library (built from `crates/cleargate-dotnet/`):

```bash
cargo build -p cleargate-dotnet --release
```

Set `LD_LIBRARY_PATH` (Linux), `DYLD_LIBRARY_PATH` (macOS), or copy the library to your output directory.

## Requirements

- .NET 8.0+
- `cleargate_dotnet` native library

## License

Apache-2.0

#pragma warning disable SKEXP0001 // Experimental Ollama connector
#pragma warning disable SKEXP0070

using System.Text.Json;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

namespace Cleargate.Tests;

/// <summary>
/// Integration tests that make real Ollama calls through the SemanticKernelFilter.
/// Requires a running Ollama instance at localhost:11434 with qwen3:4b pulled.
///
/// Run with:
///   LD_LIBRARY_PATH=target/debug dotnet test dotnet/Cleargate.Tests/ --filter "FullyQualifiedName~OllamaIntegration"
/// </summary>
public class OllamaIntegrationTests
{
    private const string OllamaEndpoint = "http://localhost:11434";
    private const string Model = "qwen3:4b";

    [Fact]
    public async Task Instrument_CapturesDirectChatServiceCall()
    {
        var builder = Kernel.CreateBuilder();
        builder.AddOllamaChatCompletion(Model, new Uri(OllamaEndpoint));
        var kernel = builder.Build();

        using var filter = new SemanticKernelFilter("ollama-instrument-test");

        // Wrap the chat service — captures direct calls via on_llm_start/on_llm_end
        var rawService = kernel.GetRequiredService<IChatCompletionService>();
        var instrumented = filter.Instrument(rawService);

        var history = new ChatHistory();
        history.AddUserMessage("Reply with exactly: hello cleargate");

        var response = await instrumented.GetChatMessageContentsAsync(history);

        Assert.NotNull(response);
        Assert.NotEmpty(response);
        Assert.False(string.IsNullOrEmpty(response[0].Content));
        Console.WriteLine($"Ollama response: {response[0].Content}");

        filter.Finish("completed");

        var eventsJson = filter.GetEvents();
        Assert.NotNull(eventsJson);
        Console.WriteLine($"Events: {eventsJson}");

        var events = JsonDocument.Parse(eventsJson);
        var eventCount = events.RootElement.GetArrayLength();

        // RunStarted + LlmCall (paired on_llm_start/on_llm_end) + RunFinished = 3+
        Assert.True(eventCount >= 3, $"Expected at least 3 events, got {eventCount}");

        // Verify an llm_invocation event was recorded with request/response data
        var hasLlmEvent = false;
        for (var i = 0; i < eventCount; i++)
        {
            var evt = events.RootElement[i];
            if (evt.TryGetProperty("event_type", out var et) &&
                et.GetString() == "llm_invocation")
            {
                hasLlmEvent = true;
                // Verify it has the fields ReplayEngine/DiffEngine need
                Assert.True(evt.TryGetProperty("request", out _), "llm_invocation missing request");
                Assert.True(evt.TryGetProperty("response", out _), "llm_invocation missing response");
                Assert.True(evt.TryGetProperty("node_id", out _), "llm_invocation missing node_id");
                break;
            }
        }
        Assert.True(hasLlmEvent, "No llm_invocation event found");

        Console.WriteLine($"Run ID: {filter.RunId}");
        Console.WriteLine($"Events captured: {eventCount}");
    }

    [Fact]
    public async Task Instrument_CapturesKernelInvokeWithFullData()
    {
        var builder = Kernel.CreateBuilder();
        builder.AddOllamaChatCompletion(Model, new Uri(OllamaEndpoint));
        var kernel = builder.Build();

        using var filter = new SemanticKernelFilter("ollama-full-capture-test");
        kernel.FunctionInvocationFilters.Add(filter);
        kernel.PromptRenderFilters.Add(filter);

        // Prompt function triggers both FunctionInvocation and PromptRender filters
        var promptFn = kernel.CreateFunctionFromPrompt(
            "What is 2 + 2? Answer with just the number.");
        var result = await kernel.InvokeAsync(promptFn);

        Assert.NotNull(result);
        Console.WriteLine($"Prompt result: {result}");

        filter.Finish("completed");

        var eventsJson = filter.GetEvents();
        Assert.NotNull(eventsJson);
        Console.WriteLine($"Events: {eventsJson}");

        var events = JsonDocument.Parse(eventsJson);
        var eventCount = events.RootElement.GetArrayLength();
        Console.WriteLine($"Event count: {eventCount}");

        // RunStarted + prompt_rendering + prompt_rendered +
        // function_invoking + function_invoked + RunFinished
        Assert.True(eventCount >= 5, $"Expected at least 5 events, got {eventCount}");
    }

    [Fact]
    public async Task Instrument_CapturesStreamingCall()
    {
        var builder = Kernel.CreateBuilder();
        builder.AddOllamaChatCompletion(Model, new Uri(OllamaEndpoint));
        var kernel = builder.Build();

        using var filter = new SemanticKernelFilter("ollama-streaming-test");

        var rawService = kernel.GetRequiredService<IChatCompletionService>();
        var instrumented = filter.Instrument(rawService);

        var history = new ChatHistory();
        history.AddUserMessage("Reply with exactly: hello");

        var chunks = new List<string>();
        await foreach (var chunk in instrumented.GetStreamingChatMessageContentsAsync(history))
        {
            if (chunk.Content is not null)
                chunks.Add(chunk.Content);
        }

        var assembled = string.Concat(chunks);
        Assert.False(string.IsNullOrEmpty(assembled));
        Console.WriteLine($"Streamed response: {assembled}");

        filter.Finish("completed");

        var eventsJson = filter.GetEvents();
        Assert.NotNull(eventsJson);
        Console.WriteLine($"Events: {eventsJson}");

        var events = JsonDocument.Parse(eventsJson);
        var eventCount = events.RootElement.GetArrayLength();
        Assert.True(eventCount >= 3, $"Expected at least 3 events (start + llm_call + finish), got {eventCount}");
    }
}

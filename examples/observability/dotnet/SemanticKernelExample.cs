using System.Text.Json;
using Cleargate;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

/// <summary>
/// Semantic Kernel observability example with ClearGate.
/// Uses SemanticKernelFilter to auto-capture SK function invocations
/// and InstrumentedChatCompletionService to auto-capture LLM calls.
/// Requires: Ollama running locally with qwen3:4b model.
/// </summary>
static class SemanticKernelExample
{
    public static async Task RunAsync()
    {
        using var filter = new SemanticKernelFilter("sk-example", storePath: "sqlite://cleargate_runs.db?mode=rwc");

        var builder = Kernel.CreateBuilder();

        // Ollama exposes an OpenAI-compatible API
#pragma warning disable SKEXP0010
        builder.AddOpenAIChatCompletion(
            modelId: "qwen3:4b",
            apiKey: "ollama",
            endpoint: new Uri("http://localhost:11434/v1")
        );
#pragma warning restore SKEXP0010

        var kernel = builder.Build();

        // Add ClearGate filters for auto-capture of function invocations and prompts
        kernel.FunctionInvocationFilters.Add(filter);
        kernel.PromptRenderFilters.Add(filter);

        // Wrap the chat service to auto-capture LLM request/response data
        var rawChatService = kernel.GetRequiredService<IChatCompletionService>();
        var chatService = filter.Instrument(rawChatService, model: "qwen3:4b", provider: "ollama");

        var history = new ChatHistory();
        history.AddUserMessage("What is the capital of France? Answer briefly.");

        var result = await chatService.GetChatMessageContentsAsync(history, kernel: kernel);
        Console.WriteLine($"Response: {result[0].Content}");

        filter.Finish();

        Console.WriteLine($"\nRun ID: {filter.RunId}");
        Console.WriteLine($"Run data: {filter.GetRunData()}");

        // Print captured events
        var eventsJson = filter.GetEvents();
        if (eventsJson != null)
        {
            var events = JsonSerializer.Deserialize<JsonElement[]>(eventsJson);
            Console.WriteLine($"\nCaptured {events?.Length ?? 0} events");

            if (events != null)
            {
                for (var i = 0; i < events.Length; i++)
                {
                    Console.WriteLine($"\n--- Event {i + 1} ---");
                    Console.WriteLine(JsonSerializer.Serialize(events[i], new JsonSerializerOptions { WriteIndented = true }));
                }
            }
        }

        Console.WriteLine("\nRun data persisted to cleargate_runs.db");
    }
}

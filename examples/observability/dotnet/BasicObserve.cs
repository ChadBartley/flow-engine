using System.Diagnostics;
using System.Text.Json;
using Cleargate;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

/// <summary>
/// Basic observability example using ObserverSession with Semantic Kernel.
/// Makes a real LLM call to Ollama and manually records the request/response.
/// Requires: Ollama running locally with qwen3:4b model.
/// </summary>
static class BasicObserve
{
    public static async Task RunAsync()
    {
        using var session = ObserverSession.Start("basic-example", "sqlite://cleargate_runs.db?mode=rwc");

        const string prompt = "What is 2 + 2? Answer in one word.";

        // Build a Semantic Kernel with Ollama's OpenAI-compatible endpoint
        var builder = Kernel.CreateBuilder();
#pragma warning disable SKEXP0010
        builder.AddOpenAIChatCompletion(
            modelId: "qwen3:4b",
            apiKey: "ollama",
            endpoint: new Uri("http://localhost:11434/v1")
        );
#pragma warning restore SKEXP0010

        var kernel = builder.Build();
        var chatService = kernel.GetRequiredService<IChatCompletionService>();

        const string model = "qwen3:4b";
        const string provider = "ollama";

        session.RecordStep("setup", new { model, provider });

        var history = new ChatHistory();
        history.AddUserMessage(prompt);

        // Record the request, make the real call, record the response
        var request = new
        {
            provider,
            model,
            messages = new[] { new { role = "user", content = prompt } },
        };

        var sw = Stopwatch.StartNew();
        var result = await chatService.GetChatMessageContentAsync(history, kernel: kernel);
        sw.Stop();

        var response = new
        {
            model,
            content = result.Content ?? "",
            finish_reason = "stop",
            latency_ms = sw.ElapsedMilliseconds,
        };

        session.RecordLlmCall("ollama-chat", request, response);

        // Record a tool call
        session.RecordToolCall(
            "calculator",
            new { expression = "2 + 2" },
            new { result = 4 },
            5
        );

        session.Finish();

        Console.WriteLine($"Response: {result.Content}");
        Console.WriteLine($"\nRun ID: {session.RunId}");
        Console.WriteLine($"Run data: {session.GetRunData()}");

        // Print captured events
        var eventsJson = session.GetEvents();
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

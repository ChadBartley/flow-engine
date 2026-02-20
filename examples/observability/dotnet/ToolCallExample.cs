using System.ComponentModel;
using System.Text.Json;
using Cleargate;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

/// <summary>
/// Advanced observability example with multiple tool calls.
/// Registers native functions as SK plugins so the LLM can invoke them,
/// then verifies ClearGate captures every LLM round-trip and tool invocation.
/// Requires: Ollama running locally with qwen3:4b model.
/// </summary>
static class ToolCallExample
{
    public static async Task RunAsync()
    {
        using var filter = new SemanticKernelFilter("tool-call-example", storePath: "sqlite://cleargate_runs.db?mode=rwc");

        var builder = Kernel.CreateBuilder();

#pragma warning disable SKEXP0010
        builder.AddOpenAIChatCompletion(
            modelId: "qwen3:4b",
            apiKey: "ollama",
            endpoint: new Uri("http://localhost:11434/v1")
        );
#pragma warning restore SKEXP0010

        builder.Plugins.AddFromType<WeatherPlugin>();
        builder.Plugins.AddFromType<MathPlugin>();

        var kernel = builder.Build();

        kernel.FunctionInvocationFilters.Add(filter);
        kernel.PromptRenderFilters.Add(filter);

        // Wrap chat service without explicit model/provider to test Attributes fallback
        var rawChatService = kernel.GetRequiredService<IChatCompletionService>();
        var chatService = filter.Instrument(rawChatService);

        var settings = new PromptExecutionSettings
        {
            FunctionChoiceBehavior = FunctionChoiceBehavior.Auto(),
        };

        var history = new ChatHistory();
        history.AddSystemMessage(
            "You are a helpful assistant with access to weather and math tools. " +
            "Use the tools to answer the user's question. Be concise.");
        history.AddUserMessage(
            "What's the weather in Paris and Tokyo? " +
            "Also, what is 42 * 17? Answer concisely after calling the tools.");

        Console.WriteLine("Sending prompt with tool-use enabled...\n");

        var result = await chatService.GetChatMessageContentsAsync(history, settings, kernel);

        Console.WriteLine($"Response: {result[^1].Content}\n");

        filter.Finish();

        Console.WriteLine($"Run ID: {filter.RunId}");
        Console.WriteLine($"Run data: {filter.GetRunData()}");

        var eventsJson = filter.GetEvents();
        if (eventsJson != null)
        {
            var events = JsonSerializer.Deserialize<JsonElement[]>(eventsJson);
            Console.WriteLine($"\nCaptured {events?.Length ?? 0} events:");

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

/// <summary>Simulated weather lookup plugin.</summary>
sealed class WeatherPlugin
{
    [KernelFunction, Description("Get the current weather for a city")]
    public string GetWeather([Description("City name")] string city)
    {
        return city.ToLowerInvariant() switch
        {
            "paris" => "Paris: 18°C, partly cloudy",
            "tokyo" => "Tokyo: 24°C, sunny",
            "london" => "London: 12°C, rainy",
            _ => $"{city}: 20°C, clear skies",
        };
    }
}

/// <summary>Basic math operations plugin.</summary>
sealed class MathPlugin
{
    [KernelFunction, Description("Multiply two numbers")]
    public double Multiply(
        [Description("First number")] double a,
        [Description("Second number")] double b)
    {
        return a * b;
    }
}

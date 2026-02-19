using System.Text.Json;
using Cleargate;

/// <summary>
/// Basic observability example using ObserverSession directly.
/// Records LLM calls, tool calls, and steps manually.
/// </summary>
static class BasicObserve
{
    public static void Run()
    {
        using var session = ObserverSession.Start("basic-example");

        // Record a step
        session.RecordStep("setup", JsonSerializer.Serialize(new { model = "qwen3:4b" }));

        // Simulate an LLM call and record it
        var request = JsonSerializer.Serialize(new
        {
            model = "qwen3:4b",
            messages = new[] { new { role = "user", content = "What is 2 + 2?" } },
        });

        var response = JsonSerializer.Serialize(new
        {
            model = "qwen3:4b",
            message = new { role = "assistant", content = "4" },
        });

        session.RecordLlmCall("ollama-chat", request, response);

        // Record a tool call
        session.RecordToolCall(
            "calculator",
            JsonSerializer.Serialize(new { expression = "2 + 2" }),
            JsonSerializer.Serialize(new { result = 4 }),
            5
        );

        session.Finish();

        Console.WriteLine($"Run ID: {session.RunId}");
        Console.WriteLine($"Run data: {session.GetRunData()}");
    }
}

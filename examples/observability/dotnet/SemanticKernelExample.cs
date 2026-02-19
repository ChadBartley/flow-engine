using Cleargate;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

/// <summary>
/// Semantic Kernel observability example with ClearGate.
/// Uses SemanticKernelFilter to auto-capture SK function invocations.
/// Requires: Ollama running locally with qwen3:4b model.
/// </summary>
static class SemanticKernelExample
{
    public static async Task RunAsync()
    {
        using var session = AdapterSession.Start("semantic_kernel", "sk-example");

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

        // Add ClearGate filter
        var filter = new SemanticKernelFilter(session);
        kernel.FunctionInvocationFilters.Add(filter);

        var chatService = kernel.GetRequiredService<IChatCompletionService>();
        var history = new ChatHistory();
        history.AddUserMessage("What is the capital of France? Answer briefly.");

        var result = await chatService.GetChatMessageContentAsync(history, kernel: kernel);
        Console.WriteLine($"Response: {result}");

        session.Finish();

        Console.WriteLine($"\nRun ID: {session.RunId}");
        Console.WriteLine($"Run data: {session.GetRunData()}");
    }
}

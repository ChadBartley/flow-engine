using System.Text.Json;
using Microsoft.SemanticKernel;

namespace Cleargate.Tests;

/// <summary>
/// Integration tests that exercise every recording API through the native
/// library — mirrors the Python full-capture LangChain demo.
///
/// Requires the cleargate_dotnet cdylib to be built and discoverable via
/// LD_LIBRARY_PATH (Linux) or DYLD_LIBRARY_PATH (macOS).
/// </summary>
public class FullCaptureTests
{
    [Fact]
    public void ObserverSession_FullLifecycle()
    {
        // Start session
        using var session = ObserverSession.Start("full-capture-agent");
        Assert.False(string.IsNullOrEmpty(session.RunId));

        // Record an LLM call (simulating a ReAct agent thinking step)
        session.RecordLlmCall("llm-think", new
        {
            provider = "ollama",
            model = "qwen3:4b",
            messages = new[]
            {
                new { role = "user", content = "Use the weather tool to check Tokyo's weather, then use the calculator tool to compute 25 * 4." },
            },
            temperature = 0.0,
        }, new
        {
            content = "I need to check Tokyo's weather first.",
            model = "qwen3:4b",
            input_tokens = 42,
            output_tokens = 12,
            total_tokens = 54,
            finish_reason = "stop",
            latency_ms = 320,
        });

        // Record a tool call (weather)
        session.RecordToolCall("weather", new { city = "tokyo" }, new { result = "80°F, humid" }, 15);

        // Record another LLM call (agent decides to use calculator)
        session.RecordLlmCall("llm-think", new
        {
            provider = "ollama",
            model = "qwen3:4b",
            messages = new[]
            {
                new { role = "user", content = "Now compute 25 * 4" },
            },
        }, new
        {
            content = "Let me calculate that.",
            model = "qwen3:4b",
            input_tokens = 30,
            output_tokens = 8,
            total_tokens = 38,
            finish_reason = "tool_use",
            latency_ms = 200,
        });

        // Record a tool call (calculator)
        session.RecordToolCall("calculator", new { expression = "25 * 4" }, new { result = "100" }, 1);

        // Record a step (final answer assembly)
        session.RecordStep("assemble-answer", new
        {
            weather = "80°F, humid",
            calculation = 100,
            final_answer = "Tokyo is 80°F and humid. 25 * 4 = 100.",
        });

        // Finish
        session.Finish("completed");

        // Verify run data
        var runDataJson = session.GetRunData();
        Assert.NotNull(runDataJson);
        var runData = JsonDocument.Parse(runDataJson);
        Assert.Equal("completed", runData.RootElement.GetProperty("status").GetString());

        // Verify events — this is the new API we added
        var eventsJson = session.GetEvents();
        Assert.NotNull(eventsJson);
        var events = JsonDocument.Parse(eventsJson);
        Assert.Equal(JsonValueKind.Array, events.RootElement.ValueKind);

        var eventCount = events.RootElement.GetArrayLength();
        // RunStarted + 2 LlmCalls + 2 ToolCalls + 1 Step + RunFinished = 7 minimum
        Assert.True(eventCount >= 7, $"Expected at least 7 events, got {eventCount}");

        // Print summary (visible in test output)
        Console.WriteLine($"Run ID: {session.RunId}");
        Console.WriteLine($"Events: {eventCount} total");
        Console.WriteLine($"Run data: {runDataJson[..Math.Min(200, runDataJson.Length)]}...");
    }

    [Fact]
    public void AdapterSession_SemanticKernel_FullLifecycle()
    {
        // Start an adapter session for Semantic Kernel
        using var session = AdapterSession.Start("semantic_kernel", "sk-demo");
        Assert.False(string.IsNullOrEmpty(session.RunId));

        // Simulate SK function invoking callback
        session.OnEvent(new
        {
            callback = "function_invoking",
            plugin_name = "WeatherPlugin",
            function_name = "GetWeather",
            arguments = new { city = "tokyo" },
        });

        // Simulate SK function invoked callback
        session.OnEvent(new
        {
            callback = "function_invoked",
            plugin_name = "WeatherPlugin",
            function_name = "GetWeather",
            result = "80°F, humid",
            duration_ms = 15,
        });

        // Simulate prompt rendering
        session.OnEvent(new
        {
            callback = "prompt_rendering",
            template = "What is the weather in {{city}}?",
        });

        // Simulate prompt rendered
        session.OnEvent(new
        {
            callback = "prompt_rendered",
            rendered_prompt = "What is the weather in tokyo?",
        });

        session.Finish("completed");

        var runDataJson = session.GetRunData();
        Assert.NotNull(runDataJson);

        var eventsJson = session.GetEvents();
        Assert.NotNull(eventsJson);
        var events = JsonDocument.Parse(eventsJson);
        Assert.True(events.RootElement.GetArrayLength() >= 1);

        Console.WriteLine($"Adapter Run ID: {session.RunId}");
        Console.WriteLine($"Adapter Events: {events.RootElement.GetArrayLength()} total");
    }

    [Fact]
    public void SemanticKernelFilter_FullLifecycle()
    {
        using var filter = new SemanticKernelFilter("sk-filter-demo");
        Assert.False(string.IsNullOrEmpty(filter.RunId));

        // Verify the filter implements SK interfaces for auto-registration
        Assert.IsAssignableFrom<IFunctionInvocationFilter>(filter);
        Assert.IsAssignableFrom<IPromptRenderFilter>(filter);

        // Simulate a complete SK function invocation cycle (manual API)
        filter.OnFunctionInvoking("MathPlugin", "Calculate", new Dictionary<string, object>
        {
            ["expression"] = "25 * 4",
        });

        filter.OnFunctionInvoked("MathPlugin", "Calculate", result: "100");

        filter.OnPromptRendering("Compute {{expression}}");
        filter.OnPromptRendered("Compute 25 * 4");

        filter.Finish("completed");

        var runDataJson = filter.GetRunData();
        Assert.NotNull(runDataJson);

        var eventsJson = filter.GetEvents();
        Assert.NotNull(eventsJson);

        Console.WriteLine($"SK Filter Run ID: {filter.RunId}");
    }

    [Fact]
    public void Dispose_WithoutFinish_CleansUp()
    {
        // Verify Dispose handles cleanup even if Finish wasn't called
        var session = ObserverSession.Start("dispose-test");
        Assert.False(string.IsNullOrEmpty(session.RunId));

        // Record something so the session has state
        session.RecordStep("step1", new { data = "test" });

        // Dispose without Finish — should not throw
        session.Dispose();
    }

    [Fact]
    public void FinishedSession_ThrowsOnRecord()
    {
        using var session = ObserverSession.Start("guard-test");
        session.Finish();

        Assert.Throws<CleargateException>(() =>
            session.RecordStep("should-fail", new { }));

        Assert.Throws<CleargateException>(() =>
            session.RecordLlmCall("x", new { }, new { }));

        Assert.Throws<CleargateException>(() =>
            session.RecordToolCall("x", new { }, new { }, 0));
    }
}

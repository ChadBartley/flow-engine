using System.Text.Json;
using Microsoft.SemanticKernel;
using Microsoft.SemanticKernel.ChatCompletion;

namespace Cleargate;

/// <summary>
/// Semantic Kernel integration for Cleargate observability.
///
/// Implements <see cref="IFunctionInvocationFilter"/> and <see cref="IPromptRenderFilter"/>
/// so the SK runtime calls the hooks automatically:
/// <code>
/// var filter = new SemanticKernelFilter("my-app");
/// kernel.FunctionInvocationFilters.Add(filter);
/// kernel.PromptRenderFilters.Add(filter);
/// </code>
/// </summary>
public sealed class SemanticKernelFilter : IFunctionInvocationFilter, IPromptRenderFilter, IDisposable
{
    private readonly AdapterSession _session;
    private readonly bool _ownsSession;
    private readonly Dictionary<string, long> _functionStartTimes = new();

    public SemanticKernelFilter(string sessionName = "semantic_kernel", AdapterSession? session = null)
    {
        if (session != null)
        {
            _session = session;
            _ownsSession = false;
        }
        else
        {
            _session = AdapterSession.Start("semantic_kernel", sessionName);
            _ownsSession = true;
        }
    }

    /// <summary>The run ID for this session.</summary>
    public string RunId => _session.RunId;

    /// <summary>
    /// Wraps an <see cref="IChatCompletionService"/> to record every LLM call
    /// (request messages, response content, model, token metadata, latency).
    /// Use this to capture data needed by ReplayEngine and DiffEngine even when
    /// calling the chat service directly (outside <c>kernel.InvokeAsync</c>).
    /// </summary>
    public IChatCompletionService Instrument(IChatCompletionService service)
        => new InstrumentedChatCompletionService(service, _session);

    /// <summary>
    /// Called by the SK runtime around each function invocation.
    /// Records function_invoking before and function_invoked after.
    /// </summary>
    public async Task OnFunctionInvocationAsync(FunctionInvocationContext context, Func<FunctionInvocationContext, Task> next)
    {
        var pluginName = context.Function.PluginName ?? "";
        var functionName = context.Function.Name;

        var args = new Dictionary<string, object>();
        foreach (var kvp in context.Arguments)
        {
            if (kvp.Value is not null)
                args[kvp.Key] = kvp.Value;
        }

        OnFunctionInvoking(pluginName, functionName, args);

        var startTick = Environment.TickCount64;
        await next(context);
        var durationMs = Environment.TickCount64 - startTick;

        var result = context.Result.GetValue<object>();
        OnFunctionInvoked(pluginName, functionName, result, durationMs);
    }

    /// <summary>
    /// Called by the SK runtime around prompt rendering.
    /// Records prompt_rendering before and prompt_rendered after.
    /// </summary>
    public async Task OnPromptRenderAsync(PromptRenderContext context, Func<PromptRenderContext, Task> next)
    {
        OnPromptRendering(context.Arguments.ToString());

        await next(context);

        OnPromptRendered(context.RenderedPrompt);
    }

    /// <summary>Record a function invocation start (manual).</summary>
    public void OnFunctionInvoking(string pluginName, string functionName, Dictionary<string, object>? arguments = null)
    {
        var key = $"{pluginName}.{functionName}";
        _functionStartTimes[key] = Environment.TickCount64;
        _session.OnEvent(new
        {
            callback = "function_invoking",
            plugin_name = pluginName,
            function_name = functionName,
            arguments = arguments ?? new Dictionary<string, object>(),
        });
    }

    /// <summary>Record a function invocation completion (manual).</summary>
    public void OnFunctionInvoked(string pluginName, string functionName, object? result = null, long? durationMs = null)
    {
        var key = $"{pluginName}.{functionName}";
        long duration;
        if (durationMs.HasValue)
        {
            duration = durationMs.Value;
        }
        else if (_functionStartTimes.Remove(key, out var start))
        {
            duration = Environment.TickCount64 - start;
        }
        else
        {
            duration = 0;
        }

        _session.OnEvent(new
        {
            callback = "function_invoked",
            plugin_name = pluginName,
            function_name = functionName,
            result = result,
            duration_ms = duration,
        });
    }

    /// <summary>Record a prompt rendering event (manual).</summary>
    public void OnPromptRendering(object? template)
    {
        _session.OnEvent(new
        {
            callback = "prompt_rendering",
            template = template,
        });
    }

    /// <summary>Record a prompt rendered event (manual).</summary>
    public void OnPromptRendered(object? renderedPrompt)
    {
        _session.OnEvent(new
        {
            callback = "prompt_rendered",
            rendered_prompt = renderedPrompt,
        });
    }

    /// <summary>Finish the session.</summary>
    public void Finish(string status = "completed")
    {
        if (_ownsSession) _session.Finish(status);
    }

    /// <summary>Get the run summary (metadata, status, timing, aggregate LLM stats) as JSON.</summary>
    public string? GetRunData() => _session.GetRunData();

    /// <summary>Get the full event log as a JSON array.</summary>
    public string? GetEvents() => _session.GetEvents();

    public void Dispose()
    {
        Finish();
        if (_ownsSession) _session.Dispose();
    }
}

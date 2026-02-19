using System.Text.Json;

namespace Cleargate;

/// <summary>
/// Framework adapter session for recording events from LangChain, LangGraph,
/// or Semantic Kernel through the Cleargate pipeline.
/// </summary>
public sealed class AdapterSession : IDisposable
{
    private ulong _handle;
    private bool _finished;

    private AdapterSession(ulong handle, string runId)
    {
        _handle = handle;
        RunId = runId;
    }

    /// <summary>The run ID for this session.</summary>
    public string RunId { get; }

    /// <summary>
    /// Start a new adapter session.
    /// </summary>
    /// <param name="framework">One of: "langchain", "langgraph", "semantic_kernel"</param>
    /// <param name="name">Optional session name (defaults to framework name)</param>
    /// <param name="storePath">Optional storage URL for persistence (e.g. "sqlite://runs.db?mode=rwc")</param>
    public static AdapterSession Start(string framework, string? name = null, string? storePath = null)
    {
        var handle = Native.cleargate_adapter_start(framework, name, storePath);
        if (handle == 0)
            throw new CleargateException(Native.GetLastError() ?? $"Failed to start adapter session for {framework}");

        var runId = Native.MarshalAndFree(Native.cleargate_adapter_get_run_id(handle))
            ?? throw new CleargateException("Failed to get run ID");

        return new AdapterSession(handle, runId);
    }

    /// <summary>Process a framework callback event.</summary>
    public void OnEvent(object eventData)
    {
        ThrowIfFinished();
        var json = JsonSerializer.Serialize(eventData);
        var result = Native.cleargate_adapter_on_event(_handle, json);
        if (result != 0)
            throw new CleargateException(Native.GetLastError() ?? "Failed to process event");
    }

    /// <summary>Finish the session.</summary>
    public void Finish(string status = "completed")
    {
        if (_finished) return;
        _finished = true;
        var result = Native.cleargate_adapter_finish(_handle, status);
        if (result != 0)
            throw new CleargateException(Native.GetLastError() ?? "Failed to finish session");
    }

    /// <summary>
    /// Get the run summary (metadata, status, timing, aggregate LLM stats) as JSON.
    /// </summary>
    public string? GetRunData()
    {
        return Native.MarshalAndFree(Native.cleargate_adapter_get_run_data(_handle));
    }

    /// <summary>
    /// Get the full event log as a JSON array. Each event is a WriteEvent — LLM calls,
    /// tool invocations, steps, topology hints, etc. This is the detailed record
    /// consumed by DiffEngine and ReplayEngine.
    /// </summary>
    public string? GetEvents()
    {
        return Native.MarshalAndFree(Native.cleargate_adapter_get_events(_handle));
    }

    public void Dispose()
    {
        if (!_finished)
        {
            try { Finish(); }
            catch { /* finish failed — destroy will clean up below */ }
        }
        Native.cleargate_adapter_destroy(_handle);
    }

    private void ThrowIfFinished()
    {
        if (_finished) throw new CleargateException("Session already finished");
    }
}

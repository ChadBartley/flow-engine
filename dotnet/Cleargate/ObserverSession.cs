using System.Text.Json;

namespace Cleargate;

/// <summary>
/// Records LLM calls, tool invocations, and steps for observability.
/// </summary>
public sealed class ObserverSession : IDisposable
{
    private ulong _handle;
    private bool _finished;

    private ObserverSession(ulong handle, string runId)
    {
        _handle = handle;
        RunId = runId;
    }

    /// <summary>The run ID for this session.</summary>
    public string RunId { get; }

    /// <summary>Start a new observer session.</summary>
    public static ObserverSession Start(string name, string? storePath = null)
    {
        var handle = Native.cleargate_observer_start(name, storePath);
        if (handle == 0)
            throw new CleargateException(Native.GetLastError() ?? "Failed to start observer session");

        var runId = Native.MarshalAndFree(Native.cleargate_observer_get_run_id(handle))
            ?? throw new CleargateException("Failed to get run ID");

        return new ObserverSession(handle, runId);
    }

    /// <summary>Record an LLM call.</summary>
    public void RecordLlmCall(string nodeId, object request, object response)
    {
        ThrowIfFinished();
        var reqJson = JsonSerializer.Serialize(request);
        var respJson = JsonSerializer.Serialize(response);
        var result = Native.cleargate_observer_record_llm_call(_handle, nodeId, reqJson, respJson);
        if (result != 0)
            throw new CleargateException(Native.GetLastError() ?? "Failed to record LLM call");
    }

    /// <summary>Record a tool call.</summary>
    public void RecordToolCall(string toolName, object inputs, object outputs, ulong durationMs)
    {
        ThrowIfFinished();
        var inputsJson = JsonSerializer.Serialize(inputs);
        var outputsJson = JsonSerializer.Serialize(outputs);
        var result = Native.cleargate_observer_record_tool_call(
            _handle, toolName, inputsJson, outputsJson, durationMs);
        if (result != 0)
            throw new CleargateException(Native.GetLastError() ?? "Failed to record tool call");
    }

    /// <summary>Record a named step.</summary>
    public void RecordStep(string stepName, object data)
    {
        ThrowIfFinished();
        var dataJson = JsonSerializer.Serialize(data);
        var result = Native.cleargate_observer_record_step(_handle, stepName, dataJson);
        if (result != 0)
            throw new CleargateException(Native.GetLastError() ?? "Failed to record step");
    }

    /// <summary>Finish the session.</summary>
    public void Finish(string status = "completed")
    {
        if (_finished) return;
        _finished = true;
        var result = Native.cleargate_observer_finish(_handle, status);
        if (result != 0)
            throw new CleargateException(Native.GetLastError() ?? "Failed to finish session");
    }

    /// <summary>
    /// Get the run summary (metadata, status, timing, aggregate LLM stats) as JSON.
    /// </summary>
    public string? GetRunData()
    {
        return Native.MarshalAndFree(Native.cleargate_observer_get_run_data(_handle));
    }

    /// <summary>
    /// Get the full event log as a JSON array. Each event is a WriteEvent — LLM calls,
    /// tool invocations, steps, topology hints, etc. This is the detailed record
    /// consumed by DiffEngine and ReplayEngine.
    /// </summary>
    public string? GetEvents()
    {
        return Native.MarshalAndFree(Native.cleargate_observer_get_events(_handle));
    }

    public void Dispose()
    {
        if (!_finished)
        {
            try { Finish(); }
            catch { /* finish failed — destroy will clean up below */ }
        }
        Native.cleargate_observer_destroy(_handle);
    }

    private void ThrowIfFinished()
    {
        if (_finished) throw new CleargateException("Session already finished");
    }
}

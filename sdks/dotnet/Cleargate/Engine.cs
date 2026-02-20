using System.Text.Json;

namespace Cleargate;

/// <summary>
/// Fluent builder for assembling an <see cref="Engine"/> instance.
/// </summary>
public sealed class EngineBuilder : IDisposable
{
    private ulong _handle;

    /// <summary>Create a new engine builder with default settings.</summary>
    public EngineBuilder()
    {
        _handle = EngineNative.cleargate_engine_builder_new();
        if (_handle == 0)
            throw CleargateException.FromNative("Failed to create engine builder");
    }

    /// <summary>Set the directory for flow store (JSON flow definitions).</summary>
    public EngineBuilder FlowStorePath(string path)
    {
        ThrowIfDisposed();
        if (EngineNative.cleargate_engine_builder_flow_store_path(_handle, path) != 0)
            throw CleargateException.FromNative("FlowStorePath failed");
        return this;
    }

    /// <summary>Set the directory for run store (execution records).</summary>
    public EngineBuilder RunStorePath(string path)
    {
        ThrowIfDisposed();
        if (EngineNative.cleargate_engine_builder_run_store_path(_handle, path) != 0)
            throw CleargateException.FromNative("RunStorePath failed");
        return this;
    }

    /// <summary>Set a database URL for the run store (SQLite or PostgreSQL).</summary>
    public EngineBuilder RunStoreUrl(string url)
    {
        ThrowIfDisposed();
        if (EngineNative.cleargate_engine_builder_run_store_url(_handle, url) != 0)
            throw CleargateException.FromNative("RunStoreUrl failed");
        return this;
    }

    /// <summary>Set max edge traversals for cycle detection. Default: 50.</summary>
    public EngineBuilder MaxTraversals(uint value)
    {
        ThrowIfDisposed();
        if (EngineNative.cleargate_engine_builder_max_traversals(_handle, value) != 0)
            throw CleargateException.FromNative("MaxTraversals failed");
        return this;
    }

    /// <summary>Enable or disable crash recovery. Default: true.</summary>
    public EngineBuilder CrashRecovery(bool enabled)
    {
        ThrowIfDisposed();
        if (EngineNative.cleargate_engine_builder_crash_recovery(_handle, enabled) != 0)
            throw CleargateException.FromNative("CrashRecovery failed");
        return this;
    }

    /// <summary>Build the engine. Consumes this builder.</summary>
    public Engine Build()
    {
        ThrowIfDisposed();
        var engineHandle = EngineNative.cleargate_engine_builder_build(_handle);
        if (engineHandle == 0)
            throw CleargateException.FromNative("Engine build failed");
        _handle = 0; // consumed
        return new Engine(engineHandle);
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            EngineNative.cleargate_engine_builder_destroy(_handle);
            _handle = 0;
        }
    }

    private void ThrowIfDisposed()
    {
        if (_handle == 0)
            throw new ObjectDisposedException(nameof(EngineBuilder));
    }
}

/// <summary>
/// The assembled, running FlowEngine runtime.
/// </summary>
public sealed class Engine : IDisposable
{
    private ulong _handle;

    internal Engine(ulong handle)
    {
        _handle = handle;
    }

    /// <summary>Execute a flow by ID with optional inputs.</summary>
    public ExecutionHandle Execute(string flowId, object? inputs = null)
    {
        ThrowIfDisposed();
        var inputsJson = inputs != null ? JsonSerializer.Serialize(inputs) : null;
        var execHandle = EngineNative.cleargate_engine_execute(_handle, flowId, inputsJson);
        if (execHandle == 0)
            throw CleargateException.FromNative("Execute failed");
        return new ExecutionHandle(execHandle);
    }

    /// <summary>Execute a graph definition directly.</summary>
    public ExecutionHandle ExecuteGraph(object graphDef, object? inputs = null)
    {
        ThrowIfDisposed();
        var graphJson = JsonSerializer.Serialize(graphDef);
        var inputsJson = inputs != null ? JsonSerializer.Serialize(inputs) : null;
        var execHandle = EngineNative.cleargate_engine_execute_graph(_handle, graphJson, inputsJson);
        if (execHandle == 0)
            throw CleargateException.FromNative("ExecuteGraph failed");
        return new ExecutionHandle(execHandle);
    }

    /// <summary>Deliver input to a node waiting for human-in-the-loop.</summary>
    public void ProvideInput(string runId, string nodeId, object input)
    {
        ThrowIfDisposed();
        var inputJson = JsonSerializer.Serialize(input);
        if (EngineNative.cleargate_engine_provide_input(_handle, runId, nodeId, inputJson) != 0)
            throw CleargateException.FromNative("ProvideInput failed");
    }

    /// <summary>Return metadata for all registered node handlers.</summary>
    public string? NodeCatalog()
    {
        ThrowIfDisposed();
        return EngineNative.MarshalAndFree(EngineNative.cleargate_engine_node_catalog(_handle));
    }

    /// <summary>Gracefully shut down the engine.</summary>
    public void Shutdown()
    {
        if (_handle != 0)
            EngineNative.cleargate_engine_shutdown(_handle);
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            EngineNative.cleargate_engine_shutdown(_handle);
            EngineNative.cleargate_engine_destroy(_handle);
            _handle = 0;
        }
    }

    private void ThrowIfDisposed()
    {
        if (_handle == 0)
            throw new ObjectDisposedException(nameof(Engine));
    }
}

/// <summary>
/// Handle to a running execution. Supports event polling and cancellation.
/// </summary>
public sealed class ExecutionHandle : IDisposable
{
    private ulong _handle;

    internal ExecutionHandle(ulong handle)
    {
        _handle = handle;
    }

    /// <summary>The unique run ID for this execution.</summary>
    public string? RunId
    {
        get
        {
            if (_handle == 0) return null;
            return EngineNative.MarshalAndFree(EngineNative.cleargate_execution_get_run_id(_handle));
        }
    }

    /// <summary>Poll for the next event. Returns null if the stream is closed.</summary>
    public string? NextEvent()
    {
        if (_handle == 0) return null;
        return EngineNative.MarshalAndFree(EngineNative.cleargate_execution_next_event(_handle));
    }

    /// <summary>Cancel the running execution.</summary>
    public void Cancel()
    {
        if (_handle != 0)
        {
            if (EngineNative.cleargate_execution_cancel(_handle) != 0)
                throw CleargateException.FromNative("Cancel failed");
        }
    }

    /// <summary>Block until the execution completes. Returns all events as a JSON array.</summary>
    public string? Wait()
    {
        if (_handle == 0) return null;
        return EngineNative.MarshalAndFree(EngineNative.cleargate_execution_wait(_handle));
    }

    /// <summary>Enumerate events as they arrive.</summary>
    public IEnumerable<string> Events()
    {
        while (true)
        {
            var evt = NextEvent();
            if (evt == null) yield break;
            yield return evt;
        }
    }

    public void Dispose()
    {
        if (_handle != 0)
        {
            EngineNative.cleargate_execution_destroy(_handle);
            _handle = 0;
        }
    }
}

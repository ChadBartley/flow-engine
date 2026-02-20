using System.Runtime.InteropServices;

namespace Cleargate;

/// <summary>
/// P/Invoke bindings to the cleargate-dotnet native library.
/// </summary>
internal static class Native
{
    private const string LibName = "cleargate_dotnet";

    // Error
    [DllImport(LibName, EntryPoint = "cleargate_last_error")]
    internal static extern IntPtr cleargate_last_error();

    // Observer
    [DllImport(LibName, EntryPoint = "cleargate_observer_start", CharSet = CharSet.Ansi)]
    internal static extern ulong cleargate_observer_start(
        [MarshalAs(UnmanagedType.LPUTF8Str)] string name,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? storePath);

    [DllImport(LibName, EntryPoint = "cleargate_observer_record_llm_call", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_observer_record_llm_call(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string nodeId,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string requestJson,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string responseJson);

    [DllImport(LibName, EntryPoint = "cleargate_observer_record_tool_call", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_observer_record_tool_call(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string toolName,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string inputsJson,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string outputsJson,
        ulong durationMs);

    [DllImport(LibName, EntryPoint = "cleargate_observer_record_step", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_observer_record_step(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string stepName,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string dataJson);

    [DllImport(LibName, EntryPoint = "cleargate_observer_finish", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_observer_finish(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string status);

    [DllImport(LibName, EntryPoint = "cleargate_observer_get_run_data")]
    internal static extern IntPtr cleargate_observer_get_run_data(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_observer_get_run_id")]
    internal static extern IntPtr cleargate_observer_get_run_id(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_observer_get_events")]
    internal static extern IntPtr cleargate_observer_get_events(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_observer_destroy")]
    internal static extern void cleargate_observer_destroy(ulong handle);

    // Adapter
    [DllImport(LibName, EntryPoint = "cleargate_adapter_start", CharSet = CharSet.Ansi)]
    internal static extern ulong cleargate_adapter_start(
        [MarshalAs(UnmanagedType.LPUTF8Str)] string framework,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? name,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? storePath);

    [DllImport(LibName, EntryPoint = "cleargate_adapter_on_event", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_adapter_on_event(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string eventJson);

    [DllImport(LibName, EntryPoint = "cleargate_adapter_finish", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_adapter_finish(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string status);

    [DllImport(LibName, EntryPoint = "cleargate_adapter_get_run_data")]
    internal static extern IntPtr cleargate_adapter_get_run_data(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_adapter_get_run_id")]
    internal static extern IntPtr cleargate_adapter_get_run_id(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_adapter_get_events")]
    internal static extern IntPtr cleargate_adapter_get_events(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_adapter_destroy")]
    internal static extern void cleargate_adapter_destroy(ulong handle);

    // Memory
    [DllImport(LibName, EntryPoint = "cleargate_free_string")]
    internal static extern void cleargate_free_string(IntPtr ptr);
}

/// <summary>
/// P/Invoke bindings for engine orchestration functions.
/// </summary>
internal static class EngineNative
{
    private const string LibName = "cleargate_dotnet";

    // EngineBuilder
    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_new")]
    internal static extern ulong cleargate_engine_builder_new();

    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_flow_store_path", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_engine_builder_flow_store_path(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string path);

    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_run_store_path", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_engine_builder_run_store_path(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string path);

    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_run_store_url", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_engine_builder_run_store_url(
        ulong handle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string url);

    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_max_traversals")]
    internal static extern int cleargate_engine_builder_max_traversals(ulong handle, uint value);

    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_crash_recovery")]
    internal static extern int cleargate_engine_builder_crash_recovery(
        ulong handle,
        [MarshalAs(UnmanagedType.U1)] bool enabled);

    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_build")]
    internal static extern ulong cleargate_engine_builder_build(ulong builderHandle);

    [DllImport(LibName, EntryPoint = "cleargate_engine_builder_destroy")]
    internal static extern void cleargate_engine_builder_destroy(ulong handle);

    // Engine
    [DllImport(LibName, EntryPoint = "cleargate_engine_execute", CharSet = CharSet.Ansi)]
    internal static extern ulong cleargate_engine_execute(
        ulong engineHandle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string flowId,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? inputsJson);

    [DllImport(LibName, EntryPoint = "cleargate_engine_execute_graph", CharSet = CharSet.Ansi)]
    internal static extern ulong cleargate_engine_execute_graph(
        ulong engineHandle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string graphJson,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? inputsJson);

    [DllImport(LibName, EntryPoint = "cleargate_engine_provide_input", CharSet = CharSet.Ansi)]
    internal static extern int cleargate_engine_provide_input(
        ulong engineHandle,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string runId,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string nodeId,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string inputJson);

    [DllImport(LibName, EntryPoint = "cleargate_engine_node_catalog")]
    internal static extern IntPtr cleargate_engine_node_catalog(ulong engineHandle);

    [DllImport(LibName, EntryPoint = "cleargate_engine_shutdown")]
    internal static extern void cleargate_engine_shutdown(ulong engineHandle);

    [DllImport(LibName, EntryPoint = "cleargate_engine_destroy")]
    internal static extern void cleargate_engine_destroy(ulong engineHandle);

    // Execution
    [DllImport(LibName, EntryPoint = "cleargate_execution_get_run_id")]
    internal static extern IntPtr cleargate_execution_get_run_id(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_execution_next_event")]
    internal static extern IntPtr cleargate_execution_next_event(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_execution_cancel")]
    internal static extern int cleargate_execution_cancel(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_execution_wait")]
    internal static extern IntPtr cleargate_execution_wait(ulong handle);

    [DllImport(LibName, EntryPoint = "cleargate_execution_destroy")]
    internal static extern void cleargate_execution_destroy(ulong handle);

    /// <summary>Marshal a native string pointer to a managed string and free the native memory.</summary>
    internal static string? MarshalAndFree(IntPtr ptr)
    {
        return Native.MarshalAndFree(ptr);
    }

    /// <summary>
    /// Marshal a native string pointer to a managed string and free the native memory.
    /// </summary>
    internal static string? MarshalAndFree(IntPtr ptr)
    {
        if (ptr == IntPtr.Zero) return null;
        var str = Marshal.PtrToStringUTF8(ptr);
        cleargate_free_string(ptr);
        return str;
    }

    /// <summary>
    /// Get the last error message from the native library, or null if none.
    /// </summary>
    internal static string? GetLastError()
    {
        return MarshalAndFree(cleargate_last_error());
    }
}

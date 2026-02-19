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
        [MarshalAs(UnmanagedType.LPUTF8Str)] string? name);

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

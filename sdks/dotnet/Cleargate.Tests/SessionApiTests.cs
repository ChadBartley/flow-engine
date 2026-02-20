using Microsoft.SemanticKernel;

namespace Cleargate.Tests;

/// <summary>
/// Smoke tests verifying the C# wrapper API surface compiles and has the
/// expected public methods. These tests cannot call into the native library
/// (the cdylib is not loaded in the test host), so they validate the managed
/// side only — constructor guards, ThrowIfFinished, Dispose patterns, etc.
/// </summary>
public class SessionApiTests
{
    [Fact]
    public void ObserverSession_HasExpectedPublicApi()
    {
        var type = typeof(ObserverSession);

        // Public methods that must exist
        Assert.NotNull(type.GetMethod("Start"));
        Assert.NotNull(type.GetMethod("RecordLlmCall"));
        Assert.NotNull(type.GetMethod("RecordToolCall"));
        Assert.NotNull(type.GetMethod("RecordStep"));
        Assert.NotNull(type.GetMethod("Finish"));
        Assert.NotNull(type.GetMethod("GetRunData"));
        Assert.NotNull(type.GetMethod("GetEvents"));
        Assert.NotNull(type.GetMethod("Dispose"));

        // Must have RunId property
        Assert.NotNull(type.GetProperty("RunId"));

        // Must be sealed and IDisposable
        Assert.True(type.IsSealed);
        Assert.True(typeof(IDisposable).IsAssignableFrom(type));
    }

    [Fact]
    public void AdapterSession_HasExpectedPublicApi()
    {
        var type = typeof(AdapterSession);

        Assert.NotNull(type.GetMethod("Start"));
        Assert.NotNull(type.GetMethod("OnEvent"));
        Assert.NotNull(type.GetMethod("Finish"));
        Assert.NotNull(type.GetMethod("GetRunData"));
        Assert.NotNull(type.GetMethod("GetEvents"));
        Assert.NotNull(type.GetMethod("Dispose"));

        Assert.NotNull(type.GetProperty("RunId"));
        Assert.True(type.IsSealed);
        Assert.True(typeof(IDisposable).IsAssignableFrom(type));
    }

    [Fact]
    public void SemanticKernelFilter_HasExpectedPublicApi()
    {
        var type = typeof(SemanticKernelFilter);

        // LLM call instrumentation — accepts optional model/provider overrides
        var instrument = type.GetMethod("Instrument");
        Assert.NotNull(instrument);
        var parameters = instrument!.GetParameters();
        Assert.Equal(3, parameters.Length);
        Assert.Equal("service", parameters[0].Name);
        Assert.Equal("model", parameters[1].Name);
        Assert.True(parameters[1].IsOptional);
        Assert.Equal("provider", parameters[2].Name);
        Assert.True(parameters[2].IsOptional);

        // Manual methods
        Assert.NotNull(type.GetMethod("OnFunctionInvoking"));
        Assert.NotNull(type.GetMethod("OnFunctionInvoked"));
        Assert.NotNull(type.GetMethod("OnPromptRendering"));
        Assert.NotNull(type.GetMethod("OnPromptRendered"));
        Assert.NotNull(type.GetMethod("Finish"));
        Assert.NotNull(type.GetMethod("GetRunData"));
        Assert.NotNull(type.GetMethod("GetEvents"));
        Assert.NotNull(type.GetMethod("Dispose"));

        Assert.NotNull(type.GetProperty("RunId"));
        Assert.True(type.IsSealed);
        Assert.True(typeof(IDisposable).IsAssignableFrom(type));

        // SK filter interfaces for auto-capture
        Assert.True(typeof(IFunctionInvocationFilter).IsAssignableFrom(type));
        Assert.True(typeof(IPromptRenderFilter).IsAssignableFrom(type));
    }

    [Fact]
    public void CleargateException_IsProperException()
    {
        var ex = new CleargateException("test error");
        Assert.Equal("test error", ex.Message);
        Assert.IsAssignableFrom<Exception>(ex);

        var inner = new InvalidOperationException("inner");
        var ex2 = new CleargateException("outer", inner);
        Assert.Equal("outer", ex2.Message);
        Assert.Same(inner, ex2.InnerException);
    }

    [Fact]
    public void NativeBindings_WereRemoved()
    {
        // Verify the dead duplicate NativeBindings class no longer exists
        var type = typeof(ObserverSession).Assembly.GetType("Cleargate.NativeBindings");
        Assert.Null(type);
    }
}

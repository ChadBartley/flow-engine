using System.Text.Json;
using Cleargate;

namespace Cleargate.Orchestration.Examples;

/// <summary>
/// Basic flow execution: load and execute an echo flow, print events.
/// </summary>
public static class BasicFlow
{
    public static void Run(string flowDir, string runDir)
    {
        Console.WriteLine("=== Basic Flow ===\n");

        using var builder = new EngineBuilder();
        builder.FlowStorePath(flowDir);
        builder.RunStorePath(runDir);
        builder.CrashRecovery(false);

        using var engine = builder.Build();

        Console.WriteLine("Engine built. Node catalog:");
        var catalog = engine.NodeCatalog();
        if (catalog != null)
        {
            using var doc = JsonDocument.Parse(catalog);
            foreach (var node in doc.RootElement.EnumerateArray())
            {
                var nodeType = node.GetProperty("node_type").GetString();
                var category = node.GetProperty("category").GetString();
                Console.WriteLine($"  - {nodeType} ({category})");
            }
        }

        Console.WriteLine("\nExecuting echo-flow...");
        using var handle = engine.Execute("echo-flow", new { message = "Hello from .NET!" });
        Console.WriteLine($"Run ID: {handle.RunId}");

        var eventCount = 0;
        foreach (var evt in handle.Events())
        {
            eventCount++;
            using var doc = JsonDocument.Parse(evt);
            var root = doc.RootElement;
            foreach (var prop in root.EnumerateObject())
            {
                Console.WriteLine($"  [{prop.Name}]");
                break;
            }
        }

        Console.WriteLine($"\nReceived {eventCount} events");
        engine.Shutdown();
    }
}

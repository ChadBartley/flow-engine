using System.Text.Json;
using Cleargate;

namespace Cleargate.Orchestration.Examples;

/// <summary>
/// Execute an ad-hoc graph definition without storing it in the flow store.
/// </summary>
public static class ExecuteGraph
{
    public static void Run(string flowDir, string runDir)
    {
        Console.WriteLine("=== Execute Graph ===\n");

        using var builder = new EngineBuilder();
        builder.FlowStorePath(flowDir);
        builder.RunStorePath(runDir);
        builder.CrashRecovery(false);

        using var engine = builder.Build();

        var graph = new
        {
            schema_version = 1,
            id = "adhoc-pipeline",
            name = "Ad-hoc Pipeline",
            version = "1.0",
            nodes = new[]
            {
                new
                {
                    instance_id = "step1",
                    node_type = "jq",
                    config = new { expression = "{greeting: (\"Hello, \" + (.name // \"World\") + \"!\")}" }
                },
                new
                {
                    instance_id = "step2",
                    node_type = "jq",
                    config = new { expression = "{result: .greeting, length: (.greeting | length)}" }
                }
            },
            edges = new[]
            {
                new
                {
                    id = "e1",
                    from_node = "step1",
                    from_port = "output",
                    to_node = "step2",
                    to_port = "input"
                }
            },
            tool_definitions = new { },
            metadata = new { }
        };

        Console.WriteLine("Executing ad-hoc graph...");
        using var handle = engine.ExecuteGraph(graph, new { name = "Cleargate" });
        Console.WriteLine($"Run ID: {handle.RunId}");

        foreach (var evt in handle.Events())
        {
            using var doc = JsonDocument.Parse(evt);
            var root = doc.RootElement;
            foreach (var prop in root.EnumerateObject())
            {
                Console.WriteLine($"  [{prop.Name}]");
                if (prop.Name == "NodeCompleted")
                {
                    var payload = prop.Value;
                    if (payload.TryGetProperty("outputs", out var outputs))
                    {
                        var nodeId = payload.GetProperty("node_instance_id").GetString();
                        Console.WriteLine($"    node={nodeId} outputs={outputs}");
                    }
                }
                break;
            }
        }

        engine.Shutdown();
        Console.WriteLine("\nDone!");
    }
}

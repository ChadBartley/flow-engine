using Cleargate.Orchestration.Examples;

// Set up temp directories with the echo flow
var tmpDir = Path.Combine(Path.GetTempPath(), $"cleargate-{Guid.NewGuid():N}");
var flowDir = Path.Combine(tmpDir, "flows");
var runDir = Path.Combine(tmpDir, "runs");

var flowDest = Path.Combine(flowDir, "echo-flow");
Directory.CreateDirectory(flowDest);
Directory.CreateDirectory(runDir);

// Copy the echo flow
var flowSrc = Path.Combine(
    AppContext.BaseDirectory, "..", "..", "..", "..", "flows", "echo-flow.json"
);
if (File.Exists(flowSrc))
{
    File.Copy(flowSrc, Path.Combine(flowDest, "flow.json"));
}
else
{
    Console.WriteLine($"Warning: Flow file not found at {flowSrc}");
    Console.WriteLine("Creating a minimal echo flow...");
    File.WriteAllText(
        Path.Combine(flowDest, "flow.json"),
        """
        {
          "schema_version": 1, "id": "echo-flow", "name": "Echo Flow", "version": "1.0",
          "nodes": [{"instance_id": "transform", "node_type": "jq",
                     "config": {"expression": "{message: (\"Echo: \" + (.message // \"hello\"))}"}}],
          "edges": [], "tool_definitions": {}, "metadata": {}
        }
        """
    );
}

try
{
    BasicFlow.Run(flowDir, runDir);
    Console.WriteLine();
    ExecuteGraph.Run(flowDir, runDir);
}
finally
{
    Directory.Delete(tmpDir, recursive: true);
}

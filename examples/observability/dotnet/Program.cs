// Entry point — run examples by name or default to basic.
// Usage: dotnet run [basic|semantic-kernel|tool-call]

var example = args.Length > 0 ? args[0] : "basic";

switch (example)
{
    case "basic":
        await BasicObserve.RunAsync();
        break;
    case "semantic-kernel":
        await SemanticKernelExample.RunAsync();
        break;
    case "tool-call":
        await ToolCallExample.RunAsync();
        break;
    default:
        Console.WriteLine($"Unknown example: {example}");
        Console.WriteLine("Available: basic, semantic-kernel, tool-call");
        break;
}

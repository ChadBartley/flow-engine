// Entry point — run examples by name or default to basic.
// Usage: dotnet run [basic|semantic-kernel]

var example = args.Length > 0 ? args[0] : "basic";

switch (example)
{
    case "basic":
        BasicObserve.Run();
        break;
    case "semantic-kernel":
        await SemanticKernelExample.RunAsync();
        break;
    default:
        Console.WriteLine($"Unknown example: {example}");
        Console.WriteLine("Available: basic, semantic-kernel");
        break;
}

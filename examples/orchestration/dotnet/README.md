# Cleargate Orchestration — .NET Examples

Run flow graphs through the Cleargate engine from C#/.NET.

## Prerequisites

1. Build the native library:
   ```bash
   cargo build --release -p cleargate-dotnet
   ```

2. Copy the native library to the output directory or set `LD_LIBRARY_PATH`/`DYLD_LIBRARY_PATH`.

## Examples

| Class | Description |
|-------|-------------|
| `BasicFlow` | Load and execute a simple echo flow |
| `ExecuteGraph` | Execute an ad-hoc graph definition |
| `StreamingEvents` | Stream events in real-time |

## Running

```bash
dotnet run
```

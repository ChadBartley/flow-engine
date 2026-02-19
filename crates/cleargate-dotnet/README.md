# cleargate-dotnet

.NET native bindings for ClearGate via C FFI. Produces a shared library consumed by the managed C# SDK through P/Invoke.

## Overview

This crate exposes a flat C ABI using handle-based session management. Each session is created, manipulated, and freed through exported C functions, with the managed C# SDK providing an idiomatic wrapper.

## Exported Functions

### Observer Session

- `cleargate_observer_session_new` -- Create a new observer session
- `cleargate_observer_session_record_llm_call` -- Record an LLM call event
- `cleargate_observer_session_record_tool_call` -- Record a tool call event
- `cleargate_observer_session_end` -- Finalize and persist the session
- `cleargate_observer_session_free` -- Release the session handle

### Adapter Session

- `cleargate_adapter_session_new` -- Create a framework-specific adapter session
- `cleargate_adapter_session_record_event` -- Record a framework event
- `cleargate_adapter_session_end` -- Finalize the adapter session
- `cleargate_adapter_session_free` -- Release the adapter handle

## Building

```bash
cargo build --release -p cleargate-dotnet
```

The output shared library (`libcleargate_dotnet.so` / `.dylib` / `.dll`) must be placed where the .NET runtime can locate it.

## License

Apache-2.0

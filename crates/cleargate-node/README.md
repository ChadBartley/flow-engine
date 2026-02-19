# cleargate-node

Node.js native bindings for ClearGate, built with NAPI-RS. Provides high-performance access to ClearGate's observability and adapter sessions from JavaScript and TypeScript.

## Overview

This crate compiles to a platform-specific native addon (`.node` file) that is loaded by the managed TypeScript SDK in `sdks/node/`.

## Exports

| Class | Description |
|-------|-------------|
| `ObserverSession` | Record LLM calls, tool calls, and steps for replay |
| `AdapterSession` | Framework-specific wrapper around ObserverSession |

## Supported Platforms

- Linux x64 (glibc and musl)
- macOS x64 and ARM64
- Windows x64

## Building

```bash
cd crates/cleargate-node
npm install
npm run build
```

The build produces a `.node` binary in the project root, which the TypeScript SDK references at runtime.

## Architecture

```
JavaScript/TypeScript
    |
    v
NAPI-RS bindings (this crate)
    |
    v
cleargate-adapters / cleargate-providers (Rust)
```

## License

Apache-2.0

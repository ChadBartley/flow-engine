# Contributing to ClearGate Flow Engine

Thank you for your interest in contributing!

## Development Setup

### Rust (required)

1. Install [Rust](https://rustup.rs/) (stable toolchain)
2. Clone the repository
3. Build and test:

```bash
cargo build --workspace
cargo test --workspace
```

### Python SDK

1. Install Python 3.9+ and [uv](https://docs.astral.sh/uv/)
2. Install [Maturin](https://www.maturin.rs/): `pip install maturin`
3. Build the native module:

```bash
cd crates/cleargate-python
maturin develop --release
```

### Node.js SDK

1. Install Node.js 18+
2. Build the native module:

```bash
cd crates/cleargate-node
npm run build
```

3. Install TypeScript SDK dependencies:

```bash
cd sdks/node
npm install
```

### .NET SDK

1. Install [.NET 8.0 SDK](https://dotnet.microsoft.com/download)
2. Build the native library:

```bash
cargo build --release -p cleargate-dotnet
```

3. Build the managed SDK:

```bash
cd sdks/dotnet/Cleargate
dotnet build
```

## Directory Structure

| Directory | What's There |
|-----------|-------------|
| `crates/` | Rust workspace crates (core engine, storage, adapters, providers, native bindings) |
| `sdks/dotnet/` | Managed C# SDK |
| `sdks/node/` | TypeScript SDK |
| `examples/flow-engine-agents/` | Flow configuration examples (JSON) |
| `examples/observability/` | Runnable SDK examples (Python, Node.js, .NET) |

## Quality Gates

All contributions must pass:

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo doc --workspace --no-deps
cargo fmt --all -- --check
```

Run `./fmt.sh` to auto-format before committing.

## Running Examples

See [`examples/`](examples/) for runnable examples. Each subdirectory has its own README with setup instructions. Examples use [Ollama](https://ollama.ai/) for local LLM inference.

## Pull Requests

- Use [Conventional Commits](https://www.conventionalcommits.org/) for commit messages
- Ensure all quality gates pass
- Add tests for new functionality
- Update documentation for public API changes

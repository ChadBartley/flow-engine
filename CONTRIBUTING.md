# Contributing to ClearGate Flow Engine

Thank you for your interest in contributing!

## Development Setup

1. Install Rust (stable toolchain)
2. Clone the repository
3. Run `cargo build --workspace`

## Quality Gates

All contributions must pass:

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo doc --workspace --no-deps
cargo fmt --all -- --check
```

Run `./fmt.sh` to auto-format before committing.

## Pull Requests

- Use [Conventional Commits](https://www.conventionalcommits.org/) for commit messages
- Ensure all quality gates pass
- Add tests for new functionality
- Update documentation for public API changes

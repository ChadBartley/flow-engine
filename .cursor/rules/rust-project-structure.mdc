---
alwaysApply: true
---

# Rust Project Structure & Hygiene

Conventions for keeping the codebase organized, clean, and consistent.

## File Size & Module Organization

- **500-line limit**: When a file exceeds ~500 lines, split it into a module directory (`foo.rs` → `foo/mod.rs` + submodules)
- **Module directory pattern**: Use `foo/mod.rs` as the coordinator — re-exports, shared helpers, tests. Move logical units into named submodules (e.g., `foo/builder.rs`, `foo/error.rs`)
- **Visibility in module dirs**: Use `pub(super)` for items shared between submodules within the same directory, `pub` only for items that escape the parent module
- **No glob re-exports in lib.rs**: Always use explicit `pub use module::{Type1, Type2}` — never `pub use module::*`. This makes the public API surface auditable and prevents accidental exposure

## Dead Code & Lint Suppression

- **No blanket `#[allow(dead_code)]`** on structs or enums — target individual fields with a justification comment: `#[allow(dead_code)] // Stored for future API layer access`
- **Prefer `_prefix`** for intentionally unused variables/fields over `#[allow]`
- **Remove stale allows**: When code evolves and the suppressed warning no longer applies, remove the `#[allow]`

## Test Utilities

- **Feature-gate test support**: Public test utilities (e.g., `TestNodeCtx`) must be behind `#[cfg(any(test, feature = "test-support"))]` so they compile for the crate's own tests AND for downstream crates that opt in
- **Never expose test helpers unconditionally**: Test builders, mock providers, and inspectors should not be in the default public API

## Quality Gates (all must pass before completing any task)

- `cargo clippy --workspace` — zero warnings
- `cargo doc --no-deps --workspace` — zero warnings
- `cargo test --workspace` — all tests pass, count unchanged unless intentionally added/removed
- Escape angle brackets in doc comments with backticks (e.g., `` `Arc<dyn Trait>` `` not `Arc<dyn Trait>`)

## TODO/FIXME Policy

- **Stub endpoints**: Return `501 Not Implemented` with a JSON error body — never leave empty handler bodies
- **Auth placeholders**: Document with a comment explaining intent (e.g., "Auth fields intentionally empty — populated once auth middleware is wired")
- **No orphan TODOs**: Every TODO must reference a tracking issue or be replaced with a working implementation

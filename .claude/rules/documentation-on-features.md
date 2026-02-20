# Documentation on Features

Feature work is not complete until documentation is added or updated.

## Requirements

- **Module-level docs**: Update module doc comments (`//!`) when a feature changes the module's responsibilities or capabilities
- **Public API docs**: All new public types, functions, and traits must have doc comments explaining purpose and usage
- **Roadmap/changelog**: Update relevant tracking documents with completion status and notes for future contributors

## Quality Standards

- **Add insight, not noise**: Document the "why", constraints, and non-obvious behavior — not what the code already says
- **Professional tone**: Concise, precise, no filler
- **Skip the obvious**: If reading the function signature and body takes 10 seconds to understand, a doc comment restating it adds no value
- **Include gotchas**: Feature gates, ordering dependencies, required configuration, error behavior — things a consumer would otherwise learn the hard way
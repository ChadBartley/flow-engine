---
alwaysApply: true
---

# Conventional Commits Specification

All commit messages must follow the [Conventional Commits](https://www.conventionalcommits.org/) specification.

## Commit Message Format

```
<type>[optional scope]: <description>

[optional body]

[optional footer(s)]
```

## Commit Types

Use one of the following types:

- **feat**: A new feature
- **fix**: A bug fix
- **docs**: Documentation only changes
- **style**: Changes that do not affect the meaning of the code (white-space, formatting, missing semi-colons, etc.)
- **refactor**: A code change that neither fixes a bug nor adds a feature
- **perf**: A code change that improves performance
- **test**: Adding missing tests or correcting existing tests
- **build**: Changes that affect the build system or external dependencies (example scopes: cargo, npm)
- **ci**: Changes to CI configuration files and scripts
- **chore**: Other changes that don't modify src or test files
- **revert**: Reverts a previous commit

## Scope (Optional)

The scope should be the name of the package, module, or area affected (as perceived by the person reading the changelog). Examples:
- `cleargate-core`
- `cleargate-storage`
- `cleargate-providers`
- `cleargate-node`
- `cleargate-dotnet`
- `config`
- `deps`

## Description

- Use the imperative, present tense: "change" not "changed" nor "changes"
- Don't capitalize the first letter
- No dot (.) at the end
- Keep it concise but descriptive

## Body (Optional)

- Use the imperative, present tense
- Include motivation for the change and contrasts with previous behavior
- Wrap at 72 characters

## Footer (Optional)

- **Breaking changes**: Start with `BREAKING CHANGE:` followed by a description
- **Issue references**: Use `Closes #123` or `Fixes #456`

## Examples

```
feat(cleargate-core): add support for provider health checks

Implement periodic health checks for configured providers with
configurable intervals and failure thresholds.

Closes #123
```

```
fix(cleargate-storage): resolve database connection pool exhaustion

The connection pool was not properly releasing connections after
query completion, leading to pool exhaustion under high load.

Fixes #456
```

```
docs: update API documentation for new routing rules

BREAKING CHANGE: The routing configuration format has changed.
See CONFIGURATION.md for migration guide.
```

```
refactor(cleargate-core): simplify flow engine state management

Extract state management logic into dedicated modules to improve
testability and reduce complexity.
```

## Requirements

- **All commits must use conventional commit format**
- **Type and description are required**
- **Scope is recommended for clarity when applicable**
- **Breaking changes must be documented in the footer**
- **Reference issues when applicable**

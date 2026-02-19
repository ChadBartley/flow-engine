---
alwaysApply: true
---

# Enterprise Coding Standards

This is an enterprise project. All code must adhere to the following standards:

## Code Consumability

- **Clear Naming**: Use descriptive, self-documenting names for functions, variables, and types
- **Logical Structure**: Organize code in a way that follows a clear, predictable flow
- **Minimal Cognitive Load**: Avoid unnecessary complexity; prefer simple, straightforward solutions
- **Consistent Patterns**: Follow established patterns within the codebase for similar functionality
- **Type Safety**: Leverage the type system to make code self-documenting and catch errors at compile time

## Documentation Standards

- **Document Public APIs**: All public functions, types, and modules must have clear documentation
- **Explain "Why", Not "What"**: Focus documentation on intent, rationale, and non-obvious behavior
- **Avoid Redundancy**: Don't document what the code already clearly expresses
- **Use Doc Comments**: Prefer doc comments (`///` in Rust, `/** */` in other languages) for public APIs
- **Include Examples**: Provide usage examples for complex or non-obvious APIs
- **Document Edge Cases**: Explicitly document error conditions, limitations, and important assumptions

## Code Segregation

- **Single Responsibility**: Each module, struct, and function should have one clear purpose
- **Separation of Concerns**: Keep business logic, data access, and presentation layers distinct
- **Modular Design**: Break down large modules into smaller, focused units
- **Dependency Management**: Minimize coupling between modules; prefer composition over tight coupling
- **Clear Boundaries**: Use clear interfaces and abstractions to define module boundaries
- **Feature Isolation**: Keep features and functionality isolated to enable independent development and testing

## Implementation Guidelines

- **Prefer Composition**: Use composition and dependency injection over global state
- **Error Handling**: Handle errors explicitly and provide meaningful error messages
- **Testability**: Structure code to be easily testable with clear dependencies
- **Maintainability**: Write code that future developers (including yourself) can easily understand and modify
- **Dependency Impact Review**: When making a change, assess all dependent code that must be updated alongside it, and apply those updates together
- **Datestamped Migrations**: All database migrations must be datestamped to ensure proper ordering and traceability (e.g., `YYYYMMDD_HHMMSS_description.sql` or similar format)
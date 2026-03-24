# Development Rules

This document defines the rules all agents must follow when working on this
project. Violations will be caught during review.

## File Ownership

| Role | Allowed Files | Forbidden Files |
|------|--------------|-----------------|
| Coder (cursor agent) | `src/<module>/mod.rs` | Any file in a `test/` directory |
| Tester (claude) | `src/<module>/test/mod.rs` | Any file NOT in a `test/` directory |
| Reviewer (codex) | Read-only access to all files | Should not write code directly |

## Test Placement Rules

1. **All tests MUST be placed in a `test/` subfolder** next to the business
   logic they test.
2. Business logic files (`mod.rs`) must declare the test module as:
   ```rust
   #[cfg(test)]
   mod test;
   ```
3. Test files (`test/mod.rs`) must NOT contain any business logic — only
   `#[test]` functions and test helpers.
4. **No inline `#[cfg(test)]` blocks** are allowed in business logic files
   beyond the single `mod test;` declaration.

## Code Style Rules

1. All functions must be `pub` so they are accessible from `main.rs` and tests.
2. Use `f64` for all numeric parameters and return types.
3. No external dependencies — only the Rust standard library.
4. No `unwrap()` or `expect()` in library code (these are pure math functions;
   there is nothing to unwrap).
5. Tests should use `assert!((result - expected).abs() < 1e-10)` for floating
   point comparisons.

## Test Requirements

Each module must have **at least 8 tests** covering:

1. Basic conversion with a known value (one per function = 4 tests)
2. Zero input (one per function = 4 tests)
3. Negative input (at least 2 tests)
4. Round-trip accuracy: `reverse(forward(x)) ≈ x` (at least 2 tests)
5. Edge cases specific to the domain (e.g., absolute zero, -40°F = -40°C)

## Communication

Agents communicate via the `.orchestrator/messages/` filesystem queues.
When you complete your task, send a message to the reviewer confirming what
you finished and the results of any local verification.

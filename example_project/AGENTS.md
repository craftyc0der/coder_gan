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

1. All public functions and methods must be `pub`.
2. Use standard library types only (`HashMap`, `HashSet`, `VecDeque`, `Vec`, `String`).
3. No external dependencies — only the Rust standard library.
4. No `unwrap()` or `expect()` in library code — return `Result` or `Option`.
5. Error types must implement `Display` for human-readable messages.

## Test Requirements

Each module must have **at least 10 tests** covering:

1. Normal/happy-path behavior
2. Error conditions (invalid input, missing nodes, etc.)
3. Edge cases (empty graph, self-loops, disconnected components)
4. Algorithmic correctness (valid topological orderings, complete transitive closures)

## Communication

Agents communicate via the `.orchestrator/messages/` filesystem queues.
When you complete your task, send a message to the reviewer confirming what
you finished and the results of any local verification.

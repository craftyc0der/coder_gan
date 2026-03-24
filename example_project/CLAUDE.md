# Claude Agent Rules (Tester)

You are a **tester** agent. You write tests only.

## Hard Constraints

1. **NEVER** create, read, or modify any file outside of a `tests/` directory.
2. You may only edit `src/<module>/tests/**` files.
3. Do not modify the non test rust code or `Cargo.toml`.
4. Use `super::super::*` to import the types and functions under test.
5. For resolver tests, also import `use crate::graph::Graph;` to construct test graphs.

## Your Task

Write comprehensive tests for the data structures and algorithms in your
assigned module. Refer to README.md for the expected behavior and edge cases.

Each module needs **at least 10 tests** covering:
- Normal/happy-path behavior
- Error conditions
- Edge cases (empty inputs, missing nodes)
- Algorithmic correctness

## When Done

Run `cargo test` and send a message to the reviewer with the results.

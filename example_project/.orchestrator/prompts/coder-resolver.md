# Coder Agent — Resolver Module

You are a **coder** agent responsible for implementing the dependency
resolution algorithms.

## Project Location

Project root: `{{project_root}}`

## Your Task

Implement the 4 resolver functions in `src/resolver/mod.rs`. The function
signatures and algorithms are documented in the TODO comments. You must
implement:

1. `pub fn topological_sort(graph: &Graph) -> Result<Vec<String>, ResolverError>`
   - Use Kahn's algorithm (BFS with in-degree tracking)
   - Return nodes so that dependencies come before dependents
   - If A depends on B (edge A→B), then B appears before A in the result
   - Return `CycleDetected` if not all nodes can be processed

2. `pub fn detect_cycle(graph: &Graph) -> Option<Vec<String>>`
   - Use DFS with three-color marking (white/gray/black)
   - Return `Some(path)` where path starts and ends with the same node
   - Return `None` for acyclic graphs

3. `pub fn transitive_deps(graph: &Graph, node: &str) -> Result<HashSet<String>, ResolverError>`
   - BFS/DFS from the given node, following all outgoing edges recursively
   - Return ALL reachable nodes (direct + indirect), excluding the start node
   - Return `NodeNotFound` if the node doesn't exist in the graph

4. `pub fn install_order(graph: &Graph) -> Result<Vec<String>, ResolverError>`
   - Reverse of topological sort: leaves (no dependencies) first, roots last
   - Return `CycleDetected` if the graph has a cycle

## Graph API Reference

The `Graph` type (from `crate::graph`) provides:
- `graph.nodes() -> Vec<String>` — all nodes, sorted
- `graph.neighbors(name) -> Result<Vec<String>, GraphError>` — outgoing edges
- `graph.has_node(name) -> bool`
- `graph.node_count() -> usize`

## Rules

- Keep the existing `#[cfg(test)] mod test;` declaration.
- Keep the existing `ResolverError` enum and `Display` impl.
- All functions must be `pub`.
- No external dependencies — only the Rust standard library.
- The `use` statements for `HashMap`, `HashSet`, `VecDeque`, and `Graph` are already present.
- Do NOT touch any `test/` directory or any file outside `src/resolver/mod.rs`.

## When Done

1. Run `cargo build` to verify it compiles.
2. Send a message to `tester-resolver` via `{{messages_dir}}` confirming completion.
3. Send a message to `reviewer` confirming completion.

Your agent ID: `{{agent_id}}`

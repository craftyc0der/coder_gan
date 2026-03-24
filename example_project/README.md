# Dependency Graph Resolver

A Rust library and CLI tool for building and resolving dependency graphs.
This project serves as the canonical demo for the **coder_gan** multi-agent
orchestration system.

## Problem Statement

Build a dependency graph library with **two independent modules**:

### Module 1: Graph (`src/graph/`)

Implement the `Graph` data structure in `src/graph/mod.rs`:

| Method | Description |
|---|---|
| `Graph::new() -> Graph` | Create an empty graph |
| `Graph::add_node(&mut self, name: &str)` | Add a node (idempotent) |
| `Graph::add_edge(&mut self, from: &str, to: &str) -> Result<(), GraphError>` | Add directed edge; both nodes must exist |
| `Graph::neighbors(&self, name: &str) -> Result<Vec<String>, GraphError>` | Direct dependencies (outgoing edges) |
| `Graph::has_node(&self, name: &str) -> bool` | Check if node exists |
| `Graph::has_edge(&self, from: &str, to: &str) -> bool` | Check if edge exists |
| `Graph::nodes(&self) -> Vec<String>` | All node names, sorted alphabetically |
| `Graph::node_count(&self) -> usize` | Number of nodes |
| `Graph::edge_count(&self) -> usize` | Total number of edges |

**Error conditions:**
- `add_edge` returns `NodeNotFound` if either node doesn't exist
- `add_edge` returns `DuplicateEdge` if the edge already exists
- `neighbors` returns `NodeNotFound` if the node doesn't exist

**Edge semantics:** An edge from A to B means "A depends on B".

### Module 2: Resolver (`src/resolver/`)

Implement dependency resolution algorithms in `src/resolver/mod.rs`:

| Function | Description | Algorithm |
|---|---|---|
| `topological_sort(graph) -> Result<Vec<String>, ResolverError>` | Dependency-first ordering | Kahn's algorithm (BFS + in-degree) |
| `detect_cycle(graph) -> Option<Vec<String>>` | Find a cycle if one exists | DFS with 3-color marking |
| `transitive_deps(graph, node) -> Result<HashSet<String>, ResolverError>` | All direct + indirect deps | BFS/DFS traversal |
| `install_order(graph) -> Result<Vec<String>, ResolverError>` | Leaves-first ordering | Reverse of topological sort |

**Topological sort example:**
```
Given: app→[api,cli], api→[core,utils], cli→[core,utils], core→[utils]
Valid output: [utils, core, api, cli, app]  (dependencies before dependents)
```

**Cycle detection example:**
```
Given: a→b, b→c, c→a
Returns: Some(["a", "b", "c", "a"])
```

**Transitive deps example:**
```
Given: app→api, api→core, core→utils
transitive_deps(app) = {api, core, utils}
```

### CLI (`src/main.rs`)

The `main.rs` demonstrates all functionality with a sample package dependency
graph. It is already provided — the agents just need to implement the module
functions so it compiles and runs.

## Architecture

```
src/
├── main.rs                  # CLI demo (provided, do not modify)
├── lib.rs                   # Module declarations (provided, do not modify)
├── graph/
│   ├── mod.rs               # ← Coder Team 1 implements this
│   └── test/
│       └── mod.rs           # ← Tester Team 1 implements this
└── resolver/
    ├── mod.rs               # ← Coder Team 2 implements this
    └── test/
        └── mod.rs           # ← Tester Team 2 implements this
```

## Development Rules

See [AGENTS.md](AGENTS.md) for the full development rules.

**Key rule:** All tests live in a `test/` subfolder next to the business logic,
never inline in the implementation file.

## Acceptance Criteria

The demo is **complete** when:

1. `cargo build` succeeds with no warnings
2. `cargo test` passes all tests (minimum 10 tests per module = 20 total)
3. `cargo run` prints correct graph operations output

## Reviewer TODO List

The reviewer agent (codex) should use this checklist to guide both teams:

### Phase 1 — Implementation (parallel)
- [ ] **Team Graph**: Implement all 9 methods on `Graph` in `src/graph/mod.rs`
- [ ] **Team Resolver**: Implement all 4 functions in `src/resolver/mod.rs`
- [ ] Verify: `cargo build` succeeds

### Phase 2 — Testing (parallel)
- [ ] **Team Graph**: Write tests in `src/graph/test/mod.rs` (>=10 tests)
- [ ] **Team Resolver**: Write tests in `src/resolver/test/mod.rs` (>=10 tests)
- [ ] Verify: `cargo test` passes all tests

### Phase 3 — Validation
- [ ] Verify: `cargo run` prints correct output
- [ ] Verify: No compiler warnings
- [ ] Verify: Cycle detection works correctly
- [ ] Verify: Topological sort produces valid orderings
- [ ] Signal completion to orchestrator

## Quick Verification

Run these commands to confirm the demo is complete:

```bash
cargo build 2>&1 | tail -1        # should show "Finished"
cargo test  2>&1 | tail -1        # should show "test result: ok"
cargo run                          # should print graph operations
```

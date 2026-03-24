// TODO: Implement tests for the resolver algorithms.
//
// Required tests (minimum 10):
//
// topological_sort:
//   - Linear chain: a → b → c produces [c, b, a]
//   - Diamond: a → [b,c], b → d, c → d produces valid ordering
//   - Single node (no edges) returns [node]
//   - Cycle returns CycleDetected error
//
// detect_cycle:
//   - Acyclic graph returns None
//   - Simple cycle (a → b → a) returns Some with cycle path
//   - Self-loop (a → a) returns Some
//   - Complex graph with one cycle embedded
//
// transitive_deps:
//   - Direct deps only (no transitive)
//   - Deep chain: a → b → c → d, transitive_deps(a) = {b, c, d}
//   - Diamond: no duplicates in result
//   - Leaf node returns empty set
//   - Non-existent node returns NodeNotFound
//
// install_order:
//   - Reverse of topological sort
//   - Leaves appear first
//
// Use: use crate::graph::Graph; and use super::super::*;

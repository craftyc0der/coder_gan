#[cfg(test)]
mod test;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fmt;

use crate::graph::Graph;

/// Error type for resolver operations.
#[derive(Debug, Clone, PartialEq)]
pub enum ResolverError {
    /// The graph contains a cycle (includes the cycle path).
    CycleDetected(Vec<String>),
    /// A referenced node does not exist.
    NodeNotFound(String),
}

impl fmt::Display for ResolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ResolverError::CycleDetected(path) => {
                write!(f, "cycle detected: {}", path.join(" → "))
            }
            ResolverError::NodeNotFound(n) => write!(f, "node not found: {}", n),
        }
    }
}

// TODO: Implement the following functions:
//
// pub fn topological_sort(graph: &Graph) -> Result<Vec<String>, ResolverError>
//   Return nodes in dependency order: if A depends on B, then B appears
//   before A. Use Kahn's algorithm (BFS with in-degree tracking).
//   Return CycleDetected if the graph has a cycle.
//
// pub fn detect_cycle(graph: &Graph) -> Option<Vec<String>>
//   Return Some(cycle_path) if the graph contains a cycle, None otherwise.
//   The cycle path should start and end with the same node.
//   Use DFS with coloring (white/gray/black).
//
// pub fn transitive_deps(graph: &Graph, node: &str) -> Result<HashSet<String>, ResolverError>
//   Return ALL dependencies of a node (direct and indirect), NOT including
//   the node itself. Use BFS or DFS from the node following edges.
//   Return NodeNotFound if the node doesn't exist.
//
// pub fn install_order(graph: &Graph) -> Result<Vec<String>, ResolverError>
//   Return nodes in install order: leaves (no dependencies) first,
//   root nodes last. This is the reverse of topological_sort.
//   Return CycleDetected if the graph has a cycle.
//
// See README.md for full specifications and examples.

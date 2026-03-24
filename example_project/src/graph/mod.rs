#[cfg(test)]
mod test;

use std::collections::HashMap;
use std::fmt;

/// Error type for graph operations.
#[derive(Debug, Clone, PartialEq)]
pub enum GraphError {
    /// A referenced node does not exist in the graph.
    NodeNotFound(String),
    /// The edge already exists.
    DuplicateEdge(String, String),
}

impl fmt::Display for GraphError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GraphError::NodeNotFound(n) => write!(f, "node not found: {}", n),
            GraphError::DuplicateEdge(a, b) => write!(f, "duplicate edge: {} → {}", a, b),
        }
    }
}

/// A directed graph represented as an adjacency list.
///
/// Nodes are identified by string names. Edges represent "depends on"
/// relationships: an edge from A to B means "A depends on B".
pub struct Graph {
    adjacency: HashMap<String, Vec<String>>,
}

// TODO: Implement the following methods on Graph:
//
// pub fn new() -> Graph
//   Create an empty graph.
//
// pub fn add_node(&mut self, name: &str)
//   Add a node. If it already exists, do nothing.
//
// pub fn add_edge(&mut self, from: &str, to: &str) -> Result<(), GraphError>
//   Add a directed edge from → to. Both nodes must exist (NodeNotFound).
//   If the edge already exists, return DuplicateEdge.
//
// pub fn neighbors(&self, name: &str) -> Result<Vec<String>, GraphError>
//   Return the direct dependencies of a node (outgoing edges).
//   Return NodeNotFound if the node doesn't exist.
//
// pub fn has_node(&self, name: &str) -> bool
//
// pub fn has_edge(&self, from: &str, to: &str) -> bool
//
// pub fn nodes(&self) -> Vec<String>
//   Return all node names (sorted alphabetically for determinism).
//
// pub fn node_count(&self) -> usize
//
// pub fn edge_count(&self) -> usize
//   Total number of edges in the graph.
//
// See README.md for full specifications.

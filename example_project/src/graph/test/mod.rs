// TODO: Implement tests for the Graph data structure.
//
// Required tests (minimum 10):
//
// Construction & nodes:
//   - new() creates an empty graph (node_count == 0)
//   - add_node adds a node, has_node returns true
//   - add_node is idempotent (adding twice doesn't duplicate)
//   - nodes() returns sorted list
//
// Edges:
//   - add_edge between existing nodes succeeds
//   - add_edge with missing source returns NodeNotFound
//   - add_edge with missing target returns NodeNotFound
//   - add_edge duplicate returns DuplicateEdge
//   - has_edge returns true for existing, false for non-existing
//   - edge_count tracks correctly
//
// Neighbors:
//   - neighbors returns direct dependencies only
//   - neighbors on node with no edges returns empty vec
//   - neighbors on non-existent node returns NodeNotFound
//
// Use: use super::super::*;

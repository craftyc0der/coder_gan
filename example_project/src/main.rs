use dep_graph::graph::Graph;
use dep_graph::resolver;

fn main() {
    println!("=== Dependency Graph Demo ===\n");

    // Build a package dependency graph:
    //
    //   app → [api, cli]
    //   api → [core, utils]
    //   cli → [core, utils]
    //   core → [utils]
    //   utils → []
    //
    let mut g = Graph::new();
    for name in &["app", "api", "cli", "core", "utils"] {
        g.add_node(name);
    }
    g.add_edge("app", "api").unwrap();
    g.add_edge("app", "cli").unwrap();
    g.add_edge("api", "core").unwrap();
    g.add_edge("api", "utils").unwrap();
    g.add_edge("cli", "core").unwrap();
    g.add_edge("cli", "utils").unwrap();
    g.add_edge("core", "utils").unwrap();

    println!("Graph: {} nodes, {} edges", g.node_count(), g.edge_count());
    println!();

    // Show direct dependencies
    println!("--- Direct Dependencies ---");
    for node in &["app", "api", "cli", "core", "utils"] {
        let deps = g.neighbors(node).unwrap();
        println!("  {:<6} → {:?}", node, deps);
    }
    println!();

    // Topological sort
    println!("--- Topological Sort ---");
    match resolver::topological_sort(&g) {
        Ok(order) => println!("  Build order: {:?}", order),
        Err(e) => println!("  Error: {}", e),
    }
    println!();

    // Install order (reverse topo — leaves first)
    println!("--- Install Order ---");
    match resolver::install_order(&g) {
        Ok(order) => println!("  Install order: {:?}", order),
        Err(e) => println!("  Error: {}", e),
    }
    println!();

    // Transitive dependencies
    println!("--- Transitive Dependencies ---");
    for node in &["app", "api", "cli"] {
        match resolver::transitive_deps(&g, node) {
            Ok(deps) => {
                let mut sorted: Vec<_> = deps.into_iter().collect();
                sorted.sort();
                println!("  {:<6} → {:?}", node, sorted);
            }
            Err(e) => println!("  {}: {}", node, e),
        }
    }
    println!();

    // Cycle detection on the acyclic graph
    println!("--- Cycle Detection ---");
    match resolver::detect_cycle(&g) {
        Some(cycle) => println!("  Cycle found: {:?}", cycle),
        None => println!("  No cycles detected (graph is a DAG)"),
    }

    // Now add a cycle and detect it
    println!();
    println!("--- Adding cycle: utils → app ---");
    g.add_edge("utils", "app").unwrap();
    match resolver::detect_cycle(&g) {
        Some(cycle) => println!("  Cycle found: {:?}", cycle),
        None => println!("  No cycles detected"),
    }

    println!("\nDone.");
}

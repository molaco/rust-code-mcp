//! Integration tests for hypergraph builder
//!
//! Run with: cargo test --test builder_integration -- --nocapture

use file_search_mcp::hypergraph::{HypergraphBuilder, HyperedgeType, NodeType};
use std::path::PathBuf;
use std::collections::HashMap;

#[test]
fn test_build_from_src_directory() {
    let src_dir = PathBuf::from("./src");

    if !src_dir.exists() {
        println!("Skipping test: ./src directory not found");
        return;
    }

    println!("\n========================================");
    println!("Building Hypergraph from ./src");
    println!("========================================\n");

    // Build hypergraph
    let builder = HypergraphBuilder::new().expect("Failed to create builder");
    let hg = builder.build_from_directory(&src_dir).expect("Failed to build hypergraph");

    // Get statistics
    let stats = hg.stats();
    println!("📊 Statistics:");
    println!("{}\n", stats);

    // Analyze nodes
    println!("========================================");
    println!("Node Analysis");
    println!("========================================\n");

    let mut file_nodes = 0;
    let mut symbol_nodes = 0;
    let mut sample_files = Vec::new();
    let mut sample_symbols = Vec::new();

    for node_id in 0..stats.node_count {
        if let Ok(node) = hg.get_node(file_search_mcp::hypergraph::NodeId(node_id)) {
            match &node.node_type {
                NodeType::File { .. } => {
                    file_nodes += 1;
                    if sample_files.len() < 5 {
                        sample_files.push(node.name.clone());
                    }
                }
                NodeType::Symbol { .. } => {
                    symbol_nodes += 1;
                    if sample_symbols.len() < 5 {
                        sample_symbols.push(format!("{} ({}:{})",
                            node.name,
                            node.file_path.display(),
                            node.line_start
                        ));
                    }
                }
            }
        }
    }

    println!("📁 File nodes: {}", file_nodes);
    println!("Sample files:");
    for (i, file) in sample_files.iter().enumerate() {
        println!("  {}. {}", i + 1, file);
    }

    println!("\n🔧 Symbol nodes: {}", symbol_nodes);
    println!("Sample symbols:");
    for (i, symbol) in sample_symbols.iter().enumerate() {
        println!("  {}. {}", i + 1, symbol);
    }

    // Analyze edges by type
    println!("\n========================================");
    println!("Edge Analysis");
    println!("========================================\n");

    let mut edge_type_counts: HashMap<String, usize> = HashMap::new();
    let mut sample_edges: HashMap<String, Vec<String>> = HashMap::new();

    for edge_id in 0..stats.edge_count {
        if let Ok(edge) = hg.get_hyperedge(file_search_mcp::hypergraph::HyperedgeId(edge_id)) {
            let type_name = match &edge.edge_type {
                HyperedgeType::ModuleContainment => "ModuleContainment",
                HyperedgeType::CallPattern => "CallPattern",
                HyperedgeType::ImportCluster => "ImportCluster",
                HyperedgeType::TraitImpl { .. } => "TraitImpl",
                HyperedgeType::TypeComposition { .. } => "TypeComposition",
            };

            *edge_type_counts.entry(type_name.to_string()).or_insert(0) += 1;

            // Collect sample edges (up to 3 per type)
            let samples = sample_edges.entry(type_name.to_string()).or_insert_with(Vec::new);
            if samples.len() < 3 {
                // Get source and target node names
                let source_names: Vec<String> = edge.sources.iter()
                    .filter_map(|&node_id| hg.get_node(node_id).ok())
                    .map(|n| n.name.clone())
                    .collect();

                let target_names: Vec<String> = edge.targets.iter()
                    .filter_map(|&node_id| hg.get_node(node_id).ok())
                    .map(|n| n.name.clone())
                    .collect();

                let edge_desc = format!(
                    "  {{{}}} → {{{}}}",
                    source_names.join(", "),
                    if target_names.len() > 5 {
                        format!("{} targets", target_names.len())
                    } else {
                        target_names.join(", ")
                    }
                );
                samples.push(edge_desc);
            }
        }
    }

    println!("📈 Edge type distribution:");
    for (edge_type, count) in &edge_type_counts {
        println!("  {}: {} edges", edge_type, count);
    }

    println!("\n📋 Sample edges by type:\n");
    for (edge_type, samples) in &sample_edges {
        if !samples.is_empty() {
            println!("{}:", edge_type);
            for sample in samples {
                println!("{}", sample);
            }
            println!();
        }
    }

    // Find interesting patterns
    println!("========================================");
    println!("Interesting Patterns");
    println!("========================================\n");

    // Find largest hyperedge
    let mut largest_edge_size = 0;
    let mut largest_edge_desc = String::new();

    for edge_id in 0..stats.edge_count {
        if let Ok(edge) = hg.get_hyperedge(file_search_mcp::hypergraph::HyperedgeId(edge_id)) {
            let size = edge.order();
            if size > largest_edge_size {
                largest_edge_size = size;

                let source_names: Vec<String> = edge.sources.iter()
                    .filter_map(|&node_id| hg.get_node(node_id).ok())
                    .map(|n| n.name.clone())
                    .collect();

                largest_edge_desc = format!(
                    "{}: {} → {} targets (order: {})",
                    format!("{}", edge.edge_type),
                    source_names.join(", "),
                    edge.targets.len(),
                    size
                );
            }
        }
    }

    println!("🏆 Largest hyperedge:");
    println!("  {}\n", largest_edge_desc);

    // Assertions
    assert!(file_nodes > 0, "Should have file nodes");
    assert!(symbol_nodes > 0, "Should have symbol nodes");
    assert!(stats.edge_count > 0, "Should have edges");
    assert!(stats.node_count == file_nodes + symbol_nodes, "Node count should match");

    println!("========================================");
    println!("✅ All assertions passed!");
    println!("========================================\n");
}

#[test]
fn test_hypergraph_structure() {
    let src_dir = PathBuf::from("./src");

    if !src_dir.exists() {
        println!("Skipping test: ./src directory not found");
        return;
    }

    let builder = HypergraphBuilder::new().expect("Failed to create builder");
    let hg = builder.build_from_directory(&src_dir).expect("Failed to build hypergraph");
    let stats = hg.stats();

    // Verify basic structure
    assert!(stats.node_count > 100, "Should have substantial number of nodes");
    assert!(stats.edge_count > 50, "Should have substantial number of edges");
    assert!(stats.avg_order >= 2.0, "Average edge order should be at least 2");
    assert!(stats.max_order >= 2, "Max edge order should be at least 2");
}

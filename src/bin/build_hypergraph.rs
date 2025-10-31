//! CLI tool to build hypergraph from a directory

use file_search_mcp::hypergraph::HypergraphBuilder;
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <directory>", args[0]);
        eprintln!("\nExample:");
        eprintln!("  {} ./src", args[0]);
        std::process::exit(1);
    }

    let dir = PathBuf::from(&args[1]);

    if !dir.exists() {
        eprintln!("Error: Directory does not exist: {:?}", dir);
        std::process::exit(1);
    }

    println!("Building hypergraph from: {:?}\n", dir);

    // Build hypergraph
    let builder = HypergraphBuilder::new()?;
    let hypergraph = builder.build_from_directory(&dir)?;

    // Print statistics
    let stats = hypergraph.stats();
    println!("\n✓ Hypergraph built successfully!");
    println!("{}", stats);

    Ok(())
}

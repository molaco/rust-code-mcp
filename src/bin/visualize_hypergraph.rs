//! CLI tool to visualize hypergraph in 3D

use file_search_mcp::hypergraph::{HypergraphBuilder, visualize};
use std::path::PathBuf;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: {} <directory>", args[0]);
        eprintln!("\nBuilds a hypergraph from Rust source files and visualizes it in 3D");
        eprintln!("\nExample:");
        eprintln!("  {} ./src", args[0]);
        eprintln!("\nControls:");
        eprintln!("  Left mouse drag:  Rotate camera");
        eprintln!("  Right mouse drag: Pan camera");
        eprintln!("  Scroll wheel:     Zoom");
        eprintln!("  ESC:              Exit");
        std::process::exit(1);
    }

    let dir = PathBuf::from(&args[1]);

    if !dir.exists() {
        eprintln!("Error: Directory does not exist: {:?}", dir);
        std::process::exit(1);
    }

    println!("Building hypergraph from: {:?}\n", dir);

    // Build hypergraph (Phase 2)
    let builder = HypergraphBuilder::new()?;
    let hypergraph = builder.build_from_directory(&dir)?;

    // Print statistics
    let stats = hypergraph.stats();
    println!("\n✓ Hypergraph built successfully!");
    println!("{}", stats);

    println!("\nLaunching 3D visualization...");
    println!("(This may take a moment on first run)\n");

    // Launch visualization (Phase 3)
    visualize(hypergraph);

    Ok(())
}

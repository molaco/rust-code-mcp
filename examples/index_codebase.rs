//! Index rust-code-mcp codebase with GPU

use file_search_mcp::indexing::IncrementalIndexer;
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("\n{}", "=".repeat(60));
    println!("INDEXING rust-code-mcp CODEBASE");
    println!("{}\n", "=".repeat(60));

    let codebase = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let cache = PathBuf::from(".cache_bench");
    let tantivy = PathBuf::from(".tantivy_bench");

    println!("Codebase: {}", codebase.display());
    println!("Initializing indexer...\n");

    let mut indexer = IncrementalIndexer::new(
        &cache,
        &tantivy,
        "http://localhost:6333",
        &format!("rust_code_bench_{}", uuid::Uuid::new_v4()),
        384,
        None,
    )
    .await
    .expect("Failed to create indexer");

    println!("🚀 Starting full indexing with GPU acceleration...\n");
    let start = Instant::now();

    let stats = indexer
        .indexer_mut()
        .index_directory_parallel(&codebase)
        .await
        .expect("Indexing failed");

    let duration = start.elapsed();

    println!("\n{}", "=".repeat(60));
    println!("INDEXING COMPLETE");
    println!("{}", "=".repeat(60));
    println!("Files indexed:       {}", stats.indexed_files);
    println!("Chunks generated:    {}", stats.total_chunks);
    println!("Total time:          {:.2}s", duration.as_secs_f64());
    println!("Throughput:          {:.1} files/sec", stats.indexed_files as f64 / duration.as_secs_f64());
    println!("                     {:.1} chunks/sec", stats.total_chunks as f64 / duration.as_secs_f64());
    println!("{}\n", "=".repeat(60));
}

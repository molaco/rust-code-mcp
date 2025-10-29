//! Quick GPU benchmark - run with: cargo run --release --bin quick_bench

use file_search_mcp::indexing::IncrementalIndexer;
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    println!("\n{}", "=".repeat(60));
    println!("GPU PERFORMANCE BENCHMARK");
    println!("{}\n", "=".repeat(60));

    let codebase_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let temp_dir = tempfile::TempDir::new()?;

    let cache_path = temp_dir.path().join("cache");
    let tantivy_path = temp_dir.path().join("tantivy");
    let collection_name = format!("quick_bench_{}", uuid::Uuid::new_v4());

    println!("📁 Codebase: {}", codebase_path.display());
    println!("🔧 Initializing indexer with GPU acceleration...\n");

    let mut indexer = IncrementalIndexer::new(
        &cache_path,
        &tantivy_path,
        "http://localhost:6333",
        &collection_name,
        384,
        None,
    )
    .await?;

    println!("🔥 Warming up GPU (loading model into VRAM)...");
    let warmup_file = codebase_path.join("src/embeddings/mod.rs");
    if warmup_file.exists() {
        let _ = indexer.indexer_mut().index_file(&warmup_file).await;
    }
    println!("   Warmup complete\n");

    indexer.clear_all_data().await?;

    println!("🚀 Starting full codebase indexing with GPU...\n");
    let start = Instant::now();
    let stats = indexer.indexer_mut().index_directory_parallel(&codebase_path).await?;
    let duration = start.elapsed();

    println!("\n{}", "=".repeat(60));
    println!("RESULTS");
    println!("{}", "=".repeat(60));
    println!("Files indexed:       {}", stats.indexed_files);
    println!("Chunks generated:    {}", stats.total_chunks);
    println!("Total time:          {:.2}s", duration.as_secs_f64());
    println!("Throughput:          {:.1} files/sec", stats.indexed_files as f64 / duration.as_secs_f64());
    println!("                     {:.1} chunks/sec", stats.total_chunks as f64 / duration.as_secs_f64());

    let chunks_per_sec = stats.total_chunks as f64 / duration.as_secs_f64();

    println!("\n{}", "=".repeat(60));
    if chunks_per_sec > 150.0 {
        println!("✅ GPU ACCELERATION ACTIVE");
        println!("   Estimated speedup: {:.1}x vs CPU", chunks_per_sec / 50.0);
    } else {
        println!("⚠️  GPU may not be active ({:.0} chunks/sec)", chunks_per_sec);
    }

    if duration.as_secs_f64() < 10.0 {
        println!("🎉 Excellent performance!");
    }

    println!("{}\n", "=".repeat(60));

    Ok(())
}

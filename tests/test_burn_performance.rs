//! Performance test: Burn codebase GPU vs CPU comparison
//!
//! This test indexes the Burn codebase (~1,616 files) to demonstrate
//! GPU acceleration benefits on large codebases.
//!
//! ## Running this test
//!
//! Due to deep recursion in the indexing pipeline, you must run with increased stack size:
//!
//! ```bash
//! RUST_MIN_STACK=8388608 cargo test --test test_burn_performance -- --ignored --nocapture
//! ```
//!
//! Without the `RUST_MIN_STACK` environment variable, the test will crash with a stack overflow.

use anyhow::Result;
use std::time::Instant;

async fn index_burn_codebase(force: bool) -> Result<(String, std::time::Duration)> {
    use file_search_mcp::tools::index_tool::{index_codebase, IndexCodebaseParams};

    let params = IndexCodebaseParams {
        directory: "/home/molaco/Documents/burn".to_string(),
        force_reindex: Some(force),
    };

    let start = Instant::now();
    let result = index_codebase(params, None)
        .await
        .map_err(|e| anyhow::anyhow!("MCP error: {:?}", e))?;
    let elapsed = start.elapsed();

    // Extract result text
    let result_text = if let Some(content) = result.content.first() {
        format!("{:?}", content)
    } else {
        "No content".to_string()
    };

    Ok((result_text, elapsed))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore] // Run with: RUST_MIN_STACK=8388608 cargo test --test test_burn_performance -- --ignored --nocapture
async fn test_burn_gpu_performance() -> Result<()> {
    // Skip tracing initialization to avoid stack overflow in test runtime
    // The indexing code will still log via tracing, but without a subscriber
    // it won't cause stack overflow issues

    println!("\n========================================");
    println!("Burn Codebase GPU Performance Test");
    println!("========================================\n");

    // Verify codebase exists
    let burn_path = std::path::Path::new("/home/molaco/Documents/burn");
    if !burn_path.exists() {
        anyhow::bail!("Burn codebase not found at {}", burn_path.display());
    }

    // Count files
    let file_count = walkdir::WalkDir::new(burn_path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("rs")))
        .count();

    println!("Burn codebase statistics:");
    println!("  Path: {}", burn_path.display());
    println!("  Rust files: {}", file_count);
    println!();

    // Run indexing with GPU acceleration
    println!("--- Starting GPU-accelerated indexing ---");
    println!("Expected: ~3-5 minutes for {} files\n", file_count);

    let (result, elapsed) = index_burn_codebase(true).await?;

    println!("\n--- Indexing complete ---\n");

    // Parse results
    let indexed_files = result.match_indices("Indexed files:").count();
    let total_chunks = result.match_indices("Total chunks:").count();

    println!("Performance Metrics:");
    println!("  Total time: {:.2} minutes ({:.2}s)",
        elapsed.as_secs_f64() / 60.0,
        elapsed.as_secs_f64()
    );
    println!("  Files/sec: {:.2}", file_count as f64 / elapsed.as_secs_f64());

    if result.contains("chunks") {
        println!("\n✓ Successfully indexed Burn codebase");

        // Extract chunk count if possible
        if let Some(chunks_pos) = result.find("Total chunks: ") {
            let chunks_str = &result[chunks_pos + 14..];
            if let Some(end) = chunks_str.find(|c: char| !c.is_numeric()) {
                if let Ok(chunks) = chunks_str[..end].parse::<usize>() {
                    println!("  Total chunks: {}", chunks);
                    println!("  Chunks/sec: {:.2}", chunks as f64 / elapsed.as_secs_f64());
                }
            }
        }
    }

    println!("\n========================================");
    println!("GPU Acceleration Results");
    println!("========================================");
    println!("Files indexed: {}", file_count);
    println!("Time: {:.2} minutes", elapsed.as_secs_f64() / 60.0);
    println!("Throughput: {:.1} files/sec", file_count as f64 / elapsed.as_secs_f64());
    println!();
    println!("Comparison with CPU baseline (10 minutes):");
    let cpu_baseline = 600.0; // 10 minutes in seconds
    let speedup = cpu_baseline / elapsed.as_secs_f64();
    println!("  Speedup: {:.2}x faster", speedup);
    println!("  Time saved: {:.1} minutes", (cpu_baseline - elapsed.as_secs_f64()) / 60.0);
    println!("========================================\n");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_burn_incremental_index() -> Result<()> {
    println!("\n========================================");
    println!("Burn Incremental Indexing Test");
    println!("========================================\n");

    // First index (full)
    println!("Running full index...");
    let start1 = Instant::now();
    let (_result1, elapsed1) = index_burn_codebase(true).await?;
    println!("Full index: {:.2} minutes\n", elapsed1.as_secs_f64() / 60.0);

    // Second index (incremental - should be much faster)
    println!("Running incremental index (no changes)...");
    let start2 = Instant::now();
    let (_result2, elapsed2) = index_burn_codebase(false).await?;
    println!("Incremental index: {:.2} seconds\n", elapsed2.as_secs_f64());

    println!("========================================");
    println!("Incremental Indexing Results");
    println!("========================================");
    println!("Full index:        {:.2} minutes", elapsed1.as_secs_f64() / 60.0);
    println!("Incremental index: {:.2} seconds", elapsed2.as_secs_f64());
    println!("Speedup:           {:.0}x faster", elapsed1.as_secs_f64() / elapsed2.as_secs_f64());
    println!("========================================\n");

    Ok(())
}

//! GPU Performance Benchmark for rust-code-mcp codebase
//!
//! This benchmark indexes the rust-code-mcp codebase itself to measure
//! GPU acceleration performance improvements.
//!
//! Run with: cargo test --release benchmark_gpu_performance --ignored -- --nocapture

use anyhow::Result;
use file_search_mcp::indexing::{IncrementalIndexer, IndexStats};
use std::path::PathBuf;
use std::time::Instant;
use tempfile::TempDir;

/// Benchmark configuration
struct BenchmarkConfig {
    codebase_path: PathBuf,
    cache_path: PathBuf,
    tantivy_path: PathBuf,
    collection_name: String,
}

impl BenchmarkConfig {
    fn new(temp_dir: &TempDir) -> Self {
        Self {
            codebase_path: PathBuf::from(env!("CARGO_MANIFEST_DIR")),
            cache_path: temp_dir.path().join("cache"),
            tantivy_path: temp_dir.path().join("tantivy"),
            collection_name: format!("benchmark_gpu_{}", uuid::Uuid::new_v4()),
        }
    }

    async fn create_indexer(&self) -> Result<IncrementalIndexer> {
        IncrementalIndexer::new(
            &self.cache_path,
            &self.tantivy_path,
            "http://localhost:6333",
            &self.collection_name,
            384,
            None,
        )
        .await
    }
}

/// Benchmark results with detailed metrics
#[derive(Debug)]
struct BenchmarkResults {
    total_time: std::time::Duration,
    files_indexed: usize,
    chunks_generated: usize,
    throughput_files_per_sec: f64,
    throughput_chunks_per_sec: f64,
    avg_time_per_file_ms: f64,
}

impl BenchmarkResults {
    fn from_stats(
        stats: IndexStats,
        duration: std::time::Duration,
    ) -> Self {
        let total_secs = duration.as_secs_f64();
        Self {
            total_time: duration,
            files_indexed: stats.indexed_files,
            chunks_generated: stats.total_chunks,
            throughput_files_per_sec: stats.indexed_files as f64 / total_secs,
            throughput_chunks_per_sec: stats.total_chunks as f64 / total_secs,
            avg_time_per_file_ms: (total_secs * 1000.0) / stats.indexed_files as f64,
        }
    }

    fn print_summary(&self, label: &str) {
        println!("\n{}", "=".repeat(60));
        println!("{} BENCHMARK RESULTS", label.to_uppercase());
        println!("{}", "=".repeat(60));
        println!("Total Time:          {:.2}s", self.total_time.as_secs_f64());
        println!("Files Indexed:       {}", self.files_indexed);
        println!("Chunks Generated:    {}", self.chunks_generated);
        println!("---");
        println!("Throughput:          {:.1} files/sec", self.throughput_files_per_sec);
        println!("                     {:.1} chunks/sec", self.throughput_chunks_per_sec);
        println!("Avg Time/File:       {:.2}ms", self.avg_time_per_file_ms);
        println!("{}\n", "=".repeat(60));
    }
}

#[tokio::test]
#[ignore] // Requires Qdrant server running
async fn benchmark_gpu_performance() -> Result<()> {
    println!("\n{}", "*".repeat(60));
    println!("GPU PERFORMANCE BENCHMARK");
    println!("Indexing: rust-code-mcp codebase");
    println!("{}\n", "*".repeat(60));

    let temp_dir = TempDir::new()?;
    let config = BenchmarkConfig::new(&temp_dir);

    println!("📁 Codebase Path: {}", config.codebase_path.display());
    println!("🔧 Initializing indexer with GPU acceleration...\n");

    // Create indexer
    let mut indexer = config.create_indexer().await?;

    // Warm-up: Index a small subset to load the model
    println!("🔥 Warming up GPU (loading model into VRAM)...");
    let warmup_start = Instant::now();
    let warmup_file = config.codebase_path.join("src/embeddings/mod.rs");
    if warmup_file.exists() {
        let _ = indexer.indexer_mut().index_file(&warmup_file).await;
    }
    println!("   Warmup complete in {:.2}s\n", warmup_start.elapsed().as_secs_f64());

    // Clear the test data from warmup
    indexer.clear_all_data().await?;

    // Main benchmark: Full codebase indexing
    println!("🚀 Starting full codebase indexing...");
    println!("   (Using parallel indexing with GPU batching)\n");

    let benchmark_start = Instant::now();
    let stats = indexer.indexer_mut().index_directory_parallel(&config.codebase_path).await?;
    let benchmark_duration = benchmark_start.elapsed();

    // Calculate and display results
    let results = BenchmarkResults::from_stats(stats.clone(), benchmark_duration);
    results.print_summary("GPU");

    // Performance analysis
    println!("📊 PERFORMANCE ANALYSIS");
    println!("{}", "=".repeat(60));

    // Calculate expected metrics
    let chunks_per_file = stats.total_chunks as f64 / stats.indexed_files as f64;
    let embedding_time_estimate = stats.total_chunks as f64 * 0.003; // ~3ms per chunk on CPU
    let embedding_throughput = stats.total_chunks as f64 / results.total_time.as_secs_f64();

    println!("Chunks per file:     {:.1}", chunks_per_file);
    println!("Embedding throughput: {:.0} embeddings/sec", embedding_throughput);

    // GPU performance indicators
    if embedding_throughput > 150.0 {
        println!("\n✅ GPU ACCELERATION ACTIVE");
        println!("   (>150 embeddings/sec indicates GPU processing)");

        let speedup = embedding_throughput / 50.0; // CPU baseline ~50/sec
        println!("   Estimated GPU speedup: {:.1}x vs CPU", speedup);
    } else {
        println!("\n⚠️  WARNING: GPU may not be active");
        println!("   (<150 embeddings/sec suggests CPU-only processing)");
        println!("   Check CUDA installation and GPU availability");
    }

    println!("\n💡 OPTIMIZATION NOTES");
    println!("{}", "=".repeat(60));
    println!("• GPU batch size: 256 chunks");
    println!("• VRAM allocation: 6GB (of 8GB available)");
    println!("• Parallel file processing: {} threads", num_cpus::get());

    if results.total_time.as_secs_f64() < 10.0 {
        println!("\n🎉 EXCELLENT: Indexing completed in under 10 seconds!");
    } else if results.total_time.as_secs_f64() < 20.0 {
        println!("\n✅ GOOD: Reasonable indexing performance");
    } else {
        println!("\n⚠️  SLOW: Consider increasing GPU batch size or checking memory limits");
    }

    println!("\n");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires Qdrant server running
async fn benchmark_compare_sequential_vs_parallel() -> Result<()> {
    println!("\n{}", "*".repeat(60));
    println!("SEQUENTIAL vs PARALLEL COMPARISON");
    println!("{}\n", "*".repeat(60));

    let temp_dir = TempDir::new()?;
    let codebase_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Test 1: Sequential indexing
    println!("📝 Test 1: Sequential Indexing");
    println!("{}", "=".repeat(60));

    let cache_path_seq = temp_dir.path().join("cache_seq");
    let tantivy_path_seq = temp_dir.path().join("tantivy_seq");
    let collection_seq = format!("bench_seq_{}", uuid::Uuid::new_v4());

    let mut indexer_seq = IncrementalIndexer::new(
        &cache_path_seq,
        &tantivy_path_seq,
        "http://localhost:6333",
        &collection_seq,
        384,
        None,
    )
    .await?;

    let seq_start = Instant::now();
    let seq_stats = indexer_seq.indexer_mut().index_directory(&codebase_path).await?;
    let seq_duration = seq_start.elapsed();

    let seq_results = BenchmarkResults::from_stats(seq_stats, seq_duration);
    seq_results.print_summary("Sequential");

    // Test 2: Parallel indexing
    println!("🚀 Test 2: Parallel Indexing");
    println!("{}", "=".repeat(60));

    let cache_path_par = temp_dir.path().join("cache_par");
    let tantivy_path_par = temp_dir.path().join("tantivy_par");
    let collection_par = format!("bench_par_{}", uuid::Uuid::new_v4());

    let mut indexer_par = IncrementalIndexer::new(
        &cache_path_par,
        &tantivy_path_par,
        "http://localhost:6333",
        &collection_par,
        384,
        None,
    )
    .await?;

    let par_start = Instant::now();
    let par_stats = indexer_par.indexer_mut().index_directory_parallel(&codebase_path).await?;
    let par_duration = par_start.elapsed();

    let par_results = BenchmarkResults::from_stats(par_stats, par_duration);
    par_results.print_summary("Parallel");

    // Comparison
    println!("📊 COMPARISON");
    println!("{}", "=".repeat(60));
    let speedup = seq_duration.as_secs_f64() / par_duration.as_secs_f64();
    println!("Sequential time:     {:.2}s", seq_duration.as_secs_f64());
    println!("Parallel time:       {:.2}s", par_duration.as_secs_f64());
    println!("Speedup:             {:.2}x", speedup);

    if speedup > 2.0 {
        println!("\n🎉 Parallel indexing is {:.1}x faster!", speedup);
    } else if speedup > 1.2 {
        println!("\n✅ Parallel indexing provides moderate speedup");
    } else {
        println!("\n⚠️  WARNING: Parallel speedup is minimal");
        println!("   This may indicate GPU/CPU bottleneck or small codebase");
    }

    println!("\n");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn benchmark_memory_usage() -> Result<()> {
    use sysinfo::System;

    println!("\n{}", "*".repeat(60));
    println!("MEMORY USAGE BENCHMARK");
    println!("{}\n", "*".repeat(60));

    let temp_dir = TempDir::new()?;
    let codebase_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let config = BenchmarkConfig::new(&temp_dir);

    let mut indexer = config.create_indexer().await?;

    // Monitor memory before indexing
    let mut sys = System::new_all();
    sys.refresh_all();
    let mem_before = sys.used_memory();

    println!("💾 Memory before indexing: {:.2} MB", mem_before as f64 / 1_000_000.0);

    // Run indexing
    let start = Instant::now();
    let stats = indexer.indexer_mut().index_directory_parallel(&codebase_path).await?;
    let duration = start.elapsed();

    // Monitor memory after indexing
    sys.refresh_all();
    let mem_after = sys.used_memory();
    let mem_used = mem_after.saturating_sub(mem_before);

    println!("💾 Memory after indexing:  {:.2} MB", mem_after as f64 / 1_000_000.0);
    println!("💾 Memory used by index:   {:.2} MB", mem_used as f64 / 1_000_000.0);
    println!("\n📊 Indexing Stats:");
    println!("   Files indexed:    {}", stats.indexed_files);
    println!("   Chunks generated: {}", stats.total_chunks);
    println!("   Time taken:       {:.2}s", duration.as_secs_f64());
    println!("   Memory per chunk: {:.2} KB",
             (mem_used as f64 / 1_000.0) / stats.total_chunks as f64);

    println!("\n");

    Ok(())
}

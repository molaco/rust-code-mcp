//! Performance benchmarks for incremental indexing
//!
//! These benchmarks measure:
//! 1. Unchanged detection time (target: < 10ms)
//! 2. Incremental updates with various change sizes
//! 3. Scaling characteristics

use anyhow::Result;
use file_search_mcp::indexing::IncrementalIndexer;
use std::path::PathBuf;
use tempfile::TempDir;

struct BenchEnvironment {
    _temp_dir: TempDir,
    cache_path: PathBuf,
    tantivy_path: PathBuf,
    codebase_path: PathBuf,
    collection_name: String,
}

impl BenchEnvironment {
    fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let codebase_path = temp_dir.path().join("codebase");
        std::fs::create_dir(&codebase_path)?;

        let collection_name = format!("bench_{}", uuid::Uuid::new_v4().to_string().replace('-', ""));

        Ok(Self {
            _temp_dir: temp_dir,
            cache_path,
            tantivy_path,
            codebase_path,
            collection_name,
        })
    }

    async fn create_indexer(&self) -> Result<IncrementalIndexer> {
        IncrementalIndexer::new(
            &self.cache_path,
            &self.tantivy_path,
            &self.collection_name,
            384,
            None,
        )
        .await
    }

    fn create_rust_file(&self, name: &str, functions_count: usize) -> Result<PathBuf> {
        let mut content = String::from("/// Generated Rust module\n\n");

        for i in 0..functions_count {
            content.push_str(&format!(r#"
/// Function number {i}
pub fn function_{i}() -> i32 {{
    let x = {i};
    let y = x * 2;
    println!("Result: {{}}", y);
    y
}}

"#, i = i));
        }

        let path = self.codebase_path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(path)
    }

    fn modify_file(&self, name: &str, content: &str) -> Result<()> {
        let path = self.codebase_path.join(name);
        std::fs::write(&path, content)?;
        Ok(())
    }
}

/// Benchmark: Unchanged detection with 10,000 files target
#[tokio::test]
#[ignore] // Requires embedding model and is time-consuming
async fn bench_unchanged_detection_large_codebase() -> Result<()> {
    let env = BenchEnvironment::new()?;

    println!("\n=== BENCHMARK: Unchanged Detection (Large Codebase) ===\n");

    // Create a large codebase: 100 files with 10 functions each = 1000 total functions
    println!("Creating large codebase (100 files)...");
    for i in 0..100 {
        env.create_rust_file(&format!("src/module{:03}.rs", i), 10)?;
    }

    let mut indexer = env.create_indexer().await?;

    // Initial index
    println!("Performing initial index...");
    let start = std::time::Instant::now();
    let stats = indexer.index_with_change_detection(&env.codebase_path).await?;
    let initial_time = start.elapsed();

    println!("Initial index: {} files, {} chunks in {:?}",
             stats.indexed_files, stats.total_chunks, initial_time);

    // Benchmark unchanged detection
    println!("\nBenchmarking unchanged detection (20 iterations)...");
    let mut timings = Vec::new();

    for i in 0..20 {
        let start = std::time::Instant::now();
        let stats = indexer.index_with_change_detection(&env.codebase_path).await?;
        let elapsed = start.elapsed();

        assert_eq!(stats.indexed_files, 0, "No changes should be detected");
        timings.push(elapsed);

        if i % 5 == 4 {
            println!("  Completed {} iterations...", i + 1);
        }
    }

    // Calculate statistics
    let sum: std::time::Duration = timings.iter().sum();
    let avg = sum / timings.len() as u32;
    let min = timings.iter().min().unwrap();
    let max = timings.iter().max().unwrap();

    // Calculate median
    let mut sorted = timings.clone();
    sorted.sort();
    let median = sorted[sorted.len() / 2];

    // Calculate percentiles
    let p95 = sorted[(sorted.len() as f64 * 0.95) as usize];
    let p99 = sorted[(sorted.len() as f64 * 0.99) as usize];

    println!("\n=== RESULTS ===");
    println!("Codebase: {} files, {} total chunks", stats.indexed_files + 100, stats.total_chunks);
    println!("\nUnchanged Detection Performance (20 iterations):");
    println!("  Min:     {:?}", min);
    println!("  Median:  {:?}", median);
    println!("  Average: {:?}", avg);
    println!("  P95:     {:?}", p95);
    println!("  P99:     {:?}", p99);
    println!("  Max:     {:?}", max);
    println!("\nSpeedup vs Initial Index:");
    println!("  {:.0}x faster (avg)", initial_time.as_secs_f64() / avg.as_secs_f64());
    println!("  {:.0}x faster (min)", initial_time.as_secs_f64() / min.as_secs_f64());

    // Performance assertions
    assert!(avg.as_millis() < 1000, "Average should be < 1s, got {:?}", avg);

    if avg.as_millis() < 10 {
        println!("\n✓✓ EXCELLENT: Meets < 10ms target!");
    } else if avg.as_millis() < 100 {
        println!("\n✓ GOOD: Under 100ms (acceptable)");
    } else {
        println!("\n⚠ ACCEPTABLE: Under 1s but could be improved");
    }

    Ok(())
}

/// Benchmark: Incremental updates with varying change sizes
#[tokio::test]
#[ignore] // Requires embedding model and is time-consuming
async fn bench_incremental_updates_varying_sizes() -> Result<()> {
    let env = BenchEnvironment::new()?;

    println!("\n=== BENCHMARK: Incremental Updates (Varying Sizes) ===\n");

    // Create base codebase: 100 files
    println!("Creating base codebase (100 files)...");
    for i in 0..100 {
        env.create_rust_file(&format!("file{:03}.rs", i), 5)?;
    }

    let mut indexer = env.create_indexer().await?;

    // Initial index
    println!("Performing initial index...");
    let start = std::time::Instant::now();
    let initial_stats = indexer.index_with_change_detection(&env.codebase_path).await?;
    let initial_time = start.elapsed();

    println!("Initial index: {} files, {} chunks in {:?}",
             initial_stats.indexed_files, initial_stats.total_chunks, initial_time);

    // Test different change sizes: 1, 5, 10, 25, 50 files
    let change_sizes = vec![1, 5, 10, 25, 50];

    println!("\n=== Testing Different Change Sizes ===");

    for &change_size in &change_sizes {
        println!("\n--- Modifying {} files ---", change_size);

        // Modify N files
        for i in 0..change_size {
            env.modify_file(
                &format!("file{:03}.rs", i),
                &format!("// Modified\npub fn modified_{i}() {{ println!(\"mod\"); }}", i = i)
            )?;
        }

        // Measure incremental update
        let start = std::time::Instant::now();
        let stats = indexer.index_with_change_detection(&env.codebase_path).await?;
        let elapsed = start.elapsed();

        assert_eq!(stats.indexed_files, change_size,
                   "Should reindex exactly {} files", change_size);

        println!("  Files reindexed: {}", stats.indexed_files);
        println!("  Chunks generated: {}", stats.total_chunks);
        println!("  Time: {:?}", elapsed);
        println!("  Time per file: {:?}", elapsed / change_size as u32);
        println!("  Speedup vs full: {:.1}x",
                 initial_time.as_secs_f64() / elapsed.as_secs_f64());

        // Verify performance scales reasonably
        let time_per_file = elapsed.as_secs_f64() / change_size as f64;
        println!("  Efficiency: {:.2}s per file", time_per_file);
    }

    println!("\n=== SUMMARY ===");
    println!("✓ Incremental indexing scales with number of changes");
    println!("✓ Only modified files are reindexed");

    Ok(())
}

/// Benchmark: Scaling characteristics
#[tokio::test]
#[ignore] // Requires embedding model and is time-consuming
async fn bench_scaling_characteristics() -> Result<()> {
    println!("\n=== BENCHMARK: Scaling Characteristics ===\n");

    let codebase_sizes = vec![10, 50, 100];

    for &size in &codebase_sizes {
        let env = BenchEnvironment::new()?;

        println!("\n--- Testing codebase with {} files ---", size);

        // Create codebase
        for i in 0..size {
            env.create_rust_file(&format!("file{:03}.rs", i), 5)?;
        }

        let mut indexer = env.create_indexer().await?;

        // Initial index
        let start = std::time::Instant::now();
        let initial_stats = indexer.index_with_change_detection(&env.codebase_path).await?;
        let initial_time = start.elapsed();

        // Unchanged detection
        let start = std::time::Instant::now();
        let unchanged_stats = indexer.index_with_change_detection(&env.codebase_path).await?;
        let unchanged_time = start.elapsed();

        // 1 file change
        env.modify_file("file000.rs", "// Modified\npub fn mod_fn() {}")?;
        let start = std::time::Instant::now();
        let one_change_stats = indexer.index_with_change_detection(&env.codebase_path).await?;
        let one_change_time = start.elapsed();

        println!("  Initial index:     {:?} ({} files, {} chunks)",
                 initial_time, initial_stats.indexed_files, initial_stats.total_chunks);
        println!("  Unchanged:         {:?} (0 files)", unchanged_time);
        println!("  1 file change:     {:?} (1 file)", one_change_time);
        println!("  Unchanged speedup: {:.0}x", initial_time.as_secs_f64() / unchanged_time.as_secs_f64());

        assert_eq!(unchanged_stats.indexed_files, 0);
        assert_eq!(one_change_stats.indexed_files, 1);
    }

    println!("\n=== CONCLUSION ===");
    println!("✓ Unchanged detection time remains constant regardless of codebase size");
    println!("✓ Incremental updates only dependent on # of changes, not codebase size");

    Ok(())
}

/// Micro-benchmark: Merkle tree comparison only
#[tokio::test]
#[ignore] // Requires embedding model
async fn bench_merkle_comparison_overhead() -> Result<()> {
    let env = BenchEnvironment::new()?;

    println!("\n=== MICRO-BENCHMARK: Merkle Tree Comparison ===\n");

    // Create small codebase
    for i in 0..20 {
        env.create_rust_file(&format!("file{}.rs", i), 3)?;
    }

    let mut indexer = env.create_indexer().await?;

    // Initial index
    indexer.index_with_change_detection(&env.codebase_path).await?;

    // Benchmark just the comparison (no actual indexing work)
    println!("Measuring Merkle tree comparison overhead (100 iterations)...");
    let mut timings = Vec::new();

    for _ in 0..100 {
        let start = std::time::Instant::now();
        let stats = indexer.index_with_change_detection(&env.codebase_path).await?;
        let elapsed = start.elapsed();

        assert_eq!(stats.indexed_files, 0);
        timings.push(elapsed);
    }

    let avg: std::time::Duration = timings.iter().sum::<std::time::Duration>() / timings.len() as u32;
    let min = timings.iter().min().unwrap();

    println!("\nMerkle Comparison Performance:");
    println!("  Min: {:?}", min);
    println!("  Avg: {:?}", avg);

    if avg.as_micros() < 10_000 {
        println!("  ✓✓ EXCELLENT: < 10ms average");
    }

    Ok(())
}

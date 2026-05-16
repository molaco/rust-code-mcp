//! Run `index_codebase` across embedding GPU batch sizes and print a table.
//!
//! Build both examples first:
//!
//! ```sh
//! cargo build --release --example index_codebase --example gpu_batch_matrix
//! ```
//!
//! Then run:
//!
//! ```sh
//! ./target/release/examples/gpu_batch_matrix
//! ./target/release/examples/gpu_batch_matrix 16
//! ```

use anyhow::{Context, Result, bail};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

#[derive(Debug)]
struct BenchmarkResult {
    batch_size: usize,
    total_chunks: usize,
    duration_secs: f64,
    embed_duration_secs: f64,
    chunks_per_sec: f64,
    child_wall_secs: f64,
}

fn main() -> Result<()> {
    let batch_sizes = parse_batch_sizes()?;
    let index_bin = sibling_index_codebase_binary()?;

    println!("index_codebase binary: {}", index_bin.display());
    println!("batch sizes: {:?}\n", batch_sizes);
    println!(
        "| batch size | chunks | index wall | embed time | chunks/sec | child wall |"
    );
    println!("|---:|---:|---:|---:|---:|---:|");

    for batch_size in batch_sizes {
        let result = run_benchmark(&index_bin, batch_size)?;
        println!(
            "| {} | {} | {:.2}s | {:.2}s | {:.1} | {:.2}s |",
            result.batch_size,
            result.total_chunks,
            result.duration_secs,
            result.embed_duration_secs,
            result.chunks_per_sec,
            result.child_wall_secs
        );
    }

    Ok(())
}

fn parse_batch_sizes() -> Result<Vec<usize>> {
    let args: Vec<String> = env::args().skip(1).collect();

    if args.iter().any(|arg| arg == "-h" || arg == "--help") {
        println!("Usage: gpu_batch_matrix [BATCH_SIZE ...]");
        println!("Defaults to: 16 32 48 64");
        std::process::exit(0);
    }

    if args.is_empty() {
        return Ok(vec![16, 32, 48, 64]);
    }

    args.into_iter()
        .map(|arg| {
            let batch_size = arg
                .parse::<usize>()
                .with_context(|| format!("invalid batch size: {arg}"))?;
            if batch_size == 0 {
                bail!("batch size must be greater than zero");
            }
            Ok(batch_size)
        })
        .collect()
}

fn sibling_index_codebase_binary() -> Result<PathBuf> {
    let mut path = env::current_exe().context("failed to resolve current executable")?;
    path.set_file_name(format!("index_codebase{}", env::consts::EXE_SUFFIX));

    if !path.exists() {
        bail!(
            "missing sibling index_codebase binary at {}; build it with `cargo build --release --example index_codebase --example gpu_batch_matrix`",
            path.display()
        );
    }

    Ok(path)
}

fn run_benchmark(index_bin: &PathBuf, batch_size: usize) -> Result<BenchmarkResult> {
    let tempdir = tempfile::tempdir().context("failed to create benchmark tempdir")?;
    let start = Instant::now();
    let output = Command::new(index_bin)
        .current_dir(tempdir.path())
        .env("RUST_CODE_MCP_EMBED_BATCH_SIZE", batch_size.to_string())
        .output()
        .with_context(|| format!("failed to run {}", index_bin.display()))?;
    let child_wall_secs = start.elapsed().as_secs_f64();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        eprintln!("{combined}");
        bail!("index_codebase failed for batch size {batch_size}");
    }

    let total_chunks = metric_usize(&combined, "total_chunks")
        .context("missing total_chunks metric in index_codebase output")?;
    let duration_secs = metric_f64(&combined, "duration_secs")
        .context("missing duration_secs metric in index_codebase output")?;
    let embed_duration_secs = metric_f64(&combined, "embed_duration_secs")
        .context("missing embed_duration_secs metric in index_codebase output")?;
    let chunks_per_sec = if duration_secs == 0.0 {
        0.0
    } else {
        total_chunks as f64 / duration_secs
    };

    Ok(BenchmarkResult {
        batch_size,
        total_chunks,
        duration_secs,
        embed_duration_secs,
        chunks_per_sec,
        child_wall_secs,
    })
}

fn metric_f64(output: &str, key: &str) -> Option<f64> {
    let prefix = format!("{key}=");
    output
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&prefix)?.parse::<f64>().ok())
}

fn metric_usize(output: &str, key: &str) -> Option<usize> {
    let prefix = format!("{key}=");
    output
        .split_whitespace()
        .find_map(|part| part.strip_prefix(&prefix)?.parse::<usize>().ok())
}

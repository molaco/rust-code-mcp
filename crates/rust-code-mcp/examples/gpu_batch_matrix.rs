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
//! ./target/release/examples/gpu_batch_matrix --profile local-gpu-small 16
//! ```

use anyhow::{Context, Result, bail};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

#[derive(Debug)]
struct BenchmarkResult {
    profile: String,
    batch_size: usize,
    vector_dim: usize,
    total_chunks: usize,
    duration_secs: f64,
    embed_duration_secs: f64,
    chunks_per_sec: f64,
    padded_tokens_total: Option<usize>,
    padded_tokens_per_sec: Option<f64>,
    child_wall_secs: f64,
}

fn main() -> Result<()> {
    let args = MatrixArgs::parse()?;
    let index_bin = sibling_index_codebase_binary()?;

    println!("index_codebase binary: {}", index_bin.display());
    println!("profile: {}", args.profile);
    println!("batch sizes: {:?}\n", args.batch_sizes);
    println!(
        "| profile | batch size | dim | chunks | index wall | embed time | chunks/sec | padded tokens | padded tokens/sec | child wall |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|");

    for batch_size in args.batch_sizes {
        let result = run_benchmark(&index_bin, &args.profile, batch_size)?;
        println!(
            "| {} | {} | {} | {} | {:.2}s | {:.2}s | {:.1} | {} | {} | {:.2}s |",
            result.profile,
            result.batch_size,
            result.vector_dim,
            result.total_chunks,
            result.duration_secs,
            result.embed_duration_secs,
            result.chunks_per_sec,
            result
                .padded_tokens_total
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string()),
            result
                .padded_tokens_per_sec
                .map(|value| format!("{value:.1}"))
                .unwrap_or_else(|| "n/a".to_string()),
            result.child_wall_secs
        );
    }

    Ok(())
}

#[derive(Debug)]
struct MatrixArgs {
    profile: String,
    batch_sizes: Vec<usize>,
}

impl MatrixArgs {
    fn parse() -> Result<Self> {
        let mut profile = "local-gpu-small".to_string();
        let mut batch_sizes = Vec::new();
        let mut args = env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                "--profile" => {
                    profile = args.next().context("--profile requires a profile name")?;
                }
                raw_batch_size => {
                    let batch_size = parse_batch_size(raw_batch_size)?;
                    batch_sizes.push(batch_size);
                }
            }
        }

        if batch_sizes.is_empty() {
            batch_sizes = vec![16, 32, 48, 64];
        }

        Ok(Self {
            profile,
            batch_sizes,
        })
    }
}

fn print_usage() {
    println!("Usage: gpu_batch_matrix [--profile PROFILE] [BATCH_SIZE ...]");
    println!("Defaults to: --profile local-gpu-small 16 32 48 64");
}

fn parse_batch_size(arg: &str) -> Result<usize> {
    let batch_size = arg
        .parse::<usize>()
        .with_context(|| format!("invalid batch size: {arg}"))?;
    if batch_size == 0 {
        bail!("batch size must be greater than zero");
    }
    Ok(batch_size)
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

fn run_benchmark(index_bin: &PathBuf, profile: &str, batch_size: usize) -> Result<BenchmarkResult> {
    let tempdir = tempfile::tempdir().context("failed to create benchmark tempdir")?;
    let start = Instant::now();
    let output = Command::new(index_bin)
        .current_dir(tempdir.path())
        .args(["--profile", profile])
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

    let profile = metric_string(&combined, "embedding_profile")
        .unwrap_or_else(|| profile.to_string());
    let vector_dim = metric_usize(&combined, "vector_dim")
        .context("missing vector_dim metric in index_codebase output")?;
    let total_chunks = metric_usize(&combined, "total_chunks")
        .context("missing total_chunks metric in index_codebase output")?;
    let duration_secs = metric_f64(&combined, "duration_secs")
        .context("missing duration_secs metric in index_codebase output")?;
    let embed_duration_secs = metric_f64(&combined, "embed_duration_secs")
        .context("missing embed_duration_secs metric in index_codebase output")?;
    let padded_tokens_total = metric_usize(&combined, "padded_tokens_total");
    let padded_tokens_per_sec = metric_f64(&combined, "padded_tokens_per_sec");
    let chunks_per_sec = if duration_secs == 0.0 {
        0.0
    } else {
        total_chunks as f64 / duration_secs
    };

    Ok(BenchmarkResult {
        profile,
        batch_size,
        vector_dim,
        total_chunks,
        duration_secs,
        embed_duration_secs,
        chunks_per_sec,
        padded_tokens_total,
        padded_tokens_per_sec,
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

fn metric_string(output: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}=");
    output
        .split_whitespace()
        .find_map(|part| Some(part.strip_prefix(&prefix)?.to_string()))
}

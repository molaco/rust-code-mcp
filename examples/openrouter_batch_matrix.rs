//! Run `index_codebase` across OpenRouter embedding batch settings.
//!
//! Build both examples first:
//!
//! ```sh
//! cargo build --release --example index_codebase --example openrouter_batch_matrix
//! ```
//!
//! Then run:
//!
//! ```sh
//! ./target/release/examples/openrouter_batch_matrix
//! ./target/release/examples/openrouter_batch_matrix --inputs 64,128 --tokens 65536,131072 --concurrency 2,4
//! ```

use anyhow::{Context, Result, bail};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

const PROFILE: &str = "openrouter-qwen3-8b";

#[derive(Debug)]
struct BenchmarkResult {
    max_batch_inputs: usize,
    max_batch_tokens: usize,
    concurrency: usize,
    total_chunks: usize,
    duration_secs: f64,
    embed_duration_secs: f64,
    request_count: usize,
    retry_count: usize,
    split_count: usize,
    failed_request_count: usize,
    provider_preferences: bool,
    avg_request_latency_secs: f64,
    estimated_tokens: usize,
    padded_tokens_per_sec: f64,
    child_wall_secs: f64,
}

fn main() -> Result<()> {
    let args = MatrixArgs::parse()?;

    if !has_openrouter_key() {
        println!("openrouter_benchmark=skipped_missing_api_key");
        return Ok(());
    }

    let index_bin = sibling_index_codebase_binary()?;

    println!("index_codebase binary: {}", index_bin.display());
    println!("profile: {PROFILE}");
    println!("max batch inputs: {:?}", args.max_batch_inputs);
    println!("max batch tokens: {:?}", args.max_batch_tokens);
    println!("concurrency: {:?}\n", args.concurrency);
    println!(
        "| inputs | tokens | concurrency | provider | chunks | index wall | embed time | requests | retries | splits | failed | avg req latency | estimated tokens | padded tokens/sec | child wall |"
    );
    println!("|---:|---:|---:|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|");

    for max_batch_inputs in &args.max_batch_inputs {
        for max_batch_tokens in &args.max_batch_tokens {
            for concurrency in &args.concurrency {
                let result = run_benchmark(
                    &index_bin,
                    *max_batch_inputs,
                    *max_batch_tokens,
                    *concurrency,
                )?;
                println!(
                    "| {} | {} | {} | {} | {} | {:.2}s | {:.2}s | {} | {} | {} | {} | {:.3}s | {} | {:.1} | {:.2}s |",
                    result.max_batch_inputs,
                    result.max_batch_tokens,
                    result.concurrency,
                    result.provider_preferences,
                    result.total_chunks,
                    result.duration_secs,
                    result.embed_duration_secs,
                    result.request_count,
                    result.retry_count,
                    result.split_count,
                    result.failed_request_count,
                    result.avg_request_latency_secs,
                    result.estimated_tokens,
                    result.padded_tokens_per_sec,
                    result.child_wall_secs
                );
            }
        }
    }

    Ok(())
}

#[derive(Debug)]
struct MatrixArgs {
    max_batch_inputs: Vec<usize>,
    max_batch_tokens: Vec<usize>,
    concurrency: Vec<usize>,
}

impl MatrixArgs {
    fn parse() -> Result<Self> {
        let mut max_batch_inputs = vec![32, 64, 128];
        let mut max_batch_tokens = vec![32_768, 65_536, 131_072];
        let mut concurrency = vec![1, 2, 4, 8];
        let mut args = env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                "--inputs" => {
                    let value = args.next().context("--inputs requires a comma list")?;
                    max_batch_inputs = parse_usize_list(&value, "--inputs")?;
                }
                "--tokens" => {
                    let value = args.next().context("--tokens requires a comma list")?;
                    max_batch_tokens = parse_usize_list(&value, "--tokens")?;
                }
                "--concurrency" => {
                    let value = args.next().context("--concurrency requires a comma list")?;
                    concurrency = parse_usize_list(&value, "--concurrency")?;
                }
                other => bail!("unknown argument `{other}`"),
            }
        }

        Ok(Self {
            max_batch_inputs,
            max_batch_tokens,
            concurrency,
        })
    }
}

fn print_usage() {
    println!(
        "Usage: openrouter_batch_matrix [--inputs LIST] [--tokens LIST] [--concurrency LIST]"
    );
    println!("Defaults to: --inputs 32,64,128 --tokens 32768,65536,131072 --concurrency 1,2,4,8");
}

fn parse_usize_list(raw: &str, label: &str) -> Result<Vec<usize>> {
    let mut values = Vec::new();
    for part in raw.split(',') {
        let value = part
            .trim()
            .parse::<usize>()
            .with_context(|| format!("invalid {label} value: {part}"))?;
        if value == 0 {
            bail!("{label} values must be greater than zero");
        }
        values.push(value);
    }
    if values.is_empty() {
        bail!("{label} requires at least one value");
    }
    Ok(values)
}

fn has_openrouter_key() -> bool {
    env::var("RUST_CODE_MCP_OPENROUTER_API_KEY")
        .or_else(|_| env::var("OPENROUTER_API_KEY"))
        .ok()
        .is_some_and(|value| !value.trim().is_empty())
}

fn sibling_index_codebase_binary() -> Result<PathBuf> {
    let mut path = env::current_exe().context("failed to resolve current executable")?;
    path.set_file_name(format!("index_codebase{}", env::consts::EXE_SUFFIX));

    if !path.exists() {
        bail!(
            "missing sibling index_codebase binary at {}; build it with `cargo build --release --example index_codebase --example openrouter_batch_matrix`",
            path.display()
        );
    }

    Ok(path)
}

fn run_benchmark(
    index_bin: &PathBuf,
    max_batch_inputs: usize,
    max_batch_tokens: usize,
    concurrency: usize,
) -> Result<BenchmarkResult> {
    let tempdir = tempfile::tempdir().context("failed to create benchmark tempdir")?;
    let start = Instant::now();
    let output = Command::new(index_bin)
        .current_dir(tempdir.path())
        .args(["--profile", PROFILE])
        .env(
            "RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS",
            max_batch_inputs.to_string(),
        )
        .env(
            "RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS",
            max_batch_tokens.to_string(),
        )
        .env(
            "RUST_CODE_MCP_OPENROUTER_CONCURRENCY",
            concurrency.to_string(),
        )
        .env("RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT", "float")
        .output()
        .with_context(|| format!("failed to run {}", index_bin.display()))?;
    let child_wall_secs = start.elapsed().as_secs_f64();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let combined = format!("{stdout}\n{stderr}");

    if !output.status.success() {
        eprintln!("{combined}");
        bail!(
            "index_codebase failed for inputs={max_batch_inputs} tokens={max_batch_tokens} concurrency={concurrency}"
        );
    }

    let total_chunks = metric_usize(&combined, "total_chunks")
        .context("missing total_chunks metric in index_codebase output")?;
    let duration_secs = metric_f64(&combined, "duration_secs")
        .context("missing duration_secs metric in index_codebase output")?;
    let embed_duration_secs = metric_f64(&combined, "embed_duration_secs")
        .context("missing embed_duration_secs metric in index_codebase output")?;
    let request_count = metric_sum_usize(&combined, "openrouter_request_count");
    let retry_count = metric_sum_usize(&combined, "openrouter_retry_count");
    let split_count = metric_sum_usize(&combined, "openrouter_split_count");
    let failed_request_count =
        metric_sum_usize(&combined, "openrouter_failed_request_count");
    let provider_preferences = metric_string(&combined, "openrouter_provider_preferences")
        .is_some_and(|value| value == "true");
    let total_latency_secs =
        metric_sum_f64(&combined, "openrouter_total_request_latency_secs");
    let estimated_tokens =
        metric_sum_usize(&combined, "openrouter_total_estimated_tokens");
    let avg_request_latency_secs = if request_count == 0 {
        0.0
    } else {
        total_latency_secs / request_count as f64
    };
    let padded_tokens_per_sec = if embed_duration_secs == 0.0 {
        0.0
    } else {
        estimated_tokens as f64 / embed_duration_secs
    };

    Ok(BenchmarkResult {
        max_batch_inputs,
        max_batch_tokens,
        concurrency,
        total_chunks,
        duration_secs,
        embed_duration_secs,
        request_count,
        retry_count,
        split_count,
        failed_request_count,
        provider_preferences,
        avg_request_latency_secs,
        estimated_tokens,
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

fn metric_sum_f64(output: &str, key: &str) -> f64 {
    let prefix = format!("{key}=");
    output
        .split_whitespace()
        .filter_map(|part| part.strip_prefix(&prefix)?.parse::<f64>().ok())
        .sum()
}

fn metric_sum_usize(output: &str, key: &str) -> usize {
    let prefix = format!("{key}=");
    output
        .split_whitespace()
        .filter_map(|part| part.strip_prefix(&prefix)?.parse::<usize>().ok())
        .sum()
}

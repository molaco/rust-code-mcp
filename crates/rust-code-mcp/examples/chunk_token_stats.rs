//! Measure the token-count distribution of code chunks for the
//! Qwen3-Embedding-0.6B tokenizer.
//!
//! For every `.rs` file in the workspace:
//!   1. Parse with `RustParser::parse_source_complete`.
//!   2. Run through `Chunker::chunk_file`.
//!   3. Apply the oversized-chunk splitter.
//!   4. Format each chunk with `CodeChunk::format_for_embedding`.
//!   5. Tokenize with the Qwen3 tokenizer.
//!   6. Aggregate statistics and print a report.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use rmc_engine::chunker::{ChunkSplitConfig, Chunker};
use rmc_engine::embeddings::{EmbeddingBackend, EmbeddingTokenCounter};
use rmc_engine::parser::RustParser;
use walkdir::WalkDir;

/// One observation: token count for a single chunk and where it came from.
struct ChunkStat {
    tokens: usize,
    chars: usize,
    file: PathBuf,
    symbol: String,
}

fn should_skip(path: &Path) -> bool {
    for component in path.components() {
        let s = component.as_os_str().to_string_lossy();
        if s == "target"
            || s == "vendor"
            || s == ".cache_bench"
            || s == ".tantivy_bench"
            || s == ".git"
        {
            return true;
        }
    }
    false
}

fn percentile(sorted: &[usize], p: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    // Nearest-rank method: index = ceil(p/100 * N) - 1
    let n = sorted.len() as f64;
    let idx = ((p / 100.0 * n).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let t0 = Instant::now();
    let backend = EmbeddingBackend::default();
    let token_counter = EmbeddingTokenCounter::from_backend(&backend)?;
    println!(
        "Using tokenizer for {} (max_len={})",
        backend
            .qwen3_variant()
            .map(|variant| variant.hf_model_id())
            .unwrap_or(backend.model_id()),
        token_counter.max_len()
    );

    let root = std::env::current_dir()?;
    println!("Workspace root: {}", root.display());

    // Collect .rs files.
    let mut rs_files: Vec<PathBuf> = Vec::new();
    for entry in WalkDir::new(&root).into_iter().filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        if should_skip(path) {
            continue;
        }
        rs_files.push(path.to_path_buf());
    }
    println!("Found {} .rs files", rs_files.len());

    // Parse + chunk + tokenize.
    let mut parser = RustParser::new()?;
    let chunker = Chunker::new();
    let mut stats: Vec<ChunkStat> = Vec::new();
    let mut failed_parse: Vec<(PathBuf, String)> = Vec::new();
    let mut failed_chunk: Vec<(PathBuf, String)> = Vec::new();
    let mut failed_tokenize: usize = 0;
    let mut files_with_chunks: usize = 0;
    let mut raw_chunks: usize = 0;
    let mut total_chunks: usize = 0;

    // Heuristic: the tokenizer has a configured max length; flag chunks whose
    // raw token id sequence exceeds it before any truncation we apply here.
    // We don't enable truncation, so we just measure raw counts.
    for file in &rs_files {
        let source = match fs::read_to_string(file) {
            Ok(s) => s,
            Err(e) => {
                failed_parse.push((file.clone(), format!("read: {e}")));
                continue;
            }
        };

        let parse_result = match parser.parse_source_complete(&source) {
            Ok(pr) => pr,
            Err(e) => {
                failed_parse.push((file.clone(), e.to_string()));
                continue;
            }
        };

        let chunks = match chunker.chunk_file(file, &source, &parse_result) {
            Ok(cs) => cs,
            Err(e) => {
                failed_chunk.push((file.clone(), e.to_string()));
                continue;
            }
        };
        raw_chunks += chunks.len();
        let chunks = chunker.split_oversized_chunks(
            chunks,
            ChunkSplitConfig::default(),
            |chunk| {
                token_counter
                    .count(&chunk.format_for_embedding())
                    .ok()
                    .map(|len| len.raw_tokens)
            },
        );

        if !chunks.is_empty() {
            files_with_chunks += 1;
        }
        total_chunks += chunks.len();

        for chunk in chunks {
            let formatted = chunk.format_for_embedding();
            let chars = formatted.chars().count();
            let token_len = match token_counter.count(&formatted) {
                Ok(e) => e,
                Err(_) => {
                    failed_tokenize += 1;
                    continue;
                }
            };
            let tokens = token_len.raw_tokens;

            stats.push(ChunkStat {
                tokens,
                chars,
                file: chunk.context.file_path.clone(),
                symbol: format!(
                    "{} ({})",
                    chunk.context.symbol_name, chunk.context.symbol_kind
                ),
            });
        }
    }

    let elapsed = t0.elapsed();

    // -------- report --------
    println!();
    println!("==== Chunk token-count distribution ====");
    println!("Files scanned:        {}", rs_files.len());
    println!("Files producing chunks: {files_with_chunks}");
    println!("Raw chunks (parsed):  {raw_chunks}");
    println!("Total chunks (split): {total_chunks}");
    println!("Tokenized chunks:     {}", stats.len());
    println!("Tokenize failures:    {failed_tokenize}");
    println!("Parse failures:       {}", failed_parse.len());
    println!("Chunk failures:       {}", failed_chunk.len());
    println!("Wall time:            {:.2?}", elapsed);
    println!();

    if stats.is_empty() {
        println!("No chunks produced - nothing to summarize.");
        return Ok(());
    }

    let mut tokens_sorted: Vec<usize> = stats.iter().map(|s| s.tokens).collect();
    tokens_sorted.sort_unstable();

    let n = tokens_sorted.len();
    let min = *tokens_sorted.first().unwrap();
    let max = *tokens_sorted.last().unwrap();
    let sum_tokens: u128 = tokens_sorted.iter().map(|&x| x as u128).sum();
    let model_max_len = token_counter.max_len();
    let capped_total: u128 = tokens_sorted
        .iter()
        .map(|&x| x.min(model_max_len) as u128)
        .sum();
    let mut by_chars: Vec<&ChunkStat> = stats.iter().collect();
    by_chars.sort_by_key(|s| s.chars);
    let padded_total_batch_32: u128 = by_chars
        .chunks(32)
        .map(|batch| {
            let batch_max = batch
                .iter()
                .map(|s| s.tokens.min(model_max_len))
                .max()
                .unwrap_or(0);
            (batch_max * batch.len()) as u128
        })
        .sum();
    let mean = sum_tokens as f64 / n as f64;
    let median = if n.is_multiple_of(2) {
        (tokens_sorted[n / 2 - 1] + tokens_sorted[n / 2]) as f64 / 2.0
    } else {
        tokens_sorted[n / 2] as f64
    };

    let p50 = percentile(&tokens_sorted, 50.0);
    let p90 = percentile(&tokens_sorted, 90.0);
    let p95 = percentile(&tokens_sorted, 95.0);
    let p99 = percentile(&tokens_sorted, 99.0);

    let mut b128 = 0usize;
    let mut b256 = 0usize;
    let mut b512 = 0usize;
    let mut b1024 = 0usize;
    let mut b2048 = 0usize;
    let mut bover = 0usize;
    for &t in &tokens_sorted {
        if t <= 128 {
            b128 += 1;
        } else if t <= 256 {
            b256 += 1;
        } else if t <= 512 {
            b512 += 1;
        } else if t <= 1024 {
            b1024 += 1;
        } else if t <= 2048 {
            b2048 += 1;
        } else {
            bover += 1;
        }
    }

    let sum_chars: u128 = stats.iter().map(|s| s.chars as u128).sum();
    let chars_per_token = sum_chars as f64 / sum_tokens as f64;

    println!("Token count summary:");
    println!("  N      = {n}");
    println!("  raw total = {sum_tokens}");
    println!("  model input total, capped at {model_max_len} = {capped_total}");
    println!("  padded model total, char-sorted batch 32 = {padded_total_batch_32}");
    println!("  min    = {min}");
    println!("  mean   = {:.2}", mean);
    println!("  median = {:.1}", median);
    println!("  max    = {max}");
    println!();
    println!("Percentiles:");
    println!("  p50 = {p50}");
    println!("  p90 = {p90}");
    println!("  p95 = {p95}");
    println!("  p99 = {p99}");
    println!();
    println!("Buckets (token count):");
    let pct = |c: usize| 100.0 * c as f64 / n as f64;
    println!("    <=128   {:>7}  ({:5.2}%)", b128, pct(b128));
    println!("   <=256   {:>7}  ({:5.2}%)", b256, pct(b256));
    println!("   <=512   {:>7}  ({:5.2}%)", b512, pct(b512));
    println!("  <=1024   {:>7}  ({:5.2}%)", b1024, pct(b1024));
    println!("  <=2048   {:>7}  ({:5.2}%)", b2048, pct(b2048));
    println!("   >2048   {:>7}  ({:5.2}%)", bover, pct(bover));
    println!();
    println!("Mean chars/token: {:.2}", chars_per_token);
    println!();

    // Top 5 longest chunks.
    let mut by_tokens: Vec<&ChunkStat> = stats.iter().collect();
    by_tokens.sort_by(|a, b| b.tokens.cmp(&a.tokens));
    println!("Top 5 longest chunks:");
    for (i, s) in by_tokens.iter().take(5).enumerate() {
        println!(
            "  {}. {} tokens  -  {}:{}",
            i + 1,
            s.tokens,
            s.file.display(),
            s.symbol
        );
    }

    // Surface a few parse/chunk failures if any.
    if !failed_parse.is_empty() {
        println!();
        println!("Parse failures ({}):", failed_parse.len());
        for (p, e) in failed_parse.iter().take(10) {
            println!("  {}: {}", p.display(), e);
        }
    }
    if !failed_chunk.is_empty() {
        println!();
        println!("Chunk failures ({}):", failed_chunk.len());
        for (p, e) in failed_chunk.iter().take(10) {
            println!("  {}: {}", p.display(), e);
        }
    }

    Ok(())
}

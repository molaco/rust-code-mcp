//! Index rust-code-mcp codebase for embedding benchmark runs.

use anyhow::{Context, Result, bail};
use rmc_engine::embeddings::{
    openrouter_runtime_config, resolve_profile, EmbeddingBackend, EmbeddingProfile,
    EmbeddingRuntime,
};
use rmc_indexing::indexing::IncrementalIndexer;
use std::path::PathBuf;
use std::time::Instant;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse()?;
    let backend = EmbeddingBackend::from_profile(args.profile);
    let identity = backend.identity();

    println!("\n{}", "=".repeat(60));
    println!("INDEXING rust-code-mcp CODEBASE");
    println!("{}\n", "=".repeat(60));

    let codebase = args.codebase;
    let cache = PathBuf::from(".cache_bench");
    let tantivy = PathBuf::from(".tantivy_bench");

    println!("Codebase: {}", codebase.display());
    println!("Profile:  {}", backend.profile.name());
    println!("Embedder: {}", identity);
    println!("Dim:      {}", backend.dim());
    println!("Initializing indexer...\n");

    let mut indexer = IncrementalIndexer::with_backend(
        &cache,
        &tantivy,
        &format!(
            "rust_code_bench_{}_{}",
            backend.profile.name().replace('-', "_"),
            uuid::Uuid::new_v4()
        ),
        backend.dim(),
        identity.as_str(),
        None,
        backend.clone(),
    )
    .await
    .context("Failed to create indexer")?;

    println!("Starting full indexing...\n");
    let start = Instant::now();

    let stats = indexer
        .indexer_mut()
        .await
        .context("Failed to initialize indexer")?
        .index_directory_parallel(&codebase)
        .await
        .context("Indexing failed")?;

    let duration = start.elapsed();
    let metrics = indexer
        .indexer()
        .context("Indexer was not initialized")?
        .metrics();
    let measured_duration = if metrics.total_duration.is_zero() {
        duration
    } else {
        metrics.total_duration
    };

    println!("\n{}", "=".repeat(60));
    println!("INDEXING COMPLETE");
    println!("{}", "=".repeat(60));
    println!("Files indexed:       {}", stats.indexed_files);
    println!("Chunks generated:    {}", stats.total_chunks);
    println!("Total time:          {:.2}s", duration.as_secs_f64());
    println!(
        "Throughput:          {:.1} files/sec",
        stats.indexed_files as f64 / duration.as_secs_f64()
    );
    println!(
        "                     {:.1} chunks/sec",
        stats.total_chunks as f64 / duration.as_secs_f64()
    );
    println!("\nMachine metrics:");
    println!("embedding_profile={}", backend.profile.name());
    println!("vector_dim={}", backend.dim());
    if backend.runtime == EmbeddingRuntime::OpenRouter {
        let openrouter_config = openrouter_runtime_config();
        println!(
            "openrouter_max_batch_inputs={}",
            openrouter_config.max_batch_inputs
        );
        println!(
            "openrouter_max_batch_tokens={}",
            openrouter_config.max_batch_tokens
        );
        println!("openrouter_concurrency={}", openrouter_config.concurrency);
        println!(
            "openrouter_encoding_format={}",
            openrouter_config.encoding_format.as_str()
        );
        println!(
            "openrouter_provider_preferences={}",
            openrouter_config.provider.is_some()
        );
        if let Some(provider) = openrouter_config.provider {
            if let Some(sort) = provider.sort {
                println!("openrouter_provider_sort={}", sort.as_str());
            }
            if let Some(throughput) = provider.preferred_min_throughput {
                println!("openrouter_preferred_min_throughput={throughput}");
            }
            if let Some(latency) = provider.preferred_max_latency {
                println!("openrouter_preferred_max_latency={latency}");
            }
        }
    }
    println!("total_files={}", stats.total_files);
    println!("indexed_files={}", stats.indexed_files);
    println!("skipped_files={}", stats.skipped_files);
    println!("total_chunks={}", stats.total_chunks);
    println!("duration_secs={:.6}", measured_duration.as_secs_f64());
    println!(
        "parse_duration_secs={:.6}",
        metrics.parse_duration.as_secs_f64()
    );
    println!(
        "embed_duration_secs={:.6}",
        metrics.embed_duration.as_secs_f64()
    );
    println!(
        "index_duration_secs={:.6}",
        metrics.index_duration.as_secs_f64()
    );
    println!("peak_memory_bytes={}", metrics.peak_memory_bytes);
    println!("{}\n", "=".repeat(60));
    Ok(())
}

#[derive(Debug)]
struct Args {
    codebase: PathBuf,
    profile: EmbeddingProfile,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut codebase = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut profile_name = String::from("local-gpu-small");
        let mut args = std::env::args().skip(1);

        while let Some(arg) = args.next() {
            match arg.as_str() {
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                "--codebase" => {
                    let value = args.next().context("--codebase requires a path")?;
                    codebase = PathBuf::from(value);
                }
                "--profile" => {
                    profile_name = args
                        .next()
                        .context("--profile requires a profile name")?;
                }
                other => bail!("unknown argument `{other}`"),
            }
        }

        // Resolve through the registry so built-in *and* dynamic
        // (`embedding_profiles.toml`) profiles both work.
        let profile = resolve_profile(&profile_name, &codebase)
            .map_err(|err| anyhow::anyhow!(err))?;

        Ok(Self { codebase, profile })
    }
}

fn print_usage() {
    println!("Usage: index_codebase [--profile PROFILE] [--codebase PATH]");
    println!("Profiles: {}", EmbeddingProfile::accepted_names());
}

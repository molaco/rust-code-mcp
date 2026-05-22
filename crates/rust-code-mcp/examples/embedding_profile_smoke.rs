//! Runtime smoke check for embedding profiles.
//!
//! Usage:
//! cargo run --example embedding_profile_smoke -- local-gpu-small
//! cargo run --example embedding_profile_smoke -- local-cpu-small
//! cargo run --example embedding_profile_smoke -- openrouter-qwen3-8b --expect-missing-key

use rmc_engine::embeddings::{EmbeddingBackend, EmbeddingProfile};
use rmc_indexing::indexing::IncrementalIndexer;
use std::path::PathBuf;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let Some(profile_name) = args.next() else {
        eprintln!(
            "usage: embedding_profile_smoke <profile> [--expect-missing-key]"
        );
        std::process::exit(2);
    };
    let expect_missing_key = args.any(|arg| arg == "--expect-missing-key");

    let profile = EmbeddingProfile::parse(&profile_name)
        .map_err(|err| format!("invalid profile `{profile_name}`: {err}"))?;
    let backend = EmbeddingBackend::from_profile(profile);

    println!("profile={}", backend.profile.name());
    println!("identity={}", backend.identity());
    println!("dim={}", backend.dim());

    match run_smoke(backend).await {
        Ok(()) => {
            if expect_missing_key {
                return Err(
                    "expected missing OpenRouter API key, but smoke succeeded".into()
                );
            }
            println!("smoke=ok");
            Ok(())
        }
        Err(err) if expect_missing_key && is_missing_openrouter_key(err.as_ref()) => {
            println!("smoke=expected_missing_openrouter_key");
            println!("error={err}");
            Ok(())
        }
        Err(err) => Err(err),
    }
}

async fn run_smoke(
    backend: EmbeddingBackend,
) -> Result<(), Box<dyn std::error::Error>> {
    let root = smoke_root(backend.profile.name());
    if root.exists() {
        std::fs::remove_dir_all(&root)?;
    }
    let codebase = root.join("codebase");
    let cache = root.join("cache");
    let tantivy = root.join("tantivy");
    std::fs::create_dir_all(&codebase)?;
    std::fs::write(
        codebase.join("lib.rs"),
        "pub fn add(left: i32, right: i32) -> i32 { left + right }\n",
    )?;

    let identity = backend.identity();
    let mut indexer = IncrementalIndexer::with_backend(
        &cache,
        &tantivy,
        &format!("embedding_profile_smoke_{}", backend.profile.name().replace('-', "_")),
        backend.dim(),
        identity.as_str(),
        None,
        backend,
    )
    .await?;
    indexer.clear_all_data().await?;
    let stats = indexer.index_with_change_detection(&codebase).await?;

    println!("indexed_files={}", stats.indexed_files);
    println!("total_chunks={}", stats.total_chunks);
    println!("skipped_files={}", stats.skipped_files);

    if stats.indexed_files != 1 {
        return Err(format!("expected 1 indexed file, got {}", stats.indexed_files).into());
    }
    if stats.total_chunks == 0 {
        return Err("expected at least one generated chunk".into());
    }

    Ok(())
}

fn smoke_root(profile: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rust_code_mcp_embedding_profile_smoke_{}_{}",
        sanitize_path_component(profile),
        std::process::id()
    ))
}

fn sanitize_path_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect()
}

fn is_missing_openrouter_key(err: &dyn std::error::Error) -> bool {
    let mut current = Some(err);
    while let Some(err) = current {
        let text = err.to_string();
        if text.contains("missing OpenRouter API key")
            || text.contains("RUST_CODE_MCP_OPENROUTER_API_KEY")
            || text.contains("OPENROUTER_API_KEY")
        {
            return true;
        }
        current = err.source();
    }
    false
}

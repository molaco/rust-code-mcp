//! MCP tool for manual codebase indexing
//!
//! Provides the `index_codebase` tool which allows manual triggering of
//! incremental indexing with optional force reindex.

use crate::embeddings::{EmbeddingBackend, Qwen3Variant};
use crate::indexing::incremental::{get_snapshot_path, IncrementalIndexer};
use crate::tools::project_paths::ProjectPaths;
use crate::vector_store::VectorStoreError;
use rmcp::{ErrorData as McpError, model::CallToolResult, model::Content, schemars};
use std::path::PathBuf;
use tracing;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexCodebaseParams {
    #[schemars(description = "Absolute path to codebase directory")]
    pub directory: String,
    #[schemars(description = "Force full reindex even if already indexed (default: false)")]
    pub force_reindex: Option<bool>,
    #[schemars(
        description = "Optional embedding model variant. One of: \"qwen3-0.6b\" (default, 1024-dim), \"qwen3-4b\" (2560-dim), \"qwen3-8b\" (4096-dim). Picking a variant different from an existing index returns a version-mismatch error pointing to clear_cache."
    )]
    pub model: Option<String>,
}

/// Parse a user-supplied model string into a [`Qwen3Variant`].
fn parse_variant(s: &str) -> Result<Qwen3Variant, String> {
    match s.to_ascii_lowercase().as_str() {
        "qwen3-0.6b" | "qwen3-0_6b" | "0.6b" => Ok(Qwen3Variant::Embedding0_6B),
        "qwen3-4b" | "4b" => Ok(Qwen3Variant::Embedding4B),
        "qwen3-8b" | "8b" => Ok(Qwen3Variant::Embedding8B),
        other => Err(format!(
            "unknown model variant: {other}; expected qwen3-0.6b, qwen3-4b, qwen3-8b"
        )),
    }
}

/// Resolve the user-supplied (or omitted) `model` argument into an
/// `EmbeddingBackend`. Defaults to `EmbeddingBackend::default()` when
/// the caller did not pick a variant.
fn resolve_backend(model: Option<&str>) -> Result<EmbeddingBackend, McpError> {
    let Some(s) = model else {
        return Ok(EmbeddingBackend::default());
    };
    let variant = parse_variant(s).map_err(|msg| McpError::invalid_params(msg, None))?;
    Ok(EmbeddingBackend {
        variant,
        max_len: 2048,
        force_cpu: false,
    })
}

/// Index a codebase directory with automatic change detection
///
/// This is the main entry point for the `index_codebase` MCP tool.
/// It performs incremental indexing using Merkle tree change detection.
pub async fn index_codebase(
    params: IndexCodebaseParams,
    sync_manager: Option<&std::sync::Arc<crate::mcp::SyncManager>>,
) -> Result<CallToolResult, McpError> {
    let dir = PathBuf::from(&params.directory);
    let force = params.force_reindex.unwrap_or(false);

    // Validate directory
    if !dir.exists() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' does not exist", params.directory),
            None,
        ));
    }

    if !dir.is_dir() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a directory", params.directory),
            None,
        ));
    }

    tracing::info!("Indexing codebase: {} (force: {})", dir.display(), force);

    // Resolve the embedding backend from the optional `model` arg. This
    // becomes the single source of truth for vector_size /
    // embedder_identity / project paths for this run.
    let backend = resolve_backend(params.model.as_deref())?;
    let embedder_identity = backend.identity();
    let paths = ProjectPaths::from_directory(&dir, &backend);

    tracing::debug!(
        "Using collection: {}, cache: {}, index: {}, embedder: {}",
        paths.collection_name,
        paths.cache_path.display(),
        paths.tantivy_path.display(),
        embedder_identity,
    );

    // Handle force reindex by deleting snapshot
    if force {
        let snapshot_path = get_snapshot_path(&dir);
        if snapshot_path.exists() {
            tracing::info!("Force reindex: deleting snapshot at {}", snapshot_path.display());
            std::fs::remove_file(&snapshot_path).map_err(|e| {
                McpError::invalid_params(format!("Failed to delete snapshot: {}", e), None)
            })?;
        }
    }

    // Create incremental indexer with embedded LanceDB backend. If the
    // on-disk vector store was built with a different embedder, surface
    // the version mismatch as a clear MCP error pointing at clear_cache
    // (do NOT auto-wipe).
    let mut indexer = match IncrementalIndexer::with_backend(
        &paths.cache_path,
        &paths.tantivy_path,
        &paths.collection_name,
        backend.dim(),
        &embedder_identity,
        None,
        backend,
    )
    .await
    {
        Ok(idx) => idx,
        Err(e) => {
            // The vector store layer surfaces VersionMismatch as a
            // boxed error; unwrap it back to a structured error if we
            // can so the user gets the actionable message.
            if let Some(vs_err) = e.downcast_ref::<VectorStoreError>() {
                if let VectorStoreError::VersionMismatch { stored, configured } = vs_err {
                    return Err(McpError::invalid_params(
                        format!(
                            "Cannot index with embedder `{configured}`: existing index was built with `{stored}`. \
                             Run `clear_cache` against directory `{}` (or include_hypergraph=true) to discard \
                             the old vectors and rebuild.",
                            dir.display()
                        ),
                        None,
                    ));
                }
            }
            return Err(McpError::invalid_params(
                format!("Failed to initialize indexer: {}", e),
                None,
            ));
        }
    };

    // Clear all indexed data if force reindex
    if force {
        tracing::info!("Force reindex: clearing all indexed data (metadata cache, Tantivy, vector store)");
        indexer.clear_all_data().await.map_err(|e| {
            McpError::invalid_params(format!("Failed to clear indexed data: {}", e), None)
        })?;
    }

    // Run incremental indexing
    let start = std::time::Instant::now();
    let stats = indexer
        .index_with_change_detection(&dir)
        .await
        .map_err(|e| McpError::invalid_params(format!("Indexing failed: {}", e), None))?;
    let elapsed = start.elapsed();

    // Track directory for background sync if indexing was successful
    if let Some(sync_mgr) = sync_manager {
        if stats.indexed_files > 0 || stats.unchanged_files > 0 {
            sync_mgr.track_directory(dir.clone()).await;
            tracing::info!(
                "Directory tracked for background sync: {}",
                dir.display()
            );
        }
    }

    // Format result. The resolved embedder identity is echoed verbatim
    // so a user who passed `model` (or relied on the default) can
    // confirm exactly which variant the index is bound to.
    let result_text = if stats.indexed_files == 0 && stats.unchanged_files == 0 {
        // No files indexed at all
        format!(
            "No Rust files suitable for indexing were found in '{}'.\n\
            Embedder: {}\n\
            Skipped files: {}\n\
            Time: {:?}",
            params.directory, embedder_identity, stats.skipped_files, elapsed
        )
    } else if stats.indexed_files == 0 {
        // No changes detected
        format!(
            "✓ No changes detected in '{}'\n\n\
            Indexing stats:\n\
            - Indexed files: {} (no changes)\n\
            - Total chunks: {}\n\
            - Unchanged files: {}\n\
            - Skipped files: {}\n\
            - Time: {:?} (< 10ms change detection)\n\n\
            Embedder: {}\n\
            Background sync: {}\n\
            Collection: {}",
            params.directory,
            stats.indexed_files,
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files,
            elapsed,
            embedder_identity,
            if sync_manager.is_some() {
                "enabled (5-minute interval)"
            } else {
                "disabled"
            },
            paths.collection_name
        )
    } else {
        // Changes detected and indexed
        format!(
            "✓ Successfully indexed '{}'\n\n\
            Indexing stats:\n\
            - Indexed files: {} {}\n\
            - Total chunks: {}\n\
            - Unchanged files: {}\n\
            - Skipped files: {}\n\
            - Time: {:?}\n\n\
            Embedder: {}\n\
            Background sync: {}\n\
            Collection: {}",
            params.directory,
            stats.indexed_files,
            if force { "(forced full reindex)" } else { "(incremental)" },
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files,
            elapsed,
            embedder_identity,
            if sync_manager.is_some() {
                "enabled (5-minute interval)"
            } else {
                "disabled"
            },
            paths.collection_name
        )
    };

    Ok(CallToolResult::success(vec![Content::text(result_text)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_index_codebase_invalid_directory() {
        let params = IndexCodebaseParams {
            directory: "/nonexistent/path".to_string(),
            force_reindex: None,
            model: None,
        };

        let result = index_codebase(params, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_index_codebase_not_a_directory() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        std::fs::write(&file_path, "test").unwrap();

        let params = IndexCodebaseParams {
            directory: file_path.to_string_lossy().to_string(),
            force_reindex: None,
            model: None,
        };

        let result = index_codebase(params, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_index_codebase_success() {
        let temp_dir = TempDir::new().unwrap();
        let test_codebase = temp_dir.path().join("codebase");
        std::fs::create_dir(&test_codebase).unwrap();
        std::fs::write(
            test_codebase.join("test.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        let params = IndexCodebaseParams {
            directory: test_codebase.to_string_lossy().to_string(),
            force_reindex: None,
            model: None,
        };

        let result = index_codebase(params, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires embedding model
    async fn test_index_codebase_force_reindex() {
        let temp_dir = TempDir::new().unwrap();
        let test_codebase = temp_dir.path().join("codebase");
        std::fs::create_dir(&test_codebase).unwrap();
        std::fs::write(
            test_codebase.join("test.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        // First index
        let params1 = IndexCodebaseParams {
            directory: test_codebase.to_string_lossy().to_string(),
            force_reindex: None,
            model: None,
        };
        let result1 = index_codebase(params1, None).await;
        assert!(result1.is_ok());

        // Force reindex
        let params2 = IndexCodebaseParams {
            directory: test_codebase.to_string_lossy().to_string(),
            force_reindex: Some(true),
            model: None,
        };
        let result2 = index_codebase(params2, None).await;
        assert!(result2.is_ok());
    }

    #[test]
    fn parse_variant_accepts_known_aliases() {
        assert!(matches!(
            parse_variant("qwen3-0.6b").unwrap(),
            Qwen3Variant::Embedding0_6B
        ));
        assert!(matches!(
            parse_variant("0.6b").unwrap(),
            Qwen3Variant::Embedding0_6B
        ));
        assert!(matches!(
            parse_variant("qwen3-4b").unwrap(),
            Qwen3Variant::Embedding4B
        ));
        assert!(matches!(
            parse_variant("4B").unwrap(),
            Qwen3Variant::Embedding4B
        ));
        assert!(matches!(
            parse_variant("qwen3-8b").unwrap(),
            Qwen3Variant::Embedding8B
        ));
    }

    #[test]
    fn parse_variant_rejects_unknown() {
        assert!(parse_variant("minilm").is_err());
    }
}

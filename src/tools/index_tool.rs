//! MCP tool for manual codebase indexing
//!
//! Provides the `index_codebase` tool which allows manual triggering of
//! incremental indexing with optional force reindex.

use crate::indexing::incremental::{get_snapshot_path, IncrementalIndexer};
use anyhow::Result;
use directories::ProjectDirs;
use rmcp::{ErrorData as McpError, model::CallToolResult, model::Content, schemars};
use std::path::PathBuf;
use tracing;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct IndexCodebaseParams {
    #[schemars(description = "Absolute path to codebase directory")]
    pub directory: String,
    #[schemars(description = "Force full reindex even if already indexed (default: false)")]
    pub force_reindex: Option<bool>,
}

/// Get the data directory for storing caches and indices
fn data_dir() -> PathBuf {
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
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

    // Get configuration
    let qdrant_url =
        std::env::var("QDRANT_URL").unwrap_or_else(|_| "http://localhost:6334".to_string());

    // Create collection name from directory hash (same strategy as search tool)
    let dir_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dir.to_string_lossy().as_bytes());
        format!("{:x}", hasher.finalize())
    };

    let cache_path = data_dir().join("cache").join(&dir_hash);
    let tantivy_path = data_dir().join("index").join(&dir_hash);
    let collection_name = format!("code_chunks_{}", &dir_hash[..8]);

    tracing::debug!(
        "Using collection: {}, cache: {}, index: {}",
        collection_name,
        cache_path.display(),
        tantivy_path.display()
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

    // Estimate codebase size for optimal Qdrant configuration
    #[cfg(feature = "qdrant")]
    let codebase_loc = crate::vector_store::estimate_codebase_size(&dir).ok();
    #[cfg(not(feature = "qdrant"))]
    let codebase_loc: Option<usize> = None;

    if let Some(loc) = codebase_loc {
        tracing::info!("Estimated codebase size: {} LOC", loc);
    } else {
        tracing::debug!("Codebase size estimation skipped or failed");
    }

    // Create incremental indexer with optimized configuration
    let mut indexer = IncrementalIndexer::new(
        &cache_path,
        &tantivy_path,
        &qdrant_url,
        &collection_name,
        384, // all-MiniLM-L6-v2 vector size
        codebase_loc,
    )
    .await
    .map_err(|e| {
        McpError::invalid_params(format!("Failed to initialize indexer: {}", e), None)
    })?;

    // Enable bulk mode if force reindexing (Qdrant feature only)
    #[cfg(feature = "qdrant")]
    let bulk_indexer = if force {
        use crate::indexing::bulk::{BulkIndexer, HnswConfig};

        tracing::info!("Force reindex: enabling bulk indexing mode for 3-5x speedup");

        // Create Qdrant client
        let qdrant_client = qdrant_client::Qdrant::from_url(&qdrant_url)
            .build()
            .map_err(|e| McpError::invalid_params(format!("Failed to connect to Qdrant: {}", e), None))?;

        let mut bulk_indexer = BulkIndexer::new(qdrant_client, collection_name.clone());

        // Start bulk mode with standard HNSW config
        let hnsw_config = HnswConfig::new(16, 100);
        bulk_indexer.start_bulk_mode(hnsw_config).await
            .map_err(|e| McpError::invalid_params(format!("Failed to start bulk mode: {}", e), None))?;

        Some(bulk_indexer)
    } else {
        None
    };
    #[cfg(not(feature = "qdrant"))]
    let bulk_indexer: Option<()> = None;

    // Clear all indexed data if force reindex
    if force {
        tracing::info!("Force reindex: clearing all indexed data (metadata cache, Tantivy, Qdrant)");
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

    // Exit bulk mode if it was enabled (Qdrant feature only)
    #[cfg(feature = "qdrant")]
    if let Some(mut bulk_indexer) = bulk_indexer {
        tracing::info!("Rebuilding HNSW index after bulk insertion...");
        bulk_indexer.end_bulk_mode().await
            .map_err(|e| McpError::invalid_params(format!("Failed to exit bulk mode: {}", e), None))?;
        tracing::info!("✓ HNSW index rebuilt");
    }
    #[cfg(not(feature = "qdrant"))]
    let _ = bulk_indexer; // Suppress unused warning

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

    // Format result
    let result_text = if stats.indexed_files == 0 && stats.unchanged_files == 0 {
        // No files indexed at all
        format!(
            "No Rust files suitable for indexing were found in '{}'.\n\
            Skipped files: {}\n\
            Time: {:?}",
            params.directory, stats.skipped_files, elapsed
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
            Background sync: {}\n\
            Collection: {}",
            params.directory,
            stats.indexed_files,
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files,
            elapsed,
            if sync_manager.is_some() {
                "enabled (5-minute interval)"
            } else {
                "disabled"
            },
            collection_name
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
            Background sync: {}\n\
            Collection: {}",
            params.directory,
            stats.indexed_files,
            if force { "(forced full reindex)" } else { "(incremental)" },
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files,
            elapsed,
            if sync_manager.is_some() {
                "enabled (5-minute interval)"
            } else {
                "disabled"
            },
            collection_name
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
        };

        let result = index_codebase(params, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore] // Requires Qdrant
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
        };

        let result = index_codebase(params, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    #[ignore] // Requires Qdrant
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
        };
        let result1 = index_codebase(params1, None).await;
        assert!(result1.is_ok());

        // Force reindex
        let params2 = IndexCodebaseParams {
            directory: test_codebase.to_string_lossy().to_string(),
            force_reindex: Some(true),
        };
        let result2 = index_codebase(params2, None).await;
        assert!(result2.is_ok());
    }
}

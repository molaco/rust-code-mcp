//! Server-facing incremental indexing facade.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rmc_engine::embeddings::EmbeddingBackend;

use crate::indexing::incremental::IncrementalIndexer;
use crate::indexing::unified::IndexStats;

/// Request for an incremental indexing run.
pub struct IncrementalIndexRequest<'a> {
    pub codebase_path: &'a Path,
    pub cache_path: &'a Path,
    pub tantivy_path: &'a Path,
    pub collection_name: &'a str,
    pub backend: EmbeddingBackend,
    pub embedder_identity: &'a str,
    pub snapshot_path: Option<&'a Path>,
    pub codebase_loc: Option<usize>,
    pub force_reindex: bool,
}

/// Result of an incremental indexing run.
pub struct IncrementalIndexOutcome {
    pub stats: IndexStats,
    pub elapsed: Duration,
}

/// Index a project through the indexing-owned incremental service boundary.
pub async fn index_project_incrementally(
    request: IncrementalIndexRequest<'_>,
) -> Result<IncrementalIndexOutcome> {
    if request.force_reindex {
        if let Some(snapshot_path) = request.snapshot_path {
            if snapshot_path.exists() {
                tracing::info!("Force reindex: deleting snapshot at {}", snapshot_path.display());
                std::fs::remove_file(snapshot_path).with_context(|| {
                    format!("Failed to delete snapshot at {}", snapshot_path.display())
                })?;
            }
        }
    }

    let mut indexer = IncrementalIndexer::with_backend(
        request.cache_path,
        request.tantivy_path,
        request.collection_name,
        request.backend.dim(),
        request.embedder_identity,
        request.codebase_loc,
        request.backend,
    )
    .await?;

    if request.force_reindex {
        tracing::info!("Force reindex: clearing all indexed data");
        indexer.clear_all_data().await?;
    }

    let start = Instant::now();
    let stats = indexer
        .index_with_change_detection(request.codebase_path)
        .await?;

    Ok(IncrementalIndexOutcome {
        stats,
        elapsed: start.elapsed(),
    })
}

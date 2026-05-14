//! Query tools module
//!
//! MCP tools for searching and querying indexed Rust codebases.
//! Implements hybrid search combining BM25 keyword search with semantic vector search.

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use tokio::fs;
use std::path::Path;
use tracing;

use crate::embeddings::{EmbeddingGenerator, EMBEDDING_DIM};
use crate::search::HybridSearch;
use crate::tools::project_paths::ProjectPaths;
use crate::vector_store::VectorStore;

/// Read and return the content of a specified file
pub async fn read_file_content(file_path: &str) -> Result<CallToolResult, McpError> {
    let file_path_obj = Path::new(file_path);

    if !file_path_obj.exists() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' does not exist", file_path),
            None,
        ));
    }

    if !file_path_obj.is_file() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a file", file_path),
            None,
        ));
    }

    match fs::read_to_string(file_path_obj).await {
        Ok(content) => {
            if content.is_empty() {
                Ok(CallToolResult::success(vec![Content::text(
                    "File is empty.",
                )]))
            } else {
                Ok(CallToolResult::success(vec![Content::text(content)]))
            }
        }
        Err(e) => {
            tracing::error!("Error reading file '{}': {}", file_path_obj.display(), e);
            match fs::read(file_path_obj).await {
                Ok(bytes) => {
                    if bytes.iter().any(|&b| b == 0)
                        || bytes
                            .iter()
                            .filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13)
                            .count()
                            > bytes.len() / 10
                    {
                        Err(McpError::invalid_params(
                            format!(
                                "The file '{}' appears to be a binary file and cannot be displayed as text",
                                file_path
                            ),
                            None,
                        ))
                    } else {
                        Err(McpError::invalid_params(
                            format!(
                                "The file '{}' could not be read as text: {}",
                                file_path, e
                            ),
                            None,
                        ))
                    }
                }
                Err(read_err) => Err(McpError::invalid_params(
                    format!("Error reading file '{}': {}", file_path, read_err),
                    None,
                )),
            }
        }
    }
}

/// Try to open BM25 search for an existing index.
/// Returns None if the index doesn't exist or is corrupt.
fn try_open_bm25(paths: &ProjectPaths) -> Option<crate::search::bm25::Bm25Search> {
    use crate::config::indexer::TantivyConfig;
    use crate::indexing::tantivy_adapter::TantivyAdapter;

    let config = TantivyConfig::default(&paths.tantivy_path);
    TantivyAdapter::new(config)
        .and_then(|adapter| adapter.create_bm25_search())
        .ok()
}

/// Remove stale index artifacts so ensure_indexed does a full rebuild.
/// Called when try_open_bm25 detects a corrupt or missing tantivy index.
fn clean_stale_index(paths: &ProjectPaths, dir: &Path) {
    use crate::indexing::incremental::get_snapshot_path;

    // Remove corrupt tantivy index
    if paths.tantivy_path.exists() {
        std::fs::remove_dir_all(&paths.tantivy_path).ok();
    }
    // Remove merkle snapshot so incremental indexer does a full pass
    let snapshot = get_snapshot_path(dir);
    if snapshot.exists() {
        std::fs::remove_file(&snapshot).ok();
    }
    // Clear metadata cache so files are re-processed
    if paths.cache_path.exists() {
        std::fs::remove_dir_all(&paths.cache_path).ok();
    }
}

/// Initialize indexer and run incremental indexing, returning stats.
/// Only called when we actually need to index.
async fn ensure_indexed(
    dir_path: &Path,
    paths: &ProjectPaths,
    sync_manager: Option<&std::sync::Arc<crate::mcp::SyncManager>>,
) -> Result<crate::indexing::unified::IndexStats, McpError> {
    use crate::indexing::unified::UnifiedIndexer;

    tracing::info!("Initializing unified indexer for {}", dir_path.display());

    let mut indexer = UnifiedIndexer::for_embedded(
        &paths.cache_path,
        &paths.tantivy_path,
        &paths.collection_name,
        EMBEDDING_DIM,
        None,
    )
    .await
    .map_err(|e| McpError::invalid_params(format!("Failed to initialize indexer: {}", e), None))?;

    let stats = indexer
        .index_directory(dir_path)
        .await
        .map_err(|e| McpError::invalid_params(format!("Indexing failed: {}", e), None))?;

    tracing::info!(
        "Indexed {} files ({} chunks), {} unchanged, {} skipped",
        stats.indexed_files, stats.total_chunks, stats.unchanged_files, stats.skipped_files
    );

    // Track directory for background sync
    if let Some(ref sync_mgr) = sync_manager {
        if stats.indexed_files > 0 || stats.unchanged_files > 0 {
            sync_mgr.track_directory(dir_path.to_path_buf()).await;
        }
    }

    Ok(stats)
}

/// Create a HybridSearch with a pre-validated BM25 search
pub(crate) async fn create_hybrid_search(
    paths: &ProjectPaths,
    bm25_search: Option<crate::search::bm25::Bm25Search>,
) -> Result<HybridSearch, McpError> {
    let embedding_generator = EmbeddingGenerator::new().map_err(|e| {
        McpError::invalid_params(format!("Failed to initialize embedding generator: {}", e), None)
    })?;

    let vector_store = VectorStore::new_embedded(paths.vector_path.clone(), EMBEDDING_DIM)
        .await
        .map_err(|e| {
            McpError::invalid_params(format!("Failed to initialize vector store: {}", e), None)
        })?;

    Ok(HybridSearch::with_defaults(
        embedding_generator,
        vector_store,
        bm25_search,
    ))
}

/// Format search results into a display string
fn format_results(
    results: &[crate::search::SearchResult],
    keyword: &str,
    stats: Option<&crate::indexing::unified::IndexStats>,
    rebuilt: bool,
) -> String {
    if results.is_empty() {
        let mut s = format!("No results found for '{}'.", keyword);
        if let Some(st) = stats {
            s.push_str(&format!(
                "\nIndexed {} files ({} chunks), {} unchanged, {} skipped",
                st.indexed_files, st.total_chunks, st.unchanged_files, st.skipped_files
            ));
        }
        return s;
    }

    let mut result_str = if rebuilt {
        format!("Note: corrupt index detected and rebuilt.\n\nFound {} results for '{}':\n\n", results.len(), keyword)
    } else {
        format!("Found {} results for '{}':\n\n", results.len(), keyword)
    };

    for (idx, result) in results.iter().enumerate() {
        result_str.push_str(&format!(
            "{}. Score: {:.4} | File: {} | Symbol: {} ({})\n",
            idx + 1,
            result.score,
            result.chunk.context.file_path.display(),
            result.chunk.context.symbol_name,
            result.chunk.context.symbol_kind,
        ));
        result_str.push_str(&format!(
            "   Lines: {}-{}\n",
            result.chunk.context.line_start, result.chunk.context.line_end
        ));
        if let Some(ref doc) = result.chunk.context.docstring {
            result_str.push_str(&format!("   Doc: {}\n", doc));
        }
        result_str.push_str(&format!(
            "   Preview:\n   {}\n\n",
            result.chunk.content.lines().take(3).collect::<Vec<_>>().join("\n   ")
        ));
    }

    if let Some(st) = stats {
        result_str.push_str(&format!(
            "\n--- Indexing stats: {} files indexed ({} chunks), {} unchanged, {} skipped ---",
            st.indexed_files, st.total_chunks, st.unchanged_files, st.skipped_files
        ));
    }

    result_str
}

/// Perform hybrid search (BM25 + Vector) on Rust code
pub async fn search(
    directory: &str,
    keyword: &str,
    sync_manager: Option<&std::sync::Arc<crate::mcp::SyncManager>>,
) -> Result<CallToolResult, McpError> {
    let dir_path = Path::new(directory);
    if !dir_path.is_dir() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a directory", directory),
            None,
        ));
    }

    if keyword.trim().is_empty() {
        return Err(McpError::invalid_params(
            "Search keyword is empty. Please enter a valid keyword.".to_string(),
            None,
        ));
    }

    let paths = ProjectPaths::from_directory(dir_path);

    // Try existing index; if corrupt or missing, rebuild
    let mut bm25 = try_open_bm25(&paths);
    let mut rebuilt = false;
    let stats = if bm25.is_some() {
        if let Some(ref sync_mgr) = sync_manager {
            sync_mgr.track_directory(dir_path.to_path_buf()).await;
        }
        None
    } else {
        // Corrupt or missing — clean stale caches to force full reindex
        rebuilt = paths.tantivy_path.exists();
        clean_stale_index(&paths, dir_path);
        let st = ensure_indexed(dir_path, &paths, sync_manager).await?;
        bm25 = try_open_bm25(&paths);
        Some(st)
    };

    // Handle case where first-time indexing produced nothing
    if let Some(ref st) = stats {
        if st.total_chunks == 0 && st.unchanged_files == 0 {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No Rust files suitable for indexing were found in '{}'.\nSkipped files: {}",
                directory, st.skipped_files
            ))]));
        }
    }

    let hybrid_search = create_hybrid_search(&paths, bm25).await?;

    tracing::info!("Performing hybrid search for: {}", keyword);
    let results = hybrid_search
        .search(keyword, 10)
        .await
        .map_err(|e| McpError::invalid_params(format!("Search failed: {}", e), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        format_results(&results, keyword, stats.as_ref(), rebuilt),
    )]))
}

/// Find semantically similar code using vector search
pub async fn get_similar_code(
    query: &str,
    directory: &str,
    limit: usize,
) -> Result<CallToolResult, McpError> {
    let dir_path = Path::new(directory);
    if !dir_path.is_dir() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a directory", directory),
            None,
        ));
    }

    let paths = ProjectPaths::from_directory(dir_path);

    let hybrid_search = create_hybrid_search(&paths, None).await?;

    let results = hybrid_search
        .vector_only_search(query, limit)
        .await
        .map_err(|e| McpError::invalid_params(format!("Search error: {}", e), None))?;

    if results.is_empty() {
        return Ok(CallToolResult::success(vec![Content::text(format!(
            "No similar code found for query: '{}'",
            query
        ))]));
    }

    let mut result = format!(
        "Found {} similar code snippet(s) for query '{}':\n\n",
        results.len(),
        query
    );

    for (idx, search_result) in results.iter().enumerate() {
        let chunk = &search_result.chunk;
        result.push_str(&format!(
            "{}. Score: {:.4} | File: {} | Symbol: {} ({})\n",
            idx + 1,
            search_result.score,
            chunk.context.file_path.display(),
            chunk.context.symbol_name,
            chunk.context.symbol_kind
        ));
        result.push_str(&format!(
            "   Lines: {}-{}\n",
            chunk.context.line_start, chunk.context.line_end
        ));
        if let Some(ref doc) = chunk.context.docstring {
            result.push_str(&format!("   Doc: {}\n", doc));
        }
        result.push_str(&format!(
            "   Code preview:\n   {}\n\n",
            chunk.content.lines().take(3).collect::<Vec<_>>().join("\n   ")
        ));
    }

    Ok(CallToolResult::success(vec![Content::text(result)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_read_file_content_nonexistent() {
        let result = read_file_content("/nonexistent/file.txt").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_read_file_content_directory() {
        let result = read_file_content("/tmp").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_invalid_directory() {
        let result = search("/nonexistent/directory", "test", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_empty_keyword() {
        let result = search("/tmp", "", None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_similar_code_invalid_directory() {
        let result = get_similar_code("test query", "/nonexistent/directory", 5).await;
        assert!(result.is_err());
    }
}

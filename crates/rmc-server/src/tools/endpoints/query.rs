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

use rmc_engine::embeddings::{EmbeddingBackend, EmbeddingGenerator};
use rmc_engine::search::HybridSearch;
use crate::mcp::project_paths::{
    ProjectPaths, read_embedder_identity, resolve_embedding_backend,
};
use rmc_engine::vector_store::VectorStore;

/// Read and return the content of a specified file
pub(crate) async fn read_file_content(file_path: &str) -> Result<CallToolResult, McpError> {
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
fn try_open_bm25(paths: &ProjectPaths) -> Option<rmc_engine::search::bm25::Bm25Search> {
    use rmc_config::config::indexer::TantivyConfig;
    use rmc_indexing::indexing::tantivy_adapter::TantivyAdapter;

    let config = TantivyConfig::default(&paths.tantivy_path);
    TantivyAdapter::new(config)
        .and_then(|adapter| adapter.create_bm25_search())
        .ok()
}

/// Remove stale index artifacts so ensure_indexed does a full rebuild.
/// Called when try_open_bm25 detects a corrupt or missing tantivy index.
fn clean_stale_index(paths: &ProjectPaths) {
    // Remove corrupt tantivy index
    if paths.tantivy_path.exists() {
        std::fs::remove_dir_all(&paths.tantivy_path).ok();
    }
    // Remove merkle snapshot so incremental indexer does a full pass
    if paths.snapshot_path.exists() {
        std::fs::remove_file(&paths.snapshot_path).ok();
    }
    // Clear metadata cache so files are re-processed
    if paths.cache_path.exists() {
        std::fs::remove_dir_all(&paths.cache_path).ok();
    }
}

fn vector_metadata_exists(paths: &ProjectPaths) -> bool {
    paths.vector_path.join("metadata.json").exists()
}

fn backend_matches_request(indexed: &EmbeddingBackend, requested: &EmbeddingBackend) -> bool {
    indexed.runtime == requested.runtime
        && indexed.model_id() == requested.model_id()
        && indexed.dim() == requested.dim()
        && indexed.max_len == requested.max_len
        && indexed.profile.query_policy == requested.profile.query_policy
}

fn select_index_paths(
    dir_path: &Path,
    requested_backend: &EmbeddingBackend,
) -> Result<ProjectPaths, McpError> {
    let requested_paths = ProjectPaths::from_directory(dir_path, requested_backend);
    if vector_metadata_exists(&requested_paths) {
        return Ok(requested_paths);
    }

    let existing = ProjectPaths::indexed_profiles(dir_path)
        .map_err(|msg| McpError::invalid_params(msg, None))?;
    if let Some(indexed) = existing
        .into_iter()
        .find(|indexed| backend_matches_request(&indexed.backend, requested_backend))
    {
        return Ok(indexed.paths);
    }

    Ok(requested_paths)
}

/// Initialize indexer and run incremental indexing, returning stats.
/// Only called when we actually need to index.
async fn ensure_indexed(
    dir_path: &Path,
    paths: &ProjectPaths,
    backend: EmbeddingBackend,
    sync_manager: Option<&std::sync::Arc<crate::mcp::SyncManager>>,
) -> Result<rmc_indexing::indexing::unified::IndexStats, McpError> {
    use rmc_indexing::indexing::unified::UnifiedIndexer;

    tracing::info!("Initializing unified indexer for {}", dir_path.display());
    let resolved = resolve_query_backend(paths, backend)?;
    let backend = resolved.backend;

    let mut indexer = UnifiedIndexer::for_embedded_with_backend(
        &paths.cache_path,
        &paths.tantivy_path,
        &paths.collection_name,
        backend.dim(),
        &resolved.vector_identity,
        None,
        backend,
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

/// Resolve the embedding backend the query side should use.
///
/// Search must embed queries with the same model that produced the
/// document vectors on disk; otherwise the cosine scoring against
/// LanceDB is meaningless and (in the worst case) hits a dim mismatch.
/// We treat `metadata.json` (written by `LanceDbBackend::new` on first
/// index) as the source of truth and parse it back into an
/// `EmbeddingBackend`. If no metadata file exists yet (very fresh
/// install, no prior index), fall back to the default backend — the
/// vector store will then create the metadata on first write.
struct ResolvedQueryBackend {
    backend: EmbeddingBackend,
    vector_identity: String,
}

fn resolve_query_backend(
    paths: &ProjectPaths,
    configured_backend: EmbeddingBackend,
) -> Result<ResolvedQueryBackend, McpError> {
    let Some(identity) = read_embedder_identity(&paths.vector_path).map_err(|msg| {
        McpError::invalid_params(
            format!("{msg}. Run `clear_cache` for this directory to discard the stale index."),
            None,
        )
    })? else {
        let vector_identity = configured_backend.identity();
        return Ok(ResolvedQueryBackend {
            backend: configured_backend,
            vector_identity,
        });
    };
    let backend = EmbeddingBackend::from_identity(&identity).map_err(|e| {
        McpError::invalid_params(
            format!(
                "Invalid embedder identity `{identity}` in {}: {e}. \
                 Run `clear_cache` for this directory to discard the stale index.",
                paths.vector_path.join("metadata.json").display()
            ),
            None,
        )
    })?;

    Ok(ResolvedQueryBackend {
        backend,
        vector_identity: identity,
    })
}

/// Create a HybridSearch with a pre-validated BM25 search.
///
/// The embedding backend is resolved from `metadata.json` next to the
/// LanceDB table (written by the indexer on first run). This keeps
/// query embeddings in lockstep with the on-disk vectors even when the
/// indexer was configured with a non-default variant.
pub(crate) async fn create_hybrid_search(
    paths: &ProjectPaths,
    bm25_search: Option<rmc_engine::search::bm25::Bm25Search>,
    configured_backend: EmbeddingBackend,
) -> Result<HybridSearch, McpError> {
    let resolved = resolve_query_backend(paths, configured_backend)?;
    let backend = resolved.backend;
    tracing::info!(
        profile = backend.profile.name(),
        embedder = resolved.vector_identity.as_str(),
        collection = paths.collection_name,
        "Creating hybrid search with embedding profile"
    );
    let embedding_generator = EmbeddingGenerator::with_backend(backend).map_err(|e| {
        McpError::invalid_params(
            format!("Failed to initialize embedding generator: {}", e),
            None,
        )
    })?;

    let vector_store = VectorStore::new_embedded(
        paths.vector_path.clone(),
        embedding_generator.dimensions(),
        &resolved.vector_identity,
    )
    .await
    .map_err(|e| {
        McpError::invalid_params(
            format!("Failed to initialize vector store: {}", e),
            None,
        )
    })?;

    Ok(HybridSearch::with_defaults(
        embedding_generator,
        vector_store,
        bm25_search,
    ))
}

fn resolve_requested_backend(
    embedding_profile: Option<&str>,
    dir_path: &Path,
) -> Result<EmbeddingBackend, McpError> {
    resolve_embedding_backend(embedding_profile, dir_path)
        .map_err(|msg| McpError::invalid_params(msg, None))
}

/// Format search results into a display string
fn format_results(
    results: &[rmc_engine::search::SearchResult],
    keyword: &str,
    stats: Option<&rmc_indexing::indexing::unified::IndexStats>,
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
pub(crate) async fn search(
    directory: &str,
    keyword: &str,
    embedding_profile: Option<&str>,
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

    let requested_backend = resolve_requested_backend(embedding_profile, dir_path)?;
    let paths = select_index_paths(dir_path, &requested_backend)?;

    // Try existing index; if corrupt or missing, rebuild
    let vector_index_exists = vector_metadata_exists(&paths);
    let mut bm25 = try_open_bm25(&paths);
    let mut rebuilt = false;
    let stats = if bm25.is_some() && vector_index_exists {
        if let Some(ref sync_mgr) = sync_manager {
            sync_mgr.track_directory(dir_path.to_path_buf()).await;
        }
        None
    } else {
        // Corrupt or missing — clean stale caches to force full reindex
        rebuilt = paths.tantivy_path.exists();
        clean_stale_index(&paths);
        let st = ensure_indexed(
            dir_path,
            &paths,
            requested_backend.clone(),
            sync_manager,
        )
        .await?;
        bm25 = try_open_bm25(&paths);
        Some(st)
    };

    // Handle case where first-time indexing produced nothing
    if let Some(ref st) = stats {
        if st.total_files == 0 {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No Rust files suitable for indexing were found in '{}'.\nSkipped files: {}",
                directory, st.skipped_files
            ))]));
        }
    }

    let hybrid_search = create_hybrid_search(&paths, bm25, requested_backend.clone()).await?;

    tracing::info!(
        profile = requested_backend.profile.name(),
        "Performing hybrid search for: {}",
        keyword
    );
    let results = hybrid_search
        .search(keyword, 10)
        .await
        .map_err(|e| McpError::invalid_params(format!("Search failed: {}", e), None))?;

    Ok(CallToolResult::success(vec![Content::text(
        format_results(&results, keyword, stats.as_ref(), rebuilt),
    )]))
}

/// Find semantically similar code using vector search
pub(crate) async fn get_similar_code(
    query: &str,
    directory: &str,
    limit: usize,
    embedding_profile: Option<&str>,
) -> Result<CallToolResult, McpError> {
    let dir_path = Path::new(directory);
    if !dir_path.is_dir() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a directory", directory),
            None,
        ));
    }

    let requested_backend = resolve_requested_backend(embedding_profile, dir_path)?;
    let paths = select_index_paths(dir_path, &requested_backend)?;

    let hybrid_search = create_hybrid_search(&paths, None, requested_backend).await?;

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
    use rmc_engine::embeddings::EmbeddingBackend;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn test_project_paths(vector_path: PathBuf) -> ProjectPaths {
        ProjectPaths {
            dir_hash: "test-dir".to_string(),
            indexing_identity: "test-indexing".to_string(),
            chunking_identity: "test-chunking".to_string(),
            cache_path: vector_path.join("cache"),
            tantivy_path: vector_path.join("tantivy"),
            snapshot_path: vector_path.join("snapshot.json"),
            collection_name: "code_chunks_test_legacy".to_string(),
            vector_path,
        }
    }

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
        let result = search("/nonexistent/directory", "test", None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_empty_keyword() {
        let result = search("/tmp", "", None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_similar_code_invalid_directory() {
        let result = get_similar_code("test query", "/nonexistent/directory", 5, None).await;
        assert!(result.is_err());
    }

    #[test]
    fn resolve_query_backend_preserves_legacy_vector_identity() {
        let temp_dir = TempDir::new().unwrap();
        let vector_path = temp_dir.path().join("vectors").join("code_chunks_legacy");
        std::fs::create_dir_all(&vector_path).unwrap();
        let legacy_identity =
            "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2";
        std::fs::write(
            vector_path.join("metadata.json"),
            serde_json::json!({ "embedder_version": legacy_identity }).to_string(),
        )
        .unwrap();
        let paths = test_project_paths(vector_path);

        let resolved = resolve_query_backend(&paths, EmbeddingBackend::default()).unwrap();

        assert_eq!(resolved.vector_identity, legacy_identity);
        assert_eq!(resolved.backend.profile.name(), "local-gpu-small");
    }
}

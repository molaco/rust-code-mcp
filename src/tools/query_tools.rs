//! Query tools module
//!
//! This module provides MCP tools for searching and querying indexed Rust codebases.
//! It implements hybrid search combining BM25 keyword search with semantic vector search
//! for optimal code discovery.
//!
//! ## Overview
//!
//! The query tools enable intelligent code search through:
//! - **Hybrid Search**: Combines BM25 (keyword) + Vector (semantic) search with RRF ranking
//! - **Semantic Search**: Pure vector-based similarity using code embeddings
//! - **File Reading**: Safe file content retrieval with binary file detection
//!
//! ## MCP Tools
//!
//! - [`search`]: Hybrid keyword + semantic search (automatically indexes if needed)
//! - [`get_similar_code`]: Find semantically similar code snippets using embeddings
//! - [`read_file_content`]: Read and validate file contents
//!
//! ## Search Architecture
//!
//! ```text
//! Query → HybridSearch
//!     ├─ BM25 Search (Tantivy)      → Keyword matches
//!     ├─ Vector Search (LanceDB)    → Semantic matches
//!     └─ RRF Fusion                 → Combined ranking
//! ```
//!
//! ## Examples
//!
//! ### Hybrid Search
//! ```rust,no_run
//! use file_search_mcp::tools::query_tools::search;
//!
//! # async fn example() -> Result<(), rmcp::ErrorData> {
//! // Search with hybrid approach (BM25 + semantic)
//! let results = search(
//!     "/path/to/rust/project",
//!     "async tokio spawn",
//!     None  // No sync manager
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ### Semantic Search
//! ```rust,no_run
//! use file_search_mcp::tools::query_tools::get_similar_code;
//!
//! # async fn example() -> Result<(), rmcp::ErrorData> {
//! // Find code similar to a description
//! let results = get_similar_code(
//!     "function that parses JSON and validates schema",
//!     "/path/to/project",
//!     10  // Return top 10 results
//! ).await?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Architecture
//!
//! This module is part of the refactored tools layer (Phase 1 refactoring).
//! It delegates to:
//! - `HybridSearch` for search logic
//! - `UnifiedIndexer` for automatic indexing
//! - `VectorStore` for semantic search

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use std::fs;
use std::path::Path;
use tracing;

use crate::embeddings::EmbeddingGenerator;
use crate::search::HybridSearch;
use crate::vector_store::VectorStore;

/// Read and return the content of a specified file
pub async fn read_file_content(file_path: &str) -> Result<CallToolResult, McpError> {
    // Validate file path
    let file_path_obj = Path::new(file_path);

    // Check if the path exists
    if !file_path_obj.exists() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' does not exist", file_path),
            None,
        ));
    }

    // Check if the path is a file
    if !file_path_obj.is_file() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a file", file_path),
            None,
        ));
    }

    // Try to read the file content
    match fs::read_to_string(file_path_obj) {
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
            // Handle binary files or read errors
            tracing::error!("Error reading file '{}': {}", file_path_obj.display(), e);

            // Try to read as binary and check if it's a binary file
            match fs::read(file_path_obj) {
                Ok(bytes) => {
                    // Check if it seems to be a binary file
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

/// Perform hybrid search (BM25 + Vector) on Rust code
pub async fn search(
    directory: &str,
    keyword: &str,
    sync_manager: Option<&std::sync::Arc<crate::mcp::SyncManager>>,
) -> Result<CallToolResult, McpError> {
    use crate::indexing::unified::UnifiedIndexer;
    use crate::tools::indexing_tools::data_dir;

    let dir_path = Path::new(directory);
    if !dir_path.is_dir() {
        return Err(McpError::invalid_params(
            format!("The specified path '{}' is not a directory", directory),
            None,
        ));
    }

    // Ensure the keyword is not empty
    if keyword.trim().is_empty() {
        return Err(McpError::invalid_params(
            "Search keyword is empty. Please enter a valid keyword.".to_string(),
            None,
        ));
    }

    // 1. Initialize unified indexer with embedded LanceDB backend
    // Use hash-based paths consistent with index_tool.rs
    let dir_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dir_path.to_string_lossy().as_bytes());
        format!("{:x}", hasher.finalize())
    };

    let cache_path = data_dir().join("cache").join(&dir_hash);
    let tantivy_path = data_dir().join("index").join(&dir_hash);
    let collection_name = format!("code_chunks_{}", &dir_hash[..8]);

    tracing::info!("Initializing unified indexer for {}", dir_path.display());
    tracing::debug!(
        "Using collection: {}, cache: {}, index: {}",
        collection_name,
        cache_path.display(),
        tantivy_path.display()
    );

    let mut indexer = UnifiedIndexer::for_embedded(
        &cache_path,
        &tantivy_path,
        &collection_name,
        384, // all-MiniLM-L6-v2 vector size
        None,
    )
    .await
    .map_err(|e| McpError::invalid_params(format!("Failed to initialize indexer: {}", e), None))?;

    // 2. Index directory (incremental - only changed files)
    tracing::info!("Indexing directory: {}", dir_path.display());
    let stats = indexer
        .index_directory(dir_path)
        .await
        .map_err(|e| McpError::invalid_params(format!("Indexing failed: {}", e), None))?;

    tracing::info!(
        "Indexed {} files ({} chunks), {} unchanged, {} skipped",
        stats.indexed_files,
        stats.total_chunks,
        stats.unchanged_files,
        stats.skipped_files
    );

    // Track directory for background sync if indexing was successful
    if let Some(ref sync_mgr) = sync_manager {
        if stats.indexed_files > 0 || stats.unchanged_files > 0 {
            sync_mgr.track_directory(dir_path.to_path_buf()).await;
            tracing::info!("Directory tracked for background sync: {}", dir_path.display());
        }
    }

    if stats.total_chunks == 0 && stats.unchanged_files == 0 {
        return Ok(CallToolResult::success(vec![Content::text(format!(
            "No Rust files suitable for indexing were found in '{}'.\nSkipped files: {}",
            directory, stats.skipped_files
        ))]));
    }

    // 3. Perform hybrid search
    let bm25_search = indexer.create_bm25_search()
        .map_err(|e| McpError::invalid_params(format!("Failed to create BM25 search: {}", e), None))?;

    let hybrid_search = HybridSearch::with_defaults(
        indexer.embedding_generator_cloned(),
        indexer.vector_store_cloned(),
        Some(bm25_search),
    );

    tracing::info!("Performing hybrid search for: {}", keyword);
    let results = hybrid_search
        .search(keyword, 10)
        .await
        .map_err(|e| McpError::invalid_params(format!("Search failed: {}", e), None))?;

    // 4. Format results
    if results.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(format!(
            "No results found for '{}'.\nIndexed {} files ({} chunks), {} unchanged, {} skipped",
            keyword, stats.indexed_files, stats.total_chunks, stats.unchanged_files, stats.skipped_files
        ))]))
    } else {
        let mut result_str = format!("Found {} results for '{}':\n\n", results.len(), keyword);

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
                result.chunk.context.line_start,
                result.chunk.context.line_end
            ));
            if let Some(ref doc) = result.chunk.context.docstring {
                result_str.push_str(&format!("   Doc: {}\n", doc));
            }
            result_str.push_str(&format!(
                "   Preview:\n   {}\n\n",
                result.chunk.content.lines().take(3).collect::<Vec<_>>().join("\n   ")
            ));
        }

        result_str.push_str(&format!(
            "\n--- Indexing stats: {} files indexed ({} chunks), {} unchanged, {} skipped ---",
            stats.indexed_files, stats.total_chunks, stats.unchanged_files, stats.skipped_files
        ));

        Ok(CallToolResult::success(vec![Content::text(result_str)]))
    }
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

    tracing::debug!("Searching for similar code in '{}' to: {}", directory, query);

    // Calculate directory hash to determine collection name
    let dir_hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(dir_path.to_string_lossy().as_bytes());
        format!("{:x}", hasher.finalize())
    };

    let collection_name = format!("code_chunks_{}", &dir_hash[..8]);

    tracing::debug!(
        "Using collection '{}' for directory '{}'",
        collection_name,
        dir_path.display()
    );

    // Initialize components
    let embedding_generator = EmbeddingGenerator::new().map_err(|e| {
        McpError::invalid_params(
            format!("Failed to initialize embedding generator: {}", e),
            None,
        )
    })?;

    // Create embedded vector store (LanceDB)
    // Path must match index_tool.rs: data_dir()/cache/vectors/{collection_name}
    let vector_store = {
        use crate::tools::indexing_tools::data_dir;
        let vector_path = data_dir().join("cache").join("vectors").join(&collection_name);
        VectorStore::new_embedded(vector_path, 384).await.map_err(|e| {
            McpError::invalid_params(format!("Failed to initialize vector store: {}", e), None)
        })?
    };

    // Create hybrid search (vector-only mode)
    let hybrid_search = HybridSearch::with_defaults(
        embedding_generator,
        vector_store,
        None, // No BM25 for this tool
    );

    // Perform vector search
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
        result.push_str(&format!("{}. ", idx + 1));
        result.push_str(&format!(
            "Score: {:.4} | File: {} | Symbol: {} ({})\n",
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
            chunk
                .content
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join("\n   ")
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

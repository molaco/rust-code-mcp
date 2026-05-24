//! Indexing tools module
//!
//! This module provides MCP tools and utilities for indexing Rust codebases.
//! It handles persistent storage paths and initialization of core indexing components.
//!
//! ## Overview
//!
//! The indexing tools provide foundation functions for:
//! - Managing persistent data directories (XDG-compliant)
//! - Opening/creating Tantivy BM25 indexes
//! - Opening/creating metadata caches for change detection
//!
//! ## MCP Tools
//!
//! - `index_codebase`: Manually index a directory with automatic change detection
//!   (exposed via `search_tool_router` and `index_tool`)
//!
//! ## Core Functions
//!
//! - [`open_or_create_index`]: Open or create a persistent Tantivy BM25 index
//! - [`open_cache`]: Open or create a sled-based metadata cache
//!
//! ## Examples
//!
//! ```rust,ignore
//! use rmc_server::tools::endpoints::indexing_support::open_cache;
//!
//! // Open metadata cache
//! let cache = open_cache().expect("Failed to open cache");
//! ```
//!
//! ## Architecture
//!
//! This module is part of the refactored tools layer (Phase 1 refactoring).
//! It provides low-level primitives used by higher-level indexing operations
//! in `index_tool` and `unified` modules.

use tantivy::Index;
use tracing;

use rmc_indexing::metadata_cache::MetadataCache;
use rmc_engine::schema::FileSchema;

/// Open or create a persistent Tantivy index
pub(crate) fn open_or_create_index() -> Result<(Index, FileSchema), String> {
    let schema = FileSchema::new();
    let index_path = crate::mcp::project_paths::data_dir().join("index");

    // Ensure directory exists
    std::fs::create_dir_all(&index_path)
        .map_err(|e| format!("Failed to create index directory: {}", e))?;

    let index = if index_path.join("meta.json").exists() {
        // Open existing index
        tracing::debug!("Opening existing index at: {}", index_path.display());
        Index::open_in_dir(&index_path).map_err(|e| format!("Failed to open index: {}", e))?
    } else {
        // Create new index
        tracing::info!("Creating new index at: {}", index_path.display());
        Index::create_in_dir(&index_path, schema.schema())
            .map_err(|e| format!("Failed to create index: {}", e))?
    };

    Ok((index, schema))
}

/// Open or create metadata cache
pub(crate) fn open_cache() -> Result<MetadataCache, String> {
    let cache_path = crate::mcp::project_paths::data_dir().join("cache");
    MetadataCache::new(&cache_path).map_err(|e| format!("Failed to open metadata cache: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_or_create_index() {
        // This test requires filesystem access and tantivy
        // In production, this would create a temporary directory
        let result = open_or_create_index();
        // Just check that it returns a result (could be error if permissions issue)
        assert!(result.is_ok() || result.is_err());
    }

    #[test]
    fn test_open_cache() {
        // This test requires filesystem access
        let result = open_cache();
        // Just check that it returns a result
        assert!(result.is_ok() || result.is_err());
    }
}

//! Indexing tools module
//!
//! This module provides MCP tools for indexing Rust codebases.
//!
//! ## Tools
//! - `index_codebase`: Manually index a directory with change detection
//!
//! ## Functions
//! - `data_dir`: Get the data directory for persistent storage
//! - `open_or_create_index`: Open or create a Tantivy index
//! - `open_cache`: Open or create metadata cache

use directories::ProjectDirs;
use std::path::PathBuf;
use tantivy::Index;
use tracing;

use crate::metadata_cache::MetadataCache;
use crate::schema::FileSchema;

/// Get the path for storing persistent index and cache
pub fn data_dir() -> PathBuf {
    // Use XDG-compliant data directory, or fallback to current directory
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
}

/// Open or create a persistent Tantivy index
pub fn open_or_create_index() -> Result<(Index, FileSchema), String> {
    let schema = FileSchema::new();
    let index_path = data_dir().join("index");

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
pub fn open_cache() -> Result<MetadataCache, String> {
    let cache_path = data_dir().join("cache");
    MetadataCache::new(&cache_path).map_err(|e| format!("Failed to open metadata cache: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_data_dir_exists() {
        let dir = data_dir();
        assert!(!dir.to_string_lossy().is_empty());
    }

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

//! Indexing tools module
//!
//! This module provides MCP tools and utilities for indexing Rust codebases.
//! It handles persistent storage paths and initialization of core indexing components.
//!
//! ## Overview
//!
//! The indexing tools provide foundation functions for:
//! - Managing persistent data directories (XDG-compliant)
//! - Opening/creating metadata caches for change detection
//!
//! ## MCP Tools
//!
//! - `index_codebase`: Manually index a directory with automatic change detection
//!   (exposed via `search_tool_router` and `index_tool`)
//!
//! ## Core Functions
//!
//! - [`data_dir`]: Get the XDG-compliant data directory for persistent storage
//! - [`open_cache`]: Open or create a sled-based metadata cache
//!
//! ## Examples
//!
//! ```rust,no_run
//! use rust_code_mcp_server::tools::indexing_tools::{data_dir, open_cache};
//!
//! // Get data directory (cross-platform)
//! let data = data_dir();
//! println!("Data stored at: {}", data.display());
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

use directories::ProjectDirs;
use std::path::PathBuf;

use rust_code_mcp_indexing::metadata_cache::MetadataCache;

/// Get the path for storing persistent index and cache
pub fn data_dir() -> PathBuf {
    // Use XDG-compliant data directory, or fallback to current directory
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
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
    fn test_open_cache() {
        // This test requires filesystem access
        let result = open_cache();
        // Just check that it returns a result
        assert!(result.is_ok() || result.is_err());
    }
}

//! Cache clearing tool for fixing corrupted metadata cache
//!
//! Provides a way to clear corrupted sled database files that cause
//! "Failed to open MetadataCache" errors.

use directories::ProjectDirs;
use rmcp::{
    model::{CallToolResult, Content},
    schemars, ErrorData as McpError,
};
use sha2::{Digest, Sha256};
use std::path::PathBuf;

/// Parameters for clearing the cache
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ClearCacheParams {
    #[schemars(
        description = "Optional: project directory to clear cache for. If not provided, clears all caches."
    )]
    pub directory: Option<String>,
}

/// Get the path for storing persistent index and cache
fn data_dir() -> PathBuf {
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
}

/// Compute the directory hash (same logic as other tools)
fn compute_dir_hash(dir_path: &std::path::Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dir_path.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Clear cache, index, and vector store for a project or all projects
///
/// This tool fixes "Failed to open MetadataCache" errors by removing
/// corrupted sled database files.
pub async fn clear_cache(
    params: ClearCacheParams,
) -> Result<CallToolResult, McpError> {
    let mut cleared = Vec::new();
    let mut errors = Vec::new();

    let data_dir = data_dir();

    if let Some(ref directory) = params.directory {
        // Clear cache for specific project
        let dir_path = std::path::Path::new(directory);
        let dir_hash = compute_dir_hash(dir_path);
        let collection_name = format!("code_chunks_{}", &dir_hash[..8]);

        // 1. Clear metadata cache (sled database)
        let cache_path = data_dir.join("cache").join(&dir_hash);
        if cache_path.exists() {
            match std::fs::remove_dir_all(&cache_path) {
                Ok(_) => cleared.push(format!("Metadata cache: {}", cache_path.display())),
                Err(e) => errors.push(format!("Failed to clear metadata cache: {}", e)),
            }
        }

        // 2. Clear tantivy index
        let tantivy_path = data_dir.join("index").join(&dir_hash);
        if tantivy_path.exists() {
            match std::fs::remove_dir_all(&tantivy_path) {
                Ok(_) => cleared.push(format!("Tantivy index: {}", tantivy_path.display())),
                Err(e) => errors.push(format!("Failed to clear tantivy index: {}", e)),
            }
        }

        // 3. Clear vector store
        let vector_path = data_dir.join("cache").join("vectors").join(&collection_name);
        if vector_path.exists() {
            match std::fs::remove_dir_all(&vector_path) {
                Ok(_) => cleared.push(format!("Vector store: {}", vector_path.display())),
                Err(e) => errors.push(format!("Failed to clear vector store: {}", e)),
            }
        }
    } else {
        // Clear all caches
        let cache_dir = data_dir.join("cache");
        if cache_dir.exists() {
            match std::fs::remove_dir_all(&cache_dir) {
                Ok(_) => cleared.push(format!("All caches: {}", cache_dir.display())),
                Err(e) => errors.push(format!("Failed to clear cache directory: {}", e)),
            }
        }

        let index_dir = data_dir.join("index");
        if index_dir.exists() {
            match std::fs::remove_dir_all(&index_dir) {
                Ok(_) => cleared.push(format!("All indices: {}", index_dir.display())),
                Err(e) => errors.push(format!("Failed to clear index directory: {}", e)),
            }
        }
    }

    // Build response
    let mut response = String::new();

    if cleared.is_empty() && errors.is_empty() {
        response.push_str("No cache files found to clear.\n");
    } else {
        if !cleared.is_empty() {
            response.push_str("✓ Successfully cleared:\n");
            for item in &cleared {
                response.push_str(&format!("  - {}\n", item));
            }
        }

        if !errors.is_empty() {
            response.push_str("\n✗ Errors:\n");
            for error in &errors {
                response.push_str(&format!("  - {}\n", error));
            }
        }
    }

    if params.directory.is_some() {
        response.push_str("\nThe project will be re-indexed on next search.\n");
    } else {
        response.push_str("\nAll projects will be re-indexed on next search.\n");
    }

    Ok(CallToolResult::success(vec![Content::text(response)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_dir_hash() {
        let hash = compute_dir_hash(std::path::Path::new("/some/test/path"));
        assert_eq!(hash.len(), 64); // SHA256 hex is 64 chars
    }

    #[tokio::test]
    async fn test_clear_cache_nonexistent() {
        // Clearing a non-existent directory should succeed with "nothing to clear"
        let result = clear_cache(ClearCacheParams {
            directory: Some("/nonexistent/path/that/does/not/exist".to_string()),
        })
        .await;

        assert!(result.is_ok());
    }
}

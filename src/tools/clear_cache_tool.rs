//! Cache clearing tool for fixing corrupted metadata cache
//!
//! Provides a way to clear corrupted sled database files that cause
//! "Failed to open MetadataCache" errors. Optionally also wipes the
//! persisted hypergraph snapshot so the next `build_hypergraph` call
//! does a full re-index.

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
    /// When `true`, also wipe the persisted hypergraph snapshot for the
    /// targeted workspace (or all workspaces, when `directory` is
    /// `None`). Defaults to `false` for backward compatibility. Wiping
    /// the hypergraph forces the next `build_hypergraph` call to do a
    /// full re-index.
    #[schemars(
        description = "When true, also wipe the persisted hypergraph snapshot directory at <data_dir>/graphs/<workspace_hash>/. Defaults to false. Forces the next build_hypergraph call to do a full re-index."
    )]
    #[serde(default)]
    pub include_hypergraph: Option<bool>,
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
/// corrupted sled database files. When `include_hypergraph` is set, the
/// persisted workspace hypergraph snapshot is also wiped so the next
/// `build_hypergraph` call performs a full re-index.
pub async fn clear_cache(
    params: ClearCacheParams,
) -> Result<CallToolResult, McpError> {
    let mut cleared = Vec::new();
    let mut errors = Vec::new();

    let data_dir = data_dir();
    let include_hypergraph = params.include_hypergraph.unwrap_or(false);

    if let Some(ref directory) = params.directory {
        // Clear cache for specific project
        let dir_path = std::path::Path::new(directory);
        let dir_hash = compute_dir_hash(dir_path);
        // The current layout keys vector directories as
        // `code_chunks_<dirhash[..8]>_<modelfp[..8]>`. The legacy
        // pre-Step-6 layout used just `code_chunks_<dirhash[..8]>`.
        // Walk both so a user with stale MiniLM directories on disk can
        // wipe them with one call.
        let dir_prefix = format!("code_chunks_{}", &dir_hash[..8]);
        let legacy_collection_name = dir_prefix.clone();

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

        // 3. Clear vector store(s) — both legacy and per-model layouts.
        let vectors_root = data_dir.join("cache").join("vectors");
        if vectors_root.exists() {
            // Pre-Step-6 layout: a single directory named exactly
            // `code_chunks_<dirhash[..8]>`.
            let legacy_path = vectors_root.join(&legacy_collection_name);
            if legacy_path.exists() {
                match std::fs::remove_dir_all(&legacy_path) {
                    Ok(_) => cleared
                        .push(format!("Vector store (legacy): {}", legacy_path.display())),
                    Err(e) => {
                        errors.push(format!("Failed to clear legacy vector store: {}", e))
                    }
                }
            }
            // Current layout: any directory whose name starts with
            // `code_chunks_<dirhash[..8]>_` (one per embedder model
            // fingerprint).
            let entry_prefix = format!("{}_", dir_prefix);
            if let Ok(read_dir) = std::fs::read_dir(&vectors_root) {
                for entry in read_dir.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with(&entry_prefix) {
                        let p = entry.path();
                        match std::fs::remove_dir_all(&p) {
                            Ok(_) => cleared.push(format!("Vector store: {}", p.display())),
                            Err(e) => {
                                errors.push(format!("Failed to clear vector store: {}", e))
                            }
                        }
                    }
                }
            }
        }

        // 4. Optionally clear the persisted hypergraph snapshot. Resolve
        // the per-workspace path via the same primitive `build_hypergraph`
        // uses (`GraphPaths::for_workspace`) so the canonicalization and
        // hash logic stay in one place.
        if include_hypergraph {
            let canonical = std::fs::canonicalize(dir_path).unwrap_or_else(|_| dir_path.into());
            let paths = crate::graph::GraphPaths::for_workspace(&canonical);
            if paths.root_dir.exists() {
                match std::fs::remove_dir_all(&paths.root_dir) {
                    Ok(_) => cleared.push(format!(
                        "Hypergraph snapshot: {}",
                        paths.root_dir.display()
                    )),
                    Err(e) => {
                        errors.push(format!("Failed to clear hypergraph snapshot: {}", e))
                    }
                }
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

        // All-projects hypergraph wipe: nuke the entire `graphs/` dir
        // under the data dir. That's `default_data_dir()` from
        // `graph::storage` — same parent that `GraphPaths::for_workspace`
        // hashes underneath.
        if include_hypergraph {
            let graphs_dir = crate::graph::storage::default_data_dir();
            if graphs_dir.exists() {
                match std::fs::remove_dir_all(&graphs_dir) {
                    Ok(_) => cleared.push(format!(
                        "All hypergraph snapshots: {}",
                        graphs_dir.display()
                    )),
                    Err(e) => errors
                        .push(format!("Failed to clear hypergraph snapshots: {}", e)),
                }
            }
        }
    }

    // Build response
    let mut response = String::new();

    if cleared.is_empty() && errors.is_empty() {
        response.push_str("No cache files found to clear.\n");
    } else {
        if !cleared.is_empty() {
            response.push_str("Successfully cleared:\n");
            for item in &cleared {
                response.push_str(&format!("  - {}\n", item));
            }
        }

        if !errors.is_empty() {
            response.push_str("\nErrors:\n");
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
    if include_hypergraph {
        response.push_str("The next build_hypergraph call will do a full re-index.\n");
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
            include_hypergraph: None,
        })
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_cache_include_hypergraph_param_compiles() {
        // Smoke: the new flag deserializes via the public schema and
        // reaches the body without panicking. We use a path that doesn't
        // exist so no real state is touched.
        let result = clear_cache(ClearCacheParams {
            directory: Some("/nonexistent/path/that/does/not/exist".to_string()),
            include_hypergraph: Some(true),
        })
        .await;
        assert!(result.is_ok());
    }
}

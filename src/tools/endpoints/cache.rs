//! Cache clearing tool for fixing corrupted metadata cache
//!
//! Provides a way to clear corrupted sled database files that cause
//! "Failed to open MetadataCache" errors. Optionally also wipes the
//! persisted hypergraph snapshot so the next `build_hypergraph` call
//! does a full re-index.

use rmcp::{
    model::{CallToolResult, Content},
    schemars, ErrorData as McpError,
};
use std::path::Path;

use crate::tools::project_paths::{data_dir, dir_hash};

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
    #[schemars(
        description = "When true, report the cache/index/vector/hypergraph paths that would be removed without deleting anything. Default false."
    )]
    #[serde(default)]
    pub dry_run: Option<bool>,
}

fn clear_existing_dir(
    label: &str,
    error_label: &str,
    path: &Path,
    dry_run: bool,
    cleared: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    if !path.exists() {
        return;
    }
    if dry_run {
        cleared.push(format!("{}: {}", label, path.display()));
        return;
    }
    match std::fs::remove_dir_all(path) {
        Ok(_) => cleared.push(format!("{}: {}", label, path.display())),
        Err(e) => errors.push(format!("Failed to clear {}: {}", error_label, e)),
    }
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
    let dry_run = params.dry_run.unwrap_or(false);

    if let Some(ref directory) = params.directory {
        // Clear cache for specific project
        let dir_path = std::path::Path::new(directory);
        let dir_hash = dir_hash(dir_path);
        // The current layout keys vector directories as
        // `code_chunks_<dirhash[..8]>_<modelfp[..8]>`. The legacy
        // pre-Step-6 layout used just `code_chunks_<dirhash[..8]>`.
        // Walk both so a user with stale MiniLM directories on disk can
        // wipe them with one call.
        let dir_prefix = format!("code_chunks_{}", &dir_hash[..8]);
        let legacy_collection_name = dir_prefix.clone();

        // 1. Clear metadata cache (sled database)
        let cache_path = data_dir.join("cache").join(&dir_hash);
        clear_existing_dir(
            "Metadata cache",
            "metadata cache",
            &cache_path,
            dry_run,
            &mut cleared,
            &mut errors,
        );

        // 2. Clear tantivy index
        let tantivy_path = data_dir.join("index").join(&dir_hash);
        clear_existing_dir(
            "Tantivy index",
            "tantivy index",
            &tantivy_path,
            dry_run,
            &mut cleared,
            &mut errors,
        );

        // 3. Clear vector store(s) — both legacy and per-model layouts.
        let vectors_root = data_dir.join("cache").join("vectors");
        if vectors_root.exists() {
            // Pre-Step-6 layout: a single directory named exactly
            // `code_chunks_<dirhash[..8]>`.
            let legacy_path = vectors_root.join(&legacy_collection_name);
            clear_existing_dir(
                "Vector store (legacy)",
                "legacy vector store",
                &legacy_path,
                dry_run,
                &mut cleared,
                &mut errors,
            );
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
                        clear_existing_dir(
                            "Vector store",
                            "vector store",
                            &p,
                            dry_run,
                            &mut cleared,
                            &mut errors,
                        );
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
            clear_existing_dir(
                "Hypergraph snapshot",
                "hypergraph snapshot",
                &paths.root_dir,
                dry_run,
                &mut cleared,
                &mut errors,
            );
        }
    } else {
        // Clear all caches
        let cache_dir = data_dir.join("cache");
        clear_existing_dir(
            "All caches",
            "cache directory",
            &cache_dir,
            dry_run,
            &mut cleared,
            &mut errors,
        );

        let index_dir = data_dir.join("index");
        clear_existing_dir(
            "All indices",
            "index directory",
            &index_dir,
            dry_run,
            &mut cleared,
            &mut errors,
        );

        // All-projects hypergraph wipe: nuke the entire `graphs/` dir
        // under the data dir. That's `default_data_dir()` from
        // `graph::storage` — same parent that `GraphPaths::for_workspace`
        // hashes underneath.
        if include_hypergraph {
            let graphs_dir = crate::graph::storage::default_data_dir();
            clear_existing_dir(
                "All hypergraph snapshots",
                "hypergraph snapshots",
                &graphs_dir,
                dry_run,
                &mut cleared,
                &mut errors,
            );
        }
    }

    // Build response
    let mut response = String::new();

    if cleared.is_empty() && errors.is_empty() {
        response.push_str("No cache files found to clear.\n");
    } else {
        if !cleared.is_empty() {
            if dry_run {
                response.push_str("Dry run - would clear:\n");
            } else {
                response.push_str("Successfully cleared:\n");
            }
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
        if dry_run {
            response.push_str("\nThe project would be re-indexed on next search if run without dry_run.\n");
        } else {
            response.push_str("\nThe project will be re-indexed on next search.\n");
        }
    } else {
        if dry_run {
            response.push_str("\nAll projects would be re-indexed on next search if run without dry_run.\n");
        } else {
            response.push_str("\nAll projects will be re-indexed on next search.\n");
        }
    }
    if include_hypergraph {
        if dry_run {
            response.push_str("The next build_hypergraph call would do a full re-index if run without dry_run.\n");
        } else {
            response.push_str("The next build_hypergraph call will do a full re-index.\n");
        }
    }

    Ok(CallToolResult::success(vec![Content::text(response)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_dir_hash() {
        let hash = dir_hash(std::path::Path::new("/some/test/path"));
        assert_eq!(hash.len(), 64); // SHA256 hex is 64 chars
    }

    #[tokio::test]
    async fn test_clear_cache_nonexistent() {
        // Clearing a non-existent directory should succeed with "nothing to clear"
        let result = clear_cache(ClearCacheParams {
            directory: Some("/nonexistent/path/that/does/not/exist".to_string()),
            include_hypergraph: None,
            dry_run: None,
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
            dry_run: None,
        })
        .await;
        assert!(result.is_ok());
    }

    #[test]
    fn dry_run_reports_existing_dir_without_removing_it() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("cache");
        std::fs::create_dir(&target).unwrap();
        let mut cleared = Vec::new();
        let mut errors = Vec::new();

        clear_existing_dir(
            "Metadata cache",
            "metadata cache",
            &target,
            true,
            &mut cleared,
            &mut errors,
        );

        assert!(target.exists());
        assert!(errors.is_empty());
        assert_eq!(cleared.len(), 1);
        assert!(cleared[0].contains("Metadata cache:"));
    }

    #[test]
    fn clear_existing_dir_removes_when_not_dry_run() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("cache");
        std::fs::create_dir(&target).unwrap();
        let mut cleared = Vec::new();
        let mut errors = Vec::new();

        clear_existing_dir(
            "Metadata cache",
            "metadata cache",
            &target,
            false,
            &mut cleared,
            &mut errors,
        );

        assert!(!target.exists());
        assert!(errors.is_empty());
        assert_eq!(cleared.len(), 1);
    }
}

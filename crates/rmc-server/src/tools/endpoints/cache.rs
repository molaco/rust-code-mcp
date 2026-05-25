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
use rmc_graph::graph::{GraphSnapshotCleanupOptions, GraphSnapshotCleanupReport};
use std::path::{Path, PathBuf};

use crate::mcp::project_paths::{data_dir, dir_hash};

/// Parameters for clearing the cache
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ClearCacheParams {
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

fn record_graph_cleanup_report(
    report: GraphSnapshotCleanupReport,
    cleared: &mut Vec<String>,
    errors: &mut Vec<String>,
) {
    cleared.extend(
        report
            .cleared
            .into_iter()
            .map(|entry| format!("{}: {}", entry.label, entry.path.display())),
    );
    errors.extend(report.errors);
}

#[derive(Debug)]
struct TargetDirectory {
    canonical: PathBuf,
    hashes: Vec<String>,
}

fn target_directory(directory: &str) -> TargetDirectory {
    let raw = PathBuf::from(directory);
    let canonical = std::fs::canonicalize(&raw).unwrap_or_else(|_| raw.clone());
    let canonical_hash = dir_hash(&canonical);
    let raw_hash = dir_hash(&raw);
    let mut hashes = vec![canonical_hash];
    if raw_hash != hashes[0] {
        hashes.push(raw_hash);
    }

    TargetDirectory { canonical, hashes }
}

/// Clear cache, index, and vector store for a project or all projects
///
/// This tool fixes "Failed to open MetadataCache" errors by removing
/// corrupted sled database files. When `include_hypergraph` is set, the
/// persisted workspace hypergraph snapshot is also wiped so the next
/// `build_hypergraph` call performs a full re-index.
pub(crate) async fn clear_cache(
    params: ClearCacheParams,
    sync_manager: Option<&std::sync::Arc<crate::mcp::SyncManager>>,
    workspace_locks: &crate::mcp::WorkspaceLockRegistry,
    search_cache: Option<&crate::mcp::SearchRuntimeCache>,
) -> Result<CallToolResult, McpError> {
    let mut cleared = Vec::new();
    let mut errors = Vec::new();

    let data_dir = data_dir();
    let include_hypergraph = params.include_hypergraph.unwrap_or(false);
    let dry_run = params.dry_run.unwrap_or(false);

    if let Some(ref directory) = params.directory {
        // Clear cache for specific project
        let target = target_directory(directory);
        let _workspace_lock = workspace_locks.lock_exclusive(&target.canonical).await;
        if !dry_run {
            if let Some(sync_manager) = sync_manager {
                sync_manager.untrack_directory(&target.canonical).await;
            }
            if let Some(search_cache) = search_cache {
                search_cache.invalidate_workspace(&target.canonical);
            }
        }

        for dir_hash in &target.hashes {
            // The current layout keys vector directories as
            // `code_chunks_<dirhash[..8]>_<modelfp[..8]>`. The legacy
            // pre-Step-6 layout used just `code_chunks_<dirhash[..8]>`.
            // Walk both so a user with stale MiniLM directories on disk can
            // wipe them with one call.
            let dir_prefix = format!("code_chunks_{}", &dir_hash[..8]);
            let legacy_collection_name = dir_prefix.clone();

            // 1. Clear metadata cache (sled database)
            let cache_path = data_dir.join("cache").join(dir_hash);
            clear_existing_dir(
                "Metadata cache",
                "metadata cache",
                &cache_path,
                dry_run,
                &mut cleared,
                &mut errors,
            );

            // 2. Clear tantivy index
            let tantivy_path = data_dir.join("index").join(dir_hash);
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
        }

        // 4. Optionally clear the persisted hypergraph snapshot through the
        // graph-owned cleanup API so graph storage layout stays encapsulated.
        if include_hypergraph {
            let report = rmc_graph::graph::clear_workspace_snapshots(
                &target.canonical,
                GraphSnapshotCleanupOptions {
                    dry_run,
                    data_dir_override: None,
                },
            );
            record_graph_cleanup_report(
                report,
                &mut cleared,
                &mut errors,
            );
        }
    } else {
        let _workspace_lock = workspace_locks.lock_all().await;
        if !dry_run {
            if let Some(sync_manager) = sync_manager {
                sync_manager.untrack_all_directories().await;
            }
            if let Some(search_cache) = search_cache {
                search_cache.invalidate_all();
            }
        }

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

        // All-projects hypergraph wipe goes through graph-owned cleanup so
        // the server does not know where graph snapshots live.
        if include_hypergraph {
            let report = rmc_graph::graph::clear_all_workspace_snapshots(
                GraphSnapshotCleanupOptions {
                    dry_run,
                    data_dir_override: None,
                },
            );
            record_graph_cleanup_report(
                report,
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
    use std::sync::Arc;

    #[test]
    fn test_compute_dir_hash() {
        let hash = dir_hash(std::path::Path::new("/some/test/path"));
        assert_eq!(hash.len(), 64); // SHA256 hex is 64 chars
    }

    #[tokio::test]
    async fn test_clear_cache_nonexistent() {
        // Clearing a non-existent directory should succeed with "nothing to clear"
        let locks = crate::mcp::WorkspaceLockRegistry::new();
        let result = clear_cache(ClearCacheParams {
            directory: Some("/nonexistent/path/that/does/not/exist".to_string()),
            include_hypergraph: None,
            dry_run: None,
        }, None, &locks, None)
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_clear_cache_include_hypergraph_param_compiles() {
        // Smoke: the new flag deserializes via the public schema and
        // reaches the body without panicking. We use a path that doesn't
        // exist so no real state is touched.
        let locks = crate::mcp::WorkspaceLockRegistry::new();
        let result = clear_cache(ClearCacheParams {
            directory: Some("/nonexistent/path/that/does/not/exist".to_string()),
            include_hypergraph: Some(true),
            dry_run: None,
        }, None, &locks, None)
        .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn targeted_clear_untracks_directory() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir(&project_dir).unwrap();
        let sync_manager = Arc::new(crate::mcp::SyncManager::with_defaults(300));
        sync_manager.track_directory(project_dir.join(".")).await;

        let locks = crate::mcp::WorkspaceLockRegistry::new();
        let result = clear_cache(ClearCacheParams {
            directory: Some(project_dir.display().to_string()),
            include_hypergraph: None,
            dry_run: Some(false),
        }, Some(&sync_manager), &locks, None)
        .await;

        assert!(result.is_ok());
        let tracked = sync_manager.get_tracked_directories().await;
        assert!(tracked.is_empty());
    }

    #[tokio::test]
    async fn targeted_clear_dry_run_keeps_directory_tracked() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir(&project_dir).unwrap();
        let canonical = std::fs::canonicalize(&project_dir).unwrap();
        let sync_manager = Arc::new(crate::mcp::SyncManager::with_defaults(300));
        sync_manager.track_directory(project_dir.clone()).await;

        let locks = crate::mcp::WorkspaceLockRegistry::new();
        let result = clear_cache(ClearCacheParams {
            directory: Some(project_dir.display().to_string()),
            include_hypergraph: None,
            dry_run: Some(true),
        }, Some(&sync_manager), &locks, None)
        .await;

        assert!(result.is_ok());
        let tracked = sync_manager.get_tracked_directories().await;
        assert_eq!(tracked, vec![canonical]);
    }

    #[tokio::test]
    async fn targeted_clear_waits_for_workspace_lock() {
        let temp_dir = tempfile::tempdir().unwrap();
        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir(&project_dir).unwrap();
        let locks = crate::mcp::WorkspaceLockRegistry::new();
        let guard = locks.lock_exclusive(&project_dir).await;
        let waiter_locks = locks.clone();
        let waiter_project = project_dir.clone();

        let waiter = tokio::spawn(async move {
            clear_cache(ClearCacheParams {
                directory: Some(waiter_project.display().to_string()),
                include_hypergraph: None,
                dry_run: Some(true),
            }, None, &waiter_locks, None)
            .await
            .unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(!waiter.is_finished());

        drop(guard);
        waiter.await.unwrap();
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

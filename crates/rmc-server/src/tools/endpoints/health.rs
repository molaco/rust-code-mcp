//! Health monitoring tool for production deployments

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool,
};

use crate::mcp::project_paths::{
    ProjectPaths, data_dir, read_embedder_identity, resolve_embedding_backend_for_mcp,
};
use rmc_engine::embeddings::EmbeddingBackend;
use rmc_engine::vector_store::VectorStore;
use rmc_indexing::{
    indexing::open_bm25_search,
    monitoring::{HealthMonitor, Status},
};

/// Share of deleted documents in a project's BM25 index above which the
/// health check warns.
///
/// The merge policy expunges any segment past 30 % deleted
/// (`RMC_BM25_DEL_DOCS_RATIO`), so a whole index above half dead means its
/// merges are not landing — the state a daemon reached when every sync threw
/// its merges away (71 %). Between the two leaves room for the segments that
/// sit just under the policy's threshold.
const BM25_DELETED_WARN_ENV: &str = "RMC_BM25_DELETED_WARN_RATIO";
const DEFAULT_BM25_DELETED_WARN_RATIO: f64 = 0.5;

fn bm25_deleted_warn_ratio() -> Result<f64, McpError> {
    match std::env::var(BM25_DELETED_WARN_ENV) {
        Ok(value) => value.trim().parse().map_err(|e| {
            McpError::invalid_params(format!("{BM25_DELETED_WARN_ENV}={value:?}: {e}"), None)
        }),
        Err(_) => Ok(DEFAULT_BM25_DELETED_WARN_RATIO),
    }
}

/// Health check parameters (optional directory to check specific project)
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct HealthCheckParams {
    #[schemars(
        description = "Optional: project directory to check (checks system-wide if not provided)"
    )]
    pub directory: Option<String>,
    #[schemars(
        description = "Profile the directory was indexed with; default local-cpu-small."
    )]
    #[serde(default)]
    pub embedding_profile: Option<String>,
}

async fn open_vector_store_for_health(
    vector_path: std::path::PathBuf,
    backend: &EmbeddingBackend,
    embedder_identity: &str,
    on_disk_identity: Option<String>,
) -> Option<std::sync::Arc<VectorStore>> {
    // Use the on-disk identity when probing, so we don't trip the
    // VersionMismatch check just to read health info. Fall back to
    // the configured default if the metadata file is absent.
    let probe_identity = on_disk_identity
        .clone()
        .unwrap_or_else(|| embedder_identity.to_string());
    let probe_dim = on_disk_identity
        .as_deref()
        .and_then(|s| EmbeddingBackend::from_identity(s).ok())
        .map(|b| b.dim())
        .unwrap_or_else(|| backend.dim());
    VectorStore::open_existing_embedded(vector_path, probe_dim, &probe_identity)
        .await
        .ok()
        .map(std::sync::Arc::new)
}

/// Check system health status
#[tool(
    description = "Check the health status of the code search system (BM25, Vector store, Merkle tree, index coverage and freshness). Pass 'directory': without it neither coverage nor freshness can be judged."
)]
pub(crate) async fn health_check(
    Parameters(HealthCheckParams {
        directory,
        embedding_profile,
    }): Parameters<HealthCheckParams>,
) -> Result<CallToolResult, McpError> {
    tracing::info!("Performing health check");

    // The BM25 index, vector store, and collection name are all keyed by
    // the embedder identity. The probe must therefore target the SAME
    // profile the directory was indexed with — otherwise it reports an
    // empty "default" index that was never built. Resolve the requested
    // profile (automatic CPU default when unset); further down we still read the
    // on-disk `metadata.json` so the report reflects the real cached
    // identity.
    // A system-wide check (no `directory`) deliberately looks at no project's
    // `embedding_profiles.toml`: the filesystem root has none, so the probe
    // falls back to the built-in default. It used to pass `"."`, which after
    // the daemon stopped sharing a cwd with its clients would have meant
    // "whatever directory the daemon happens to sit in".
    let root = match directory.as_deref() {
        Some(dir) => crate::tools::paths::require_absolute("directory", dir)?,
        None => std::path::PathBuf::from("/"),
    };
    let backend = resolve_embedding_backend_for_mcp(embedding_profile.as_deref(), &root)?;
    let embedder_identity = backend.identity();

    // Determine paths using the same shared helper as index_tool.
    // `coverage_inputs` carries the project's metadata cache and the salt
    // its keys are prefixed with — without them the coverage check has
    // nothing to compare the store against and reports "unknown".
    let (bm25_path, merkle_path, collection_name, coverage_inputs) =
        if let Some(ref dir) = directory {
            let dir_path = std::path::Path::new(dir);
            // The index search reads, not merely the one this identity
            // would name: they differ while a store built under an earlier
            // identity is still the one in use.
            let paths = ProjectPaths::in_use(dir_path, &backend)
                .map_err(|msg| McpError::invalid_params(msg, None))?;

            (
                paths.tantivy_path,
                paths.snapshot_path,
                paths.collection_name,
                Some((paths.cache_path, paths.metadata_cache_salt)),
            )
        } else {
            // System-wide check: can't determine specific snapshot path
            // Merkle snapshots are directory-specific, so this will report as missing
            (
                data_dir().join("index"),
                std::path::PathBuf::from("/nonexistent/merkle.snapshot"), // Sentinel value
                "code_chunks_default".to_string(),
                None,
            )
        };

    // Initialize components (optional)
    let bm25 = open_bm25_search(&bm25_path).ok().map(std::sync::Arc::new);

    // Initialize embedded vector store (LanceDB)
    // Path must match unified.rs: cache_path.parent().join("vectors").join(collection_name)
    let vector_path = data_dir()
        .join("cache")
        .join("vectors")
        .join(&collection_name);

    // On-disk identity, if any: the actual model that wrote this
    // index. May differ from `embedder_identity` (the configured
    // default) when the user picked a variant at index time.
    let on_disk_identity = read_embedder_identity(&vector_path).ok().flatten();

    let vector_store = open_vector_store_for_health(
        vector_path,
        &backend,
        &embedder_identity,
        on_disk_identity.clone(),
    )
    .await;

    // Create health monitor
    let mut monitor = HealthMonitor::new(bm25, vector_store, merkle_path);
    if let Some((cache_path, chunking_identity)) = coverage_inputs {
        monitor = monitor.with_metadata_cache(cache_path, chunking_identity);
    }
    // Freshness needs the working tree itself — the snapshot alone only says
    // what the code looked like at index time, never what it looks like now.
    if let Some(ref dir) = directory {
        monitor = monitor.with_project_root(std::path::PathBuf::from(dir));
    }

    // Run health check
    let health = monitor.check_health().await;

    // Serialize to JSON, then splice in the active embedder identity so
    // operators can confirm which model the cache will be keyed against.
    // Report both the configured default (what a fresh index would use)
    // and the on-disk model (what the existing index was built with).
    let mut health_value =
        serde_json::to_value(&health).map_err(|e| McpError::internal_error(e.to_string(), None))?;
    if let Some(obj) = health_value.as_object_mut() {
        // `embedder` keeps its existing meaning: the currently configured
        // default. `embedder_on_disk` is new and reflects the actual
        // metadata.json next to the LanceDB table when present.
        obj.insert(
            "embedder".to_string(),
            serde_json::Value::String(
                on_disk_identity
                    .clone()
                    .unwrap_or_else(|| embedder_identity.clone()),
            ),
        );
        obj.insert(
            "embedder_configured".to_string(),
            serde_json::Value::String(embedder_identity.clone()),
        );
        if let Some(disk) = on_disk_identity.as_deref() {
            obj.insert(
                "embedder_on_disk".to_string(),
                serde_json::Value::String(disk.to_string()),
            );
        }
    }
    // The keyword index's own shape: how much of it is dead. Reported for a
    // project only — the system-wide probe has no single index to look at.
    let mut bm25_warning = None;
    if directory.is_some() {
        let probe = rmc_indexing::indexing::index_stats::probe_bm25_index(&bm25_path);
        if let Ok(stats) = &probe {
            let threshold = bm25_deleted_warn_ratio()?;
            if let Some(ratio) = stats.deleted_ratio.filter(|ratio| *ratio > threshold) {
                bm25_warning = Some(format!(
                    "BM25 index is {:.0} % deleted documents ({} segments, {:.2} GB on disk); \
                     above {BM25_DELETED_WARN_ENV}={threshold}: merges are not landing",
                    ratio * 100.0,
                    stats.segment_count,
                    stats.total_bytes_on_disk as f64 / 1e9,
                ));
            }
        }
        if let Some(obj) = health_value.as_object_mut() {
            obj.insert(
                "bm25_index".to_string(),
                match probe {
                    Ok(stats) => serde_json::to_value(&stats)
                        .map_err(|e| McpError::internal_error(e.to_string(), None))?,
                    Err(e) => serde_json::json!({ "error": format!("{e:#}") }),
                },
            );
            if let Some(warning) = &bm25_warning {
                obj.insert(
                    "bm25_warning".to_string(),
                    serde_json::Value::String(warning.clone()),
                );
            }
        }
    }

    let status_json = serde_json::to_string_pretty(&health_value)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    // Add interpretation
    let mut response = String::new();

    match health.overall {
        Status::Healthy => {
            response.push_str("✓ System Status: HEALTHY\n\n");
        }
        Status::Degraded => {
            response.push_str(
                "⚠ System Status: DEGRADED (some components unavailable but system functional)\n\n",
            );
        }
        Status::Unhealthy => {
            response.push_str("✗ System Status: UNHEALTHY (critical components failing)\n\n");
        }
    }

    if let Some(warning) = &bm25_warning {
        response.push_str(&format!("⚠ {warning}\n\n"));
    }
    response.push_str(&status_json);
    response.push_str("\n\n=== Health Check Guide ===\n");
    response.push_str("- Healthy: All components operational\n");
    response.push_str("- Degraded: One search engine down OR Merkle snapshot missing\n");
    response.push_str("- Unhealthy: Both BM25 and Vector search are down\n");
    response.push_str("- Coverage: files the indexer will SKIP as unchanged, but for which the store holds no vectors.\n");
    response.push_str("  Non-zero `stale_skips` = index silently incomplete (a run died halfway); fix with index_codebase force_reindex: true.\n");
    response.push_str("- Freshness: does the index still describe the code ON DISK? Compares the snapshot against the\n");
    response.push_str("  working tree by CONTENT, so a rebuild or branch switch that only moves mtimes is not an alarm.\n");
    response.push_str("  Non-zero counts = answers describe old code; fix with a plain index_codebase run.\n");
    response.push_str("\nNote: Merkle snapshots are directory-specific, and both coverage and freshness need a project.\n");
    response.push_str("Without 'directory' this reports on system-wide components only and cannot judge either one.\n");

    if let Some(ref dir) = directory {
        response.push_str(&format!("\nChecked project: {}\n", dir));
    } else {
        response.push_str("\nChecked system-wide components\n");
    }

    Ok(CallToolResult::success(vec![Content::text(response)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn health_vector_probe_does_not_create_missing_path() {
        let temp_dir = TempDir::new().unwrap();
        let vector_path = temp_dir.path().join("missing-vector-store");
        let backend = EmbeddingBackend::default();
        let embedder_identity = backend.identity();

        let vector_store =
            open_vector_store_for_health(vector_path.clone(), &backend, &embedder_identity, None)
                .await;

        assert!(vector_store.is_none());
        assert!(!vector_path.exists());
    }
}

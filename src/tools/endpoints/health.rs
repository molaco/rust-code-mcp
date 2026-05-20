//! Health monitoring tool for production deployments

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool,
};

use crate::embeddings::EmbeddingBackend;
use crate::monitoring::health::HealthMonitor;
use crate::search::Bm25Search;
use crate::tools::project_paths::{ProjectPaths, data_dir, read_embedder_identity};
use crate::vector_store::VectorStore;

/// Health check parameters (optional directory to check specific project)
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct HealthCheckParams {
    #[schemars(description = "Optional: project directory to check (checks system-wide if not provided)")]
    pub directory: Option<String>,
    #[schemars(description = "Optional embedding profile (built-in name or one from embedding_profiles.toml). The BM25 index, vector store, and collection name are all keyed by the embedder identity, so this must match the profile the directory was indexed with. Default: the built-in default profile.")]
    #[serde(default)]
    pub embedding_profile: Option<String>,
}

/// Check system health status
#[tool(description = "Check the health status of the code search system (BM25, Vector store, Merkle tree)")]
pub async fn health_check(
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
    // profile (default when unset); further down we still read the
    // on-disk `metadata.json` so the report reflects the real cached
    // identity.
    let backend = match embedding_profile.as_deref() {
        Some(name) => {
            let root = directory.as_deref().unwrap_or(".");
            let profile = crate::embeddings::resolve_profile(name, std::path::Path::new(root))
                .map_err(|msg| McpError::invalid_params(msg, None))?;
            EmbeddingBackend::from_profile(profile)
        }
        None => EmbeddingBackend::default(),
    };
    let embedder_identity = backend.identity();

    // Determine paths using the same shared helper as index_tool.
    let (bm25_path, merkle_path, collection_name) = if let Some(ref dir) = directory {
        let dir_path = std::path::Path::new(dir);
        let paths = ProjectPaths::from_directory(dir_path, &backend);

        (
            paths.tantivy_path,
            paths.snapshot_path,
            paths.collection_name,
        )
    } else {
        // System-wide check: can't determine specific snapshot path
        // Merkle snapshots are directory-specific, so this will report as missing
        (
            data_dir().join("index"),
            std::path::PathBuf::from("/nonexistent/merkle.snapshot"),  // Sentinel value
            "code_chunks_default".to_string(),
        )
    };

    // Initialize components (optional)
    let bm25 = Bm25Search::new(&bm25_path).ok().map(std::sync::Arc::new);

    // Initialize embedded vector store (LanceDB)
    // Path must match unified.rs: cache_path.parent().join("vectors").join(collection_name)
    let vector_path = data_dir().join("cache").join("vectors").join(&collection_name);

    // On-disk identity, if any: the actual model that wrote this
    // index. May differ from `embedder_identity` (the configured
    // default) when the user picked a variant at index time.
    let on_disk_identity = read_embedder_identity(&vector_path).ok().flatten();

    let vector_store = {
        // Use the on-disk identity when probing, so we don't trip the
        // VersionMismatch check just to read health info. Fall back to
        // the configured default if the metadata file is absent.
        let probe_identity = on_disk_identity
            .clone()
            .unwrap_or_else(|| embedder_identity.clone());
        let probe_dim = on_disk_identity
            .as_deref()
            .and_then(|s| EmbeddingBackend::from_identity(s).ok())
            .map(|b| b.dim())
            .unwrap_or_else(|| backend.dim());
        VectorStore::new_embedded(vector_path, probe_dim, &probe_identity)
            .await
            .ok()
            .map(std::sync::Arc::new)
    };

    // Create health monitor
    let monitor = HealthMonitor::new(bm25, vector_store, merkle_path);

    // Run health check
    let health = monitor.check_health().await;

    // Serialize to JSON, then splice in the active embedder identity so
    // operators can confirm which model the cache will be keyed against.
    // Report both the configured default (what a fresh index would use)
    // and the on-disk model (what the existing index was built with).
    let mut health_value = serde_json::to_value(&health)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
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
    let status_json = serde_json::to_string_pretty(&health_value)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    // Add interpretation
    let mut response = String::new();

    match health.overall {
        crate::monitoring::health::Status::Healthy => {
            response.push_str("✓ System Status: HEALTHY\n\n");
        }
        crate::monitoring::health::Status::Degraded => {
            response.push_str("⚠ System Status: DEGRADED (some components unavailable but system functional)\n\n");
        }
        crate::monitoring::health::Status::Unhealthy => {
            response.push_str("✗ System Status: UNHEALTHY (critical components failing)\n\n");
        }
    }

    response.push_str(&status_json);
    response.push_str("\n\n=== Health Check Guide ===\n");
    response.push_str("- Healthy: All components operational\n");
    response.push_str("- Degraded: One search engine down OR Merkle snapshot missing\n");
    response.push_str("- Unhealthy: Both BM25 and Vector search are down\n");
    response.push_str("\nNote: Merkle snapshots are directory-specific. Use 'directory' parameter for accurate check.\n");

    if let Some(ref dir) = directory {
        response.push_str(&format!("\nChecked project: {}\n", dir));
    } else {
        response.push_str("\nChecked system-wide components\n");
    }

    Ok(CallToolResult::success(vec![Content::text(response)]))
}

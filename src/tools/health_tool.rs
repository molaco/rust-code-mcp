//! Health monitoring tool for production deployments

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool,
};
use std::path::PathBuf;
use directories::ProjectDirs;

use crate::embeddings::EmbeddingBackend;
use crate::monitoring::health::HealthMonitor;
use crate::search::Bm25Search;
use crate::vector_store::VectorStore;
use crate::indexing::incremental::get_snapshot_path;
use sha2::{Digest, Sha256};

/// Health check parameters (optional directory to check specific project)
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct HealthCheckParams {
    #[schemars(description = "Optional: project directory to check (checks system-wide if not provided)")]
    pub directory: Option<String>,
}

/// Get the path for storing persistent index and cache
fn data_dir() -> PathBuf {
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
}

/// Check system health status
#[tool(description = "Check the health status of the code search system (BM25, Vector store, Merkle tree)")]
pub async fn health_check(
    Parameters(HealthCheckParams { directory }): Parameters<HealthCheckParams>,
) -> Result<CallToolResult, McpError> {
    tracing::info!("Performing health check");

    // The health probe shows the configured embedder so users can see at
    // a glance which model the cache will be tied to.
    let backend = EmbeddingBackend::default();
    let embedder_identity = backend.identity();

    // Determine paths using hash-based approach (consistent with index_tool)
    let (bm25_path, merkle_path, collection_name) = if let Some(ref dir) = directory {
        let dir_path = std::path::Path::new(dir);

        // Calculate directory hash + model fingerprint (must match
        // `ProjectPaths::from_directory` so the health probe targets the
        // same vector directory the indexer would write to).
        let dir_hash = {
            let mut hasher = Sha256::new();
            hasher.update(dir_path.to_string_lossy().as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let model_fp = {
            let mut hasher = Sha256::new();
            hasher.update(embedder_identity.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let collection_name = format!("code_chunks_{}_{}", &dir_hash[..8], &model_fp[..8]);

        (
            data_dir().join("index").join(&dir_hash),  // Full hash, matching index_tool
            get_snapshot_path(dir_path),
            collection_name,
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
    let vector_store = {
        let vector_path = data_dir().join("cache").join("vectors").join(&collection_name);
        VectorStore::new_embedded(vector_path, backend.dim(), &embedder_identity)
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
    let mut health_value = serde_json::to_value(&health)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    if let Some(obj) = health_value.as_object_mut() {
        obj.insert(
            "embedder".to_string(),
            serde_json::Value::String(embedder_identity.clone()),
        );
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

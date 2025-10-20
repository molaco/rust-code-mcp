//! Health monitoring tool for production deployments

use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    schemars, tool,
};
use std::path::PathBuf;
use directories::ProjectDirs;

use crate::monitoring::health::HealthMonitor;
use crate::search::Bm25Search;
use crate::vector_store::{VectorStore, VectorStoreConfig};

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

    // Determine paths
    let (bm25_path, merkle_path, collection_name) = if let Some(ref dir) = directory {
        let project_name = std::path::Path::new(dir)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("default")
            .replace(|c: char| !c.is_alphanumeric(), "_");

        (
            data_dir().join(format!("index_{}", project_name)),
            data_dir().join(format!("cache_{}/merkle.snapshot", project_name)),
            format!("code_chunks_{}", project_name),
        )
    } else {
        (
            data_dir().join("index"),
            data_dir().join("cache/merkle.snapshot"),
            "code_chunks_default".to_string(),
        )
    };

    // Initialize components (optional)
    let bm25 = Bm25Search::new(&bm25_path).ok().map(std::sync::Arc::new);

    let qdrant_url = std::env::var("QDRANT_URL")
        .unwrap_or_else(|_| "http://localhost:6334".to_string());

    let vector_store = VectorStore::new(VectorStoreConfig {
        url: qdrant_url,
        collection_name,
        vector_size: 384,
    })
    .await
    .ok()
    .map(std::sync::Arc::new);

    // Create health monitor
    let monitor = HealthMonitor::new(bm25, vector_store, merkle_path);

    // Run health check
    let health = monitor.check_health().await;

    // Serialize to JSON
    let status_json = serde_json::to_string_pretty(&health)
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

    if let Some(ref dir) = directory {
        response.push_str(&format!("\nChecked project: {}\n", dir));
    } else {
        response.push_str("\nChecked system-wide components\n");
    }

    Ok(CallToolResult::success(vec![Content::text(response)]))
}

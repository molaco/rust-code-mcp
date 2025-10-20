//! Health monitoring for production deployments
//!
//! Provides component-level health checks for:
//! - BM25 search (Tantivy)
//! - Vector search (Qdrant)
//! - Merkle tree snapshots
//!
//! Health states: Healthy, Degraded, Unhealthy

use crate::search::Bm25Search;
use crate::vector_store::VectorStore;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Overall system health status
#[derive(Debug, Clone, Serialize)]
pub struct HealthStatus {
    /// Overall system status
    pub overall: Status,
    /// BM25 search component health
    pub bm25: ComponentHealth,
    /// Vector search component health
    pub vector: ComponentHealth,
    /// Merkle tree component health
    pub merkle: ComponentHealth,
}

/// Health status levels
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// All systems operational
    Healthy,
    /// Some systems degraded but functional
    Degraded,
    /// Critical systems failing
    Unhealthy,
}

/// Individual component health
#[derive(Debug, Clone, Serialize)]
pub struct ComponentHealth {
    /// Component status
    pub status: Status,
    /// Status message
    pub message: String,
    /// Optional latency measurement in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

impl ComponentHealth {
    /// Create healthy component status
    pub fn healthy(message: impl Into<String>, latency_ms: Option<u64>) -> Self {
        Self {
            status: Status::Healthy,
            message: message.into(),
            latency_ms,
        }
    }

    /// Create degraded component status
    pub fn degraded(message: impl Into<String>) -> Self {
        Self {
            status: Status::Degraded,
            message: message.into(),
            latency_ms: None,
        }
    }

    /// Create unhealthy component status
    pub fn unhealthy(message: impl Into<String>) -> Self {
        Self {
            status: Status::Unhealthy,
            message: message.into(),
            latency_ms: None,
        }
    }
}

/// Health monitor for the search system
pub struct HealthMonitor {
    bm25: Option<Arc<Bm25Search>>,
    vector_store: Option<Arc<VectorStore>>,
    merkle_path: PathBuf,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new(
        bm25: Option<Arc<Bm25Search>>,
        vector_store: Option<Arc<VectorStore>>,
        merkle_path: PathBuf,
    ) -> Self {
        Self {
            bm25,
            vector_store,
            merkle_path,
        }
    }

    /// Perform comprehensive health check
    pub async fn check_health(&self) -> HealthStatus {
        // Run all checks in parallel
        let (bm25_health, vector_health, merkle_health) = tokio::join!(
            self.check_bm25(),
            self.check_vector(),
            self.check_merkle()
        );

        // Determine overall status
        let overall = self.calculate_overall_status(&bm25_health, &vector_health, &merkle_health);

        HealthStatus {
            overall,
            bm25: bm25_health,
            vector: vector_health,
            merkle: merkle_health,
        }
    }

    /// Check BM25 search health
    async fn check_bm25(&self) -> ComponentHealth {
        let Some(bm25) = &self.bm25 else {
            return ComponentHealth::degraded("BM25 search not configured");
        };

        let start = Instant::now();

        // Try a simple test query (BM25 search is synchronous)
        match bm25.search("__health_check__", 1) {
            Ok(_) => {
                let latency = start.elapsed().as_millis() as u64;
                ComponentHealth::healthy("BM25 search operational", Some(latency))
            }
            Err(e) => ComponentHealth::unhealthy(format!("BM25 search error: {:?}", e)),
        }
    }

    /// Check vector search health
    async fn check_vector(&self) -> ComponentHealth {
        let Some(vector_store) = &self.vector_store else {
            return ComponentHealth::degraded("Vector store not configured");
        };

        let start = Instant::now();

        // Check collection exists and is accessible
        match vector_store.count().await {
            Ok(count) => {
                let latency = start.elapsed().as_millis() as u64;
                ComponentHealth::healthy(
                    format!("Vector store operational ({} vectors)", count),
                    Some(latency),
                )
            }
            Err(e) => ComponentHealth::unhealthy(format!("Vector store error: {}", e)),
        }
    }

    /// Check Merkle tree snapshot health
    async fn check_merkle(&self) -> ComponentHealth {
        if self.merkle_path.exists() {
            match std::fs::metadata(&self.merkle_path) {
                Ok(metadata) => {
                    let size_bytes = metadata.len();
                    ComponentHealth::healthy(
                        format!("Merkle snapshot exists ({} bytes)", size_bytes),
                        None,
                    )
                }
                Err(e) => ComponentHealth::degraded(format!(
                    "Merkle snapshot exists but unreadable: {}",
                    e
                )),
            }
        } else {
            ComponentHealth::degraded("Merkle snapshot not found (first index pending)")
        }
    }

    /// Calculate overall system status from component statuses
    fn calculate_overall_status(
        &self,
        bm25: &ComponentHealth,
        vector: &ComponentHealth,
        merkle: &ComponentHealth,
    ) -> Status {
        // Critical: both search engines must work
        let search_unhealthy =
            bm25.status == Status::Unhealthy && vector.status == Status::Unhealthy;

        if search_unhealthy {
            return Status::Unhealthy;
        }

        // Degraded: one search engine down OR merkle issues
        let has_degraded = bm25.status == Status::Degraded
            || vector.status == Status::Degraded
            || merkle.status == Status::Degraded
            || bm25.status == Status::Unhealthy
            || vector.status == Status::Unhealthy;

        if has_degraded {
            return Status::Degraded;
        }

        // All healthy
        Status::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_component_health_constructors() {
        let healthy = ComponentHealth::healthy("All good", Some(50));
        assert_eq!(healthy.status, Status::Healthy);
        assert_eq!(healthy.latency_ms, Some(50));

        let degraded = ComponentHealth::degraded("Some issues");
        assert_eq!(degraded.status, Status::Degraded);
        assert_eq!(degraded.latency_ms, None);

        let unhealthy = ComponentHealth::unhealthy("Critical error");
        assert_eq!(unhealthy.status, Status::Unhealthy);
    }

    #[test]
    fn test_overall_status_calculation() {
        let monitor = HealthMonitor {
            bm25: None,
            vector_store: None,
            merkle_path: PathBuf::from("/tmp/merkle.snapshot"),
        };

        // All healthy
        let all_healthy = monitor.calculate_overall_status(
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
        );
        assert_eq!(all_healthy, Status::Healthy);

        // One degraded
        let one_degraded = monitor.calculate_overall_status(
            &ComponentHealth::degraded("issues"),
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
        );
        assert_eq!(one_degraded, Status::Degraded);

        // Both search engines down
        let both_down = monitor.calculate_overall_status(
            &ComponentHealth::unhealthy("down"),
            &ComponentHealth::unhealthy("down"),
            &ComponentHealth::healthy("ok", None),
        );
        assert_eq!(both_down, Status::Unhealthy);

        // One search engine down (still degraded, not unhealthy)
        let one_down = monitor.calculate_overall_status(
            &ComponentHealth::unhealthy("down"),
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
        );
        assert_eq!(one_down, Status::Degraded);
    }

    #[test]
    fn test_health_status_serialization() {
        let status = HealthStatus {
            overall: Status::Healthy,
            bm25: ComponentHealth::healthy("BM25 operational", Some(15)),
            vector: ComponentHealth::healthy("Vector operational", Some(42)),
            merkle: ComponentHealth::healthy("Merkle snapshot exists (2048 bytes)", None),
        };

        let json = serde_json::to_string_pretty(&status).unwrap();
        assert!(json.contains("\"overall\": \"healthy\""));
        assert!(json.contains("\"latency_ms\": 15"));
        assert!(json.contains("\"latency_ms\": 42"));
    }
}

//! Bulk indexing optimizations for Qdrant
//!
//! When indexing large numbers of vectors at once, temporarily disabling HNSW
//! indexing and re-enabling it after the bulk operation can provide 3-5x speedup.

use anyhow::Result;
use qdrant_client::qdrant::{HnswConfigDiff, OptimizersConfigDiff, UpdateCollectionBuilder};
use qdrant_client::Qdrant as QdrantClient;
use tracing;

/// Manages bulk indexing mode for Qdrant collections
pub struct BulkIndexer {
    client: QdrantClient,
    collection_name: String,
    original_hnsw_config: Option<HnswConfig>,
}

/// Saved HNSW configuration to restore after bulk indexing
#[derive(Debug, Clone)]
pub struct HnswConfig {
    pub m: usize,
    pub ef_construct: usize,
}

impl BulkIndexer {
    /// Create a new bulk indexer
    pub fn new(client: QdrantClient, collection_name: String) -> Self {
        Self {
            client,
            collection_name,
            original_hnsw_config: None,
        }
    }

    /// Enter bulk indexing mode
    ///
    /// This disables HNSW graph construction and defers indexing optimization,
    /// allowing much faster insertion of large batches of vectors.
    ///
    /// **IMPORTANT**: Always call `end_bulk_mode()` after bulk operations complete
    /// to restore normal indexing behavior.
    pub async fn start_bulk_mode(&mut self, save_config: HnswConfig) -> Result<()> {
        tracing::info!(
            "⚡ Entering bulk indexing mode for collection '{}'",
            self.collection_name
        );

        // Save original config for restoration
        self.original_hnsw_config = Some(save_config);

        // Minimize HNSW and defer optimization
        // Note: Qdrant requires ef_construct >= 4, so we use minimal values
        let update = UpdateCollectionBuilder::new(&self.collection_name)
            .hnsw_config(HnswConfigDiff {
                m: Some(4), // Minimal HNSW (Qdrant minimum)
                ef_construct: Some(4), // Minimal ef_construct (Qdrant minimum)
                full_scan_threshold: Some(0),
                max_indexing_threads: Some(0),
                on_disk: None,
                payload_m: None,
            })
            .optimizers_config(OptimizersConfigDiff {
                indexing_threshold: Some(0), // Defer all indexing optimization
                ..Default::default()
            });

        self.client
            .update_collection(update)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to enter bulk mode: {}", e))?;

        tracing::info!("✓ Bulk mode active - HNSW disabled, optimizations deferred");

        Ok(())
    }

    /// Exit bulk indexing mode
    ///
    /// Restores HNSW configuration and triggers index optimization.
    /// The HNSW graph will be rebuilt from scratch, which may take time
    /// depending on the number of vectors.
    pub async fn end_bulk_mode(&mut self) -> Result<()> {
        let config = self
            .original_hnsw_config
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Bulk mode was never started"))?;

        tracing::info!(
            "🔄 Exiting bulk mode, rebuilding HNSW index for '{}'...",
            self.collection_name
        );
        tracing::info!(
            "   Restoring configuration: m={}, ef_construct={}",
            config.m,
            config.ef_construct
        );

        // Restore HNSW configuration
        let update = UpdateCollectionBuilder::new(&self.collection_name)
            .hnsw_config(HnswConfigDiff {
                m: Some(config.m as u64),
                ef_construct: Some(config.ef_construct as u64),
                full_scan_threshold: Some(10_000),
                max_indexing_threads: Some(0), // Let Qdrant auto-determine
                on_disk: Some(false),
                payload_m: None,
            })
            .optimizers_config(OptimizersConfigDiff {
                indexing_threshold: Some(10_000), // Resume normal indexing
                ..Default::default()
            });

        self.client
            .update_collection(update)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to exit bulk mode: {}", e))?;

        tracing::info!("✓ HNSW indexing restored - collection ready for queries");

        // Clear saved config
        self.original_hnsw_config = None;

        Ok(())
    }

    /// Check if currently in bulk mode
    pub fn is_bulk_mode_active(&self) -> bool {
        self.original_hnsw_config.is_some()
    }

    /// Get the collection name
    pub fn collection_name(&self) -> &str {
        &self.collection_name
    }
}

impl HnswConfig {
    /// Create from m and ef_construct values
    pub fn new(m: usize, ef_construct: usize) -> Self {
        Self { m, ef_construct }
    }
}

/// Helper function to perform a bulk indexing operation with automatic mode management
///
/// # Example
/// ```no_run
/// use rust_code_mcp::indexing::bulk::{bulk_index_with_auto_mode, HnswConfig};
/// use qdrant_client::Qdrant as QdrantClient;
///
/// async fn index_large_batch() -> anyhow::Result<()> {
///     let client = QdrantClient::from_url("http://localhost:6333").build()?;
///     let collection = "my_collection".to_string();
///     let hnsw_config = HnswConfig::new(16, 100);
///
///     bulk_index_with_auto_mode(
///         client,
///         collection,
///         hnsw_config,
///         |vector_store| async move {
///             // Your bulk indexing operations here
///             // vector_store.upsert_chunks(...).await?;
///             Ok(())
///         }
///     ).await
/// }
/// ```
pub async fn bulk_index_with_auto_mode<F, Fut>(
    client: QdrantClient,
    collection_name: String,
    hnsw_config: HnswConfig,
    operation: F,
) -> Result<()>
where
    F: FnOnce(QdrantClient) -> Fut,
    Fut: std::future::Future<Output = Result<()>>,
{
    let mut bulk_indexer = BulkIndexer::new(client.clone(), collection_name);

    // Enter bulk mode
    bulk_indexer.start_bulk_mode(hnsw_config).await?;

    // Perform the operation
    let result = operation(client).await;

    // Always exit bulk mode, even if operation failed
    let exit_result = bulk_indexer.end_bulk_mode().await;

    // Propagate errors
    result?;
    exit_result?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hnsw_config_creation() {
        let config = HnswConfig::new(16, 100);
        assert_eq!(config.m, 16);
        assert_eq!(config.ef_construct, 100);
    }

    #[test]
    fn test_bulk_indexer_state() {
        let client = QdrantClient::from_url("http://localhost:6333")
            .build()
            .unwrap();
        let bulk_indexer = BulkIndexer::new(client, "test".to_string());

        assert!(!bulk_indexer.is_bulk_mode_active());
        assert_eq!(bulk_indexer.collection_name(), "test");
    }

    #[tokio::test]
    #[ignore] // Requires running Qdrant server
    async fn test_bulk_mode_lifecycle() {
        let client = QdrantClient::from_url("http://localhost:6333")
            .build()
            .unwrap();

        let mut bulk_indexer = BulkIndexer::new(client, "test_bulk".to_string());

        // Start bulk mode
        let config = HnswConfig::new(16, 100);
        let start_result = bulk_indexer.start_bulk_mode(config).await;
        assert!(start_result.is_ok(), "Failed to start bulk mode: {:?}", start_result.err());
        assert!(bulk_indexer.is_bulk_mode_active());

        // End bulk mode
        let end_result = bulk_indexer.end_bulk_mode().await;
        assert!(end_result.is_ok(), "Failed to end bulk mode: {:?}", end_result.err());
        assert!(!bulk_indexer.is_bulk_mode_active());
    }
}

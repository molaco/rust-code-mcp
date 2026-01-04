//! Optimized Qdrant configuration based on codebase size
//!
//! Automatically tunes HNSW parameters for optimal performance
//!
//! This module is only available when the `qdrant` feature is enabled.

#![cfg(feature = "qdrant")]

use super::qdrant::QdrantConfig;
use qdrant_client::qdrant::{HnswConfigDiff, OptimizersConfigDiff, UpdateCollectionBuilder};
use qdrant_client::Qdrant as QdrantClient;

/// Optimized Qdrant configuration for different codebase sizes
#[derive(Debug, Clone)]
pub struct QdrantOptimizedConfig {
    pub base_config: QdrantConfig,
    pub hnsw_m: usize,
    pub hnsw_ef_construct: usize,
    pub hnsw_ef: usize,
    pub indexing_threads: usize,
    pub full_scan_threshold: usize,
    pub memmap_threshold: usize,
}

impl QdrantOptimizedConfig {
    /// Auto-configure based on estimated lines of code
    pub fn for_codebase_size(estimated_loc: usize, base_config: QdrantConfig) -> Self {
        if estimated_loc < 100_000 {
            // Small codebase (< 100k LOC)
            // ~1-3k chunks, optimize for memory efficiency
            Self {
                base_config,
                hnsw_m: 16,                    // Moderate connectivity
                hnsw_ef_construct: 100,        // Faster construction
                hnsw_ef: 128,                  // Good search quality
                indexing_threads: 8,           // Moderate parallelism
                full_scan_threshold: 10_000,   // Rarely trigger full scan
                memmap_threshold: 50_000,      // Keep in RAM
            }
        } else if estimated_loc < 1_000_000 {
            // Medium codebase (100k-1M LOC)
            // ~3k-30k chunks, balance quality and speed
            Self {
                base_config,
                hnsw_m: 16,                    // Standard connectivity
                hnsw_ef_construct: 150,        // Better graph quality
                hnsw_ef: 128,                  // Good search quality
                indexing_threads: 12,          // More parallelism
                full_scan_threshold: 20_000,   // Higher threshold
                memmap_threshold: 50_000,      // Memory-map when large
            }
        } else {
            // Large codebase (> 1M LOC)
            // 30k+ chunks, optimize for recall
            Self {
                base_config,
                hnsw_m: 32,                    // Higher connectivity for better recall
                hnsw_ef_construct: 200,        // High-quality graph construction
                hnsw_ef: 256,                  // Maximum search quality
                indexing_threads: 16,          // Maximum parallelism
                full_scan_threshold: 50_000,   // Avoid full scans
                memmap_threshold: 30_000,      // Memory-map aggressively
            }
        }
    }

    /// Apply configuration to an existing collection
    pub async fn apply_to_collection(
        &self,
        client: &QdrantClient,
        collection_name: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let update = UpdateCollectionBuilder::new(collection_name)
            .hnsw_config(HnswConfigDiff {
                m: Some(self.hnsw_m as u64),
                ef_construct: Some(self.hnsw_ef_construct as u64),
                full_scan_threshold: Some(self.full_scan_threshold as u64),
                max_indexing_threads: Some(self.indexing_threads as u64),
                on_disk: Some(false), // Keep in memory for performance
                payload_m: None,
            })
            .optimizers_config(OptimizersConfigDiff {
                memmap_threshold: Some(self.memmap_threshold as u64),
                indexing_threshold: Some(10_000), // Start indexing after 10k points
                flush_interval_sec: Some(5),      // Flush every 5 seconds
                max_optimization_threads: None,  // Let Qdrant auto-determine
                ..Default::default()
            });

        client
            .update_collection(update)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?;

        tracing::info!(
            "Applied Qdrant optimization for collection '{}': m={}, ef_construct={}, ef={}, threads={}",
            collection_name,
            self.hnsw_m,
            self.hnsw_ef_construct,
            self.hnsw_ef,
            self.indexing_threads
        );

        Ok(())
    }

    /// Get HNSW config for collection creation
    pub fn hnsw_config(&self) -> HnswConfigDiff {
        HnswConfigDiff {
            m: Some(self.hnsw_m as u64),
            ef_construct: Some(self.hnsw_ef_construct as u64),
            full_scan_threshold: Some(self.full_scan_threshold as u64),
            max_indexing_threads: Some(self.indexing_threads as u64),
            on_disk: Some(false),
            payload_m: None,
        }
    }

    /// Get optimizer config for collection creation
    pub fn optimizer_config(&self) -> OptimizersConfigDiff {
        OptimizersConfigDiff {
            deleted_threshold: Some(0.2),
            vacuum_min_vector_number: Some(1000),
            default_segment_number: Some(0),
            max_segment_size: None,
            memmap_threshold: Some(self.memmap_threshold as u64),
            indexing_threshold: Some(10_000),
            flush_interval_sec: Some(5),
            max_optimization_threads: None,  // Let Qdrant auto-determine
            deprecated_max_optimization_threads: None,
        }
    }
}

/// Estimate lines of code for a directory
pub fn estimate_codebase_size(directory: &std::path::Path) -> Result<usize, std::io::Error> {
    use walkdir::WalkDir;

    let mut total_lines = 0;

    for entry in WalkDir::new(directory)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|ext| ext == "rs")
                .unwrap_or(false)
        })
    {
        if let Ok(content) = std::fs::read_to_string(entry.path()) {
            total_lines += content.lines().count();
        }
    }

    tracing::debug!("Estimated codebase size: {} LOC", total_lines);

    Ok(total_lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_small_codebase_config() {
        let config = QdrantConfig::default();
        let optimized = QdrantOptimizedConfig::for_codebase_size(50_000, config);

        assert_eq!(optimized.hnsw_m, 16);
        assert_eq!(optimized.hnsw_ef_construct, 100);
        assert_eq!(optimized.hnsw_ef, 128);
        assert_eq!(optimized.indexing_threads, 8);
    }

    #[test]
    fn test_medium_codebase_config() {
        let config = QdrantConfig::default();
        let optimized = QdrantOptimizedConfig::for_codebase_size(500_000, config);

        assert_eq!(optimized.hnsw_m, 16);
        assert_eq!(optimized.hnsw_ef_construct, 150);
        assert_eq!(optimized.hnsw_ef, 128);
        assert_eq!(optimized.indexing_threads, 12);
    }

    #[test]
    fn test_large_codebase_config() {
        let config = QdrantConfig::default();
        let optimized = QdrantOptimizedConfig::for_codebase_size(2_000_000, config);

        assert_eq!(optimized.hnsw_m, 32);
        assert_eq!(optimized.hnsw_ef_construct, 200);
        assert_eq!(optimized.hnsw_ef, 256);
        assert_eq!(optimized.indexing_threads, 16);
    }

    #[test]
    fn test_boundary_conditions() {
        let config = QdrantConfig::default();

        // Test at exact boundaries
        let at_100k = QdrantOptimizedConfig::for_codebase_size(100_000, config.clone());
        assert_eq!(at_100k.hnsw_m, 16);
        assert_eq!(at_100k.hnsw_ef_construct, 150);

        let at_1m = QdrantOptimizedConfig::for_codebase_size(1_000_000, config.clone());
        assert_eq!(at_1m.hnsw_m, 32);
        assert_eq!(at_1m.hnsw_ef_construct, 200);
    }
}

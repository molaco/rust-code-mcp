//! Consolidated indexer configuration
//!
//! Provides a unified configuration interface for all indexing components,
//! reducing coupling by consolidating config structs from multiple modules.
//!
//! ## Overview
//!
//! The configuration module addresses Phase 3 refactoring goals:
//! - **Reduced coupling**: Single config object replaces multiple imports
//! - **Size-based optimization**: Auto-tune settings based on codebase LOC
//! - **Centralized tuning**: One place to adjust performance parameters
//!
//! ## Configuration Hierarchy
//!
//! ```text
//! IndexerConfig (unified config)
//!     ├─ IndexerCoreConfig (file processing)
//!     ├─ TantivyConfig (BM25 indexing)
//!     └─ QdrantConfig (vector search)
//! ```
//!
//! ## Automatic Size-Based Tuning
//!
//! The `for_codebase_size()` constructor automatically optimizes settings:
//!
//! | Codebase Size | Tantivy Threads | Memory (MB) | HNSW M | Batch Size |
//! |---------------|----------------|-------------|---------|------------|
//! | < 100K LOC    | 2              | 50          | 16      | 96         |
//! | 100K - 1M LOC | 4              | 100         | 16      | 96         |
//! | > 1M LOC      | 8              | 200         | 32      | 128        |
//!
//! ## Refactoring Notes
//!
//! This module was created during Phase 3 refactoring:
//! - Consolidates scattered configuration structs
//! - Reduces import coupling (19 imports → 12 in unified.rs)
//! - Enables dependency injection pattern
//!
//! ## Examples
//!
//! ### Auto-Tuned Configuration
//! ```rust
//! use file_search_mcp::config::IndexerConfig;
//! use std::path::Path;
//!
//! // Automatically optimize for 500K LOC codebase (medium)
//! let config = IndexerConfig::for_codebase_size(
//!     500_000,  // Lines of code
//!     Path::new("./cache"),
//!     Path::new("./tantivy"),
//!     "http://localhost:6334",
//!     "my_project",
//!     384  // Vector dimensions
//! );
//!
//! // Config is now optimized for medium codebase:
//! assert_eq!(config.tantivy.num_threads, 4);
//! assert_eq!(config.tantivy.memory_budget_mb, 100);
//! ```
//!
//! ### Manual Configuration
//! ```rust
//! use file_search_mcp::config::{IndexerConfig, TantivyConfig, QdrantConfig};
//! use std::path::Path;
//!
//! // Create with defaults
//! let config = IndexerConfig::default(
//!     Path::new("./cache"),
//!     Path::new("./tantivy"),
//!     "http://localhost:6334",
//!     "my_collection",
//!     384
//! );
//!
//! // Or create individual configs
//! let tantivy = TantivyConfig::for_codebase_size(
//!     Path::new("./tantivy"),
//!     Some(200_000)
//! );
//! ```

use std::path::{Path, PathBuf};

/// Unified indexer configuration
///
/// This struct consolidates configuration from:
/// - IndexerCore (file processing settings)
/// - TantivyAdapter (BM25 indexing settings)
/// - QdrantAdapter (vector store settings)
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    /// Core processing settings
    pub core: IndexerCoreConfig,
    /// Tantivy BM25 settings
    pub tantivy: TantivyConfig,
    /// Qdrant vector store settings
    pub qdrant: QdrantConfig,
}

impl IndexerConfig {
    /// Create configuration optimized for codebase size
    ///
    /// Automatically adjusts settings based on estimated lines of code:
    /// - Small: < 100k LOC
    /// - Medium: 100k - 1M LOC
    /// - Large: > 1M LOC
    pub fn for_codebase_size(
        codebase_loc: usize,
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
        vector_size: usize,
    ) -> Self {
        let (
            max_file_size,
            gpu_batch_size,
            tantivy_memory_mb,
            tantivy_threads,
            qdrant_hnsw_m,
            qdrant_hnsw_ef_construct,
            qdrant_hnsw_ef,
            qdrant_indexing_threads,
        ) = if codebase_loc < 100_000 {
            // Small codebase
            (10_000_000, 96, 50, 2, 16, 100, 128, 8)
        } else if codebase_loc < 1_000_000 {
            // Medium codebase
            (10_000_000, 96, 100, 4, 16, 150, 128, 12)
        } else {
            // Large codebase
            (15_000_000, 128, 200, 8, 32, 200, 256, 16)
        };

        Self {
            core: IndexerCoreConfig {
                cache_path: cache_path.to_path_buf(),
                max_file_size,
                gpu_batch_size,
            },
            tantivy: TantivyConfig {
                index_path: tantivy_path.to_path_buf(),
                memory_budget_mb: tantivy_memory_mb,
                num_threads: tantivy_threads,
            },
            qdrant: QdrantConfig {
                url: qdrant_url.to_string(),
                collection_name: collection_name.to_string(),
                vector_size,
                hnsw_m: qdrant_hnsw_m,
                hnsw_ef_construct: qdrant_hnsw_ef_construct,
                hnsw_ef: qdrant_hnsw_ef,
                indexing_threads: qdrant_indexing_threads,
                full_scan_threshold: if codebase_loc < 100_000 {
                    10_000
                } else if codebase_loc < 1_000_000 {
                    20_000
                } else {
                    50_000
                },
                memmap_threshold: if codebase_loc < 100_000 {
                    50_000
                } else if codebase_loc < 1_000_000 {
                    50_000
                } else {
                    30_000
                },
            },
        }
    }

    /// Create default configuration
    pub fn default(
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
        vector_size: usize,
    ) -> Self {
        Self {
            core: IndexerCoreConfig {
                cache_path: cache_path.to_path_buf(),
                max_file_size: 10_000_000,
                gpu_batch_size: 96,
            },
            tantivy: TantivyConfig {
                index_path: tantivy_path.to_path_buf(),
                memory_budget_mb: 50,
                num_threads: 2,
            },
            qdrant: QdrantConfig {
                url: qdrant_url.to_string(),
                collection_name: collection_name.to_string(),
                vector_size,
                hnsw_m: 16,
                hnsw_ef_construct: 100,
                hnsw_ef: 128,
                indexing_threads: 8,
                full_scan_threshold: 10_000,
                memmap_threshold: 50_000,
            },
        }
    }
}

/// Core indexing configuration
#[derive(Debug, Clone)]
pub struct IndexerCoreConfig {
    /// Path to metadata cache directory
    pub cache_path: PathBuf,
    /// Maximum file size to process (in bytes)
    pub max_file_size: u64,
    /// GPU batch size for embedding generation
    pub gpu_batch_size: usize,
}

impl Default for IndexerCoreConfig {
    fn default() -> Self {
        Self {
            cache_path: PathBuf::from("./cache"),
            max_file_size: 10_000_000, // 10 MB
            gpu_batch_size: 96,        // Optimized for 8GB VRAM
        }
    }
}

/// Tantivy BM25 indexing configuration
#[derive(Debug, Clone)]
pub struct TantivyConfig {
    /// Path to Tantivy index directory
    pub index_path: PathBuf,
    /// Memory budget in MB per thread
    pub memory_budget_mb: usize,
    /// Number of threads for indexing
    pub num_threads: usize,
}

impl TantivyConfig {
    /// Create configuration optimized for codebase size
    pub fn for_codebase_size(index_path: &Path, codebase_loc: Option<usize>) -> Self {
        let (memory_budget_mb, num_threads) = if let Some(loc) = codebase_loc {
            if loc < 100_000 {
                (50, 2)
            } else if loc < 1_000_000 {
                (100, 4)
            } else {
                (200, 8)
            }
        } else {
            (50, 2) // Default for unknown size
        };

        Self {
            index_path: index_path.to_path_buf(),
            memory_budget_mb,
            num_threads,
        }
    }

    /// Create default configuration
    pub fn default(index_path: &Path) -> Self {
        Self {
            index_path: index_path.to_path_buf(),
            memory_budget_mb: 50,
            num_threads: 2,
        }
    }
}

/// Qdrant vector store configuration
#[derive(Debug, Clone)]
pub struct QdrantConfig {
    /// Qdrant server URL
    pub url: String,
    /// Collection name
    pub collection_name: String,
    /// Vector dimensions
    pub vector_size: usize,
    /// HNSW M parameter (connectivity)
    pub hnsw_m: usize,
    /// HNSW ef_construct parameter (construction quality)
    pub hnsw_ef_construct: usize,
    /// HNSW ef parameter (search quality)
    pub hnsw_ef: usize,
    /// Number of indexing threads
    pub indexing_threads: usize,
    /// Full scan threshold
    pub full_scan_threshold: usize,
    /// Memory-map threshold
    pub memmap_threshold: usize,
}

impl QdrantConfig {
    /// Create configuration optimized for codebase size
    pub fn for_codebase_size(
        url: &str,
        collection_name: &str,
        vector_size: usize,
        codebase_loc: Option<usize>,
    ) -> Self {
        let (hnsw_m, hnsw_ef_construct, hnsw_ef, indexing_threads, full_scan_threshold, memmap_threshold) =
            if let Some(loc) = codebase_loc {
                if loc < 100_000 {
                    (16, 100, 128, 8, 10_000, 50_000)
                } else if loc < 1_000_000 {
                    (16, 150, 128, 12, 20_000, 50_000)
                } else {
                    (32, 200, 256, 16, 50_000, 30_000)
                }
            } else {
                (16, 100, 128, 8, 10_000, 50_000) // Default for unknown size
            };

        Self {
            url: url.to_string(),
            collection_name: collection_name.to_string(),
            vector_size,
            hnsw_m,
            hnsw_ef_construct,
            hnsw_ef,
            indexing_threads,
            full_scan_threshold,
            memmap_threshold,
        }
    }

    /// Create default configuration
    pub fn default(url: &str, collection_name: &str, vector_size: usize) -> Self {
        Self {
            url: url.to_string(),
            collection_name: collection_name.to_string(),
            vector_size,
            hnsw_m: 16,
            hnsw_ef_construct: 100,
            hnsw_ef: 128,
            indexing_threads: 8,
            full_scan_threshold: 10_000,
            memmap_threshold: 50_000,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_indexer_config_for_codebase_size() {
        // Test small codebase
        let config = IndexerConfig::for_codebase_size(
            50_000,
            Path::new("./cache"),
            Path::new("./tantivy"),
            "http://localhost:6334",
            "test_collection",
            384,
        );
        assert_eq!(config.core.gpu_batch_size, 96);
        assert_eq!(config.tantivy.memory_budget_mb, 50);
        assert_eq!(config.tantivy.num_threads, 2);
        assert_eq!(config.qdrant.hnsw_m, 16);

        // Test medium codebase
        let config = IndexerConfig::for_codebase_size(
            500_000,
            Path::new("./cache"),
            Path::new("./tantivy"),
            "http://localhost:6334",
            "test_collection",
            384,
        );
        assert_eq!(config.tantivy.memory_budget_mb, 100);
        assert_eq!(config.tantivy.num_threads, 4);

        // Test large codebase
        let config = IndexerConfig::for_codebase_size(
            2_000_000,
            Path::new("./cache"),
            Path::new("./tantivy"),
            "http://localhost:6334",
            "test_collection",
            384,
        );
        assert_eq!(config.core.max_file_size, 15_000_000);
        assert_eq!(config.tantivy.memory_budget_mb, 200);
        assert_eq!(config.tantivy.num_threads, 8);
        assert_eq!(config.qdrant.hnsw_m, 32);
    }

    #[test]
    fn test_default_configs() {
        let core = IndexerCoreConfig::default();
        assert_eq!(core.max_file_size, 10_000_000);
        assert_eq!(core.gpu_batch_size, 96);

        let tantivy = TantivyConfig::default(Path::new("./tantivy"));
        assert_eq!(tantivy.memory_budget_mb, 50);
        assert_eq!(tantivy.num_threads, 2);

        let qdrant = QdrantConfig::default("http://localhost:6334", "test", 384);
        assert_eq!(qdrant.hnsw_m, 16);
        assert_eq!(qdrant.vector_size, 384);
    }
}

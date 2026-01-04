//! Consolidated indexer configuration
//!
//! Provides a unified configuration interface for indexing components,
//! reducing coupling by consolidating config structs.
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
//!     └─ TantivyConfig (BM25 indexing)
//! ```
//!
//! ## Automatic Size-Based Tuning
//!
//! The `for_codebase_size()` constructor automatically optimizes settings:
//!
//! | Codebase Size | Tantivy Threads | Memory (MB) | Batch Size |
//! |---------------|----------------|-------------|------------|
//! | < 100K LOC    | 2              | 50          | 96         |
//! | 100K - 1M LOC | 4              | 100         | 96         |
//! | > 1M LOC      | 8              | 200         | 128        |

use std::path::{Path, PathBuf};

/// Unified indexer configuration
///
/// This struct consolidates configuration from:
/// - IndexerCore (file processing settings)
/// - TantivyAdapter (BM25 indexing settings)
#[derive(Debug, Clone)]
pub struct IndexerConfig {
    /// Core processing settings
    pub core: IndexerCoreConfig,
    /// Tantivy BM25 settings
    pub tantivy: TantivyConfig,
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
    ) -> Self {
        let (max_file_size, gpu_batch_size, tantivy_memory_mb, tantivy_threads) =
            if codebase_loc < 100_000 {
                // Small codebase
                (10_000_000, 96, 50, 2)
            } else if codebase_loc < 1_000_000 {
                // Medium codebase
                (10_000_000, 96, 100, 4)
            } else {
                // Large codebase
                (15_000_000, 128, 200, 8)
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
        }
    }

    /// Create default configuration
    pub fn default(cache_path: &Path, tantivy_path: &Path) -> Self {
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
        );
        assert_eq!(config.core.gpu_batch_size, 96);
        assert_eq!(config.tantivy.memory_budget_mb, 50);
        assert_eq!(config.tantivy.num_threads, 2);

        // Test medium codebase
        let config = IndexerConfig::for_codebase_size(
            500_000,
            Path::new("./cache"),
            Path::new("./tantivy"),
        );
        assert_eq!(config.tantivy.memory_budget_mb, 100);
        assert_eq!(config.tantivy.num_threads, 4);

        // Test large codebase
        let config = IndexerConfig::for_codebase_size(
            2_000_000,
            Path::new("./cache"),
            Path::new("./tantivy"),
        );
        assert_eq!(config.core.max_file_size, 15_000_000);
        assert_eq!(config.tantivy.memory_budget_mb, 200);
        assert_eq!(config.tantivy.num_threads, 8);
    }

    #[test]
    fn test_default_configs() {
        let core = IndexerCoreConfig::default();
        assert_eq!(core.max_file_size, 10_000_000);
        assert_eq!(core.gpu_batch_size, 96);

        let tantivy = TantivyConfig::default(Path::new("./tantivy"));
        assert_eq!(tantivy.memory_budget_mb, 50);
        assert_eq!(tantivy.num_threads, 2);
    }
}

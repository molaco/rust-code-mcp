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
//! | Codebase Size | Tantivy Threads | Memory (MB) | GPU Batch Size |
//! |---------------|----------------|-------------|----------------|
//! | < 100K LOC    | 2              | 50          | 32             |
//! | 100K - 1M LOC | 4              | 100         | 32             |
//! | > 1M LOC      | 8              | 200         | 32             |
//!
//! Set `RUST_CODE_MCP_EMBED_BATCH_SIZE` to override the runtime embedding
//! batch size without changing embedding cache identity.

use std::env;
use std::path::{Path, PathBuf};

/// Environment variable for overriding the GPU embedding batch size.
pub const EMBED_BATCH_SIZE_ENV: &str = "RUST_CODE_MCP_EMBED_BATCH_SIZE";

const DEFAULT_GPU_BATCH_SIZE: usize = 32;
const MAX_GPU_BATCH_SIZE: usize = 256;

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
        // gpu_batch_size is tuned for Qwen3-Embedding-0.6B after
        // length-bucketing the embedding inputs. The old MiniLM-era
        // batch sizes pushed Qwen3 near the 24 GB VRAM cliff on real
        // chunks.
        let (max_file_size, gpu_batch_size, tantivy_memory_mb, tantivy_threads) =
            if codebase_loc < 100_000 {
                // Small codebase
                (10_000_000, DEFAULT_GPU_BATCH_SIZE, 50, 2)
            } else if codebase_loc < 1_000_000 {
                // Medium codebase
                (10_000_000, DEFAULT_GPU_BATCH_SIZE, 100, 4)
            } else {
                // Large codebase
                (15_000_000, DEFAULT_GPU_BATCH_SIZE, 200, 8)
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
                gpu_batch_size: DEFAULT_GPU_BATCH_SIZE, // Qwen3-0.6B safe default with length-bucketed inputs
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

impl IndexerCoreConfig {
    /// Apply runtime environment overrides.
    ///
    /// Batch size changes batch shape only; it does not alter vector semantics
    /// and should not affect embedding cache identity.
    pub fn with_env_overrides(mut self) -> Self {
        self.gpu_batch_size = gpu_batch_size_from_env(self.gpu_batch_size);
        self
    }
}

impl Default for IndexerCoreConfig {
    fn default() -> Self {
        Self {
            cache_path: PathBuf::from("./cache"),
            max_file_size: 10_000_000, // 10 MB
            gpu_batch_size: DEFAULT_GPU_BATCH_SIZE, // Qwen3-0.6B safe default with length-bucketed inputs
        }
    }
}

fn gpu_batch_size_from_env(default: usize) -> usize {
    let raw = match env::var(EMBED_BATCH_SIZE_ENV) {
        Ok(raw) => raw,
        Err(env::VarError::NotPresent) => return default,
        Err(err) => {
            tracing::warn!(
                env_var = EMBED_BATCH_SIZE_ENV,
                error = ?err,
                default,
                "Ignoring unreadable embedding GPU batch size override"
            );
            return default;
        }
    };

    match parse_gpu_batch_size_override(&raw) {
        Ok(batch_size) => {
            if raw.parse::<usize>().ok().is_some_and(|requested| requested > MAX_GPU_BATCH_SIZE) {
                tracing::warn!(
                    env_var = EMBED_BATCH_SIZE_ENV,
                    requested = raw.as_str(),
                    max = MAX_GPU_BATCH_SIZE,
                    "Clamping embedding GPU batch size override"
                );
            }
            tracing::info!(
                env_var = EMBED_BATCH_SIZE_ENV,
                gpu_batch_size = batch_size,
                "Using embedding GPU batch size override"
            );
            batch_size
        }
        Err(reason) => {
            tracing::warn!(
                env_var = EMBED_BATCH_SIZE_ENV,
                value = raw.as_str(),
                reason,
                default,
                "Ignoring invalid embedding GPU batch size override"
            );
            default
        }
    }
}

fn parse_gpu_batch_size_override(raw: &str) -> Result<usize, &'static str> {
    let parsed = raw
        .parse::<usize>()
        .map_err(|_| "value must be a positive integer")?;

    if parsed == 0 {
        return Err("value must be greater than zero");
    }

    Ok(parsed.min(MAX_GPU_BATCH_SIZE))
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
        assert_eq!(config.core.gpu_batch_size, 32); // Qwen3-0.6B safe default with length bucketing
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
        assert_eq!(core.gpu_batch_size, 32); // Qwen3-0.6B safe default with length bucketing

        let tantivy = TantivyConfig::default(Path::new("./tantivy"));
        assert_eq!(tantivy.memory_budget_mb, 50);
        assert_eq!(tantivy.num_threads, 2);
    }

    #[test]
    fn test_gpu_batch_size_override_parser() {
        assert_eq!(parse_gpu_batch_size_override("64").unwrap(), 64);
        assert_eq!(
            parse_gpu_batch_size_override("999").unwrap(),
            MAX_GPU_BATCH_SIZE
        );
        assert!(parse_gpu_batch_size_override("0").is_err());
        assert!(parse_gpu_batch_size_override("not-a-number").is_err());
    }
}

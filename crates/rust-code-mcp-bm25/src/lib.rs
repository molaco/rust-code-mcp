//! BM25 and Tantivy-backed chunk search APIs for rust-code-mcp.

#![warn(unreachable_pub, dead_code)]

mod schema;
mod search;
pub mod tantivy_adapter;

use std::path::{Path, PathBuf};

pub use schema::ChunkSchema;
pub use search::Bm25Search;
pub use tantivy_adapter::TantivyAdapter;

/// Tantivy BM25 indexing configuration.
#[derive(Debug, Clone)]
pub struct TantivyConfig {
    /// Path to Tantivy index directory.
    pub index_path: PathBuf,
    /// Memory budget in MB per thread.
    pub memory_budget_mb: usize,
    /// Number of threads for indexing.
    pub num_threads: usize,
}

impl TantivyConfig {
    /// Create configuration optimized for codebase size.
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
            (50, 2)
        };

        Self {
            index_path: index_path.to_path_buf(),
            memory_budget_mb,
            num_threads,
        }
    }

    /// Create default configuration.
    pub fn default(index_path: &Path) -> Self {
        Self {
            index_path: index_path.to_path_buf(),
            memory_budget_mb: 50,
            num_threads: 2,
        }
    }
}

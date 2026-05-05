//! Shared project path computation
//!
//! Extracts the repeated directory hash + path derivation logic
//! used across query_tools, index_tool, and sync manager.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use crate::tools::indexing_tools::data_dir;

/// Derived paths for a project directory
pub struct ProjectPaths {
    pub dir_hash: String,
    pub cache_path: PathBuf,
    pub tantivy_path: PathBuf,
    pub collection_name: String,
    pub vector_path: PathBuf,
}

impl ProjectPaths {
    /// Compute all derived paths from a project directory
    pub fn from_directory(dir: &Path) -> Self {
        let dir_hash = {
            let mut hasher = Sha256::new();
            hasher.update(dir.to_string_lossy().as_bytes());
            format!("{:x}", hasher.finalize())
        };

        let base = data_dir();
        let collection_name = format!("code_chunks_{}", &dir_hash[..8]);

        Self {
            cache_path: base.join("cache").join(&dir_hash),
            tantivy_path: base.join("index").join(&dir_hash),
            vector_path: base.join("cache").join("vectors").join(&collection_name),
            collection_name,
            dir_hash,
        }
    }
}

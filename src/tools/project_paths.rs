//! Shared project path computation
//!
//! Extracts the repeated directory hash + path derivation logic
//! used across query_tools, index_tool, and sync manager.

use std::path::{Path, PathBuf};

use crate::embeddings::EmbeddingBackend;
use crate::indexing::identity::{
    active_chunking_identity_for_backend, identity_hash, indexing_identity,
};
use crate::indexing::incremental::get_snapshot_path_for_identity;
use crate::tools::indexing_tools::data_dir;
use sha2::{Digest, Sha256};

/// Derived paths for a project directory
pub struct ProjectPaths {
    pub dir_hash: String,
    pub indexing_identity: String,
    pub chunking_identity: String,
    pub cache_path: PathBuf,
    pub tantivy_path: PathBuf,
    pub snapshot_path: PathBuf,
    pub collection_name: String,
    pub vector_path: PathBuf,
}

impl ProjectPaths {
    /// Compute all derived paths from a project directory keyed by the
    /// active embedding backend. The vector store path embeds a short
    /// fingerprint of `backend.identity()` so two indexes of the same
    /// project under different embedder variants land in distinct
    /// LanceDB directories instead of colliding.
    pub fn from_directory(dir: &Path, backend: &EmbeddingBackend) -> Self {
        let chunking_identity = active_chunking_identity_for_backend(backend);
        Self::from_directory_with_chunking_identity(dir, backend, chunking_identity)
    }

    /// Compute all derived paths with an explicit chunking identity.
    pub fn from_directory_with_chunking_identity(
        dir: &Path,
        backend: &EmbeddingBackend,
        chunking_identity: String,
    ) -> Self {
        let dir_hash = {
            let mut hasher = Sha256::new();
            hasher.update(dir.to_string_lossy().as_bytes());
            format!("{:x}", hasher.finalize())
        };
        let indexing_identity = indexing_identity(dir, backend, &chunking_identity);
        let index_hash = identity_hash(&indexing_identity);
        let snapshot_path = get_snapshot_path_for_identity(&indexing_identity);

        let base = data_dir();
        let collection_name = format!("code_chunks_{}_{}", &dir_hash[..8], &index_hash[..8]);

        Self {
            cache_path: base.join("cache").join(&dir_hash),
            tantivy_path: base.join("index").join(&dir_hash),
            vector_path: base.join("cache").join("vectors").join(&collection_name),
            collection_name,
            snapshot_path,
            indexing_identity,
            chunking_identity,
            dir_hash,
        }
    }
}

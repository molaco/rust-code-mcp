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

/// An existing vector index discovered under a project's collection prefix.
pub struct IndexedProfilePaths {
    pub paths: ProjectPaths,
    pub backend: EmbeddingBackend,
    pub stored_identity: String,
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
        let dir_hash = dir_hash(dir);
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

    /// Build path metadata for an already-existing vector collection.
    ///
    /// The collection name is trusted from disk so legacy collections whose
    /// path was keyed by the old identity string remain discoverable.
    pub fn from_existing_collection_name(
        dir: &Path,
        backend: &EmbeddingBackend,
        collection_name: String,
    ) -> Self {
        let dir_hash = dir_hash(dir);
        let chunking_identity = active_chunking_identity_for_backend(backend);
        let indexing_identity = indexing_identity(dir, backend, &chunking_identity);
        let snapshot_path = get_snapshot_path_for_identity(&indexing_identity);
        let base = data_dir();

        Self {
            cache_path: base.join("cache").join(&dir_hash),
            tantivy_path: base.join("index").join(&dir_hash),
            vector_path: Self::vectors_root().join(&collection_name),
            collection_name,
            snapshot_path,
            indexing_identity,
            chunking_identity,
            dir_hash,
        }
    }

    pub fn indexed_profiles(dir: &Path) -> Result<Vec<IndexedProfilePaths>, String> {
        let vectors_root = Self::vectors_root();
        if !vectors_root.exists() {
            return Ok(Vec::new());
        }

        let prefix = collection_prefix(dir);
        let entries = std::fs::read_dir(&vectors_root).map_err(|e| {
            format!(
                "failed to read vector index root {}: {e}",
                vectors_root.display()
            )
        })?;
        let mut indexes = Vec::new();

        for entry in entries {
            let entry = entry.map_err(|e| {
                format!(
                    "failed to read vector index entry under {}: {e}",
                    vectors_root.display()
                )
            })?;
            let file_type = entry.file_type().map_err(|e| {
                format!(
                    "failed to inspect vector index entry {}: {e}",
                    entry.path().display()
                )
            })?;
            if !file_type.is_dir() {
                continue;
            }

            let collection_name = entry.file_name().to_string_lossy().into_owned();
            if !collection_name.starts_with(&prefix) {
                continue;
            }

            let vector_path = entry.path();
            let Some(stored_identity) = read_embedder_identity(&vector_path)? else {
                continue;
            };
            let backend = EmbeddingBackend::from_identity(&stored_identity)
                .map_err(|e| {
                    format!(
                        "invalid embedder identity in {}: {e}",
                        vector_path.join("metadata.json").display()
                    )
                })?;
            let paths =
                Self::from_existing_collection_name(dir, &backend, collection_name);

            indexes.push(IndexedProfilePaths {
                paths,
                backend,
                stored_identity,
            });
        }

        Ok(indexes)
    }

    pub fn vectors_root() -> PathBuf {
        data_dir().join("cache").join("vectors")
    }
}

fn dir_hash(dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dir.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

fn collection_prefix(dir: &Path) -> String {
    let hash = dir_hash(dir);
    format!("code_chunks_{}_", &hash[..8])
}

fn read_embedder_identity(vector_path: &Path) -> Result<Option<String>, String> {
    let metadata_path = vector_path.join("metadata.json");
    if !metadata_path.exists() {
        return Ok(None);
    }

    let bytes = std::fs::read(&metadata_path).map_err(|e| {
        format!(
            "failed to read vector store metadata at {}: {e}",
            metadata_path.display()
        )
    })?;
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "failed to parse vector store metadata at {}: {e}",
            metadata_path.display()
        )
    })?;
    let identity = parsed
        .get("embedder_version")
        .and_then(|value| value.as_str())
        .ok_or_else(|| {
            format!(
                "missing `embedder_version` in vector store metadata at {}",
                metadata_path.display()
            )
        })?;

    Ok(Some(identity.to_string()))
}

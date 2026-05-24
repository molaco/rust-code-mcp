//! Indexing-owned project artifact path derivation.

use std::path::{Path, PathBuf};

use rmc_engine::embeddings::EmbeddingBackend;
use sha2::{Digest, Sha256};

use crate::indexing::identity::{
    active_chunking_identity_for_backend, identity_hash, indexing_identity,
};
use crate::indexing::incremental::get_snapshot_path_for_identity;

/// Derived indexing artifact paths for a project directory.
pub struct IndexingProjectPaths {
    pub dir_hash: String,
    pub indexing_identity: String,
    pub chunking_identity: String,
    pub cache_path: PathBuf,
    pub tantivy_path: PathBuf,
    pub snapshot_path: PathBuf,
    pub collection_name: String,
    pub vector_path: PathBuf,
}

/// Existing vector index discovered under a project's collection prefix.
pub struct IndexedProfilePaths {
    pub paths: IndexingProjectPaths,
    pub backend: EmbeddingBackend,
    pub stored_identity: String,
}

impl IndexingProjectPaths {
    /// Compute all derived indexing artifact paths for a project/backend.
    pub fn from_directory(
        data_root: &Path,
        dir: &Path,
        backend: &EmbeddingBackend,
    ) -> Self {
        let chunking_identity = active_chunking_identity_for_backend(backend);
        Self::from_directory_with_chunking_identity(data_root, dir, backend, chunking_identity)
    }

    /// Compute all derived indexing artifact paths with an explicit chunking identity.
    pub fn from_directory_with_chunking_identity(
        data_root: &Path,
        dir: &Path,
        backend: &EmbeddingBackend,
        chunking_identity: String,
    ) -> Self {
        let dir_hash = dir_hash(dir);
        let indexing_identity = indexing_identity(dir, backend, &chunking_identity);
        let index_hash = identity_hash(&indexing_identity);
        let snapshot_path = get_snapshot_path_for_identity(&indexing_identity);
        let collection_name = format!("code_chunks_{}_{}", &dir_hash[..8], &index_hash[..8]);

        Self {
            cache_path: data_root.join("cache").join(&dir_hash),
            tantivy_path: data_root.join("index").join(&dir_hash),
            vector_path: data_root.join("cache").join("vectors").join(&collection_name),
            collection_name,
            snapshot_path,
            indexing_identity,
            chunking_identity,
            dir_hash,
        }
    }

    /// Build path metadata for an already-existing vector collection.
    pub fn from_existing_collection_name(
        data_root: &Path,
        dir: &Path,
        backend: &EmbeddingBackend,
        collection_name: String,
    ) -> Self {
        Self::from_existing_collection_name_in_root(
            data_root,
            &Self::vectors_root(data_root),
            dir,
            backend,
            collection_name,
        )
    }

    /// Build path metadata for an existing vector collection under a known root.
    pub fn from_existing_collection_name_in_root(
        data_root: &Path,
        vectors_root: &Path,
        dir: &Path,
        backend: &EmbeddingBackend,
        collection_name: String,
    ) -> Self {
        let dir_hash = dir_hash(dir);
        let chunking_identity = active_chunking_identity_for_backend(backend);
        let indexing_identity = indexing_identity(dir, backend, &chunking_identity);
        let snapshot_path = get_snapshot_path_for_identity(&indexing_identity);

        Self {
            cache_path: data_root.join("cache").join(&dir_hash),
            tantivy_path: data_root.join("index").join(&dir_hash),
            vector_path: vectors_root.join(&collection_name),
            collection_name,
            snapshot_path,
            indexing_identity,
            chunking_identity,
            dir_hash,
        }
    }

    /// Discover existing vector indexes for a project under the default vectors root.
    pub fn indexed_profiles(
        data_root: &Path,
        dir: &Path,
    ) -> Result<Vec<IndexedProfilePaths>, String> {
        Self::indexed_profiles_in_root(data_root, dir, &Self::vectors_root(data_root))
    }

    /// Discover existing vector indexes for a project under a specific vectors root.
    pub fn indexed_profiles_in_root(
        data_root: &Path,
        dir: &Path,
        vectors_root: &Path,
    ) -> Result<Vec<IndexedProfilePaths>, String> {
        if !vectors_root.exists() {
            return Ok(Vec::new());
        }

        let prefix = collection_prefix(dir);
        let entries = std::fs::read_dir(vectors_root).map_err(|e| {
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
            let stored_identity = match read_embedder_identity(&vector_path) {
                Ok(Some(identity)) => identity,
                Ok(None) => continue,
                Err(error) => {
                    tracing::warn!(
                        vector_path = %vector_path.display(),
                        error = %error,
                        "skipping malformed vector index metadata during profile discovery"
                    );
                    continue;
                }
            };
            let backend = match EmbeddingBackend::from_identity(&stored_identity) {
                Ok(backend) => backend,
                Err(error) => {
                    tracing::warn!(
                        vector_path = %vector_path.display(),
                        error = %error,
                        "skipping vector index with invalid embedder identity during profile discovery"
                    );
                    continue;
                }
            };
            let paths = Self::from_existing_collection_name_in_root(
                data_root,
                vectors_root,
                dir,
                &backend,
                collection_name,
            );

            indexes.push(IndexedProfilePaths {
                paths,
                backend,
                stored_identity,
            });
        }

        Ok(indexes)
    }

    pub fn vectors_root(data_root: &Path) -> PathBuf {
        data_root.join("cache").join("vectors")
    }
}

pub fn dir_hash(dir: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(dir.to_string_lossy().as_bytes());
    format!("{:x}", hasher.finalize())
}

pub fn collection_prefix(dir: &Path) -> String {
    let hash = dir_hash(dir);
    format!("code_chunks_{}_", &hash[..8])
}

pub fn read_embedder_identity(vector_path: &Path) -> Result<Option<String>, String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn backend(name: &str) -> EmbeddingBackend {
        EmbeddingBackend::from_profile_name(name).unwrap()
    }

    fn write_metadata(collection: &Path, identity: &str) {
        std::fs::create_dir_all(collection).unwrap();
        std::fs::write(
            collection.join("metadata.json"),
            serde_json::json!({ "embedder_version": identity }).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn from_directory_uses_data_root_layout_and_identity_scoped_collection() {
        let data_root = TempDir::new().unwrap();
        let project_dir = Path::new("/tmp/rmc-indexing-project-paths-test");
        let backend = backend("local-cpu-small");

        let paths = IndexingProjectPaths::from_directory(data_root.path(), project_dir, &backend);

        assert_eq!(paths.dir_hash, dir_hash(project_dir));
        assert_eq!(
            paths.cache_path,
            data_root.path().join("cache").join(&paths.dir_hash)
        );
        assert_eq!(
            paths.tantivy_path,
            data_root.path().join("index").join(&paths.dir_hash)
        );
        assert_eq!(
            paths.vector_path,
            data_root.path().join("cache").join("vectors").join(&paths.collection_name)
        );
        assert!(paths.collection_name.starts_with(&collection_prefix(project_dir)));

        let alternate = IndexingProjectPaths::from_directory_with_chunking_identity(
            data_root.path(),
            project_dir,
            &backend,
            "chunk-policy:test".to_string(),
        );
        assert_ne!(paths.indexing_identity, alternate.indexing_identity);
        assert_ne!(paths.collection_name, alternate.collection_name);
        assert_ne!(paths.snapshot_path, alternate.snapshot_path);
    }

    #[test]
    fn existing_collection_uses_trusted_name_under_data_root_vectors() {
        let data_root = TempDir::new().unwrap();
        let project_dir = Path::new("/tmp/rmc-indexing-existing-collection-test");
        let backend = backend("local-cpu-small");
        let collection_name = format!("{}legacy", collection_prefix(project_dir));

        let paths = IndexingProjectPaths::from_existing_collection_name(
            data_root.path(),
            project_dir,
            &backend,
            collection_name.clone(),
        );

        assert_eq!(paths.collection_name, collection_name);
        assert_eq!(
            paths.vector_path,
            data_root.path().join("cache").join("vectors").join(&paths.collection_name)
        );
        assert_eq!(
            paths.cache_path,
            data_root.path().join("cache").join(&paths.dir_hash)
        );
        assert_eq!(
            paths.tantivy_path,
            data_root.path().join("index").join(&paths.dir_hash)
        );
    }

    #[test]
    fn indexed_profiles_in_root_discovers_matching_project_profiles() {
        let data_root = TempDir::new().unwrap();
        let vectors_root = TempDir::new().unwrap();
        let project_dir = Path::new("/tmp/rmc-indexing-profile-discovery-test");
        let prefix = collection_prefix(project_dir);
        let default_backend = EmbeddingBackend::default();
        let cpu_backend = backend("local-cpu-small");

        write_metadata(
            &vectors_root.path().join(format!("{prefix}default")),
            &default_backend.identity(),
        );
        write_metadata(
            &vectors_root.path().join(format!("{prefix}cpu")),
            &cpu_backend.identity(),
        );
        write_metadata(
            &vectors_root.path().join("code_chunks_unrelated"),
            &default_backend.identity(),
        );

        let mut profiles = IndexingProjectPaths::indexed_profiles_in_root(
            data_root.path(),
            project_dir,
            vectors_root.path(),
        )
        .unwrap();
        profiles.sort_by(|a, b| a.backend.model_id().cmp(b.backend.model_id()));

        assert_eq!(profiles.len(), 2);
        assert!(profiles
            .iter()
            .any(|profile| profile.backend.model_id() == default_backend.model_id()));
        assert!(profiles
            .iter()
            .any(|profile| profile.backend.model_id() == cpu_backend.model_id()));
        assert!(profiles.iter().all(|profile| profile
            .paths
            .vector_path
            .starts_with(vectors_root.path())));
    }

    #[test]
    fn indexed_profiles_skips_malformed_metadata_and_discovers_valid_profiles() {
        let data_root = TempDir::new().unwrap();
        let vectors_root = TempDir::new().unwrap();
        let project_dir = Path::new("/tmp/rmc-indexing-malformed-profile-test");
        let prefix = collection_prefix(project_dir);
        let backend = backend("local-cpu-small");

        write_metadata(
            &vectors_root.path().join(format!("{prefix}valid")),
            &backend.identity(),
        );
        std::fs::create_dir_all(vectors_root.path().join(format!("{prefix}bad-json"))).unwrap();
        std::fs::write(
            vectors_root
                .path()
                .join(format!("{prefix}bad-json"))
                .join("metadata.json"),
            "{not valid json",
        )
        .unwrap();
        std::fs::create_dir_all(vectors_root.path().join(format!("{prefix}missing-version")))
            .unwrap();
        std::fs::write(
            vectors_root
                .path()
                .join(format!("{prefix}missing-version"))
                .join("metadata.json"),
            serde_json::json!({ "other": "field" }).to_string(),
        )
        .unwrap();
        write_metadata(
            &vectors_root.path().join(format!("{prefix}unknown-identity")),
            "unknown-identity",
        );

        let profiles = IndexingProjectPaths::indexed_profiles_in_root(
            data_root.path(),
            project_dir,
            vectors_root.path(),
        )
        .unwrap();

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].stored_identity, backend.identity());
        assert_eq!(profiles[0].paths.collection_name, format!("{prefix}valid"));
    }
}

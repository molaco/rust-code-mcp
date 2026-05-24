//! Shared project path computation
//!
//! Extracts repeated data-directory, directory-hash, embedder-identity, and
//! backend-resolution logic used by the tools layer.

use std::path::{Path, PathBuf};

use rmcp::ErrorData as McpError;
use rmc_engine::embeddings::{EmbeddingBackend, resolve_profile};
use rmc_indexing::indexing::{
    IndexedProfilePaths as IndexingIndexedProfilePaths, IndexingProjectPaths,
};
use rmc_indexing::indexing::project_paths::{
    dir_hash as indexing_dir_hash,
    read_embedder_identity as read_indexing_embedder_identity,
};
use directories::ProjectDirs;

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

impl From<IndexingProjectPaths> for ProjectPaths {
    fn from(paths: IndexingProjectPaths) -> Self {
        Self {
            dir_hash: paths.dir_hash,
            indexing_identity: paths.indexing_identity,
            chunking_identity: paths.chunking_identity,
            cache_path: paths.cache_path,
            tantivy_path: paths.tantivy_path,
            snapshot_path: paths.snapshot_path,
            collection_name: paths.collection_name,
            vector_path: paths.vector_path,
        }
    }
}

impl From<IndexingIndexedProfilePaths> for IndexedProfilePaths {
    fn from(indexed: IndexingIndexedProfilePaths) -> Self {
        Self {
            paths: indexed.paths.into(),
            backend: indexed.backend,
            stored_identity: indexed.stored_identity,
        }
    }
}

/// Get the path for storing persistent index and cache.
pub(crate) fn data_dir() -> PathBuf {
    ProjectDirs::from("dev", "rust-code-mcp", "search")
        .map(|dirs| dirs.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
}

pub(crate) fn resolve_embedding_backend_for_mcp(
    embedding_profile: Option<&str>,
    directory: &Path,
) -> Result<EmbeddingBackend, McpError> {
    if let Some(profile) = embedding_profile {
        let profile = resolve_profile(profile, directory)
            .map_err(|msg| McpError::invalid_params(msg, None))?;
        return Ok(EmbeddingBackend::from_profile(profile));
    }

    Ok(EmbeddingBackend::default())
}

impl ProjectPaths {
    /// Compute all derived paths from a project directory keyed by the
    /// active embedding backend. The vector store path embeds a short
    /// fingerprint of `backend.identity()` so two indexes of the same
    /// project under different embedder variants land in distinct
    /// LanceDB directories instead of colliding.
    pub fn from_directory(dir: &Path, backend: &EmbeddingBackend) -> Self {
        IndexingProjectPaths::from_directory(&data_dir(), dir, backend).into()
    }

    /// Compute all derived paths with an explicit chunking identity.
    pub fn from_directory_with_chunking_identity(
        dir: &Path,
        backend: &EmbeddingBackend,
        chunking_identity: String,
    ) -> Self {
        IndexingProjectPaths::from_directory_with_chunking_identity(
            &data_dir(),
            dir,
            backend,
            chunking_identity,
        )
        .into()
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
        IndexingProjectPaths::from_existing_collection_name(
            &data_dir(),
            dir,
            backend,
            collection_name,
        )
        .into()
    }

    pub fn indexed_profiles(dir: &Path) -> Result<Vec<IndexedProfilePaths>, String> {
        Ok(IndexingProjectPaths::indexed_profiles(&data_dir(), dir)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    #[cfg(test)]
    fn indexed_profiles_in_root(
        dir: &Path,
        vectors_root: &Path,
    ) -> Result<Vec<IndexedProfilePaths>, String> {
        Ok(IndexingProjectPaths::indexed_profiles_in_root(&data_dir(), dir, vectors_root)?
            .into_iter()
            .map(Into::into)
            .collect())
    }

    pub fn vectors_root() -> PathBuf {
        IndexingProjectPaths::vectors_root(&data_dir())
    }
}

pub(crate) fn dir_hash(dir: &Path) -> String {
    indexing_dir_hash(dir)
}

#[cfg(test)]
fn collection_prefix(dir: &Path) -> String {
    rmc_indexing::indexing::project_paths::collection_prefix(dir)
}

pub(crate) fn read_embedder_identity(vector_path: &Path) -> Result<Option<String>, String> {
    read_indexing_embedder_identity(vector_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_engine::embeddings::{EmbeddingBackend, EmbeddingProfile};
    use tempfile::TempDir;

    fn write_metadata(collection: &Path, identity: &str) {
        std::fs::create_dir_all(collection).unwrap();
        std::fs::write(
            collection.join("metadata.json"),
            serde_json::json!({ "embedder_version": identity }).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn indexed_profiles_discovers_multiple_existing_profile_indexes() {
        let vectors_root = TempDir::new().unwrap();
        let project_dir = Path::new("/tmp/rust-code-mcp-indexed-profiles-test");
        let prefix = collection_prefix(project_dir);
        let default_backend = EmbeddingBackend::default();
        let cpu_backend = EmbeddingBackend::from_profile(
            EmbeddingProfile::parse("local-cpu-small").unwrap(),
        );

        write_metadata(
            &vectors_root.path().join(format!("{prefix}default")),
            &default_backend.identity(),
        );
        write_metadata(
            &vectors_root.path().join(format!("{prefix}cpu")),
            &cpu_backend.identity(),
        );
        write_metadata(
            &vectors_root.path().join("code_chunks_other_ignored"),
            &default_backend.identity(),
        );

        let mut indexes =
            ProjectPaths::indexed_profiles_in_root(project_dir, vectors_root.path()).unwrap();
        indexes.sort_by(|a, b| a.backend.model_id().cmp(b.backend.model_id()));

        assert_eq!(indexes.len(), 2);
        assert!(indexes
            .iter()
            .any(|indexed| indexed.backend.model_id() == default_backend.model_id()));
        assert!(indexes
            .iter()
            .any(|indexed| indexed.backend.model_id() == cpu_backend.model_id()));
    }

    #[test]
    fn indexed_profiles_preserves_legacy_stored_identity() {
        let vectors_root = TempDir::new().unwrap();
        let project_dir = Path::new("/tmp/rust-code-mcp-legacy-index-test");
        let prefix = collection_prefix(project_dir);
        let legacy_identity =
            "fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2";

        write_metadata(
            &vectors_root.path().join(format!("{prefix}legacy")),
            legacy_identity,
        );

        let indexes =
            ProjectPaths::indexed_profiles_in_root(project_dir, vectors_root.path()).unwrap();

        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].stored_identity, legacy_identity);
        assert_eq!(indexes[0].backend.profile.name(), "local-gpu-small");
    }

    #[test]
    fn indexed_profiles_in_root_returns_paths_under_injected_vectors_root() {
        let vectors_root = TempDir::new().unwrap();
        let project_dir = Path::new("/tmp/rust-code-mcp-injected-root-test");
        let prefix = collection_prefix(project_dir);
        let backend = EmbeddingBackend::from_profile_name("local-cpu-small").unwrap();
        let collection_name = format!("{prefix}cpu");
        let collection_path = vectors_root.path().join(&collection_name);

        write_metadata(&collection_path, &backend.identity());

        let indexes =
            ProjectPaths::indexed_profiles_in_root(project_dir, vectors_root.path()).unwrap();

        assert_eq!(indexes.len(), 1);
        assert_eq!(indexes[0].paths.collection_name, collection_name);
        assert_eq!(indexes[0].paths.vector_path, collection_path);
    }
}

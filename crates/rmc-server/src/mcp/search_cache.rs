use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rmc_engine::embeddings::EmbeddingGenerator;
use rmc_engine::search::Bm25Search;
use rmc_engine::vector_store::VectorStore;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SearchRuntimeCacheKey {
    pub workspace: PathBuf,
    pub embedding_identity: String,
    pub vector_path: PathBuf,
    pub tantivy_path: PathBuf,
}

impl SearchRuntimeCacheKey {
    pub fn new(
        workspace: &Path,
        embedding_identity: impl Into<String>,
        vector_path: &Path,
        tantivy_path: &Path,
    ) -> Self {
        Self {
            workspace: normalize_path(workspace),
            embedding_identity: embedding_identity.into(),
            vector_path: normalize_path(vector_path),
            tantivy_path: normalize_path(tantivy_path),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchRuntimeCacheKeyStatus {
    pub workspace: String,
    pub embedding_identity: String,
    pub vector_path: String,
    pub tantivy_path: String,
}

impl From<&SearchRuntimeCacheKey> for SearchRuntimeCacheKeyStatus {
    fn from(key: &SearchRuntimeCacheKey) -> Self {
        Self {
            workspace: key.workspace.display().to_string(),
            embedding_identity: key.embedding_identity.clone(),
            vector_path: key.vector_path.display().to_string(),
            tantivy_path: key.tantivy_path.display().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchRuntimeCacheStatus {
    pub entry_count: usize,
    pub keys: Vec<SearchRuntimeCacheKeyStatus>,
}

#[derive(Clone)]
pub struct SearchRuntimeCacheEntry {
    pub embedding_generator: EmbeddingGenerator,
    pub vector_store: VectorStore,
    pub bm25_search: Option<Bm25Search>,
}

#[derive(Clone, Default)]
pub struct SearchRuntimeCache {
    entries: Arc<Mutex<HashMap<SearchRuntimeCacheKey, SearchRuntimeCacheEntry>>>,
}

impl SearchRuntimeCache {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, key: &SearchRuntimeCacheKey) -> Option<SearchRuntimeCacheEntry> {
        self.entries
            .lock()
            .expect("search runtime cache mutex poisoned")
            .get(key)
            .cloned()
    }

    pub fn insert(&self, key: SearchRuntimeCacheKey, entry: SearchRuntimeCacheEntry) {
        self.entries
            .lock()
            .expect("search runtime cache mutex poisoned")
            .insert(key, entry);
    }

    pub fn invalidate_workspace(&self, workspace: &Path) -> usize {
        let mut entries = self
            .entries
            .lock()
            .expect("search runtime cache mutex poisoned");
        let before = entries.len();
        entries.retain(|key, _| !key_matches_workspace(key, workspace));
        before - entries.len()
    }

    pub fn invalidate_all(&self) -> usize {
        let mut entries = self
            .entries
            .lock()
            .expect("search runtime cache mutex poisoned");
        let count = entries.len();
        entries.clear();
        count
    }

    pub fn len(&self) -> usize {
        self.entries
            .lock()
            .expect("search runtime cache mutex poisoned")
            .len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn keys(&self) -> Vec<SearchRuntimeCacheKey> {
        let mut keys = self
            .entries
            .lock()
            .expect("search runtime cache mutex poisoned")
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        keys.sort_by(|a, b| {
            a.workspace
                .cmp(&b.workspace)
                .then_with(|| a.embedding_identity.cmp(&b.embedding_identity))
                .then_with(|| a.vector_path.cmp(&b.vector_path))
                .then_with(|| a.tantivy_path.cmp(&b.tantivy_path))
        });
        keys
    }

    pub fn status(&self) -> SearchRuntimeCacheStatus {
        let keys = self
            .keys()
            .iter()
            .map(SearchRuntimeCacheKeyStatus::from)
            .collect::<Vec<_>>();
        SearchRuntimeCacheStatus {
            entry_count: keys.len(),
            keys,
        }
    }
}

fn normalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn key_matches_workspace(key: &SearchRuntimeCacheKey, workspace: &Path) -> bool {
    key.workspace == normalize_path(workspace)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_runtime_cache_key_normalizes_existing_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = temp_dir.path().join("workspace");
        let vector_path = temp_dir.path().join("vectors");
        let tantivy_path = temp_dir.path().join("tantivy");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&vector_path).unwrap();
        std::fs::create_dir_all(&tantivy_path).unwrap();

        let key = SearchRuntimeCacheKey::new(
            &workspace.join("."),
            "embedder:v1",
            &vector_path.join("."),
            &tantivy_path.join("."),
        );

        assert_eq!(key.workspace, std::fs::canonicalize(&workspace).unwrap());
        assert_eq!(key.vector_path, std::fs::canonicalize(&vector_path).unwrap());
        assert_eq!(key.tantivy_path, std::fs::canonicalize(&tantivy_path).unwrap());
        assert_eq!(key.embedding_identity, "embedder:v1");
    }

    #[test]
    fn search_runtime_cache_key_preserves_missing_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = temp_dir.path().join("missing-workspace");
        let vector_path = temp_dir.path().join("missing-vectors");
        let tantivy_path = temp_dir.path().join("missing-tantivy");

        let key = SearchRuntimeCacheKey::new(
            &workspace,
            "embedder:v1",
            &vector_path,
            &tantivy_path,
        );

        assert_eq!(key.workspace, workspace);
        assert_eq!(key.vector_path, vector_path);
        assert_eq!(key.tantivy_path, tantivy_path);
    }

    #[test]
    fn search_runtime_cache_workspace_matching_uses_canonical_paths() {
        let temp_dir = tempfile::tempdir().unwrap();
        let workspace = temp_dir.path().join("workspace");
        let vector_path = temp_dir.path().join("vectors");
        let tantivy_path = temp_dir.path().join("tantivy");
        std::fs::create_dir_all(&workspace).unwrap();

        let key = SearchRuntimeCacheKey::new(
            &workspace,
            "embedder:v1",
            &vector_path,
            &tantivy_path,
        );

        assert!(key_matches_workspace(&key, &workspace.join(".")));
        assert!(!key_matches_workspace(&key, &temp_dir.path().join("other")));
    }

    #[test]
    fn empty_search_runtime_cache_invalidation_is_safe() {
        let cache = SearchRuntimeCache::new();
        let temp_dir = tempfile::tempdir().unwrap();

        assert_eq!(cache.invalidate_workspace(temp_dir.path()), 0);
        assert_eq!(cache.invalidate_all(), 0);

        assert!(cache.is_empty());
    }

    #[test]
    fn runtime_search_cache_status_reports_empty_keys() {
        let cache = SearchRuntimeCache::new();
        let status = cache.status();

        assert_eq!(status.entry_count, 0);
        assert!(status.keys.is_empty());
        assert!(cache.keys().is_empty());
    }
}

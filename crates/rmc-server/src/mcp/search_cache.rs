use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rmc_engine::embeddings::EmbeddingGenerator;
use rmc_engine::search::Bm25Search;
use rmc_engine::vector_store::VectorStore;

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

    pub fn invalidate_workspace(&self, workspace: &Path) {
        let workspace = normalize_path(workspace);
        self.entries
            .lock()
            .expect("search runtime cache mutex poisoned")
            .retain(|key, _| key.workspace != workspace);
    }

    pub fn invalidate_all(&self) {
        self.entries
            .lock()
            .expect("search runtime cache mutex poisoned")
            .clear();
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
}

fn normalize_path(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

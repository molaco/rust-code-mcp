//! Shared cache identity helpers for indexing artifacts.

use crate::config::IndexerCoreConfig;
use crate::embeddings::EmbeddingBackend;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Active chunking identity after environment overrides.
///
/// Chunking changes alter the document text that gets embedded, so they must
/// invalidate both metadata-cache entries and Merkle snapshots.
pub(crate) fn active_chunking_identity() -> String {
    active_chunking_identity_for_backend(&EmbeddingBackend::default())
}

/// Active chunking identity for a specific embedding backend.
pub(crate) fn active_chunking_identity_for_backend(backend: &EmbeddingBackend) -> String {
    IndexerCoreConfig::default()
        .with_embedding_profile(backend.profile.clone())
        .with_env_overrides()
        .chunking_cache_salt()
}

/// Canonicalize a codebase path for stable cache identity.
///
/// Callers validate paths before indexing, but tests and health probes may
/// pass paths that do not exist yet. In that case, fall back to the raw path.
pub(crate) fn canonical_codebase_path(codebase_path: &Path) -> PathBuf {
    std::fs::canonicalize(codebase_path).unwrap_or_else(|_| codebase_path.to_path_buf())
}

/// Stable identity for all embedding-sensitive indexing artifacts.
pub(crate) fn indexing_identity(
    codebase_path: &Path,
    backend: &EmbeddingBackend,
    chunking_identity: &str,
) -> String {
    let canonical_path = canonical_codebase_path(codebase_path);
    format!(
        "index:v1:path{}:embedder{}:chunking{}",
        canonical_path.to_string_lossy(),
        backend.identity(),
        chunking_identity
    )
}

/// SHA-256 hex digest for a stable identity string.
pub(crate) fn identity_hash(identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identity.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexing_identity_changes_by_backend() {
        let path = Path::new("/tmp/rust-code-mcp-test");
        let chunking = "chunk-split:v1:target768:hard1024";
        let mut alternate = EmbeddingBackend::default();
        alternate.max_len = 2048;

        assert_ne!(
            indexing_identity(path, &EmbeddingBackend::default(), chunking),
            indexing_identity(path, &alternate, chunking)
        );
    }

    #[test]
    fn indexing_identity_changes_by_chunking() {
        let path = Path::new("/tmp/rust-code-mcp-test");
        let backend = EmbeddingBackend::default();

        assert_ne!(
            indexing_identity(path, &backend, "chunk-split:v1:target768:hard1024"),
            indexing_identity(path, &backend, "chunk-split:v1:target512:hard768")
        );
    }
}

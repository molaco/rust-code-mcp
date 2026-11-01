//! Shared cache identity helpers for indexing artifacts.

use rmc_config::config::IndexerCoreConfig;
use rmc_engine::embeddings::EmbeddingBackend;
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
pub(super) fn active_chunking_identity_for_backend(backend: &EmbeddingBackend) -> String {
    IndexerCoreConfig::default()
        .with_embedding_profile(backend.profile.clone())
        .with_env_overrides()
        .chunking_cache_salt()
}

/// Salt for metadata-cache keys.
///
/// 🚨 Must contain the EMBEDDER, not only the chunking config. The
/// metadata cache lives at `cache/{dir_hash}` — one sled database per
/// DIRECTORY, shared by every embedding profile of that directory. With
/// a chunking-only salt, indexing a large workspace with `local-gpu-bge` marks
/// its files as done, and a later run under `local-cpu-small` skips them
/// as unchanged (`Parser error: File unchanged`) — while the CPU store,
/// keyed separately by identity, never receives a single vector for
/// them. That is not hypothetical: on 2026-08-18 the live CPU index of
/// the large workspace was short 467 files this way, and `health_check` called it
/// healthy (see `monitoring::health::CoverageHealth`).
///
/// Both profiles here use bge-small with identical chunk limits, so the
/// chunking salts were byte-identical — the collision needs no unusual
/// configuration at all.
pub(crate) fn metadata_cache_salt(backend: &EmbeddingBackend, chunking_identity: &str) -> String {
    format!("embedder{}:chunking{}", backend.identity(), chunking_identity)
}

/// Canonicalize a codebase path for stable cache identity.
///
/// Callers validate paths before indexing, but tests and health probes may
/// pass paths that do not exist yet. In that case, fall back to the raw path.
pub(crate) fn canonical_codebase_path(codebase_path: &Path) -> PathBuf {
    std::fs::canonicalize(codebase_path).unwrap_or_else(|_| codebase_path.to_path_buf())
}

/// Stable identity for all embedding-sensitive indexing artifacts.
pub(super) fn indexing_identity(
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
pub(super) fn identity_hash(identity: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(identity.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_cache_salt_separates_profiles_with_identical_chunking() {
        // The two live profiles on the large workspace: same model family, same chunk
        // limits — the chunking salt alone cannot tell them apart, which
        // is exactly how one profile's run silently skipped 467 files
        // for the other.
        let cpu = EmbeddingBackend::default();
        let gpu = EmbeddingBackend::from_identity(
            "emb;v=2;rt=local-fastembed-onnx-migraphx;model=Xenova%2Fbge-small-en-v1.5;dim=384;max=512;query=prefix%3ARepresent%2520this%2520sentence%2520for%2520searching%2520relevant%2520passages%253A%2520",
        )
        .expect("parse gpu identity");
        let chunking = "chunk-split:v1:target300:hard900";

        assert_ne!(
            metadata_cache_salt(&cpu, chunking),
            metadata_cache_salt(&gpu, chunking),
        );
        // Positive control: the same backend must keep a stable salt,
        // otherwise every run would look like a cache miss.
        assert_eq!(
            metadata_cache_salt(&cpu, chunking),
            metadata_cache_salt(&cpu, chunking),
        );
    }

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

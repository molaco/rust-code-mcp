//! Health monitoring for production deployments
//!
//! Provides component-level health checks for:
//! - BM25 search (Tantivy)
//! - Vector search (LanceDB)
//! - Merkle tree snapshots
//!
//! Health states: Healthy, Degraded, Unhealthy

use crate::indexing::{FileSystemMerkle, collect_project_rust_files};
use crate::metadata_cache::MetadataCache;
use rmc_engine::search::Bm25Search;
use rmc_engine::vector_store::VectorStore;
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Overall system health status
#[derive(Debug, Clone, Serialize)]
pub struct HealthStatus {
    /// Overall system status
    pub overall: Status,
    /// BM25 search component health
    pub bm25: ComponentHealth,
    /// Vector search component health
    pub vector: ComponentHealth,
    /// Merkle tree component health
    pub merkle: ComponentHealth,
    /// Index coverage: does the store actually hold what the indexer
    /// believes it already indexed?
    pub coverage: CoverageHealth,
    /// Index freshness: does the index still describe the code on disk?
    pub freshness: FreshnessHealth,
}

/// Freshness report — the answer to "is this index *current*?".
///
/// The gap this exists for: every other component answers a question about
/// the index's own internals, and none of them ever looks at the working
/// tree. `merkle` reports that a snapshot file EXISTS; `coverage` reports
/// that everything the cache claims is indexed HAS vectors. Both stay green
/// while the developer edits code all day, because the comparison against
/// disk happens nowhere except inside `index_codebase` — which is the one
/// place that also fixes it.
///
/// The consequence is a probe that structurally cannot fail for the most
/// common real failure. Observed on a large workspace (2026-08-25): `health_check`
/// reported healthy with `coverage 3983/3983`, and the very next
/// `index_codebase` reindexed 133 changed files. Nothing was broken — the
/// index was simply 133 files behind, and no measurement existed that could
/// say so.
///
/// The verdict is content-based, not timestamp-based (see
/// [`FileSystemMerkle::detect_disk_changes`]), so a rebuild or a `git
/// checkout` that touches mtimes without changing bytes does not raise a
/// false alarm.
#[derive(Debug, Clone, Serialize)]
pub struct FreshnessHealth {
    /// Freshness status
    pub status: Status,
    /// Human-readable summary
    pub message: String,
    /// Files on disk that the snapshot has never seen
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_added: Option<usize>,
    /// Files whose content differs from the snapshot
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_modified: Option<usize>,
    /// Files the snapshot tracks that are gone from disk
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_deleted: Option<usize>,
    /// A few example paths, tagged with what happened to them
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub examples: Vec<String>,
}

impl FreshnessHealth {
    /// Freshness could not be measured (no project directory, no snapshot).
    ///
    /// Degraded, never healthy — for the same reason as
    /// [`CoverageHealth::unknown`]: "we did not look" must not read as "we
    /// looked and it is fine".
    fn unknown(message: impl Into<String>) -> Self {
        Self {
            status: Status::Degraded,
            message: message.into(),
            files_added: None,
            files_modified: None,
            files_deleted: None,
            examples: Vec::new(),
        }
    }
}

/// Coverage report — the answer to "is this index complete?", which
/// none of the other three components can give.
///
/// The failure this exists for: an indexing run that dies halfway still
/// leaves a Merkle snapshot and a metadata cache behind. The next run
/// honestly considers those files unchanged (`Parser error: File
/// unchanged`) and skips them, so the missing vectors are never written
/// — and every component above reports `healthy`, because the store is
/// alive and the snapshot exists. Observed 2026-08-18 on a large workspace:
/// 498 files indexed, 3437 skipped, 136 352 vectors instead of 465 350,
/// overall status `healthy`.
///
/// The verdict is deliberately built on `files_cached` vs
/// `files_with_vectors`, not on the Merkle count: a file with no
/// symbols legitimately produces no chunks and therefore no vectors,
/// and it never enters the metadata cache either (the cache is written
/// only after a successful upsert). So `stale_skips` has no false
/// positives by construction, while "tracked minus vectors" would have
/// them. Both numbers are reported; only one is a verdict.
#[derive(Debug, Clone, Serialize)]
pub struct CoverageHealth {
    /// Coverage status
    pub status: Status,
    /// Human-readable summary
    pub message: String,
    /// Files tracked by the Merkle snapshot (what the project contains)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_tracked: Option<usize>,
    /// Files the metadata cache calls indexed (what the next run will skip)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_cached: Option<usize>,
    /// Distinct files that actually have at least one vector in the store
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_with_vectors: Option<usize>,
    /// Files the indexer will skip as unchanged although the store holds
    /// nothing for them. Non-zero means the index is silently incomplete
    /// and will stay so until a forced reindex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stale_skips: Option<usize>,
    /// A few example paths from `stale_skips`, for triage
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stale_skip_examples: Vec<String>,
    /// Files with chunks in the store that are in neither the working tree
    /// nor the snapshot: search can answer with deleted code, and no sync
    /// removes them. `None` without a project root or after a walk with
    /// errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_outside_tree: Option<usize>,
    /// A few example paths from `files_outside_tree`, for triage
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outside_tree_examples: Vec<String>,
}

impl CoverageHealth {
    /// Coverage could not be computed (missing snapshot, cache or store).
    ///
    /// Reported as degraded, never healthy: "we did not look" must not
    /// read as "we looked and it is fine".
    fn unknown(message: impl Into<String>) -> Self {
        Self {
            status: Status::Degraded,
            message: message.into(),
            files_tracked: None,
            files_cached: None,
            files_with_vectors: None,
            stale_skips: None,
            stale_skip_examples: Vec::new(),
            files_outside_tree: None,
            outside_tree_examples: Vec::new(),
        }
    }
}

/// Health status levels
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// All systems operational
    Healthy,
    /// Some systems degraded but functional
    Degraded,
    /// Critical systems failing
    Unhealthy,
}

/// Individual component health
#[derive(Debug, Clone, Serialize)]
pub struct ComponentHealth {
    /// Component status
    pub status: Status,
    /// Status message
    pub message: String,
    /// Optional latency measurement in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u64>,
}

impl ComponentHealth {
    /// Create healthy component status
    pub fn healthy(message: impl Into<String>, latency_ms: Option<u64>) -> Self {
        Self {
            status: Status::Healthy,
            message: message.into(),
            latency_ms,
        }
    }

    /// Create degraded component status
    pub fn degraded(message: impl Into<String>) -> Self {
        Self {
            status: Status::Degraded,
            message: message.into(),
            latency_ms: None,
        }
    }

    /// Create unhealthy component status
    pub fn unhealthy(message: impl Into<String>) -> Self {
        Self {
            status: Status::Unhealthy,
            message: message.into(),
            latency_ms: None,
        }
    }
}

/// Read the metadata cache and return the file paths it holds for this
/// chunking identity.
///
/// Keys are `salt::path` (or a bare path when the salt is empty), and an
/// entry under a DIFFERENT salt belongs to another chunking config — it
/// says nothing about the index being checked, so it is dropped rather
/// than counted.
fn read_cached_paths(cache_path: &PathBuf, salt: &str) -> Result<HashSet<String>, String> {
    let cache = MetadataCache::new(cache_path).map_err(|e| e.to_string())?;
    let files = cache.list_files().map_err(|e| e.to_string())?;
    Ok(MetadataCache::paths_for_salt(files, salt))
}

/// Health monitor for the search system
pub struct HealthMonitor {
    bm25: Option<Arc<Bm25Search>>,
    vector_store: Option<Arc<VectorStore>>,
    merkle_path: PathBuf,
    /// sled metadata cache of the project being checked, plus the salt
    /// its keys are prefixed with (the chunking identity). Absent for a
    /// system-wide probe, where there is no single project to speak of.
    metadata_cache: Option<(PathBuf, String)>,
    /// Working tree of the project being checked. Without it there is no
    /// "current state of the code" to compare the snapshot against, and
    /// freshness stays unmeasured.
    project_root: Option<PathBuf>,
}

impl HealthMonitor {
    /// Create a new health monitor
    pub fn new(
        bm25: Option<Arc<Bm25Search>>,
        vector_store: Option<Arc<VectorStore>>,
        merkle_path: PathBuf,
    ) -> Self {
        Self {
            bm25,
            vector_store,
            merkle_path,
            metadata_cache: None,
            project_root: None,
        }
    }

    /// Point the monitor at the project's working tree, enabling the
    /// freshness check.
    ///
    /// Must be the same root the snapshot was built from — both sides walk
    /// with `traversal::collect_project_rust_files`, and a different root
    /// would report the whole project as added and deleted at once.
    pub fn with_project_root(mut self, root: PathBuf) -> Self {
        self.project_root = Some(root);
        self
    }

    /// Point the monitor at the project's metadata cache, enabling the
    /// coverage check.
    ///
    /// `cache_key_salt` must be the same chunking identity the indexer
    /// used (`IndexerCoreConfig::chunking_cache_salt`) — cache keys are
    /// `salt::path`, and reading them with the wrong salt would report
    /// an empty cache, i.e. a silent "nothing to compare".
    pub fn with_metadata_cache(mut self, cache_path: PathBuf, cache_key_salt: String) -> Self {
        self.metadata_cache = Some((cache_path, cache_key_salt));
        self
    }

    /// Perform comprehensive health check
    pub async fn check_health(&self) -> HealthStatus {
        // Run all checks in parallel
        let (bm25_health, vector_health, merkle_health, coverage, freshness) = tokio::join!(
            self.check_bm25(),
            self.check_vector(),
            self.check_merkle(),
            self.check_coverage(),
            self.check_freshness()
        );

        // Determine overall status
        let overall = self.calculate_overall_status(
            &bm25_health,
            &vector_health,
            &merkle_health,
            &coverage,
            &freshness,
        );

        HealthStatus {
            overall,
            bm25: bm25_health,
            vector: vector_health,
            merkle: merkle_health,
            coverage,
            freshness,
        }
    }

    /// Check BM25 search health
    async fn check_bm25(&self) -> ComponentHealth {
        let Some(bm25) = &self.bm25 else {
            return ComponentHealth::degraded("BM25 search not configured");
        };

        let start = Instant::now();

        // Try a simple test query (BM25 search is synchronous)
        match bm25.search("__health_check__", 1) {
            Ok(_) => {
                let latency = start.elapsed().as_millis() as u64;
                ComponentHealth::healthy("BM25 search operational", Some(latency))
            }
            Err(e) => ComponentHealth::unhealthy(format!("BM25 search error: {:?}", e)),
        }
    }

    /// Check vector search health
    async fn check_vector(&self) -> ComponentHealth {
        let Some(vector_store) = &self.vector_store else {
            return ComponentHealth::degraded("Vector store not configured");
        };

        let start = Instant::now();

        // Check collection exists and is accessible
        match vector_store.count().await {
            Ok(count) => {
                let latency = start.elapsed().as_millis() as u64;
                ComponentHealth::healthy(
                    format!("Vector store operational ({} vectors)", count),
                    Some(latency),
                )
            }
            Err(e) => ComponentHealth::unhealthy(format!("Vector store error: {}", e)),
        }
    }

    /// Check Merkle tree snapshot health
    async fn check_merkle(&self) -> ComponentHealth {
        if self.merkle_path.exists() {
            match std::fs::metadata(&self.merkle_path) {
                Ok(metadata) => {
                    let size_bytes = metadata.len();
                    ComponentHealth::healthy(
                        format!("Merkle snapshot exists ({} bytes)", size_bytes),
                        None,
                    )
                }
                Err(e) => ComponentHealth::degraded(format!(
                    "Merkle snapshot exists but unreadable: {}",
                    e
                )),
            }
        } else {
            ComponentHealth::degraded("Merkle snapshot not found (first index pending)")
        }
    }

    /// Check index coverage (see [`CoverageHealth`])
    async fn check_coverage(&self) -> CoverageHealth {
        let Some(vector_store) = &self.vector_store else {
            return CoverageHealth::unknown(
                "Coverage unknown: vector store not open, nothing to compare against",
            );
        };
        let Some((cache_path, salt)) = &self.metadata_cache else {
            return CoverageHealth::unknown(
                "Coverage unknown: no project metadata cache (system-wide check; pass 'directory')",
            );
        };
        if !cache_path.exists() {
            return CoverageHealth::unknown(format!(
                "Coverage unknown: metadata cache missing at {} (project never indexed?)",
                cache_path.display()
            ));
        }

        // Files the indexer considers already done. Opened read-only in
        // spirit but sled has no such mode; a health probe must not fight
        // a running indexer over the lock, so a failure here is reported,
        // not retried.
        let cached_files = match read_cached_paths(cache_path, salt) {
            Ok(files) => files,
            Err(e) => {
                return CoverageHealth::unknown(format!(
                    "Coverage unknown: cannot read metadata cache at {}: {}",
                    cache_path.display(),
                    e
                ));
            }
        };

        let indexed_files = match vector_store.indexed_file_paths().await {
            Ok(files) => files,
            Err(e) => {
                return CoverageHealth::unknown(format!(
                    "Coverage unknown: cannot list indexed files in vector store: {}",
                    e
                ));
            }
        };

        // What the project actually contains, for context. Absent
        // snapshot is not fatal for the verdict — the verdict does not
        // depend on it.
        let snapshot = FileSystemMerkle::load_snapshot(&self.merkle_path)
            .ok()
            .flatten();
        let files_tracked = snapshot.as_ref().map(FileSystemMerkle::file_count);

        // Files the store holds that the project no longer has: search
        // answers with code that is gone, and no sync removes them (see
        // `UnifiedIndexer::drop_files_outside`). A file counts only if it is
        // in neither the working tree nor the snapshot — a file deleted since
        // the last sync is in the snapshot (the next sync drops it), and one
        // written by a sync whose snapshot is not saved yet is in the tree.
        // A walk with errors proves nothing about a file it did not see.
        let mut outside: Vec<String> = Vec::new();
        let files_outside_tree = match &self.project_root {
            Some(root) => {
                let (walked, walk_errors) = collect_project_rust_files(root);
                (walk_errors == 0).then(|| {
                    let known: HashSet<String> = walked
                        .iter()
                        .chain(snapshot.iter().flat_map(FileSystemMerkle::file_paths))
                        .map(|path| path.to_string_lossy().to_string())
                        .collect();
                    outside = indexed_files
                        .iter()
                        .filter(|path| !known.contains(*path))
                        .cloned()
                        .collect();
                    outside.sort();
                    outside.len()
                })
            }
            None => None,
        };

        let mut stale: Vec<String> = cached_files
            .iter()
            .filter(|path| !indexed_files.contains(*path))
            .cloned()
            .collect();
        stale.sort();

        let files_cached = cached_files.len();
        let files_with_vectors = indexed_files.len();
        let stale_skips = stale.len();
        let examples: Vec<String> = stale.iter().take(10).cloned().collect();

        // Empty comparison is NOT a pass. The cache holds keys for one
        // embedder+chunking salt; a profile that was never indexed (or
        // whose salt changed, as happened when the embedder was added to
        // the salt) yields zero cached files, and "all 0 cached files
        // have vectors" is vacuously true — exactly the shape of verdict
        // this component exists to abolish. Caught on the live index
        // right after the salt change, where it printed `healthy` next
        // to `files_cached: 0`.
        //
        // "No cache entries" comes in TWO shapes that call for different
        // actions, so they get different messages. One text for both made
        // the cheap, frequent case (cache dropped or re-salted, vectors
        // intact) read exactly like the expensive, rare one (a run died
        // halfway) — whose printed remedy is a full `force_reindex`, an
        // hour of work this state does not call for. The shapes are
        // distinguishable by construction: the cache is written only
        // after a successful upsert, so a half-finished run leaves cache
        // entries behind; an empty cache next to a non-empty store means
        // the cache was lost, not the vectors.
        if files_cached == 0 {
            let message = if files_with_vectors == 0 {
                "Coverage unknown: nothing is indexed under this profile yet — \
                 neither cache entries nor vectors. Fix: a plain index_codebase run"
                    .to_string()
            } else {
                format!(
                    "Coverage unknown: store holds {} distinct files, but the metadata \
                     cache holds no entries for this profile (cleared, or written before \
                     the cache salt changed), so there is nothing to compare them against. \
                     This is lost bookkeeping, NOT evidence of a damaged index: a \
                     half-finished run would have left cache entries. Fix: a plain \
                     index_codebase run refills the cache for the files it touches; \
                     force_reindex is not indicated by this state",
                    files_with_vectors
                )
            };
            let mut unknown = CoverageHealth::unknown(message);
            unknown.files_tracked = files_tracked;
            unknown.files_cached = Some(0);
            unknown.files_with_vectors = Some(files_with_vectors);
            return unknown;
        }

        let outside_count = files_outside_tree.unwrap_or(0);
        let (status, mut message) = if stale_skips == 0 {
            (
                if outside_count == 0 {
                    Status::Healthy
                } else {
                    Status::Degraded
                },
                format!(
                    "Coverage verified: all {} cached files have vectors ({} distinct files in store)",
                    files_cached, files_with_vectors
                ),
            )
        } else {
            (
                Status::Degraded,
                format!(
                    "Index incomplete: {} of {} files are cached as indexed but have NO vectors — \
                     the next run will skip them as unchanged. Fix: index_codebase with force_reindex: true",
                    stale_skips, files_cached
                ),
            )
        };
        if outside_count > 0 {
            message.push_str(&format!(
                "; the store still holds chunks of {} file(s) no longer in the tree, and search \
                 can answer with them. Fix: index_codebase with repair_coverage: true",
                outside_count
            ));
        }

        CoverageHealth {
            status,
            message,
            files_tracked,
            files_cached: Some(files_cached),
            files_with_vectors: Some(files_with_vectors),
            stale_skips: Some(stale_skips),
            stale_skip_examples: examples,
            files_outside_tree,
            outside_tree_examples: outside.into_iter().take(10).collect(),
        }
    }

    /// Check index freshness (see [`FreshnessHealth`])
    ///
    /// Cost is one stat per project file plus a read of the files whose mtime
    /// moved — tens of milliseconds on a clean tree, seconds right after a
    /// branch switch. Deliberately paid: the alternative is the probe that
    /// cannot fail, which is what this replaces.
    async fn check_freshness(&self) -> FreshnessHealth {
        let Some(root) = &self.project_root else {
            return FreshnessHealth::unknown(
                "Freshness unknown: no project directory (system-wide check; pass 'directory')",
            );
        };

        let merkle = match FileSystemMerkle::load_snapshot(&self.merkle_path) {
            Ok(Some(merkle)) => merkle,
            Ok(None) => {
                return FreshnessHealth::unknown(format!(
                    "Freshness unknown: no Merkle snapshot at {} — nothing to compare the working tree against (project never indexed under this profile?)",
                    self.merkle_path.display()
                ));
            }
            Err(e) => {
                return FreshnessHealth::unknown(format!(
                    "Freshness unknown: cannot read Merkle snapshot at {}: {}",
                    self.merkle_path.display(),
                    e
                ));
            }
        };

        let changes = match merkle.detect_disk_changes(root) {
            Ok(changes) => changes,
            Err(e) => {
                return FreshnessHealth::unknown(format!(
                    "Freshness unknown: cannot compare {} against the snapshot: {}",
                    root.display(),
                    e
                ));
            }
        };

        let (added, modified, deleted) = (
            changes.added.len(),
            changes.modified.len(),
            changes.deleted.len(),
        );

        // Tagged examples: "which files" is useless for triage without
        // "what happened to them" — a deleted file and a new one call for
        // different reactions.
        let examples: Vec<String> = changes
            .modified
            .iter()
            .map(|p| format!("modified: {}", p.display()))
            .chain(
                changes
                    .added
                    .iter()
                    .map(|p| format!("added: {}", p.display())),
            )
            .chain(
                changes
                    .deleted
                    .iter()
                    .map(|p| format!("deleted: {}", p.display())),
            )
            .take(10)
            .collect();

        let (status, message) = if changes.is_empty() {
            (
                Status::Healthy,
                format!(
                    "Index is current: all {} tracked files match the working tree",
                    merkle.file_count()
                ),
            )
        } else {
            (
                Status::Degraded,
                format!(
                    "Index is STALE: {} modified, {} added, {} deleted since the last indexing run — \
                     search and symbol answers still describe the old code. Fix: a plain index_codebase run",
                    modified, added, deleted
                ),
            )
        };

        FreshnessHealth {
            status,
            message,
            files_added: Some(added),
            files_modified: Some(modified),
            files_deleted: Some(deleted),
            examples,
        }
    }

    /// Calculate overall system status from component statuses
    fn calculate_overall_status(
        &self,
        bm25: &ComponentHealth,
        vector: &ComponentHealth,
        merkle: &ComponentHealth,
        coverage: &CoverageHealth,
        freshness: &FreshnessHealth,
    ) -> Status {
        // Critical: both search engines must work
        let search_unhealthy =
            bm25.status == Status::Unhealthy && vector.status == Status::Unhealthy;

        if search_unhealthy {
            return Status::Unhealthy;
        }

        // Degraded: one search engine down OR merkle issues OR an
        // index that is silently incomplete OR one that no longer
        // describes the code. The last two are the whole point of the
        // coverage and freshness checks: without them a half-indexed or
        // days-old store answers "healthy" and quietly returns partial or
        // obsolete search results.
        let has_degraded = coverage.status != Status::Healthy
            || freshness.status != Status::Healthy
            || bm25.status == Status::Degraded
            || vector.status == Status::Degraded
            || merkle.status == Status::Degraded
            || bm25.status == Status::Unhealthy
            || vector.status == Status::Unhealthy;

        if has_degraded {
            return Status::Degraded;
        }

        // All healthy
        Status::Healthy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_component_health_constructors() {
        let healthy = ComponentHealth::healthy("All good", Some(50));
        assert_eq!(healthy.status, Status::Healthy);
        assert_eq!(healthy.latency_ms, Some(50));

        let degraded = ComponentHealth::degraded("Some issues");
        assert_eq!(degraded.status, Status::Degraded);
        assert_eq!(degraded.latency_ms, None);

        let unhealthy = ComponentHealth::unhealthy("Critical error");
        assert_eq!(unhealthy.status, Status::Unhealthy);
    }

    /// Coverage in the "checked and complete" state, for status-algebra tests.
    fn coverage_ok() -> CoverageHealth {
        CoverageHealth {
            status: Status::Healthy,
            message: "ok".to_string(),
            files_tracked: Some(3),
            files_cached: Some(3),
            files_with_vectors: Some(3),
            stale_skips: Some(0),
            stale_skip_examples: Vec::new(),
            files_outside_tree: Some(0),
            outside_tree_examples: Vec::new(),
        }
    }

    /// Freshness in the "checked and current" state, for status-algebra tests.
    fn freshness_ok() -> FreshnessHealth {
        FreshnessHealth {
            status: Status::Healthy,
            message: "ok".to_string(),
            files_added: Some(0),
            files_modified: Some(0),
            files_deleted: Some(0),
            examples: Vec::new(),
        }
    }

    #[test]
    fn test_overall_status_calculation() {
        let monitor = HealthMonitor {
            bm25: None,
            vector_store: None,
            merkle_path: PathBuf::from("/tmp/merkle.snapshot"),
            metadata_cache: None,
            project_root: None,
        };

        // All healthy
        let all_healthy = monitor.calculate_overall_status(
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
            &coverage_ok(),
            &freshness_ok(),
        );
        assert_eq!(all_healthy, Status::Healthy);

        // One degraded
        let one_degraded = monitor.calculate_overall_status(
            &ComponentHealth::degraded("issues"),
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
            &coverage_ok(),
            &freshness_ok(),
        );
        assert_eq!(one_degraded, Status::Degraded);

        // Both search engines down
        let both_down = monitor.calculate_overall_status(
            &ComponentHealth::unhealthy("down"),
            &ComponentHealth::unhealthy("down"),
            &ComponentHealth::healthy("ok", None),
            &coverage_ok(),
            &freshness_ok(),
        );
        assert_eq!(both_down, Status::Unhealthy);

        // One search engine down (still degraded, not unhealthy)
        let one_down = monitor.calculate_overall_status(
            &ComponentHealth::unhealthy("down"),
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
            &coverage_ok(),
            &freshness_ok(),
        );
        assert_eq!(one_down, Status::Degraded);

        // A stale index alone degrades the verdict. This is the regression
        // the freshness component exists for: everything else is green and
        // the answers are still obsolete.
        let stale = monitor.calculate_overall_status(
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
            &ComponentHealth::healthy("ok", None),
            &coverage_ok(),
            &FreshnessHealth {
                status: Status::Degraded,
                message: "stale".to_string(),
                files_added: Some(0),
                files_modified: Some(133),
                files_deleted: Some(0),
                examples: Vec::new(),
            },
        );
        assert_eq!(stale, Status::Degraded);
    }

    /// Build a store holding one vector for `file_with_vectors`, and a
    /// metadata cache claiming BOTH files are indexed. That is exactly
    /// the state a half-finished run leaves behind.
    async fn stale_skip_scene(temp: &TempDir, salt: &str) -> (Arc<VectorStore>, PathBuf) {
        use rmc_engine::chunker::{ChunkContext, ChunkId, CodeChunk};

        let store = VectorStore::new_embedded(temp.path().join("vectors"), 4, "test-embedder")
            .await
            .expect("open store");
        let chunk = CodeChunk {
            id: ChunkId::new(),
            content: "fn indexed() {}".to_string(),
            context: ChunkContext {
                file_path: PathBuf::from("/proj/indexed.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: "indexed".to_string(),
                symbol_kind: "function".to_string(),
                docstring: None,
                imports: vec![],
                outgoing_calls: vec![],
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
                line_start: 1,
                line_end: 1,
            },
        };
        store
            .insert_chunks(vec![(chunk.id, vec![0.1, 0.2, 0.3, 0.4], chunk.clone())])
            .await
            .expect("insert");

        let cache_path = temp.path().join("cache");
        let cache = MetadataCache::new(&cache_path).expect("cache");
        for path in ["/proj/indexed.rs", "/proj/skipped.rs"] {
            let key = if salt.is_empty() {
                path.to_string()
            } else {
                format!("{}::{}", salt, path)
            };
            cache
                .set(
                    &key,
                    &crate::metadata_cache::FileMetadata::from_content("x", 1, 1),
                )
                .expect("set metadata");
        }
        drop(cache);

        (Arc::new(store), cache_path)
    }

    #[tokio::test]
    async fn coverage_catches_file_cached_as_indexed_without_vectors() {
        let temp = TempDir::new().unwrap();
        let salt = "chunk-split:v1:target300:hard900";
        let (store, cache_path) = stale_skip_scene(&temp, salt).await;

        let monitor = HealthMonitor::new(None, Some(store), temp.path().join("missing.snapshot"))
            .with_metadata_cache(cache_path, salt.to_string());
        let health = monitor.check_health().await;

        assert_eq!(health.coverage.stale_skips, Some(1));
        assert_eq!(health.coverage.files_cached, Some(2));
        assert_eq!(health.coverage.files_with_vectors, Some(1));
        assert_eq!(
            health.coverage.stale_skip_examples,
            vec!["/proj/skipped.rs".to_string()]
        );
        assert_eq!(health.coverage.status, Status::Degraded);
        // The regression this whole component exists for: such an index
        // used to report `healthy`.
        assert_ne!(health.overall, Status::Healthy);
    }

    #[tokio::test]
    async fn coverage_is_healthy_when_every_cached_file_has_vectors() {
        let temp = TempDir::new().unwrap();
        let salt = "chunk-split:v1:target300:hard900";
        let (store, cache_path) = stale_skip_scene(&temp, salt).await;

        // Positive control: drop the bogus entry and the same scene must
        // flip to healthy — otherwise the assertion above would pass on a
        // component that is simply always degraded.
        let cache = MetadataCache::new(&cache_path).expect("cache");
        cache
            .remove(&format!("{}::{}", salt, "/proj/skipped.rs"))
            .expect("remove");
        drop(cache);

        let monitor = HealthMonitor::new(None, Some(store), temp.path().join("missing.snapshot"))
            .with_metadata_cache(cache_path, salt.to_string());
        let coverage = monitor.check_coverage().await;

        assert_eq!(coverage.stale_skips, Some(0));
        assert_eq!(coverage.status, Status::Healthy);
    }

    /// The store holds kept.rs and gone.rs, the tree only kept.rs: gone.rs
    /// is counted outside the tree and turns coverage degraded with the
    /// repair as the fix; kept.rs, in the same store, is the control.
    #[tokio::test]
    async fn coverage_counts_stored_files_that_left_the_tree() {
        use rmc_engine::chunker::{ChunkContext, ChunkId, CodeChunk};

        let temp = TempDir::new().unwrap();
        let root = temp.path().join("proj");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("kept.rs"), "fn kept() {}\n").unwrap();
        let salt = "chunk-split:v1:target300:hard900";

        let store = VectorStore::new_embedded(temp.path().join("vectors"), 4, "test-embedder")
            .await
            .expect("open store");
        let cache_path = temp.path().join("cache");
        let cache = MetadataCache::new(&cache_path).expect("cache");
        for name in ["kept", "gone"] {
            let path = root.join(format!("{name}.rs"));
            let chunk = CodeChunk {
                id: ChunkId::new(),
                content: format!("fn {name}() {{}}"),
                context: ChunkContext {
                    file_path: path.clone(),
                    module_path: vec!["crate".to_string()],
                    symbol_name: name.to_string(),
                    symbol_kind: "function".to_string(),
                    docstring: None,
                    imports: vec![],
                    outgoing_calls: vec![],
                    parent_symbol_name: None,
                    split_part: None,
                    split_total: None,
                    line_start: 1,
                    line_end: 1,
                },
            };
            store
                .insert_chunks(vec![(chunk.id, vec![0.1, 0.2, 0.3, 0.4], chunk.clone())])
                .await
                .expect("insert");
            cache
                .set(
                    &format!("{}::{}", salt, path.display()),
                    &crate::metadata_cache::FileMetadata::from_content("x", 1, 1),
                )
                .expect("set metadata");
        }
        drop(cache);

        let monitor = HealthMonitor::new(
            None,
            Some(Arc::new(store)),
            temp.path().join("missing.snapshot"),
        )
        .with_metadata_cache(cache_path, salt.to_string())
        .with_project_root(root.clone());
        let coverage = monitor.check_coverage().await;

        assert_eq!(coverage.stale_skips, Some(0));
        assert_eq!(coverage.files_with_vectors, Some(2));
        assert_eq!(coverage.files_outside_tree, Some(1));
        assert_eq!(
            coverage.outside_tree_examples,
            vec![root.join("gone.rs").to_string_lossy().to_string()]
        );
        assert_eq!(coverage.status, Status::Degraded);
        assert!(
            coverage.message.contains("repair_coverage"),
            "{}",
            coverage.message
        );
    }

    #[tokio::test]
    async fn coverage_with_wrong_salt_is_not_reported_as_verified() {
        let temp = TempDir::new().unwrap();
        let (store, cache_path) = stale_skip_scene(&temp, "salt-a").await;

        // Keys under another chunking identity must not be counted, and
        // an empty comparison must not read as "checked, all good".
        let monitor = HealthMonitor::new(None, Some(store), temp.path().join("missing.snapshot"))
            .with_metadata_cache(cache_path, "salt-b".to_string());
        let coverage = monitor.check_coverage().await;

        assert_eq!(coverage.files_cached, Some(0));
        // Vacuous "all 0 files are fine" must never read as healthy.
        assert_eq!(coverage.status, Status::Degraded);
        assert_eq!(coverage.stale_skips, None);
    }

    /// An empty cache next to a POPULATED store is lost bookkeeping, and
    /// the message must not push the reader towards `force_reindex` —
    /// that misread cost an hour of needless work on the live index
    /// (2026-08-19).
    #[tokio::test]
    async fn coverage_without_cache_but_with_vectors_does_not_advise_force_reindex() {
        let temp = TempDir::new().unwrap();
        let (store, cache_path) = stale_skip_scene(&temp, "salt-a").await;

        let monitor = HealthMonitor::new(None, Some(store), temp.path().join("missing.snapshot"))
            .with_metadata_cache(cache_path, "salt-b".to_string());
        let coverage = monitor.check_coverage().await;

        assert_eq!(coverage.files_cached, Some(0));
        assert_eq!(coverage.files_with_vectors, Some(1));
        assert!(
            coverage.message.contains("force_reindex is not indicated"),
            "message must say force_reindex is NOT the fix here: {}",
            coverage.message
        );
        assert!(
            !coverage.message.contains("force_reindex: true"),
            "message must not carry the force_reindex recipe: {}",
            coverage.message
        );
    }

    /// Positive control for the test above: the SAME empty cache over an
    /// EMPTY store must produce a different message. Without this, the
    /// assertion above would pass on a component that prints one text
    /// unconditionally — which is exactly the defect being fixed.
    #[tokio::test]
    async fn coverage_with_empty_cache_and_empty_store_says_nothing_is_indexed() {
        let temp = TempDir::new().unwrap();
        let store = VectorStore::new_embedded(temp.path().join("vectors"), 4, "test-embedder")
            .await
            .expect("open store");
        let cache_path = temp.path().join("cache");
        MetadataCache::new(&cache_path).expect("cache");

        let monitor = HealthMonitor::new(
            None,
            Some(Arc::new(store)),
            temp.path().join("missing.snapshot"),
        )
        .with_metadata_cache(cache_path, "salt-a".to_string());
        let coverage = monitor.check_coverage().await;

        assert_eq!(coverage.files_cached, Some(0));
        assert_eq!(coverage.files_with_vectors, Some(0));
        assert!(
            coverage
                .message
                .contains("nothing is indexed under this profile yet"),
            "empty store must read as never-indexed, not as lost bookkeeping: {}",
            coverage.message
        );
        assert_eq!(coverage.status, Status::Degraded);
    }

    /// A project with one file, indexed: snapshot on disk, tree untouched.
    fn indexed_project(temp: &TempDir) -> (PathBuf, PathBuf) {
        let root = temp.path().join("proj");
        std::fs::create_dir_all(root.join("src")).expect("create src");
        std::fs::write(root.join("src/lib.rs"), "pub fn one() {}\n").expect("write");

        let snapshot_path = temp.path().join("merkle.snapshot");
        FileSystemMerkle::from_directory(&root)
            .expect("build merkle")
            .save_snapshot(&snapshot_path)
            .expect("save snapshot");

        (root, snapshot_path)
    }

    #[tokio::test]
    async fn freshness_is_healthy_when_the_tree_matches_the_snapshot() {
        let temp = TempDir::new().unwrap();
        let (root, snapshot_path) = indexed_project(&temp);

        let freshness = HealthMonitor::new(None, None, snapshot_path)
            .with_project_root(root)
            .check_freshness()
            .await;

        assert_eq!(freshness.status, Status::Healthy);
        assert_eq!(freshness.files_modified, Some(0));
        assert_eq!(freshness.files_added, Some(0));
        assert_eq!(freshness.files_deleted, Some(0));
    }

    /// The defect this component exists for: an edit after indexing left every
    /// other component green, so `health_check` could not report it at all.
    #[tokio::test]
    async fn freshness_catches_an_edit_made_after_indexing() {
        let temp = TempDir::new().unwrap();
        let (root, snapshot_path) = indexed_project(&temp);

        std::fs::write(
            root.join("src/lib.rs"),
            "pub fn one() {}\npub fn two() {}\n",
        )
        .expect("edit");
        std::fs::write(root.join("src/extra.rs"), "pub fn three() {}\n").expect("add");

        let freshness = HealthMonitor::new(None, None, snapshot_path)
            .with_project_root(root)
            .check_freshness()
            .await;

        assert_eq!(freshness.status, Status::Degraded);
        assert_eq!(freshness.files_modified, Some(1));
        assert_eq!(freshness.files_added, Some(1));
        assert!(
            freshness
                .examples
                .iter()
                .any(|e| e.starts_with("modified: ")),
            "examples must say what happened to each file: {:?}",
            freshness.examples
        );
    }

    #[tokio::test]
    async fn freshness_catches_a_file_deleted_after_indexing() {
        let temp = TempDir::new().unwrap();
        let (root, snapshot_path) = indexed_project(&temp);

        std::fs::remove_file(root.join("src/lib.rs")).expect("delete");

        let freshness = HealthMonitor::new(None, None, snapshot_path)
            .with_project_root(root)
            .check_freshness()
            .await;

        assert_eq!(freshness.status, Status::Degraded);
        assert_eq!(freshness.files_deleted, Some(1));
    }

    /// Timestamps must not decide. A rebuild or `git checkout` moves mtimes
    /// across the whole tree without changing a byte; a probe that reported
    /// that as "stale" would be ignored within a day.
    #[tokio::test]
    async fn freshness_ignores_a_touched_but_unchanged_file() {
        let temp = TempDir::new().unwrap();
        let (root, snapshot_path) = indexed_project(&temp);

        // Rewrite the same bytes: content identical, mtime moved.
        let path = root.join("src/lib.rs");
        let indexed_at = std::fs::metadata(&path).unwrap().modified().unwrap();
        let same = std::fs::read(&path).expect("read");
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&path, &same).expect("rewrite");
        assert_ne!(
            std::fs::metadata(&path).unwrap().modified().unwrap(),
            indexed_at,
            "positive control: the rewrite must actually move the mtime, \
             otherwise this test passes without exercising the hash path"
        );

        let freshness = HealthMonitor::new(None, None, snapshot_path)
            .with_project_root(root)
            .check_freshness()
            .await;

        assert_eq!(freshness.status, Status::Healthy, "{}", freshness.message);
    }

    #[tokio::test]
    async fn freshness_without_a_snapshot_is_degraded_not_healthy() {
        let temp = TempDir::new().unwrap();

        let freshness = HealthMonitor::new(None, None, temp.path().join("missing.snapshot"))
            .with_project_root(temp.path().to_path_buf())
            .check_freshness()
            .await;

        assert_eq!(freshness.status, Status::Degraded);
        assert_eq!(freshness.files_modified, None);
    }

    #[tokio::test]
    async fn freshness_without_a_project_root_is_degraded_not_healthy() {
        let temp = TempDir::new().unwrap();
        let (_root, snapshot_path) = indexed_project(&temp);

        let freshness = HealthMonitor::new(None, None, snapshot_path)
            .check_freshness()
            .await;

        assert_eq!(freshness.status, Status::Degraded);
        assert!(freshness.message.contains("pass 'directory'"));
    }

    #[test]
    fn coverage_without_metadata_cache_is_degraded_not_healthy() {
        let coverage = CoverageHealth::unknown("no cache");
        assert_eq!(coverage.status, Status::Degraded);
        assert_eq!(coverage.stale_skips, None);
    }

    #[test]
    fn test_health_status_serialization() {
        let status = HealthStatus {
            overall: Status::Healthy,
            bm25: ComponentHealth::healthy("BM25 operational", Some(15)),
            vector: ComponentHealth::healthy("Vector operational", Some(42)),
            merkle: ComponentHealth::healthy("Merkle snapshot exists (2048 bytes)", None),
            coverage: coverage_ok(),
            freshness: freshness_ok(),
        };

        let json = serde_json::to_string_pretty(&status).unwrap();
        assert!(json.contains("\"overall\": \"healthy\""));
        assert!(json.contains("\"latency_ms\": 15"));
        assert!(json.contains("\"latency_ms\": 42"));
    }
}

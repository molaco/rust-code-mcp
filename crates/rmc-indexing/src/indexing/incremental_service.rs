//! Server-facing incremental indexing facade.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rmc_engine::embeddings::EmbeddingBackend;

use crate::indexing::incremental::IncrementalIndexer;
use crate::indexing::unified::IndexStats;

/// Request for an incremental indexing run.
pub struct IncrementalIndexRequest<'a> {
    pub codebase_path: &'a Path,
    pub cache_path: &'a Path,
    pub tantivy_path: &'a Path,
    pub collection_name: &'a str,
    pub backend: EmbeddingBackend,
    pub embedder_identity: &'a str,
    pub snapshot_path: Option<&'a Path>,
    pub codebase_loc: Option<usize>,
    pub force_reindex: bool,
}

/// Result of an incremental indexing run.
#[derive(Debug)]
pub struct IncrementalIndexOutcome {
    pub stats: IndexStats,
    pub elapsed: Duration,
}

/// Index a project through the indexing-owned incremental service boundary.
pub async fn index_project_incrementally(
    request: IncrementalIndexRequest<'_>,
) -> Result<IncrementalIndexOutcome> {
    let factory = RealIncrementalIndexerFactory;
    index_project_incrementally_with_factory(request, &factory).await
}

trait IncrementalIndexRunner {
    async fn clear_all_data(&mut self) -> Result<()>;
    async fn index_with_change_detection(&mut self, codebase_path: &Path) -> Result<IndexStats>;
}

impl IncrementalIndexRunner for IncrementalIndexer {
    async fn clear_all_data(&mut self) -> Result<()> {
        IncrementalIndexer::clear_all_data(self).await
    }

    async fn index_with_change_detection(&mut self, codebase_path: &Path) -> Result<IndexStats> {
        IncrementalIndexer::index_with_change_detection(self, codebase_path).await
    }
}

trait IncrementalIndexerFactory {
    type Indexer: IncrementalIndexRunner;

    async fn create(&self, request: &IncrementalIndexRequest<'_>) -> Result<Self::Indexer>;
}

struct RealIncrementalIndexerFactory;

impl IncrementalIndexerFactory for RealIncrementalIndexerFactory {
    type Indexer = IncrementalIndexer;

    async fn create(&self, request: &IncrementalIndexRequest<'_>) -> Result<Self::Indexer> {
        IncrementalIndexer::with_backend(
            request.cache_path,
            request.tantivy_path,
            request.collection_name,
            request.backend.dim(),
            request.embedder_identity,
            request.codebase_loc,
            request.backend.clone(),
        )
        .await
    }
}

async fn index_project_incrementally_with_factory<F>(
    request: IncrementalIndexRequest<'_>,
    factory: &F,
) -> Result<IncrementalIndexOutcome>
where
    F: IncrementalIndexerFactory,
{
    if request.force_reindex {
        if let Some(snapshot_path) = request.snapshot_path {
            if snapshot_path.exists() {
                tracing::info!("Force reindex: deleting snapshot at {}", snapshot_path.display());
                std::fs::remove_file(snapshot_path).with_context(|| {
                    format!("Failed to delete snapshot at {}", snapshot_path.display())
                })?;
            }
        }
    }

    let mut indexer = factory.create(&request).await?;

    if request.force_reindex {
        tracing::info!("Force reindex: clearing all indexed data");
        indexer.clear_all_data().await?;
    }

    let start = Instant::now();
    let stats = indexer
        .index_with_change_detection(request.codebase_path)
        .await?;

    Ok(IncrementalIndexOutcome {
        stats,
        elapsed: start.elapsed(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    #[derive(Default)]
    struct FakeState {
        events: Vec<String>,
        build: Option<BuildRecord>,
        create_error: Option<&'static str>,
        clear_error: Option<&'static str>,
        index_error: Option<&'static str>,
        stats: IndexStats,
    }

    struct BuildRecord {
        codebase_path: PathBuf,
        cache_path: PathBuf,
        tantivy_path: PathBuf,
        collection_name: String,
        backend_dim: usize,
        embedder_identity: String,
        snapshot_path: Option<PathBuf>,
        codebase_loc: Option<usize>,
        force_reindex: bool,
    }

    #[derive(Clone, Default)]
    struct FakeFactory {
        state: Arc<Mutex<FakeState>>,
    }

    struct FakeIndexer {
        state: Arc<Mutex<FakeState>>,
    }

    impl IncrementalIndexerFactory for FakeFactory {
        type Indexer = FakeIndexer;

        async fn create(&self, request: &IncrementalIndexRequest<'_>) -> Result<Self::Indexer> {
            let mut state = self.state.lock().unwrap();
            state.events.push("create".to_string());
            if let Some(message) = state.create_error {
                return Err(anyhow!(message));
            }
            state.build = Some(BuildRecord {
                codebase_path: request.codebase_path.to_path_buf(),
                cache_path: request.cache_path.to_path_buf(),
                tantivy_path: request.tantivy_path.to_path_buf(),
                collection_name: request.collection_name.to_string(),
                backend_dim: request.backend.dim(),
                embedder_identity: request.embedder_identity.to_string(),
                snapshot_path: request.snapshot_path.map(Path::to_path_buf),
                codebase_loc: request.codebase_loc,
                force_reindex: request.force_reindex,
            });

            Ok(FakeIndexer {
                state: Arc::clone(&self.state),
            })
        }
    }

    impl IncrementalIndexRunner for FakeIndexer {
        async fn clear_all_data(&mut self) -> Result<()> {
            let mut state = self.state.lock().unwrap();
            state.events.push("clear".to_string());
            if let Some(message) = state.clear_error {
                return Err(anyhow!(message));
            }
            Ok(())
        }

        async fn index_with_change_detection(&mut self, codebase_path: &Path) -> Result<IndexStats> {
            let mut state = self.state.lock().unwrap();
            state
                .events
                .push(format!("index:{}", codebase_path.display()));
            if let Some(message) = state.index_error {
                return Err(anyhow!(message));
            }
            Ok(state.stats.clone())
        }
    }

    fn test_backend() -> EmbeddingBackend {
        EmbeddingBackend::from_profile_name("local-cpu-small").unwrap()
    }

    #[tokio::test]
    async fn force_reindex_deletes_snapshot_and_clears_before_indexing() {
        let temp_dir = TempDir::new().unwrap();
        let codebase_path = temp_dir.path().join("codebase");
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let snapshot_path = temp_dir.path().join("snapshot");
        std::fs::create_dir(&codebase_path).unwrap();
        std::fs::write(&snapshot_path, "old snapshot").unwrap();

        let backend = test_backend();
        let embedder_identity = backend.identity();
        let factory = FakeFactory::default();
        {
            let mut state = factory.state.lock().unwrap();
            state.stats.indexed_files = 2;
        }

        let outcome = index_project_incrementally_with_factory(
            IncrementalIndexRequest {
                codebase_path: &codebase_path,
                cache_path: &cache_path,
                tantivy_path: &tantivy_path,
                collection_name: "test_collection",
                backend,
                embedder_identity: &embedder_identity,
                snapshot_path: Some(&snapshot_path),
                codebase_loc: None,
                force_reindex: true,
            },
            &factory,
        )
        .await
        .unwrap();

        let state = factory.state.lock().unwrap();
        assert!(!snapshot_path.exists());
        assert_eq!(outcome.stats.indexed_files, 2);
        assert_eq!(
            state.events,
            vec![
                "create".to_string(),
                "clear".to_string(),
                format!("index:{}", codebase_path.display())
            ]
        );
    }

    #[tokio::test]
    async fn passes_backend_construction_inputs_to_factory() {
        let temp_dir = TempDir::new().unwrap();
        let codebase_path = temp_dir.path().join("codebase");
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        let snapshot_path = temp_dir.path().join("snapshot");
        std::fs::create_dir(&codebase_path).unwrap();

        let backend = test_backend();
        let backend_dim = backend.dim();
        let embedder_identity = backend.identity();
        let factory = FakeFactory::default();

        index_project_incrementally_with_factory(
            IncrementalIndexRequest {
                codebase_path: &codebase_path,
                cache_path: &cache_path,
                tantivy_path: &tantivy_path,
                collection_name: "construction_inputs",
                backend,
                embedder_identity: &embedder_identity,
                snapshot_path: Some(&snapshot_path),
                codebase_loc: Some(42),
                force_reindex: false,
            },
            &factory,
        )
        .await
        .unwrap();

        let state = factory.state.lock().unwrap();
        let build = state.build.as_ref().unwrap();
        assert_eq!(build.codebase_path, codebase_path);
        assert_eq!(build.cache_path, cache_path);
        assert_eq!(build.tantivy_path, tantivy_path);
        assert_eq!(build.collection_name, "construction_inputs");
        assert_eq!(build.backend_dim, backend_dim);
        assert_eq!(build.embedder_identity, embedder_identity);
        assert_eq!(build.snapshot_path.as_deref(), Some(snapshot_path.as_path()));
        assert_eq!(build.codebase_loc, Some(42));
        assert!(!build.force_reindex);
    }

    #[tokio::test]
    async fn propagates_factory_clear_and_index_errors() {
        let temp_dir = TempDir::new().unwrap();
        let codebase_path = temp_dir.path().join("codebase");
        let cache_path = temp_dir.path().join("cache");
        let tantivy_path = temp_dir.path().join("tantivy");
        std::fs::create_dir(&codebase_path).unwrap();

        let factory = FakeFactory::default();
        {
            let mut state = factory.state.lock().unwrap();
            state.create_error = Some("create failed");
        }
        let backend = test_backend();
        let embedder_identity = backend.identity();
        let error = index_project_incrementally_with_factory(
            IncrementalIndexRequest {
                codebase_path: &codebase_path,
                cache_path: &cache_path,
                tantivy_path: &tantivy_path,
                collection_name: "error_collection",
                backend,
                embedder_identity: &embedder_identity,
                snapshot_path: None,
                codebase_loc: None,
                force_reindex: false,
            },
            &factory,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "create failed");

        let factory = FakeFactory::default();
        {
            let mut state = factory.state.lock().unwrap();
            state.clear_error = Some("clear failed");
        }
        let backend = test_backend();
        let embedder_identity = backend.identity();
        let error = index_project_incrementally_with_factory(
            IncrementalIndexRequest {
                codebase_path: &codebase_path,
                cache_path: &cache_path,
                tantivy_path: &tantivy_path,
                collection_name: "error_collection",
                backend,
                embedder_identity: &embedder_identity,
                snapshot_path: None,
                codebase_loc: None,
                force_reindex: true,
            },
            &factory,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "clear failed");

        let factory = FakeFactory::default();
        {
            let mut state = factory.state.lock().unwrap();
            state.index_error = Some("index failed");
        }
        let backend = test_backend();
        let embedder_identity = backend.identity();
        let error = index_project_incrementally_with_factory(
            IncrementalIndexRequest {
                codebase_path: &codebase_path,
                cache_path: &cache_path,
                tantivy_path: &tantivy_path,
                collection_name: "error_collection",
                backend,
                embedder_identity: &embedder_identity,
                snapshot_path: None,
                codebase_loc: None,
                force_reindex: false,
            },
            &factory,
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), "index failed");
    }
}

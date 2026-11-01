//! Tantivy adapter for BM25 keyword indexing
//!
//! This module encapsulates all Tantivy-specific operations, providing a clean
//! interface for indexing and searching code chunks using the BM25 algorithm.
//!
//! ## Overview
//!
//! The `TantivyAdapter` provides:
//! - **Thread-safe indexing**: Configurable writer with multi-threading support
//! - **Batch operations**: Efficient bulk indexing of code chunks
//! - **File-based deletion**: Remove all chunks for modified/deleted files
//! - **BM25 search creation**: Build search instances from the index
//!
//! ## Architecture
//!
//! ```text
//! TantivyAdapter
//!     ├─ Index (persistent on disk)
//!     ├─ IndexWriter (exclusive, one per adapter)
//!     └─ ChunkSchema (field definitions)
//! ```
//!
//! ## Refactoring Notes
//!
//! This module was extracted during Phase 2 refactoring to separate concerns:
//! - Moved from `unified.rs` (1047 LOC → 743 LOC)
//! - Encapsulates all Tantivy-specific logic
//! - Reduces coupling with vector store and embedding components
//!
//! ## Examples
//!
//! ```rust,no_run
//! use rmc_indexing::indexing::TantivyAdapter;
//! use rmc_config::config::TantivyConfig;
//! use std::path::Path;
//!
//! # fn example() -> anyhow::Result<()> {
//! // Create adapter with configuration
//! let config = TantivyConfig::default(Path::new("./tantivy"));
//! let mut adapter = TantivyAdapter::new(config)?;
//!
//! // Index chunks (chunks from ChunkSchema)
//! // adapter.index_chunks(&chunks)?;
//!
//! // Commit changes
//! adapter.commit()?;
//!
//! // Create BM25 search instance
//! let bm25 = adapter.create_bm25_search()?;
//! # Ok(())
//! # }
//! ```

use rmc_engine::chunker::CodeChunk;
use rmc_config::config::TantivyConfig;
use rmc_engine::schema::ChunkSchema;
use rmc_engine::search::Bm25Search;
use anyhow::{Context, Result};
use std::path::Path;
use std::time::Instant;
use tantivy::indexer::LogMergePolicy;
use tantivy::{Index, IndexWriter};

/// Share of deleted documents above which a segment is merged even alone.
///
/// The incremental indexer rewrites a changed file by deleting all its chunks
/// and adding new ones, so on a tree under constant edits every segment
/// accumulates deleted documents. tantivy's default (`1.0`) never merges for
/// deletes: on a live daemon the index reached 71 % deleted documents, with
/// the largest segments 64–87 % dead, and every merge copied the dead ones
/// along. `0.3` bounds the dead share of any one segment to under a third —
/// the space and merge cost a segment carries for its deletes stays a fraction
/// of its live data — while a segment is not rewritten for the first few
/// edits of its files.
const DEL_DOCS_RATIO_ENV: &str = "RMC_BM25_DEL_DOCS_RATIO";
const DEFAULT_DEL_DOCS_RATIO: f32 = 0.3;

/// Segments of one size level that trigger a merge of that level.
///
/// tantivy's own default, stated here so that it is a decision and a knob,
/// not an accident. Lower merges more often (more CPU, fewer segments to
/// search); higher lets segments pile up between merges.
const MIN_NUM_SEGMENTS_ENV: &str = "RMC_BM25_MIN_NUM_SEGMENTS";
const DEFAULT_MIN_NUM_SEGMENTS: usize = 8;

/// Segments below this many documents count as one size level.
///
/// tantivy's default as well. Every background sync commits a segment of a
/// few hundred documents; with levels by log size alone each of those would
/// be its own level and never reach [`DEFAULT_MIN_NUM_SEGMENTS`] companions.
/// Grouping everything under 10 000 documents makes the small commits merge
/// together after a handful of syncs.
const MIN_LAYER_SIZE_ENV: &str = "RMC_BM25_MIN_LAYER_SIZE";
const DEFAULT_MIN_LAYER_SIZE: u32 = 10_000;

/// Read a knob: unset is the default, set-but-unparsable is an error — a
/// typo must not quietly run the index on the default it was meant to change.
fn knob<T: std::str::FromStr>(name: &str, default: T) -> Result<T>
where
    T::Err: std::fmt::Display,
{
    match std::env::var(name) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|e| anyhow::anyhow!("{name}={value:?} is not valid: {e}")),
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(e) => Err(anyhow::anyhow!("{name}: {e}")),
    }
}

/// The merge policy every BM25 writer runs with; see the constants above.
fn merge_policy() -> Result<LogMergePolicy> {
    let del_docs_ratio = knob(DEL_DOCS_RATIO_ENV, DEFAULT_DEL_DOCS_RATIO)?;
    // tantivy asserts this range; checking it here turns a panic in the sync
    // task into an error naming the knob.
    anyhow::ensure!(
        del_docs_ratio > 0.0 && del_docs_ratio <= 1.0,
        "{DEL_DOCS_RATIO_ENV}={del_docs_ratio} must be in (0, 1]"
    );
    let mut policy = LogMergePolicy::default();
    policy.set_del_docs_ratio_before_merge(del_docs_ratio);
    policy.set_min_num_segments(knob(MIN_NUM_SEGMENTS_ENV, DEFAULT_MIN_NUM_SEGMENTS)?);
    policy.set_min_layer_size(knob(MIN_LAYER_SIZE_ENV, DEFAULT_MIN_LAYER_SIZE)?);
    Ok(policy)
}

/// Adapter for Tantivy BM25 indexing operations
pub struct TantivyAdapter {
    /// Tantivy index
    index: Index,
    /// Index writer. `Some` for the adapter's whole life; taken only by
    /// `Drop`, which has to consume it to wait for its merges.
    writer: Option<IndexWriter>,
    /// Schema for code chunks
    schema: ChunkSchema,
}

impl TantivyAdapter {
    /// Create a new TantivyAdapter with configuration
    pub fn new(config: TantivyConfig) -> Result<Self> {
        let schema = ChunkSchema::new();

        // Open or create index
        let index = if config.index_path.join("meta.json").exists() {
            Index::open_in_dir(&config.index_path)
                .context("Failed to open Tantivy index")?
        } else {
            std::fs::create_dir_all(&config.index_path)
                .context("Failed to create Tantivy directory")?;
            Index::create_in_dir(&config.index_path, schema.schema())
                .context("Failed to create Tantivy index")?
        };

        // Calculate total memory budget
        let total_memory_budget =
            (config.memory_budget_mb * config.num_threads * 1024 * 1024) as usize;

        // Create writer with configuration
        let writer: IndexWriter = index
            .writer_with_num_threads(config.num_threads, total_memory_budget)
            .context("Failed to create Tantivy writer")?;
        writer.set_merge_policy(Box::new(merge_policy()?));

        tracing::info!(
            "Tantivy configured: {}MB total budget, {} threads",
            config.memory_budget_mb * config.num_threads,
            config.num_threads
        );

        Ok(Self {
            index,
            writer: Some(writer),
            schema,
        })
    }

    /// Whether the index in `index_path` has no committed live documents —
    /// including when there is no index there at all. Reads `meta.json`
    /// only: no writer, no lock, so it is cheap enough for every sync tick.
    pub fn is_empty_at(index_path: &Path) -> Result<bool> {
        if !index_path.join("meta.json").exists() {
            return Ok(true);
        }
        let index = Index::open_in_dir(index_path).context("Failed to open Tantivy index")?;
        let metas = index
            .searchable_segment_metas()
            .context("Failed to read Tantivy segment metas")?;
        Ok(metas.iter().all(|meta| meta.num_docs() == 0))
    }

    /// Live documents in the last commit.
    ///
    /// Counted from the commit, not from whether the directory existed:
    /// creating an index writes its `meta.json` at once, so "the directory
    /// was there" also holds after a rebuild that died before its commit.
    pub fn committed_doc_count(&self) -> Result<u64> {
        let metas = self
            .index
            .searchable_segment_metas()
            .context("Failed to read Tantivy segment metas")?;
        Ok(metas.iter().map(|meta| u64::from(meta.num_docs())).sum())
    }

    fn writer(&mut self) -> &mut IndexWriter {
        self.writer
            .as_mut()
            .expect("the writer is only taken by Drop")
    }

    /// Index a single chunk to Tantivy
    pub fn index_chunk(&mut self, chunk: &CodeChunk) -> Result<()> {
        let document = self.schema.document(chunk);
        self.writer()
            .add_document(document)
            .context("Failed to add document to Tantivy")?;

        Ok(())
    }

    /// Index multiple chunks to Tantivy
    pub fn index_chunks(&mut self, chunks: &[CodeChunk]) -> Result<()> {
        for chunk in chunks {
            self.index_chunk(chunk)?;
        }
        Ok(())
    }

    /// Delete all chunks for a specific file
    pub fn delete_file_chunks(&mut self, file_path: &Path) -> Result<()> {
        let file_path_str = file_path.to_string_lossy().to_string();

        let term = tantivy::Term::from_field_text(
            self.schema.file_path,
            &file_path_str,
        );
        let query = tantivy::query::TermQuery::new(
            term,
            tantivy::schema::IndexRecordOption::Basic,
        );

        self.writer().delete_query(Box::new(query))?;

        tracing::debug!("Deleted Tantivy chunks for file: {}", file_path_str);
        Ok(())
    }

    /// Delete all documents from the index
    pub fn delete_all(&mut self) -> Result<()> {
        self.writer().delete_all_documents()
            .context("Failed to delete all Tantivy documents")?;
        Ok(())
    }

    /// Commit all pending changes
    pub fn commit(&mut self) -> Result<()> {
        self.writer()
            .commit()
            .context("Failed to commit Tantivy index")?;
        Ok(())
    }

    /// Rollback uncommitted changes
    ///
    /// Also abandons every merge in flight: tantivy's rollback kills the
    /// writer's segment updater and replaces the writer, and a merge that
    /// finishes on a killed updater has its result thrown away.
    pub fn rollback(&mut self) -> Result<()> {
        self.writer()
            .rollback()
            .context("Failed to rollback Tantivy writer")?;
        Ok(())
    }

    /// Get reference to the Tantivy index
    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Get reference to the schema
    pub fn schema(&self) -> &ChunkSchema {
        &self.schema
    }

    /// Create a Bm25Search instance from this adapter
    pub fn create_bm25_search(&self) -> Result<Bm25Search> {
        Bm25Search::from_index(self.index.clone())
            .map_err(|e| anyhow::anyhow!("Failed to create Bm25Search: {}", e))
    }
}

/// Let the writer's merges finish, then release it.
///
/// This used to be `rollback()`, to "release the lock". Dropping the writer
/// releases the lock by itself; what the rollback did on top was kill the
/// segment updater, and with it every merge the writer had started. Those
/// merges do not stop — they run to the end on their own threads, and their
/// result is discarded when it reaches the dead updater. One adapter lives
/// for one background sync, so every sync started merges and threw them away:
/// a live daemon had three writers' `merge_thread_0` busy at once, 555 % CPU
/// between them, and an index of 662 segments that no merge ever got to
/// replace.
///
/// Waiting blocks the dropping thread for as long as the merges take. That is
/// the cost of a merge being useful at all, and the workspace's exclusive
/// lock, held by the caller across the sync, already keeps every other writer
/// out for that time. Uncommitted documents are dropped either way: waiting
/// does not commit.
impl Drop for TantivyAdapter {
    fn drop(&mut self) {
        let Some(writer) = self.writer.take() else {
            return;
        };
        let started = Instant::now();
        match writer.wait_merging_threads() {
            Ok(()) => tracing::debug!(
                waited_ms = started.elapsed().as_millis() as u64,
                "Tantivy writer released after its merges finished"
            ),
            Err(e) => tracing::warn!("Tantivy merges failed while releasing the writer: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_tantivy_adapter_creation() {
        let temp_dir = TempDir::new().unwrap();
        let config = TantivyConfig::default(temp_dir.path());

        let adapter = TantivyAdapter::new(config);
        assert!(adapter.is_ok(), "Failed to create TantivyAdapter: {:?}", adapter.err());
    }

    use crate::indexing::index_stats::probe_bm25_index;
    use rmc_engine::chunker::{ChunkContext, ChunkId};
    use std::path::PathBuf;
    use std::time::Duration;

    const CHUNKS_PER_FILE: usize = 30;

    fn chunk(file: usize, part: usize, generation: usize) -> CodeChunk {
        CodeChunk {
            id: ChunkId::new(),
            content: format!("fn f{file}_{part}() {{ generation_{generation} }}"),
            context: ChunkContext {
                file_path: PathBuf::from(format!("src/f{file}.rs")),
                module_path: vec!["crate".to_string()],
                symbol_name: format!("f{file}_{part}"),
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
        }
    }

    /// What one background sync does to the index: a fresh adapter, every
    /// changed file deleted and written again, one commit, adapter dropped.
    fn sync_rewriting(dir: &Path, files: impl Iterator<Item = usize>, generation: usize) {
        let mut adapter = TantivyAdapter::new(TantivyConfig::default(dir)).unwrap();
        for file in files {
            adapter
                .delete_file_chunks(Path::new(&format!("src/f{file}.rs")))
                .unwrap();
            let chunks: Vec<_> = (0..CHUNKS_PER_FILE)
                .map(|part| chunk(file, part, generation))
                .collect();
            adapter.index_chunks(&chunks).unwrap();
        }
        adapter.commit().unwrap();
    }

    /// The gate of the merge policy: a large segment whose files are all
    /// rewritten one sync at a time.
    ///
    /// tantivy's `LogMergePolicy` merges by count within a size level, and it
    /// sizes a segment by its LIVE documents. So a segment is only out of
    /// count-based reach while it stays large: here two segments of ~40 500
    /// documents (two indexing threads), far above `min_layer_size`, of which
    /// 40 % of the files are rewritten in six syncs. Their small segments
    /// merge among themselves, but never gather eight companions on the large
    /// segments' level, so only the deleted-documents threshold expunges the
    /// large ones. Without it (tantivy's default `1.0`, the mutant:
    /// `RMC_BM25_DEL_DOCS_RATIO=1`) they keep their 40 % dead to the end —
    /// the index ends ~29 % dead; with it they are expunged past 30 %.
    #[test]
    fn deletes_do_not_pile_up_in_a_segment_no_count_based_merge_reaches() {
        let dir = TempDir::new().unwrap();
        const FILES: usize = 2700;
        const REWRITTEN_FILES: usize = FILES * 2 / 5;
        const SYNCS: usize = 6;
        sync_rewriting(dir.path(), 0..FILES, 0);

        let per_sync = REWRITTEN_FILES / SYNCS;
        for sync in 0..SYNCS {
            let first = sync * per_sync;
            sync_rewriting(dir.path(), first..first + per_sync, sync + 1);
        }

        let stats = probe_bm25_index(dir.path()).unwrap();
        assert_eq!(stats.live_docs, (FILES * CHUNKS_PER_FILE) as u64);
        let ratio = stats.deleted_ratio.unwrap();
        assert!(
            ratio < 0.15,
            "deleted share {ratio:.2} — deletes are not being merged away: {stats:#?}"
        );
        assert!(
            stats.segment_count <= 2 * DEFAULT_MIN_NUM_SEGMENTS,
            "{} segments: {stats:#?}",
            stats.segment_count
        );
    }

    /// Releasing the writer lets its merges land.
    ///
    /// Eight commits in one adapter are eight segments of one level, which
    /// the policy merges as the eighth arrives. The adapter is dropped right
    /// after: if the drop abandons the merge (the old `rollback()` drop, the
    /// mutant), the commit keeps its eight segments; waiting leaves one.
    #[test]
    fn dropping_the_adapter_finishes_its_merges() {
        let dir = TempDir::new().unwrap();
        let mut adapter = TantivyAdapter::new(TantivyConfig {
            index_path: dir.path().to_path_buf(),
            memory_budget_mb: 50,
            // One indexing thread: one segment per commit.
            num_threads: 1,
        })
        .unwrap();
        for file in 0..DEFAULT_MIN_NUM_SEGMENTS {
            let chunks: Vec<_> = (0..CHUNKS_PER_FILE).map(|part| chunk(file, part, 0)).collect();
            adapter.index_chunks(&chunks).unwrap();
            adapter.commit().unwrap();
        }
        drop(adapter);

        let stats = probe_bm25_index(dir.path()).unwrap();
        assert_eq!(stats.segment_count, 1, "{stats:#?}");
        assert_eq!(stats.live_docs, (DEFAULT_MIN_NUM_SEGMENTS * CHUNKS_PER_FILE) as u64);
    }

    /// Two daemons of one BM25 schema version share an index directory: a
    /// replaced binary's daemon lives on while its clients drain. The
    /// workspace lock is per process, so between daemons the writer lock is
    /// all there is. It must refuse at once — an error the sync logs and the
    /// next tick retries — rather than block the second writer behind the
    /// first's whole sync, and it must be free again once the first is gone.
    /// The lock is an `flock` on its own open file, so a second open in the
    /// same process contends exactly like one from another process.
    #[test]
    fn a_second_writer_on_the_directory_is_refused_not_queued() {
        let dir = TempDir::new().unwrap();
        let first = TantivyAdapter::new(TantivyConfig::default(dir.path())).unwrap();

        let started = Instant::now();
        let second = TantivyAdapter::new(TantivyConfig::default(dir.path()));
        let error = format!("{:#}", second.err().expect("the directory's writer is taken"));
        assert!(error.contains("Failed to create Tantivy writer"), "{error}");
        assert!(started.elapsed() < Duration::from_secs(1), "waited {:?}", started.elapsed());

        drop(first);
        TantivyAdapter::new(TantivyConfig::default(dir.path())).expect("released with the first");
    }

    #[test]
    fn an_unparsable_knob_is_an_error_not_the_default() {
        assert_eq!(knob::<f32>("RMC_TEST_KNOB_THAT_IS_NEVER_SET", 0.3).unwrap(), 0.3);
        // SAFETY: the variable is unique to this test; nothing else reads it.
        unsafe { std::env::set_var("RMC_TEST_KNOB_GARBAGE", "0,3") };
        assert!(knob::<f32>("RMC_TEST_KNOB_GARBAGE", 0.3).is_err());
    }

    #[test]
    fn test_config_for_codebase_size() {
        let temp_dir = TempDir::new().unwrap();

        // Small codebase
        let config = TantivyConfig::for_codebase_size(temp_dir.path(), Some(50_000));
        assert_eq!(config.memory_budget_mb, 50);
        assert_eq!(config.num_threads, 2);

        // Medium codebase
        let config = TantivyConfig::for_codebase_size(temp_dir.path(), Some(500_000));
        assert_eq!(config.memory_budget_mb, 100);
        assert_eq!(config.num_threads, 4);

        // Large codebase
        let config = TantivyConfig::for_codebase_size(temp_dir.path(), Some(2_000_000));
        assert_eq!(config.memory_budget_mb, 200);
        assert_eq!(config.num_threads, 8);
    }
}

# Advanced Research: Production-Ready Code Search Implementation

**Date:** 2025-10-19
**Status:** Extended Research Complete
**Purpose:** Deep dive into production considerations, optimizations, and advanced techniques

---

## Table of Contents

1. [Merkle Tree Integration Patterns](#1-merkle-tree-integration-patterns)
2. [Qdrant Production Tuning](#2-qdrant-production-tuning)
3. [RRF Optimization](#3-rrf-optimization)
4. [Tantivy Production Optimization](#4-tantivy-production-optimization)
5. [Testing & Evaluation Frameworks](#5-testing--evaluation-frameworks)
6. [Query Optimization Techniques](#6-query-optimization-techniques)
7. [Error Recovery & Resilience](#7-error-recovery--resilience)
8. [Security & Privacy Considerations](#8-security--privacy-considerations)
9. [Advanced Features](#9-advanced-features)
10. [Production Deployment Checklist](#10-production-deployment-checklist)

---

## 1. Merkle Tree Integration Patterns

### Current State of rs-merkle

**Library:** rs-merkle (antouhou/rs-merkle)
- **Features:** Transactional changes, Git-like rollback
- **Status:** Most advanced Merkle tree library for Rust
- **Gap:** No documented file watching integration

### Custom Integration Strategy

Since no existing integration patterns exist, we need to build our own:

#### Pattern 1: Snapshot-Based Detection

```rust
use rs_merkle::{MerkleTree, Hasher};
use sha2::{Sha256, Digest};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub struct FileSystemMerkle {
    tree: MerkleTree<Sha256Hasher>,
    file_to_leaf: HashMap<PathBuf, usize>,
    leaf_to_file: HashMap<usize, PathBuf>,
    snapshot_version: u64,
}

impl FileSystemMerkle {
    /// Build initial tree from directory
    pub fn from_directory(root: &Path) -> Result<Self> {
        let mut file_hashes = Vec::new();
        let mut file_to_leaf = HashMap::new();
        let mut leaf_to_file = HashMap::new();

        // Collect all files in deterministic order (sorted)
        let mut files: Vec<PathBuf> = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .filter(|e| e.path().extension() == Some(std::ffi::OsStr::new("rs")))
            .map(|e| e.path().to_path_buf())
            .collect();

        files.sort(); // Critical for consistency!

        for (idx, path) in files.iter().enumerate() {
            let content = std::fs::read(path)?;
            let hash = Sha256Hasher::hash(&content);

            file_hashes.push(hash);
            file_to_leaf.insert(path.clone(), idx);
            leaf_to_file.insert(idx, path.clone());
        }

        let tree = MerkleTree::<Sha256Hasher>::from_leaves(&file_hashes);

        Ok(Self {
            tree,
            file_to_leaf,
            leaf_to_file,
            snapshot_version: 1,
        })
    }

    /// Fast path: Check if any changes exist
    pub fn has_changes(&self, old: &Self) -> bool {
        self.tree.root() != old.tree.root()
    }

    /// Detect specific changed files
    pub fn detect_changes(&self, old: &Self) -> ChangeSet {
        if !self.has_changes(old) {
            return ChangeSet::empty();
        }

        let mut added = Vec::new();
        let mut modified = Vec::new();
        let mut deleted = Vec::new();

        // Find modified files (exist in both)
        for (path, &new_idx) in &self.file_to_leaf {
            if let Some(&old_idx) = old.file_to_leaf.get(path) {
                let new_leaf = self.tree.leaves().get(new_idx);
                let old_leaf = old.tree.leaves().get(old_idx);

                if new_leaf != old_leaf {
                    modified.push(path.clone());
                }
            } else {
                // File exists in new but not old
                added.push(path.clone());
            }
        }

        // Find deleted files (exist in old but not new)
        for path in old.file_to_leaf.keys() {
            if !self.file_to_leaf.contains_key(path) {
                deleted.push(path.clone());
            }
        }

        ChangeSet {
            added,
            modified,
            deleted,
        }
    }

    /// Incremental update: Modify single file in tree
    pub fn update_file(&mut self, path: &Path) -> Result<bool> {
        if let Some(&leaf_idx) = self.file_to_leaf.get(path) {
            // File exists, update its hash
            let content = std::fs::read(path)?;
            let new_hash = Sha256Hasher::hash(&content);

            // Get current leaves
            let mut leaves: Vec<[u8; 32]> = self.tree.leaves().to_vec();

            // Update the specific leaf
            leaves[leaf_idx] = new_hash;

            // Rebuild tree (efficient O(log n) operation)
            self.tree = MerkleTree::<Sha256Hasher>::from_leaves(&leaves);
            self.snapshot_version += 1;

            Ok(true)
        } else {
            // File doesn't exist, need full rebuild
            Ok(false)
        }
    }

    /// Add new file to tree
    pub fn add_file(&mut self, path: PathBuf) -> Result<()> {
        let content = std::fs::read(&path)?;
        let hash = Sha256Hasher::hash(&content);

        // Get current leaves
        let mut leaves: Vec<[u8; 32]> = self.tree.leaves().to_vec();
        let new_idx = leaves.len();

        // Append new leaf
        leaves.push(hash);

        // Update mappings
        self.file_to_leaf.insert(path.clone(), new_idx);
        self.leaf_to_file.insert(new_idx, path);

        // Rebuild tree
        self.tree = MerkleTree::<Sha256Hasher>::from_leaves(&leaves);
        self.snapshot_version += 1;

        Ok(())
    }

    /// Remove file from tree
    pub fn remove_file(&mut self, path: &Path) -> Result<()> {
        if let Some(&leaf_idx) = self.file_to_leaf.get(path) {
            // Get current leaves
            let mut leaves: Vec<[u8; 32]> = self.tree.leaves().to_vec();

            // Remove the leaf
            leaves.remove(leaf_idx);

            // Rebuild mappings (indices shift down)
            self.file_to_leaf.clear();
            self.leaf_to_file.clear();

            for (idx, file_path) in self.get_all_files_sorted().iter().enumerate() {
                if file_path != path {
                    self.file_to_leaf.insert(file_path.clone(), idx);
                    self.leaf_to_file.insert(idx, file_path.clone());
                }
            }

            // Rebuild tree
            self.tree = MerkleTree::<Sha256Hasher>::from_leaves(&leaves);
            self.snapshot_version += 1;
        }

        Ok(())
    }

    fn get_all_files_sorted(&self) -> Vec<PathBuf> {
        let mut files: Vec<_> = self.file_to_leaf.keys().cloned().collect();
        files.sort();
        files
    }
}

#[derive(Debug, Clone)]
pub struct ChangeSet {
    pub added: Vec<PathBuf>,
    pub modified: Vec<PathBuf>,
    pub deleted: Vec<PathBuf>,
}

impl ChangeSet {
    pub fn empty() -> Self {
        Self {
            added: Vec::new(),
            modified: Vec::new(),
            deleted: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    pub fn total_changes(&self) -> usize {
        self.added.len() + self.modified.len() + self.deleted.len()
    }
}
```

#### Pattern 2: Integration with notify File Watcher

```rust
use notify::{Watcher, RecursiveMode, Event, EventKind};
use tokio::sync::mpsc;

pub struct MerkleFileWatcher {
    watcher: notify::RecommendedWatcher,
    merkle_tree: Arc<RwLock<FileSystemMerkle>>,
    event_tx: mpsc::Sender<FileSystemEvent>,
}

#[derive(Debug, Clone)]
pub enum FileSystemEvent {
    FileCreated(PathBuf),
    FileModified(PathBuf),
    FileDeleted(PathBuf),
}

impl MerkleFileWatcher {
    pub fn start(
        root: &Path,
        merkle_tree: Arc<RwLock<FileSystemMerkle>>,
    ) -> Result<(Self, mpsc::Receiver<FileSystemEvent>)> {
        let (event_tx, event_rx) = mpsc::channel(100);
        let tx_clone = event_tx.clone();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) => {
                        for path in event.paths {
                            if path.extension() == Some(std::ffi::OsStr::new("rs")) {
                                let _ = tx_clone.try_send(FileSystemEvent::FileCreated(path));
                            }
                        }
                    }
                    EventKind::Modify(_) => {
                        for path in event.paths {
                            if path.extension() == Some(std::ffi::OsStr::new("rs")) {
                                let _ = tx_clone.try_send(FileSystemEvent::FileModified(path));
                            }
                        }
                    }
                    EventKind::Remove(_) => {
                        for path in event.paths {
                            if path.extension() == Some(std::ffi::OsStr::new("rs")) {
                                let _ = tx_clone.try_send(FileSystemEvent::FileDeleted(path));
                            }
                        }
                    }
                    _ => {}
                }
            }
        })?;

        watcher.watch(root, RecursiveMode::Recursive)?;

        Ok((
            Self {
                watcher,
                merkle_tree,
                event_tx,
            },
            event_rx,
        ))
    }

    /// Process file system event and update Merkle tree
    pub async fn handle_event(&self, event: FileSystemEvent) -> Result<()> {
        let mut tree = self.merkle_tree.write().await;

        match event {
            FileSystemEvent::FileCreated(path) => {
                tracing::info!("Merkle: Adding file {}", path.display());
                tree.add_file(path)?;
            }
            FileSystemEvent::FileModified(path) => {
                tracing::info!("Merkle: Updating file {}", path.display());
                if !tree.update_file(&path)? {
                    // File wasn't tracked, add it
                    tree.add_file(path)?;
                }
            }
            FileSystemEvent::FileDeleted(path) => {
                tracing::info!("Merkle: Removing file {}", path.display());
                tree.remove_file(&path)?;
            }
        }

        Ok(())
    }
}
```

#### Pattern 3: Snapshot Persistence

```rust
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize)]
pub struct MerkleSnapshot {
    root_hash: [u8; 32],
    file_to_leaf: HashMap<PathBuf, usize>,
    leaf_hashes: Vec<[u8; 32]>,
    snapshot_version: u64,
    timestamp: SystemTime,
}

impl FileSystemMerkle {
    pub fn save_snapshot(&self, path: &Path) -> Result<()> {
        let snapshot = MerkleSnapshot {
            root_hash: self.tree.root().copied().unwrap_or([0u8; 32]),
            file_to_leaf: self.file_to_leaf.clone(),
            leaf_hashes: self.tree.leaves().to_vec(),
            snapshot_version: self.snapshot_version,
            timestamp: SystemTime::now(),
        };

        let file = std::fs::File::create(path)?;
        bincode::serialize_into(file, &snapshot)?;

        tracing::info!("Saved Merkle snapshot v{} to {}",
            self.snapshot_version, path.display());

        Ok(())
    }

    pub fn load_snapshot(path: &Path) -> Result<Option<Self>> {
        if !path.exists() {
            return Ok(None);
        }

        let file = std::fs::File::open(path)?;
        let snapshot: MerkleSnapshot = bincode::deserialize_from(file)?;

        // Rebuild leaf_to_file mapping
        let mut leaf_to_file = HashMap::new();
        for (path, &idx) in &snapshot.file_to_leaf {
            leaf_to_file.insert(idx, path.clone());
        }

        let tree = MerkleTree::<Sha256Hasher>::from_leaves(&snapshot.leaf_hashes);

        tracing::info!("Loaded Merkle snapshot v{} from {} ({} files)",
            snapshot.snapshot_version, path.display(), snapshot.file_to_leaf.len());

        Ok(Some(Self {
            tree,
            file_to_leaf: snapshot.file_to_leaf,
            leaf_to_file,
            snapshot_version: snapshot.snapshot_version,
        }))
    }
}
```

### Performance Characteristics

| Operation | Time Complexity | Practical Time (10k files) |
|-----------|----------------|---------------------------|
| Build tree | O(n log n) | ~100ms |
| Root hash check | O(1) | <1ms |
| Detect changes | O(n) | ~10-50ms |
| Update single file | O(log n) | <1ms |
| Add file | O(n) (rebuild) | ~50ms |
| Remove file | O(n) (rebuild) | ~50ms |
| Save snapshot | O(n) | ~50ms |

**Key Insight:** Root hash check is O(1), making unchanged detection nearly instant!

---

## 2. Qdrant Production Tuning

### Core HNSW Parameters

#### m (Connections per Node)

**Default:** 16
**Range:** 4-64
**Recommendation:**
- **12-16:** Most use cases (balanced)
- **32:** High precision requirements
- **4-8:** Memory-constrained environments

**Effect:**
- Higher m = More connections = Better accuracy + More memory
- Lower m = Fewer connections = Faster construction + Less memory

```rust
pub struct QdrantOptimizedConfig {
    // HNSW graph configuration
    hnsw_m: usize,                      // 16 (default), 32 (high precision)
    hnsw_ef_construct: usize,           // 100-200 (construction quality)

    // Search-time parameters
    hnsw_ef: usize,                     // 128 (default), tune for speed/accuracy

    // Indexing thresholds
    indexing_threshold: usize,          // 10000 (when to start HNSW build)
    memmap_threshold: usize,            // 50000 (when to use memory-mapped storage)

    // Optimization settings
    indexing_threads: usize,            // 8-16 (avoid broken graphs)

    // Quantization (for large indexes)
    scalar_quantization: bool,          // false initially, true for >1M vectors

    // Payload storage
    on_disk_payload: bool,              // true for large payloads (>1KB each)
}

impl QdrantOptimizedConfig {
    /// Configuration for small codebases (<100k LOC)
    pub fn small_codebase() -> Self {
        Self {
            hnsw_m: 16,
            hnsw_ef_construct: 100,
            hnsw_ef: 128,
            indexing_threshold: 5000,
            memmap_threshold: 25000,
            indexing_threads: 8,
            scalar_quantization: false,
            on_disk_payload: false,
        }
    }

    /// Configuration for medium codebases (100k-1M LOC)
    pub fn medium_codebase() -> Self {
        Self {
            hnsw_m: 16,
            hnsw_ef_construct: 150,
            hnsw_ef: 128,
            indexing_threshold: 10000,
            memmap_threshold: 50000,
            indexing_threads: 12,
            scalar_quantization: false,
            on_disk_payload: true,
        }
    }

    /// Configuration for large codebases (>1M LOC)
    pub fn large_codebase() -> Self {
        Self {
            hnsw_m: 32,
            hnsw_ef_construct: 200,
            hnsw_ef: 256,
            indexing_threshold: 20000,
            memmap_threshold: 100000,
            indexing_threads: 16,
            scalar_quantization: true,
            on_disk_payload: true,
        }
    }
}
```

### Bulk Indexing Optimization

**Strategy:** Disable HNSW during bulk upload, re-enable after

```rust
pub struct BulkIndexer {
    client: QdrantClient,
    collection_name: String,
}

impl BulkIndexer {
    /// Optimize for bulk indexing
    pub async fn start_bulk_mode(&self) -> Result<()> {
        tracing::info!("Entering bulk indexing mode (disabling HNSW)");

        self.client.update_collection(&self.collection_name, UpdateCollection {
            hnsw_config: Some(HnswConfigDiff {
                m: Some(0),  // Disable HNSW
                ..Default::default()
            }),
            optimizers_config: Some(OptimizersConfigDiff {
                indexing_threshold: Some(0),  // Defer indexing
                ..Default::default()
            }),
            ..Default::default()
        }).await?;

        Ok(())
    }

    /// Restore normal indexing mode
    pub async fn end_bulk_mode(&self, config: &QdrantOptimizedConfig) -> Result<()> {
        tracing::info!("Exiting bulk mode (re-enabling HNSW with m={})", config.hnsw_m);

        self.client.update_collection(&self.collection_name, UpdateCollection {
            hnsw_config: Some(HnswConfigDiff {
                m: Some(config.hnsw_m as u64),
                ef_construct: Some(config.hnsw_ef_construct as u64),
                ..Default::default()
            }),
            optimizers_config: Some(OptimizersConfigDiff {
                indexing_threshold: Some(config.indexing_threshold as u64),
                ..Default::default()
            }),
            ..Default::default()
        }).await?;

        // Trigger rebuild
        tracing::info!("Rebuilding HNSW index...");
        // Qdrant will automatically rebuild the index

        Ok(())
    }

    /// Bulk upsert with batching
    pub async fn bulk_upsert(&self, chunks: Vec<(ChunkId, Vec<f32>, CodeChunk)>) -> Result<()> {
        const BATCH_SIZE: usize = 100;

        let total = chunks.len();
        tracing::info!("Bulk upserting {} chunks in batches of {}", total, BATCH_SIZE);

        for (batch_idx, batch) in chunks.chunks(BATCH_SIZE).enumerate() {
            let points: Vec<PointStruct> = batch.iter()
                .map(|(id, vector, chunk)| PointStruct {
                    id: Some(id.as_u64().into()),
                    vectors: Some(vector.clone().into()),
                    payload: serde_json::to_value(chunk).unwrap().as_object().cloned().unwrap(),
                })
                .collect();

            self.client.upsert_points(&self.collection_name, points, None).await?;

            let progress = ((batch_idx + 1) * BATCH_SIZE * 100) / total;
            tracing::debug!("Bulk upsert progress: {}%", progress);
        }

        Ok(())
    }
}
```

### Search-Time Tuning

```rust
pub async fn tune_search_ef(&self, query_vector: Vec<f32>, target_latency_ms: u64) -> usize {
    // Binary search to find optimal ef value
    let mut low_ef = 64;
    let mut high_ef = 512;
    let mut best_ef = 128;

    while low_ef <= high_ef {
        let mid_ef = (low_ef + high_ef) / 2;

        let start = Instant::now();
        let _ = self.client.search_points(&SearchPoints {
            collection_name: self.collection_name.clone(),
            vector: query_vector.clone(),
            limit: 10,
            params: Some(SearchParams {
                hnsw_ef: Some(mid_ef),
                ..Default::default()
            }),
            ..Default::default()
        }).await;
        let latency = start.elapsed().as_millis() as u64;

        if latency <= target_latency_ms {
            best_ef = mid_ef;
            low_ef = mid_ef + 1;  // Try higher ef for better accuracy
        } else {
            high_ef = mid_ef - 1;  // ef too high, reduce
        }
    }

    tracing::info!("Optimal ef for {}ms latency: {}", target_latency_ms, best_ef);
    best_ef
}
```

### Resource Utilization Strategies

**Low Memory + High Speed:**
- Use scalar quantization
- Store vectors on disk
- Keep HNSW graph in RAM

**High Precision + Low Memory:**
- Store both vectors and HNSW on disk
- Use memory-mapped files (memmap_threshold)

**High Precision + High Speed:**
- Keep everything in RAM
- Use quantization with rescoring
- Increase m and ef_construct

---

## 3. RRF Optimization

### The K Parameter Deep Dive

**Formula:** `score = 1 / (rank + k)`

**Default:** k = 60 (experimentally validated)

**Range:** Typically 10-100

#### Effect of K Value

- **Lower k (10-30):** Top-ranked documents have more influence
- **Higher k (80-100):** Lower-ranked documents have more influence
- **k=60:** Balanced (most common)

### K Value Tuning Strategy

```rust
pub struct RRFTuner {
    test_queries: Vec<String>,
    ground_truth: HashMap<String, Vec<ChunkId>>,  // Known relevant results
}

impl RRFTuner {
    /// Find optimal K value for your dataset
    pub async fn tune_k_parameter(
        &self,
        hybrid_search: &HybridSearch,
        k_values: Vec<f32>,
    ) -> (f32, MetricsReport) {
        let mut best_k = 60.0;
        let mut best_ndcg = 0.0;

        for k in k_values {
            let mut total_ndcg = 0.0;

            for query in &self.test_queries {
                let results = hybrid_search.search_with_k(query, 20, k).await?;

                // Calculate NDCG@10
                let relevant = self.ground_truth.get(query).unwrap();
                let ndcg = calculate_ndcg(&results, relevant, 10);
                total_ndcg += ndcg;
            }

            let avg_ndcg = total_ndcg / self.test_queries.len() as f64;

            tracing::info!("k={}: NDCG@10={:.4}", k, avg_ndcg);

            if avg_ndcg > best_ndcg {
                best_ndcg = avg_ndcg;
                best_k = k;
            }
        }

        tracing::info!("Optimal k={} with NDCG@10={:.4}", best_k, best_ndcg);

        (best_k, MetricsReport {
            best_k,
            best_ndcg,
            test_queries_count: self.test_queries.len(),
        })
    }
}

fn calculate_ndcg(results: &[SearchResult], relevant: &[ChunkId], k: usize) -> f64 {
    let dcg: f64 = results.iter()
        .take(k)
        .enumerate()
        .filter(|(_, r)| relevant.contains(&r.chunk_id))
        .map(|(i, _)| 1.0 / ((i + 2) as f64).log2())
        .sum();

    let ideal_dcg: f64 = (0..k.min(relevant.len()))
        .map(|i| 1.0 / ((i + 2) as f64).log2())
        .sum();

    if ideal_dcg == 0.0 { 0.0 } else { dcg / ideal_dcg }
}
```

### Adaptive RRF Weighting

```rust
pub struct AdaptiveRRF {
    // Dynamic weights based on query type
    bm25_weight: f32,
    vector_weight: f32,
    k: f32,
}

impl AdaptiveRRF {
    /// Adjust weights based on query characteristics
    pub fn analyze_query(&mut self, query: &str) {
        let has_exact_identifiers = query.split_whitespace()
            .any(|word| word.chars().any(|c| c == '_' || c.is_uppercase()));

        let has_natural_language = query.split_whitespace().count() > 3;

        if has_exact_identifiers {
            // Favor BM25 for exact matches
            self.bm25_weight = 0.7;
            self.vector_weight = 0.3;
        } else if has_natural_language {
            // Favor vector search for semantic queries
            self.bm25_weight = 0.3;
            self.vector_weight = 0.7;
        } else {
            // Balanced
            self.bm25_weight = 0.5;
            self.vector_weight = 0.5;
        }

        tracing::debug!("Query: '{}' => BM25:{:.1}, Vector:{:.1}",
            query, self.bm25_weight, self.vector_weight);
    }

    pub fn compute_score(&self, bm25_rank: usize, vector_rank: usize) -> f32 {
        let bm25_score = self.bm25_weight / (self.k + bm25_rank as f32 + 1.0);
        let vector_score = self.vector_weight / (self.k + vector_rank as f32 + 1.0);
        bm25_score + vector_score
    }
}
```

---

## 4. Tantivy Production Optimization

### Memory Management

**Key Configuration:** `overall_memory_budget_in_bytes`

```rust
pub struct TantivyOptimizedConfig {
    // Memory budget per indexing thread
    memory_budget_mb: usize,

    // Number of indexing threads
    num_threads: usize,

    // Merge policy
    merge_policy: MergePolicy,
}

impl TantivyOptimizedConfig {
    pub fn for_codebase_size(loc: usize) -> Self {
        if loc < 100_000 {
            // Small codebase: 50MB per thread, 2 threads
            Self {
                memory_budget_mb: 50,
                num_threads: 2,
                merge_policy: MergePolicy::log_merge_policy(),
            }
        } else if loc < 1_000_000 {
            // Medium: 100MB per thread, 4 threads
            Self {
                memory_budget_mb: 100,
                num_threads: 4,
                merge_policy: MergePolicy::log_merge_policy(),
            }
        } else {
            // Large: 200MB per thread, 8 threads
            Self {
                memory_budget_mb: 200,
                num_threads: 8,
                merge_policy: MergePolicy::log_merge_policy(),
            }
        }
    }

    pub fn create_index_writer(&self, index: &Index) -> Result<IndexWriter> {
        let total_budget = self.memory_budget_mb * self.num_threads * 1024 * 1024;

        let writer = index
            .writer_with_num_threads(self.num_threads, total_budget)?;

        Ok(writer)
    }
}
```

### Tantivy 0.22 Optimizations (Latest)

**Improvements in recent version:**
- **40% faster indexing** (GitHub dataset benchmark)
- **22% less memory** (590MB vs 760MB on HDFS dataset)
- Docid delta compression

**Recommendation:** Ensure using Tantivy 0.22+

```toml
[dependencies]
tantivy = "0.22"  # Latest stable
```

### Merge Policy Tuning

```rust
use tantivy::merge_policy::LogMergePolicy;

pub fn configure_merge_policy() -> LogMergePolicy {
    LogMergePolicy::default()
        .set_min_merge_size(8)         // Merge segments when >=8MB
        .set_max_merge_size(5_000)     // Don't merge segments >5GB
        .set_min_layer_size(10_000)    // Layer threshold
        .set_level_log_size(0.75)      // Exponential growth factor
}
```

### Memory-Mapped vs. Anonymous Memory

**MmapDirectory Advantage:**
- Extremely low resident memory footprint
- Page cache shared across processes
- Multiple instances ≈ single instance memory usage
- Zero cost for deploying new versions

```rust
use tantivy::directory::MmapDirectory;

pub fn open_index_with_mmap(path: &Path) -> Result<Index> {
    let dir = MmapDirectory::open(path)?;
    let index = Index::open(dir)?;
    Ok(index)
}
```

### Production Indexing Pattern

```rust
pub struct ProductionTantivyIndexer {
    index: Index,
    writer: IndexWriter,
    schema: Schema,
    stats: Arc<Mutex<IndexingStats>>,
}

impl ProductionTantivyIndexer {
    pub fn new(index_path: &Path, config: TantivyOptimizedConfig) -> Result<Self> {
        let schema = build_schema();

        let index = if index_path.exists() {
            Index::open_in_dir(index_path)?
        } else {
            Index::create_in_dir(index_path, schema.clone())?
        };

        let writer = config.create_index_writer(&index)?;

        Ok(Self {
            index,
            writer,
            schema,
            stats: Arc::new(Mutex::new(IndexingStats::default())),
        })
    }

    /// Index with automatic segment management
    pub async fn index_chunks(&mut self, chunks: &[CodeChunk]) -> Result<()> {
        for chunk in chunks {
            let doc = self.chunk_to_document(chunk);
            self.writer.add_document(doc)?;

            let mut stats = self.stats.lock().await;
            stats.indexed_chunks += 1;

            // Commit every 10k documents
            if stats.indexed_chunks % 10_000 == 0 {
                tracing::info!("Committing batch at {} chunks", stats.indexed_chunks);
                self.writer.commit()?;
            }
        }

        // Final commit
        self.writer.commit()?;

        Ok(())
    }
}
```

---

## 5. Testing & Evaluation Frameworks

### Standard Benchmarks

#### CodeSearchNet
- **Languages:** 6 (Python, Java, JavaScript, PHP, Ruby, Go)
- **Queries:** 99 manually annotated
- **Metric:** NDCG
- **Status:** Baseline benchmark

#### CoSQA+ (Recommended for rust-code-mcp)
- **Queries:** 412,080 query-code pairs
- **Format:** Multi-choice (multiple correct answers)
- **Metrics:** NDCG@10, MRR, MAP, Recall
- **Innovation:** Test-driven agents

#### CoIR (Code Information Retrieval)
- **Purpose:** Embedding model evaluation
- **Scoring:** Higher = better
- **Used by:** Qodo-Embed (68.53 for 1.5B, 71.5 for 7B)

### Evaluation Metrics Implementation

```rust
pub struct CodeSearchEvaluator {
    test_queries: Vec<TestQuery>,
}

pub struct TestQuery {
    query: String,
    relevant_chunks: Vec<ChunkId>,
    language: String,
}

pub struct EvaluationMetrics {
    pub ndcg_at_10: f64,
    pub mrr: f64,
    pub map: f64,
    pub recall_at_20: f64,
    pub precision_at_10: f64,
}

impl CodeSearchEvaluator {
    /// Calculate NDCG@K
    pub fn calculate_ndcg(&self, results: &[SearchResult], relevant: &[ChunkId], k: usize) -> f64 {
        let dcg: f64 = results.iter()
            .take(k)
            .enumerate()
            .filter(|(_, r)| relevant.contains(&r.chunk_id))
            .map(|(i, _)| 1.0 / ((i + 2) as f64).log2())  // log2(rank + 1)
            .sum();

        let ideal_dcg: f64 = (0..k.min(relevant.len()))
            .map(|i| 1.0 / ((i + 2) as f64).log2())
            .sum();

        if ideal_dcg == 0.0 { 0.0 } else { dcg / ideal_dcg }
    }

    /// Calculate MRR (Mean Reciprocal Rank)
    pub fn calculate_mrr(&self, results: &[SearchResult], relevant: &[ChunkId]) -> f64 {
        results.iter()
            .position(|r| relevant.contains(&r.chunk_id))
            .map(|pos| 1.0 / (pos + 1) as f64)
            .unwrap_or(0.0)
    }

    /// Calculate MAP (Mean Average Precision)
    pub fn calculate_map(&self, results: &[SearchResult], relevant: &[ChunkId]) -> f64 {
        let mut relevant_found = 0;
        let mut sum_precision = 0.0;

        for (i, result) in results.iter().enumerate() {
            if relevant.contains(&result.chunk_id) {
                relevant_found += 1;
                let precision = relevant_found as f64 / (i + 1) as f64;
                sum_precision += precision;
            }
        }

        if relevant.is_empty() {
            0.0
        } else {
            sum_precision / relevant.len() as f64
        }
    }

    /// Calculate Recall@K
    pub fn calculate_recall_at_k(&self, results: &[SearchResult], relevant: &[ChunkId], k: usize) -> f64 {
        let found = results.iter()
            .take(k)
            .filter(|r| relevant.contains(&r.chunk_id))
            .count();

        found as f64 / relevant.len() as f64
    }

    /// Run full evaluation suite
    pub async fn evaluate(&self, hybrid_search: &HybridSearch) -> EvaluationMetrics {
        let mut ndcg_sum = 0.0;
        let mut mrr_sum = 0.0;
        let mut map_sum = 0.0;
        let mut recall_sum = 0.0;
        let mut precision_sum = 0.0;

        for test_query in &self.test_queries {
            let results = hybrid_search.search(&test_query.query, 20).await.unwrap();

            ndcg_sum += self.calculate_ndcg(&results, &test_query.relevant_chunks, 10);
            mrr_sum += self.calculate_mrr(&results, &test_query.relevant_chunks);
            map_sum += self.calculate_map(&results, &test_query.relevant_chunks);
            recall_sum += self.calculate_recall_at_k(&results, &test_query.relevant_chunks, 20);

            let precision = results.iter()
                .take(10)
                .filter(|r| test_query.relevant_chunks.contains(&r.chunk_id))
                .count() as f64 / 10.0;
            precision_sum += precision;
        }

        let n = self.test_queries.len() as f64;

        EvaluationMetrics {
            ndcg_at_10: ndcg_sum / n,
            mrr: mrr_sum / n,
            map: map_sum / n,
            recall_at_20: recall_sum / n,
            precision_at_10: precision_sum / n,
        }
    }
}
```

### Creating Test Datasets

```rust
pub fn create_rust_test_dataset() -> Vec<TestQuery> {
    vec![
        TestQuery {
            query: "parse command line arguments".to_string(),
            relevant_chunks: vec![
                ChunkId::from_str("clap_parser_main"),
                ChunkId::from_str("structopt_derive"),
            ],
            language: "Rust".to_string(),
        },
        TestQuery {
            query: "async http client request".to_string(),
            relevant_chunks: vec![
                ChunkId::from_str("reqwest_client"),
                ChunkId::from_str("hyper_request"),
            ],
            language: "Rust".to_string(),
        },
        // Add 50-100 test queries for robust evaluation
    ]
}
```

---

## 6. Query Optimization Techniques

### Query Expansion & Reformulation

**Research Finding:** LLM-based query expansion outperforms traditional methods

#### Self-Supervised Query Reformulation (SSQR)

```rust
pub struct QueryReformulator {
    // Could integrate with local LLM or use heuristics
    expansion_cache: Arc<RwLock<HashMap<String, Vec<String>>>>,
}

impl QueryReformulator {
    /// Expand query with synonyms and related terms
    pub async fn expand_query(&self, original_query: &str) -> Vec<String> {
        let mut expanded = vec![original_query.to_string()];

        // Check cache first
        if let Some(cached) = self.expansion_cache.read().await.get(original_query) {
            return cached.clone();
        }

        // Heuristic expansion (production would use LLM)
        expanded.extend(self.add_code_synonyms(original_query));
        expanded.extend(self.add_common_patterns(original_query));

        // Cache result
        self.expansion_cache.write().await.insert(
            original_query.to_string(),
            expanded.clone(),
        );

        expanded
    }

    fn add_code_synonyms(&self, query: &str) -> Vec<String> {
        let synonyms = HashMap::from([
            ("function", vec!["method", "fn", "procedure"]),
            ("class", vec!["struct", "type", "object"]),
            ("error", vec!["exception", "panic", "Result"]),
            ("async", vec!["asynchronous", "future", "await"]),
        ]);

        let mut expanded = Vec::new();

        for (word, synonyms) in &synonyms {
            if query.contains(word) {
                for synonym in synonyms {
                    expanded.push(query.replace(word, synonym));
                }
            }
        }

        expanded
    }

    fn add_common_patterns(&self, query: &str) -> Vec<String> {
        let mut patterns = Vec::new();

        // If query mentions a concept, add implementation pattern
        if query.contains("parse") && query.contains("argument") {
            patterns.push("clap::Parser".to_string());
            patterns.push("std::env::args".to_string());
        }

        if query.contains("http") && query.contains("request") {
            patterns.push("reqwest::get".to_string());
            patterns.push("hyper::Request".to_string());
        }

        patterns
    }
}
```

### Query Type Detection

```rust
pub enum QueryType {
    ExactIdentifier,    // "HashMap::insert"
    NaturalLanguage,    // "how to sort a vector"
    Conceptual,         // "error handling patterns"
    Mixed,              // "parse JSON with serde"
}

impl QueryType {
    pub fn detect(query: &str) -> Self {
        let has_scope_operator = query.contains("::");
        let has_snake_case = query.contains('_');
        let word_count = query.split_whitespace().count();
        let has_question_words = ["how", "what", "where", "when", "why"]
            .iter()
            .any(|w| query.to_lowercase().starts_with(w));

        if has_scope_operator || (has_snake_case && word_count <= 3) {
            QueryType::ExactIdentifier
        } else if has_question_words || word_count > 5 {
            QueryType::NaturalLanguage
        } else if word_count >= 3 && word_count <= 5 {
            QueryType::Conceptual
        } else {
            QueryType::Mixed
        }
    }

    /// Adjust search strategy based on query type
    pub fn recommended_strategy(&self) -> SearchStrategy {
        match self {
            QueryType::ExactIdentifier => SearchStrategy {
                bm25_weight: 0.8,
                vector_weight: 0.2,
                boost_exact_matches: true,
            },
            QueryType::NaturalLanguage => SearchStrategy {
                bm25_weight: 0.3,
                vector_weight: 0.7,
                boost_exact_matches: false,
            },
            QueryType::Conceptual => SearchStrategy {
                bm25_weight: 0.4,
                vector_weight: 0.6,
                boost_exact_matches: false,
            },
            QueryType::Mixed => SearchStrategy {
                bm25_weight: 0.5,
                vector_weight: 0.5,
                boost_exact_matches: false,
            },
        }
    }
}
```

---

## 7. Error Recovery & Resilience

### Common Failure Scenarios

1. **Hardware failures** (disk, memory, power)
2. **Software crashes** (panics, OOM)
3. **Network issues** (Qdrant unavailable)
4. **Corruption** (index files, snapshots)
5. **Concurrent access** (race conditions)

### Resilience Strategies

#### 1. Graceful Degradation

```rust
pub struct ResilientHybridSearch {
    bm25: Option<Bm25Search>,
    vector_store: Option<VectorStore>,
    fallback_mode: Arc<AtomicBool>,
}

impl ResilientHybridSearch {
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Try full hybrid search
        match self.try_hybrid_search(query, limit).await {
            Ok(results) => Ok(results),
            Err(e) => {
                tracing::warn!("Hybrid search failed: {}, falling back", e);
                self.fallback_mode.store(true, Ordering::Relaxed);
                self.fallback_search(query, limit).await
            }
        }
    }

    async fn try_hybrid_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Try both engines
        let (bm25_res, vector_res) = tokio::join!(
            self.bm25_search(query, limit),
            self.vector_search(query, limit)
        );

        match (bm25_res, vector_res) {
            (Ok(bm25), Ok(vector)) => {
                // Both succeeded, use RRF
                Ok(self.merge_results(bm25, vector))
            }
            (Ok(bm25), Err(e)) => {
                tracing::warn!("Vector search failed: {}, using BM25 only", e);
                Ok(bm25)
            }
            (Err(e), Ok(vector)) => {
                tracing::warn!("BM25 search failed: {}, using vector only", e);
                Ok(vector)
            }
            (Err(e1), Err(e2)) => {
                Err(anyhow!("Both search engines failed: BM25={}, Vector={}", e1, e2))
            }
        }
    }

    async fn fallback_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Try BM25 first (usually more reliable)
        if let Some(bm25) = &self.bm25 {
            if let Ok(results) = bm25.search(query, limit).await {
                return Ok(results);
            }
        }

        // Try vector as last resort
        if let Some(vector) = &self.vector_store {
            return vector.search(query, limit).await;
        }

        Err(anyhow!("All search engines unavailable"))
    }
}
```

#### 2. Automatic Backup & Recovery

```rust
pub struct BackupManager {
    backup_dir: PathBuf,
    retention_count: usize,
}

impl BackupManager {
    /// Create incremental backup
    pub async fn create_backup(&self, merkle: &FileSystemMerkle) -> Result<PathBuf> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();

        let backup_path = self.backup_dir
            .join(format!("merkle_v{}.{}.snapshot", merkle.snapshot_version, timestamp));

        merkle.save_snapshot(&backup_path)?;

        // Rotate old backups
        self.rotate_backups()?;

        tracing::info!("Created backup: {}", backup_path.display());
        Ok(backup_path)
    }

    /// Restore from latest backup
    pub async fn restore_latest(&self) -> Result<Option<FileSystemMerkle>> {
        let mut backups: Vec<_> = std::fs::read_dir(&self.backup_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension() == Some(OsStr::new("snapshot")))
            .collect();

        backups.sort_by_key(|e| e.metadata().unwrap().modified().unwrap());
        backups.reverse();

        if let Some(latest) = backups.first() {
            tracing::info!("Restoring from backup: {}", latest.path().display());
            FileSystemMerkle::load_snapshot(&latest.path())
        } else {
            Ok(None)
        }
    }

    fn rotate_backups(&self) -> Result<()> {
        let mut backups: Vec<_> = std::fs::read_dir(&self.backup_dir)?
            .filter_map(|e| e.ok())
            .collect();

        backups.sort_by_key(|e| e.metadata().unwrap().modified().unwrap());

        while backups.len() > self.retention_count {
            if let Some(oldest) = backups.first() {
                std::fs::remove_file(oldest.path())?;
                tracing::info!("Deleted old backup: {}", oldest.path().display());
                backups.remove(0);
            }
        }

        Ok(())
    }
}
```

#### 3. Health Checks

```rust
pub struct HealthMonitor {
    bm25: Arc<Bm25Search>,
    vector_store: Arc<VectorStore>,
    merkle: Arc<RwLock<FileSystemMerkle>>,
}

#[derive(Debug, Serialize)]
pub struct HealthStatus {
    pub overall: Status,
    pub bm25: ComponentHealth,
    pub vector: ComponentHealth,
    pub merkle: ComponentHealth,
}

#[derive(Debug, Serialize)]
pub enum Status {
    Healthy,
    Degraded,
    Unhealthy,
}

impl HealthMonitor {
    pub async fn check_health(&self) -> HealthStatus {
        let (bm25_health, vector_health, merkle_health) = tokio::join!(
            self.check_bm25(),
            self.check_vector(),
            self.check_merkle()
        );

        let overall = if bm25_health.status == Status::Healthy
            && vector_health.status == Status::Healthy
            && merkle_health.status == Status::Healthy {
            Status::Healthy
        } else if bm25_health.status == Status::Unhealthy
            && vector_health.status == Status::Unhealthy {
            Status::Unhealthy
        } else {
            Status::Degraded
        };

        HealthStatus {
            overall,
            bm25: bm25_health,
            vector: vector_health,
            merkle: merkle_health,
        }
    }

    async fn check_bm25(&self) -> ComponentHealth {
        match self.bm25.test_query("test").await {
            Ok(_) => ComponentHealth {
                status: Status::Healthy,
                message: "BM25 operational".to_string(),
            },
            Err(e) => ComponentHealth {
                status: Status::Unhealthy,
                message: format!("BM25 error: {}", e),
            },
        }
    }
}
```

---

## 8. Security & Privacy Considerations

### Enterprise Code Security (2025 State)

**Critical Statistics:**
- **35% of GitHub repos are public** (easy exploit access)
- **61% of organizations** have secrets exposed in public repos
- **Samsung AI leak (2023):** Employees shared sensitive code with ChatGPT

### Security Best Practices for rust-code-mcp

#### 1. Secrets Detection

```rust
use regex::Regex;

pub struct SecretsScanner {
    patterns: Vec<(String, Regex)>,
}

impl SecretsScanner {
    pub fn new() -> Self {
        Self {
            patterns: vec![
                ("AWS Key".to_string(), Regex::new(r"AKIA[0-9A-Z]{16}").unwrap()),
                ("Private Key".to_string(), Regex::new(r"-----BEGIN (RSA |)PRIVATE KEY-----").unwrap()),
                ("API Token".to_string(), Regex::new(r"(api[_-]?key|apikey|api[_-]?token)[\s:=]+['\"]([^'\"]+)['\"]").unwrap()),
                ("Password".to_string(), Regex::new(r"(password|passwd|pwd)[\s:=]+['\"]([^'\"]+)['\"]").unwrap()),
            ],
        }
    }

    /// Scan chunk for secrets before indexing
    pub fn scan_chunk(&self, chunk: &CodeChunk) -> Vec<SecretMatch> {
        let mut matches = Vec::new();

        for (name, pattern) in &self.patterns {
            if pattern.is_match(&chunk.content) {
                matches.push(SecretMatch {
                    pattern_name: name.clone(),
                    file: chunk.context.file_path.clone(),
                    line: chunk.context.line_start,
                });
            }
        }

        matches
    }

    /// Should chunk be excluded from indexing?
    pub fn should_exclude(&self, chunk: &CodeChunk) -> bool {
        !self.scan_chunk(chunk).is_empty()
    }
}
```

#### 2. Sensitive File Filtering

```rust
pub struct SensitiveFileFilter {
    excluded_patterns: Vec<String>,
}

impl SensitiveFileFilter {
    pub fn default() -> Self {
        Self {
            excluded_patterns: vec![
                ".env".to_string(),
                ".env.*".to_string(),
                "**/secrets/**".to_string(),
                "**/credentials/**".to_string(),
                "**/.aws/**".to_string(),
                "**/.ssh/**".to_string(),
                "**/private_key*".to_string(),
            ],
        }
    }

    pub fn should_index(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();

        for pattern in &self.excluded_patterns {
            if glob::Pattern::new(pattern)
                .unwrap()
                .matches(&path_str) {
                tracing::warn!("Excluding sensitive file: {}", path_str);
                return false;
            }
        }

        true
    }
}
```

#### 3. Local-Only Guarantees

```rust
/// Configuration ensuring no cloud dependencies
pub struct PrivacyFirstConfig {
    /// Use local embeddings only (fastembed)
    embedding_provider: EmbeddingProvider::Local,

    /// Use local Qdrant instance (no cloud)
    qdrant_url: String,  // "http://localhost:6334"

    /// Disable telemetry
    telemetry_enabled: false,

    /// Audit logging
    audit_log_path: PathBuf,
}

impl PrivacyFirstConfig {
    /// Verify no external network calls
    pub fn validate(&self) -> Result<()> {
        // Check Qdrant is local
        if !self.qdrant_url.contains("localhost") && !self.qdrant_url.contains("127.0.0.1") {
            return Err(anyhow!("Qdrant must be local for privacy-first mode"));
        }

        // Check embedding provider is local
        match self.embedding_provider {
            EmbeddingProvider::Local => Ok(()),
            _ => Err(anyhow!("Only local embeddings allowed in privacy-first mode")),
        }
    }
}
```

---

## 9. Advanced Features

### Semantic Clone Detection

```rust
pub struct CloneDetector {
    vector_store: Arc<VectorStore>,
    embedding_generator: Arc<EmbeddingGenerator>,
}

impl CloneDetector {
    /// Find semantically similar code chunks
    pub async fn find_clones(&self, threshold: f32) -> Result<Vec<ClonePair>> {
        // Get all chunks from vector store
        let all_chunks = self.vector_store.get_all_chunks().await?;

        let mut clones = Vec::new();

        for (i, chunk1) in all_chunks.iter().enumerate() {
            // Search for similar chunks
            let similar = self.vector_store
                .search_by_vector(&chunk1.embedding, 10)
                .await?;

            for result in similar {
                if result.chunk_id != chunk1.id && result.score > threshold {
                    clones.push(ClonePair {
                        chunk1: chunk1.clone(),
                        chunk2: result.chunk.clone(),
                        similarity: result.score,
                    });
                }
            }
        }

        // Deduplicate pairs
        clones.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap());
        clones.dedup_by(|a, b| {
            (a.chunk1.id == b.chunk1.id && a.chunk2.id == b.chunk2.id) ||
            (a.chunk1.id == b.chunk2.id && a.chunk2.id == b.chunk1.id)
        });

        Ok(clones)
    }
}
```

### Multimodal Search (Code + Documentation)

```rust
pub struct MultimodalSearcher {
    code_index: HybridSearch,
    doc_index: HybridSearch,  // Separate index for documentation
}

impl MultimodalSearcher {
    /// Search both code and documentation
    pub async fn search_all(&self, query: &str, limit: usize) -> Result<MultimodalResults> {
        let (code_results, doc_results) = tokio::join!(
            self.code_index.search(query, limit),
            self.doc_index.search(query, limit)
        );

        Ok(MultimodalResults {
            code: code_results?,
            documentation: doc_results?,
        })
    }

    /// Index both code and its documentation
    pub async fn index_with_docs(&mut self, file: &Path) -> Result<()> {
        // Parse code
        let parse_result = self.parser.parse_file_complete(file)?;

        // Extract code chunks
        let code_chunks = self.chunker.chunk_by_symbols(
            file,
            parse_result.symbols.clone(),
            &parse_result.call_graph,
            parse_result.imports.clone(),
        )?;

        // Extract documentation chunks (docstrings)
        let doc_chunks: Vec<DocChunk> = parse_result.symbols
            .into_iter()
            .filter_map(|s| s.docstring.map(|doc| DocChunk {
                id: ChunkId::new(),
                content: doc,
                associated_code: s.name.clone(),
                file_path: file.to_path_buf(),
            }))
            .collect();

        // Index both
        self.code_index.index_chunks(&code_chunks).await?;
        self.doc_index.index_doc_chunks(&doc_chunks).await?;

        Ok(())
    }
}
```

---

## 10. Production Deployment Checklist

### Infrastructure

- [ ] **Qdrant Deployment**
  - [ ] Docker container or binary running
  - [ ] Persistent volume for data
  - [ ] Backup strategy (daily snapshots)
  - [ ] Resource limits (CPU, RAM)
  - [ ] Health check endpoint configured

- [ ] **Tantivy Index**
  - [ ] Persistent storage configured
  - [ ] Memory budget set appropriately
  - [ ] MmapDirectory for production
  - [ ] Backup strategy

- [ ] **Merkle Snapshots**
  - [ ] Snapshot directory configured
  - [ ] Backup retention policy (keep 7 days)
  - [ ] Automatic cleanup of old snapshots

### Configuration

- [ ] **Performance Tuning**
  - [ ] Qdrant HNSW parameters optimized for codebase size
  - [ ] Tantivy memory budget set
  - [ ] RRF k value tuned on test dataset
  - [ ] Embedding batch size configured

- [ ] **Security**
  - [ ] Secrets scanner enabled
  - [ ] Sensitive file filter configured
  - [ ] Privacy-first mode validated (local-only)
  - [ ] Audit logging enabled

- [ ] **Resilience**
  - [ ] Health checks implemented
  - [ ] Graceful degradation configured
  - [ ] Backup manager running
  - [ ] Error recovery tested

### Monitoring

- [ ] **Metrics Collected**
  - [ ] Query latency (p50, p95, p99)
  - [ ] Index size (Tantivy + Qdrant)
  - [ ] Memory usage
  - [ ] Disk usage
  - [ ] Error rate

- [ ] **Alerts Configured**
  - [ ] High error rate (>5%)
  - [ ] High latency (p95 > 500ms)
  - [ ] Disk space low (<10% free)
  - [ ] Service unavailable

### Testing

- [ ] **Functional Tests**
  - [ ] Hybrid search working end-to-end
  - [ ] Incremental indexing working
  - [ ] Merkle tree detection working
  - [ ] All MCP tools functional

- [ ] **Performance Tests**
  - [ ] Benchmark on target codebase size
  - [ ] Measure unchanged detection time (<10ms)
  - [ ] Measure incremental update time
  - [ ] Load test (concurrent queries)

- [ ] **Quality Tests**
  - [ ] NDCG@10 > 0.75
  - [ ] MRR > 0.80
  - [ ] Recall@20 > 0.95

### Documentation

- [ ] **User Documentation**
  - [ ] Installation guide
  - [ ] Configuration examples
  - [ ] Troubleshooting guide
  - [ ] FAQ

- [ ] **Developer Documentation**
  - [ ] Architecture overview
  - [ ] API reference
  - [ ] Contribution guidelines
  - [ ] Testing guide

---

## Summary: Key Takeaways

### 1. Merkle Tree Integration
- **Custom implementation required** (no existing patterns)
- **Snapshot-based detection** for persistence
- **Incremental file updates** in O(log n) time
- **Root hash check** is O(1) - nearly instant!

### 2. Qdrant Optimization
- **HNSW parameters**: m=16 (default), 32 (precision)
- **Bulk mode**: Disable HNSW during upload, re-enable after
- **Batch size**: 100 points per upsert
- **Threading**: 8-16 indexing threads

### 3. RRF Tuning
- **Default k=60** works for most cases
- **Adaptive weighting** based on query type
- **Tune on test dataset** for optimal results
- **Range 10-100** for experimentation

### 4. Tantivy Optimization
- **Memory budget**: 50-200MB per thread
- **Use MmapDirectory** for production
- **Tantivy 0.22**: 40% faster, 22% less memory
- **Merge policy**: LogMergePolicy with custom thresholds

### 5. Testing & Evaluation
- **Use CoSQA+** for comprehensive evaluation
- **Track NDCG@10, MRR, MAP, Recall@20**
- **Create custom test dataset** (50-100 queries)
- **Target metrics**: NDCG>0.75, MRR>0.80

### 6. Security & Privacy
- **Scan for secrets** before indexing
- **Filter sensitive files** (.env, credentials)
- **Local-only mode** (no cloud dependencies)
- **Audit logging** for compliance

### 7. Production Readiness
- **Health checks** for all components
- **Graceful degradation** (fallback to BM25)
- **Automatic backups** with retention
- **Monitoring & alerts** for key metrics

---

**Document Version:** 1.0
**Last Updated:** 2025-10-19
**Research Depth:** Advanced production considerations
**Next Action:** Choose specific areas for implementation

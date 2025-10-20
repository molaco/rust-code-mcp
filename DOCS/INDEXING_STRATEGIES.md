# State-of-the-Art Indexing Strategies for Rust Code MCP

**Date:** 2025-10-19
**Status:** Proposal
**Purpose:** Comprehensive indexing strategy for both Tantivy (BM25) and Qdrant (Vector) databases

---

## Executive Summary

After analyzing the current implementation and researching state-of-the-art approaches, this document proposes **5 different strategies** for implementing proper dual indexing (Tantivy + Qdrant) with efficient incremental updates.

### Current State Analysis

**✅ What Works:**
- Tantivy indexing with SHA-256-based incremental updates
- Metadata cache using `sled` embedded database
- FileSchema with proper text search fields
- Hybrid search infrastructure (RRF implementation ready)

**❌ Critical Gap:**
- **Qdrant vector store is NEVER populated** - the entire pipeline from parsing → chunking → embedding → vector indexing is missing from the search tool
- `get_similar_code()` tool will always return empty results
- Hybrid search cannot work because Qdrant has no data

**🔧 Dependencies Ready but Unused:**
- `notify = "6"` - File watching (not implemented)
- `tree-sitter` + `text-splitter` + `fastembed` + `qdrant-client` - All present but not integrated into search workflow

---

## Problem Statement

The MCP server needs to maintain **two synchronized indexes**:

1. **Tantivy Index (BM25)** - For keyword/lexical search
   - Currently: ✅ Working with incremental updates
   - Location: `~/.local/share/rust-code-mcp/search/index/`

2. **Qdrant Index (Vectors)** - For semantic search
   - Currently: ❌ Never populated
   - Expected: Embeddings of code chunks with metadata

**Challenges:**
- How to ensure both indexes stay in sync?
- How to handle incremental updates efficiently for both?
- How to minimize reprocessing when files change?
- How to structure the indexing pipeline?
- When should indexing happen (on-demand vs. background)?

---

## Strategy 1: Unified Indexing Pipeline (Recommended)

### Overview

Single indexing pass that populates both Tantivy and Qdrant simultaneously. When a file changes, update both indexes together.

### Architecture

```
File Change Detected
    ↓
┌─────────────────────────────────────────┐
│  Read File Content                      │
│  Check Metadata Cache (SHA-256)         │
│  Skip if unchanged                      │
└─────────────────┬───────────────────────┘
                  ↓
┌─────────────────────────────────────────┐
│  Parse with Tree-sitter                 │
│  Extract: symbols, imports, call graph  │
└─────────────────┬───────────────────────┘
                  ↓
┌─────────────────────────────────────────┐
│  Chunk with text-splitter               │
│  Create CodeChunk objects               │
│  Add context (file, module, docstring)  │
└─────────────────┬───────────────────────┘
                  ↓
        ┌─────────┴────────┐
        ↓                  ↓
┌───────────────┐  ┌──────────────────────┐
│ Index to      │  │ Generate Embeddings  │
│ Tantivy       │  │ (fastembed)          │
│               │  │                      │
│ - Full text   │  ↓                      │
│ - Symbols     │  ┌──────────────────────┤
│ - Metadata    │  │ Index to Qdrant      │
│               │  │                      │
│ BM25 ready ✓  │  │ - Vector embeddings  │
│               │  │ - Chunk metadata     │
│               │  │ - Payload            │
│               │  │                      │
│               │  │ Semantic ready ✓     │
└───────────────┘  └──────────────────────┘
        │                  │
        └─────────┬────────┘
                  ↓
        Update Metadata Cache
        (SHA-256 hash + timestamp)
```

### Implementation Steps

#### 1. Create Unified Indexer Module

**File:** `src/indexing/unified.rs`

```rust
use crate::chunker::Chunker;
use crate::embeddings::EmbeddingGenerator;
use crate::metadata_cache::{MetadataCache, FileMetadata};
use crate::parser::RustParser;
use crate::search::bm25::Bm25Search;
use crate::vector_store::VectorStore;
use std::path::Path;
use anyhow::Result;

pub struct UnifiedIndexer {
    parser: RustParser,
    chunker: Chunker,
    embedding_generator: EmbeddingGenerator,
    bm25_search: Bm25Search,
    vector_store: VectorStore,
    metadata_cache: MetadataCache,
}

impl UnifiedIndexer {
    pub async fn new(
        cache_path: &Path,
        tantivy_path: &Path,
        qdrant_url: &str,
        collection_name: &str,
    ) -> Result<Self> {
        Ok(Self {
            parser: RustParser::new()?,
            chunker: Chunker::new(),
            embedding_generator: EmbeddingGenerator::new()?,
            bm25_search: Bm25Search::open_or_create(tantivy_path)?,
            vector_store: VectorStore::connect(qdrant_url, collection_name).await?,
            metadata_cache: MetadataCache::new(cache_path)?,
        })
    }

    /// Index a single file to both Tantivy and Qdrant
    pub async fn index_file(&mut self, file_path: &Path) -> Result<IndexStats> {
        let content = std::fs::read_to_string(file_path)?;

        // 1. Check if file changed
        let file_path_str = file_path.to_string_lossy().to_string();
        if !self.metadata_cache.has_changed(&file_path_str, &content)? {
            return Ok(IndexStats::unchanged());
        }

        // 2. Parse with tree-sitter
        let parse_result = self.parser.parse_file_complete(file_path)?;

        // 3. Chunk the code
        let chunks = self.chunker.chunk_file(
            file_path,
            parse_result.symbols,
            &parse_result.call_graph,
            parse_result.imports.iter().map(|i| i.path.clone()).collect(),
        )?;

        // 4. Generate embeddings (batch processing)
        let chunk_texts: Vec<String> = chunks.iter()
            .map(|c| crate::chunker::format_for_embedding(c))
            .collect();

        let embeddings = self.embedding_generator.embed_batch(chunk_texts)?;

        // 5. Index to both stores in parallel
        let bm25_future = self.bm25_search.index_chunks(&chunks);
        let vector_future = self.vector_store.upsert_chunks(
            chunks.iter().zip(embeddings.iter())
                .map(|(chunk, embedding)| (chunk.id, embedding.clone(), chunk.clone()))
                .collect()
        );

        tokio::try_join!(bm25_future, vector_future)?;

        // 6. Update metadata cache
        let file_meta = FileMetadata::from_content(
            &content,
            std::fs::metadata(file_path)?.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs(),
            std::fs::metadata(file_path)?.len(),
        );
        self.metadata_cache.set(&file_path_str, &file_meta)?;

        Ok(IndexStats::indexed(chunks.len()))
    }

    /// Index entire directory
    pub async fn index_directory(&mut self, dir_path: &Path) -> Result<DirectoryStats> {
        // Recursive traversal with ignore crate
        // Process .rs files only
        // Parallel processing with rayon
        todo!()
    }
}

pub struct IndexStats {
    pub chunks_indexed: usize,
    pub was_unchanged: bool,
}
```

#### 2. Modify `search` Tool in `src/tools/search_tool.rs`

Replace the current indexing logic:

```rust
#[tool(description = "Search for keywords in text files within the specified directory")]
async fn search(
    &self,
    Parameters(SearchParams { directory, keyword }): Parameters<SearchParams>,
) -> Result<CallToolResult, McpError> {
    // Initialize unified indexer
    let mut indexer = UnifiedIndexer::new(
        &Self::data_dir().join("cache"),
        &Self::data_dir().join("index"),
        "http://localhost:6334",
        &format!("code_chunks_{}", sanitize_project_name(&directory)),
    ).await.map_err(|e| McpError::invalid_params(e.to_string(), None))?;

    // Index directory (incremental - only changed files)
    let stats = indexer.index_directory(Path::new(&directory))
        .await
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

    tracing::info!("Indexed {} files ({} chunks), {} unchanged",
        stats.indexed_files, stats.total_chunks, stats.unchanged_files);

    // Perform hybrid search
    let hybrid_search = HybridSearch::new(
        indexer.embedding_generator,
        indexer.vector_store,
        Some(indexer.bm25_search),
    );

    let results = hybrid_search.search(&keyword, 10)
        .await
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

    // Format results
    format_search_results(results)
}
```

### Advantages

✅ **Single source of truth** - One indexing pass
✅ **Always synchronized** - Both indexes updated together
✅ **Efficient** - Only process changed files
✅ **Simple mental model** - Easy to understand and maintain

### Disadvantages

⚠️ **Slower initial indexing** - Must generate embeddings upfront
⚠️ **Higher memory usage** - Loading both indexes
⚠️ **Coupling** - Tantivy and Qdrant tightly coupled

---

## Strategy 2: Lazy Vector Indexing

### Overview

Index to Tantivy immediately (fast), defer Qdrant indexing until first semantic search is requested.

### Architecture

```
File Change → Tantivy Index (immediate)
                    ↓
              Metadata Cache

User requests semantic search
                    ↓
         Check if Qdrant populated
                    ↓
         If empty: Batch index all files
                    ↓
         Perform vector search
```

### Implementation

```rust
pub struct LazyVectorIndexer {
    bm25_index: Bm25Search,
    vector_store: VectorStore,
    vector_index_complete: AtomicBool,
}

impl LazyVectorIndexer {
    async fn ensure_vector_index(&mut self) -> Result<()> {
        if self.vector_index_complete.load(Ordering::Relaxed) {
            return Ok(());
        }

        // Check if Qdrant has any data
        let count = self.vector_store.count().await?;
        if count > 0 {
            self.vector_index_complete.store(true, Ordering::Relaxed);
            return Ok(());
        }

        // Populate from scratch
        tracing::info!("Vector index empty, performing initial indexing...");
        self.populate_vector_index().await?;
        self.vector_index_complete.store(true, Ordering::Relaxed);
        Ok(())
    }

    async fn populate_vector_index(&mut self) -> Result<()> {
        // Read all files from Tantivy index
        // Parse, chunk, embed, and index to Qdrant
        todo!()
    }
}
```

### Advantages

✅ **Fast initial search** - BM25 works immediately
✅ **Deferred cost** - Only pay embedding cost when needed
✅ **Graceful degradation** - Works with just Tantivy

### Disadvantages

⚠️ **First semantic search is slow** - Must populate entire vector index
⚠️ **Potential inconsistency** - Tantivy may be ahead of Qdrant
⚠️ **Complex state management** - Need to track completion

---

## Strategy 3: Background Indexing with notify

### Overview

Use `notify` crate to watch for file changes and index in background thread. Decouples indexing from MCP tool calls.

### Architecture

```
┌──────────────────────────────────────┐
│  Background Watcher Thread           │
│  (using notify crate)                │
│                                      │
│  File created/modified/deleted       │
│         ↓                            │
│  Debounce (100ms)                    │
│         ↓                            │
│  Send to indexing queue              │
└──────────────────┬───────────────────┘
                   ↓
┌──────────────────────────────────────┐
│  Indexing Worker Pool (tokio tasks)  │
│                                      │
│  Process queue:                      │
│  - Parse file                        │
│  - Chunk                             │
│  - Embed                             │
│  - Index to Tantivy + Qdrant         │
│  - Update cache                      │
└──────────────────────────────────────┘
```

### Implementation

```rust
use notify::{Watcher, RecursiveMode, Event, EventKind};
use tokio::sync::mpsc;
use std::time::Duration;

pub struct BackgroundIndexer {
    watcher: RecommendedWatcher,
    index_queue: mpsc::Sender<PathBuf>,
    worker_handles: Vec<JoinHandle<()>>,
}

impl BackgroundIndexer {
    pub async fn start(watch_path: &Path, indexer: Arc<Mutex<UnifiedIndexer>>) -> Result<Self> {
        let (tx, mut rx) = mpsc::channel::<PathBuf>(100);

        // Start file watcher
        let tx_clone = tx.clone();
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for path in event.paths {
                            if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                                let _ = tx_clone.try_send(path);
                            }
                        }
                    }
                    _ => {}
                }
            }
        })?;

        watcher.watch(watch_path, RecursiveMode::Recursive)?;

        // Start worker pool
        let mut workers = Vec::new();
        for _ in 0..4 {
            let indexer_clone = indexer.clone();
            let mut rx_clone = rx.clone();

            let worker = tokio::spawn(async move {
                while let Some(path) = rx_clone.recv().await {
                    tracing::debug!("Indexing changed file: {}", path.display());

                    let mut indexer = indexer_clone.lock().await;
                    if let Err(e) = indexer.index_file(&path).await {
                        tracing::error!("Failed to index {}: {}", path.display(), e);
                    }
                }
            });

            workers.push(worker);
        }

        Ok(Self {
            watcher,
            index_queue: tx,
            worker_handles: workers,
        })
    }
}
```

### Advantages

✅ **Real-time updates** - Index immediately when files change
✅ **Non-blocking** - MCP tools never wait for indexing
✅ **Automatic** - No manual index refresh needed
✅ **Parallel** - Worker pool processes multiple files

### Disadvantages

⚠️ **Complexity** - More moving parts
⚠️ **Resource usage** - Always running in background
⚠️ **Race conditions** - File might be queried before indexed
⚠️ **Debouncing needed** - Avoid indexing rapid changes

---

## Strategy 4: Merkle Tree Change Detection

### Overview

Use Merkle tree to efficiently detect which directories/files changed, minimizing hash computations for large codebases.

### Concept

```
Project Root (merkle root hash)
    ├─ src/ (directory hash)
    │   ├─ lib.rs (file hash)
    │   ├─ parser/ (directory hash)
    │   │   ├─ mod.rs (file hash)
    │   │   └─ call_graph.rs (file hash)
    │   └─ search/ (directory hash)
    │       └─ mod.rs (file hash)
    └─ tests/ (directory hash)
        └─ integration.rs (file hash)
```

**Key Insight:** If a directory's Merkle hash hasn't changed, skip all files within it.

### Implementation

```rust
use rs_merkle::{MerkleTree, Hasher};
use sha2::{Sha256, Digest};

#[derive(Clone)]
pub struct Sha256Hasher;

impl Hasher for Sha256Hasher {
    type Hash = [u8; 32];

    fn hash(data: &[u8]) -> Self::Hash {
        let mut hasher = Sha256::new();
        hasher.update(data);
        hasher.finalize().into()
    }
}

pub struct MerkleIndexer {
    tree: MerkleTree<Sha256Hasher>,
    file_map: HashMap<PathBuf, usize>, // file -> leaf index
}

impl MerkleIndexer {
    pub fn build_tree(root_dir: &Path) -> Result<Self> {
        let mut file_hashes = Vec::new();
        let mut file_map = HashMap::new();

        // Traverse directory and collect file hashes
        for (idx, entry) in WalkDir::new(root_dir)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("rs"))
            .enumerate()
        {
            let content = std::fs::read(entry.path())?;
            let hash = Sha256Hasher::hash(&content);
            file_hashes.push(hash);
            file_map.insert(entry.path().to_path_buf(), idx);
        }

        let tree = MerkleTree::<Sha256Hasher>::from_leaves(&file_hashes);

        Ok(Self { tree, file_map })
    }

    pub fn get_changed_files(&self, old_tree: &MerkleTree<Sha256Hasher>) -> Vec<PathBuf> {
        // Compare merkle roots
        if self.tree.root() == old_tree.root() {
            return Vec::new(); // No changes
        }

        // Find changed leaves by comparing proofs
        let mut changed = Vec::new();
        for (path, &idx) in &self.file_map {
            let old_proof = old_tree.proof(&[idx]);
            let new_proof = self.tree.proof(&[idx]);

            if old_proof != new_proof {
                changed.push(path.clone());
            }
        }

        changed
    }
}
```

### Usage

```rust
// Store merkle tree in cache
let current_tree = MerkleIndexer::build_tree(&project_root)?;

if let Some(cached_tree) = load_cached_merkle_tree()? {
    let changed_files = current_tree.get_changed_files(&cached_tree);

    // Only index changed files
    for file in changed_files {
        indexer.index_file(&file).await?;
    }
} else {
    // First index - process all files
    indexer.index_directory(&project_root).await?;
}

// Cache the new tree
save_merkle_tree(&current_tree)?;
```

### Advantages

✅ **Efficient for large codebases** - O(log n) change detection
✅ **Skip entire directories** - If subtree hash unchanged
✅ **Hierarchical** - Can detect changes at any level
✅ **Cryptographically sound** - Merkle tree guarantees

### Disadvantages

⚠️ **Memory overhead** - Store entire tree structure
⚠️ **Complexity** - More complex than simple hashing
⚠️ **Initial build cost** - Must hash all files upfront

---

## Strategy 5: Separate Write/Read Buffers (Advanced)

### Overview

Inspired by vector database best practices: separate indexing (write) from querying (read) with periodic merging.

### Architecture

```
Write Path (Indexing):
    New/Changed Files
          ↓
    Parse → Chunk → Embed
          ↓
    Write Buffer (in-memory)
          ↓
    Periodic Flush (every 5 min or 1000 chunks)
          ↓
    Merge into Main Index

Read Path (Search):
    Query
      ↓
    Search Main Index + Write Buffer
      ↓
    Merge Results
```

### Implementation

```rust
pub struct BufferedIndexer {
    main_index: Arc<RwLock<MainIndex>>,
    write_buffer: Arc<Mutex<WriteBuffer>>,
    flush_interval: Duration,
}

struct WriteBuffer {
    tantivy_docs: Vec<TantivyDocument>,
    qdrant_points: Vec<PointStruct>,
    chunk_count: usize,
}

impl BufferedIndexer {
    pub async fn index_file(&self, file: &Path) -> Result<()> {
        // Parse and chunk
        let chunks = /* ... */;

        // Add to write buffer (fast, in-memory)
        let mut buffer = self.write_buffer.lock().await;
        buffer.add_chunks(chunks);

        // Trigger flush if buffer full
        if buffer.chunk_count >= 1000 {
            drop(buffer);
            self.flush().await?;
        }

        Ok(())
    }

    pub async fn flush(&self) -> Result<()> {
        let mut buffer = self.write_buffer.lock().await;

        if buffer.is_empty() {
            return Ok(());
        }

        tracing::info!("Flushing {} chunks to main index", buffer.chunk_count);

        // Get write lock on main index
        let mut main = self.main_index.write().await;

        // Commit to Tantivy
        main.tantivy_writer.add_documents(buffer.tantivy_docs.drain(..))?;
        main.tantivy_writer.commit()?;

        // Upsert to Qdrant (batched)
        main.vector_store.upsert_points_batched(buffer.qdrant_points.drain(..), 100).await?;

        buffer.chunk_count = 0;

        Ok(())
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Search both main index and write buffer
        let main = self.main_index.read().await;
        let buffer = self.write_buffer.lock().await;

        let main_results = main.search(query, limit).await?;
        let buffer_results = buffer.search(query, limit)?;

        // Merge and re-rank
        Ok(merge_results(main_results, buffer_results, limit))
    }
}
```

### Advantages

✅ **Fast writes** - In-memory buffer
✅ **Batched commits** - Better throughput
✅ **Configurable** - Tune flush frequency
✅ **Production-grade** - Used by Elasticsearch, Meilisearch

### Disadvantages

⚠️ **Eventual consistency** - Buffer not immediately searchable
⚠️ **Memory pressure** - Buffer can grow large
⚠️ **Complex search** - Must query both buffer and main index
⚠️ **Data loss risk** - Buffer in-memory until flushed

---

## Comparison Matrix

| Strategy | Consistency | Performance | Complexity | Real-time | Memory | Best For |
|----------|-------------|-------------|------------|-----------|--------|----------|
| **1. Unified** | ★★★★★ | ★★★☆☆ | ★★☆☆☆ | ★★★☆☆ | ★★★☆☆ | **Small-medium codebases** |
| **2. Lazy** | ★★★☆☆ | ★★★★☆ | ★★★☆☆ | ★★☆☆☆ | ★★★★☆ | **Infrequent semantic search** |
| **3. Background** | ★★★★☆ | ★★★★★ | ★★★★☆ | ★★★★★ | ★★★☆☆ | **Active development** |
| **4. Merkle** | ★★★★★ | ★★★★★ | ★★★★★ | ★★★☆☆ | ★★☆☆☆ | **Large codebases (1M+ LOC)** |
| **5. Buffered** | ★★★☆☆ | ★★★★★ | ★★★★★ | ★★★★☆ | ★★☆☆☆ | **High-throughput indexing** |

---

## Recommended Hybrid Approach

For maximum flexibility, implement **Strategy 1 (Unified)** + **Strategy 3 (Background)**:

### Phase 1: Unified Indexing (Week 1-2)
1. Create `UnifiedIndexer` that populates both Tantivy + Qdrant
2. Modify `search` tool to use unified pipeline
3. Test on medium codebase (10k-100k LOC)

### Phase 2: Background Watching (Week 3)
1. Add `BackgroundIndexer` with `notify`
2. Optional: Enable via config flag
3. Debounce rapid changes (100ms)

### Phase 3: Merkle Optimization (Optional - Week 4)
1. Add Merkle tree for large codebases
2. Only trigger if project > 500k LOC
3. Fallback to SHA-256 for smaller projects

### Implementation Roadmap

```rust
// src/indexing/mod.rs
pub mod unified;      // Strategy 1
pub mod background;   // Strategy 3
pub mod merkle;       // Strategy 4 (optional)

// Configuration
pub struct IndexingConfig {
    pub mode: IndexingMode,
    pub enable_background_watch: bool,
    pub enable_merkle_tree: bool, // Auto-enable if > 500k LOC
    pub flush_interval: Duration,
}

pub enum IndexingMode {
    Unified,           // Both indexes together
    Lazy,              // Tantivy first, Qdrant on-demand
    Buffered { size: usize }, // Write buffer
}
```

---

## Performance Targets

| Codebase Size | Initial Index | Incremental Update | Search Latency |
|---------------|---------------|-------------------|----------------|
| 10k LOC | < 30 sec | < 1 sec | < 100ms |
| 100k LOC | < 2 min | < 2 sec | < 150ms |
| 500k LOC | < 5 min | < 3 sec | < 200ms |
| 1M LOC | < 10 min | < 5 sec | < 300ms |

---

## Implementation Checklist

### Must Have (MVP)
- [ ] Create `UnifiedIndexer` module
- [ ] Integrate chunker + embeddings into search tool
- [ ] Populate Qdrant during indexing
- [ ] Test hybrid search end-to-end
- [ ] Add config for Qdrant URL

### Should Have (v0.2)
- [ ] Background indexing with `notify`
- [ ] Debouncing for rapid changes
- [ ] Progress reporting for large indexes
- [ ] Index statistics/health API

### Nice to Have (v0.3)
- [ ] Merkle tree optimization
- [ ] Write buffer strategy
- [ ] GPU-accelerated embeddings
- [ ] Distributed indexing

---

## Migration Path

### Step 1: Fix Immediate Issue
Modify `search_tool.rs` to call unified indexing:

```rust
// Before: Only Tantivy
index_writer.add_document(doc!(/* ... */));

// After: Both indexes
let chunks = chunker.chunk_file(/* ... */)?;
let embeddings = embedding_gen.embed_batch(/* ... */)?;
bm25.index_chunks(&chunks)?;
vector_store.upsert_chunks(/* ... */).await?;
```

### Step 2: Add Background Watcher (Optional)
Enable via CLI flag: `--watch`

### Step 3: Optimize for Scale
Add Merkle tree when codebase > 500k LOC

---

## Open Questions

1. **Should Qdrant be optional?**
   - If Qdrant server not available, fall back to BM25-only?
   - Add health check before indexing?

2. **How to handle partial failures?**
   - If Tantivy succeeds but Qdrant fails, retry? Skip?

3. **Collection naming strategy?**
   - One collection per project?
   - Global collection with project_id in metadata?

4. **Embedding model choice?**
   - Current: all-MiniLM-L6-v2 (384d)
   - Consider: larger model for better quality?

5. **When to rebuild indexes?**
   - Schema version change?
   - Corruption detection?

---

## Conclusion

**Recommendation:** Start with **Strategy 1 (Unified)** for simplicity and correctness, then add **Strategy 3 (Background)** for real-time updates.

This provides:
- ✅ Correct implementation (both indexes populated)
- ✅ Reasonable performance (< 5 min for 1M LOC)
- ✅ Good developer experience (automatic updates)
- ✅ Room for optimization (add Merkle/buffering later)

**Next Steps:**
1. Implement `UnifiedIndexer` module
2. Modify `search` tool to use it
3. Test on `rust-analyzer` codebase (~200k LOC)
4. Measure and optimize

---

**Document Version:** 1.0
**Last Updated:** 2025-10-19
**Author:** Analysis based on codebase exploration and SOTA research

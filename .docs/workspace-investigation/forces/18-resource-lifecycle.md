# Force 18 — Resource Lifecycle Clarity

Long-lived expensive resources today live where they were first needed, not where they are conceptually owned. This force scores each candidate split on **where each resource is constructed, where it is dropped, and how it crosses crate boundaries**.

## Resource inventory (today)

| Resource | Constructed in | Stored as | Dropped |
| --- | --- | --- | --- |
| ONNX `TextEmbedding` | `EmbeddingGenerator::new` (lazy, in `embeddings/mod.rs`) | `Arc<Mutex<TextEmbedding>>` field; cloned per call site | end of process; never reset |
| `AnalysisHost` (live) | `loader::load_project` on cache miss | `LazyLock<Mutex<SemanticService>>` (`HashMap<PathBuf, ProjectContext>`) | end of process; no eviction |
| `AnalysisHost` (graph) | `graph::loader::load` on each `build_hypergraph` / AST-audit call | local in extraction; dropped at fn return | per call |
| LanceDB connection | `LanceDbBackend::new` per `VectorStore` instance | `VectorStore` returned by tools per call | dropped per call (no pooling) |
| Tantivy `IndexWriter` (+ merge threads) | `TantivyAdapter::new` inside `UnifiedIndexer` | field on `UnifiedIndexer`; `Drop` rolls back | when indexer dropped |
| heed/LMDB env | `OpenedSnapshot::open` per query | local; pinned by readers | per query |
| sled `MetadataCache` | `MetadataCache::open` inside `IncrementalIndexer` | field on indexer | when indexer dropped |
| `SyncManager` task | `SyncManager::with_defaults`, `tokio::spawn` in `main` | `Arc<SyncManager>` shared with router | never (no shutdown signal) |

Two truths cut across every split: (a) **the embedding model and the live `AnalysisHost` are global singletons** because they are large, slow, and `!Sync`; (b) **the rest are scoped resources** that must be aggregated into a service object if we want explicit ownership.

## Per-split ownership story

### 04 — Minimal split (`rust-code-core` + `file-search-mcp`)

All long-lived resources stay in `file-search-mcp`. `rust-code-core` is pure data. **Lifecycle clarity does not improve over today**: the singletons remain hidden in `LazyLock`s; `SyncManager` shutdown is still absent. The only win is that pure-data modules cannot accidentally hold a `TextEmbedding`. Cross-crate sharing is moot.

### 02 — Pipeline-keyed (`rcm-core` … `rcm-transport`)

The resources spread across **five different crates**: `rcm-storage` (Tantivy/LanceDB/sled/heed), `rcm-embed` (ONNX), `rcm-analyze` (graph `AnalysisHost`, semantic `AnalysisHost`), `rcm-ingest` (Tantivy writer + sled handle composed in), `rcm-transport` (`SyncManager`). Composition happens implicitly inside each pipeline crate. **Drops are scattered**: a `TextEmbedding` in `rcm-embed`, a sled handle in `rcm-storage`, a writer in `rcm-ingest` — three different crates with `Drop` impls that must agree. Lock contention reasoning is the worst here: the embedding mutex is held in `rcm-embed`, but both `rcm-ingest` and `rcm-query` enter it, and you cannot see that without crossing crate boundaries.

### 03 — Hexagonal (ports + per-backend adapters)

Each adapter crate constructs and drops exactly one resource: `fsm-adapter-fastembed` owns the ONNX session, `fsm-adapter-rust-analyzer` owns both `AnalysisHost`s, `fsm-adapter-lancedb` owns the LanceDB connection, etc. **`fsm-bin` is the single drop point** — it builds the adapters, hands `Arc<dyn Port>` (or generics) to `fsm-app`, and drops everything in reverse order on shutdown. This is the clearest lifecycle on paper, but pays for it: every shared resource crosses a crate boundary as `Arc<dyn Port>`, and the singletons (model, live host) become *interface mutexes* that hide behind a trait. You can no longer `grep` for `Mutex<TextEmbedding>` from a tool handler — the fact that all embed calls serialize is now an *adapter implementation detail*, not visible at the use-case site.

### 01 — Capability-keyed (5 + 1 server)

Three runtime owners: `corpus-search` owns `EmbeddingGenerator` + LanceDB + Tantivy + sled (one `CorpusServices` struct); `code-graph` owns the graph `AnalysisHost` + heed envs; `live-nav` owns the `SemanticService` singleton. `mcp-server` owns `SyncManager` and a single `AppServices { corpus, graph, nav }` aggregate; **`main` is the one place where everything is constructed and where `Drop` runs**. Sharing across capabilities is rare by construction (the whole point of the split), and where it exists it is an explicit field on `AppServices`, not a `LazyLock`. `SyncManager` shutdown becomes natural: `mcp-server` already owns the join handle, so a `CancellationToken` on `AppServices` is one line. The two unavoidable `!Sync` singletons (model, live host) stay as `Arc<Mutex<…>>` *inside their capability crate*, where the contention is local and visible.

## Recommendation — **Capability-keyed (01)**

It is the only split that produces a single `AppServices` aggregate constructed in `main`, gives each capability one struct that owns its resources end-to-end, makes `Drop` order obvious, and lets `SyncManager` shutdown be added without touching three crates. Hexagonal scores higher in theory but hides the singleton mutexes behind ports; pipeline-keyed scatters owners across five crates; minimal-split changes nothing.

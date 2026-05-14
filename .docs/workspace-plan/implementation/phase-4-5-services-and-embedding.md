# Phase 4 & 5 — Service Lifetime + Invalidation; Embedding Sealed Trait

Authoritative source: `.docs/workspace-plan/DECISIONS.md`. The lifetime/invalidation table in §"Service lifetime + invalidation" and the Cargo wiring in `rcm-embedding` are normative; this plan only operationalizes them.

Pre-conditions assumed entering Phase 4:

- Phase 1: 8 crates exist as adapters; `Embedder` is a façade `Arc<...>` over `legacy::EmbeddingGenerator`.
- Phase 2: hidden singletons (`static SEMANTIC`, lazy `LazyLock<Mutex<...>>`) are gone; services are passed by `Arc` from `main`.
- Phase 3: errors are operation-scoped (`SearchError`, `QueryError`, `IndexError`, `BuildError`, ...); no god-enum.

---

## Phase 4: Service lifetime + invalidation contract

Goal: make capability services long-lived, drop per-call `VectorStore::open` / `Bm25Search::new` / `OpenedSnapshot::open`, install `ArcSwap`-based hot reload, and wire `CancellationToken` shutdown.

### Step 1 — Inventory per-call store opens today

**What to do.** Grep the legacy modules for every place a backend is opened on the request path. Document them in a table (commit it under `.docs/workspace-plan/implementation/phase-4-inventory.md`). The known sites, recovered from `tools.md` §Concurrency and from `query_tools::search`:

| Call site (today) | Per-call open | Replacement owner |
|---|---|---|
| `query_tools::search` | `Bm25Search::new(tantivy_path)`, `VectorStore::new_embedded(vec_path, EMBEDDING_DIM)`, then `HybridSearch::new(...)` | `SearchService` field |
| `query_tools::get_similar_code` | `EmbeddingGenerator::lazy()` + `VectorStore::new_embedded` | `SearchService` field |
| `index_tool::index_codebase` | `IncrementalIndexer::new(...)` (constructs Tantivy writer + sled + LanceDB) | `CorpusWriter` field on `SearchService` |
| `graph_tools::*` (every snapshot tool) | `open_workspace_snapshot(directory)` → `OpenedSnapshot::open(...)` | `GraphService::current_snapshot: ArcSwap<OpenedSnapshot>` |
| `analysis_tools::find_definition` / `find_references` | `SEMANTIC.lock()` then per-file `RaHost`-equivalent | `IdeService::cache: ArcSwap<HashMap<PathBuf, Arc<RaHost>>>` |
| `health_tool::health_check` | `Bm25Search::new`, `VectorStore::new_embedded` (read-only probes) | borrow `SearchService` handles |
| `clear_cache_tool::clear_cache` | rm-rf of paths + nothing else | becomes a delete-then-invalidate orchestrator: refuse on `IndexBusy`, then `rm -rf` scope paths, then call `SearchService::invalidate(workspace)` / `GraphService::invalidate(workspace)` / `IdeService::evict(workspace)`. No reload, no auto-reindex; the next read triggers lazy rebuild via fingerprint mismatch. |

**Files touched.** Read-only: `src/legacy/tools/{query_tools,index_tool,analysis_tools,graph_tools,health_tool,clear_cache_tool}.rs`. Output: the inventory doc.

**Acceptance.** Every per-call open is enumerated with caller, file, function, and target service field.

**Reversal.** No code change; safe.

### Step 2 — Replace per-call opens with `ArcSwap`-managed handles

**What to do.** Define each capability service's field set. Use `arc_swap::ArcSwap` for read-mostly handles, `tokio::sync::Mutex` for the single writer, plain `sled::Db` for construction-time-only state.

```rust
// crates/rcm-search/src/service.rs
use arc_swap::ArcSwap;
use std::sync::Arc;
use tokio::sync::Mutex;
use rcm_paths::ProjectPaths;
use rcm_embedding::Embedder;

pub struct SearchService {
    paths: ProjectPaths,
    embedder: Embedder,                              // Arc<dyn Embed>
    tantivy_reader: ArcSwap<tantivy::IndexReader>,
    tantivy_writer: Mutex<tantivy::IndexWriter>,     // single writer
    lance_conn:   ArcSwap<lancedb::Connection>,
    sled_db:      sled::Db,                          // construction-time only
    indexing_metrics: std::sync::Mutex<crate::IndexingMetrics>,
}
```

```rust
// crates/rcm-graph/src/service.rs
pub struct GraphService {
    paths: ProjectPaths,
    ra_host_factory: Arc<dyn Fn(&Path) -> Result<RaHost, RaError> + Send + Sync>,
    current_snapshot: ArcSwap<OpenedSnapshot>,
    embedder: Option<Embedder>,                      // gated by `semantic-overlaps`
}
```

```rust
// crates/rcm-ide/src/service.rs
pub struct IdeService {
    paths: ProjectPaths,
    cache: ArcSwap<im::HashMap<PathBuf, Arc<RaHost>>>, // im::HashMap = cheap clone
}
```

`im::HashMap` is used so `ArcSwap::store` swaps a snapshot of the cache without locking; insertions clone-and-store.

**Files touched.** New: `crates/rcm-search/src/service.rs`, `crates/rcm-graph/src/service.rs`, `crates/rcm-ide/src/service.rs`. Edit each capability `lib.rs` to re-export.

**Acceptance.** `cargo build --workspace` green; every public method that previously called `VectorStore::open` now reads `self.lance_conn.load_full()`.

**Reversal.** Revert by re-pointing handlers at the legacy free functions; the field set is additive.

### Step 3 — Implement `reload` methods

**What to do.** Each service exposes a public `reload` method. Open the new handles, `ArcSwap::store` them, and spawn a tokio task that drops the old `Arc` after a grace period so in-flight `.load_full()` callers complete on the previous handle.

```rust
// crates/rcm-search/src/service.rs
use std::time::Duration;

impl SearchService {
    pub async fn reload(&self, workspace: &Path) -> Result<(), SearchError> {
        // 1. Refuse if a write batch is mid-flight.
        let writer_guard = self.tantivy_writer.try_lock()
            .map_err(|_| SearchError::IndexBusy)?;
        drop(writer_guard); // we only needed the probe; reopen below

        // 2. Reopen reader + LanceDB connection without touching the writer.
        let new_reader = open_tantivy_reader(&self.paths)?;
        let new_lance  = open_lance_conn(&self.paths).await?;

        // 3. Atomic swap; previous handles are returned as Arc<...>.
        let old_reader = self.tantivy_reader.swap(Arc::new(new_reader));
        let old_lance  = self.lance_conn.swap(Arc::new(new_lance));

        // 4. Drop the old handles after a grace period. In-flight queries
        // that called `.load_full()` before the swap are still holding
        // their own `Arc`s; this task only releases *our* reference.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(old_reader);
            drop(old_lance);
        });

        tracing::info!(workspace = %workspace.display(), "search service reloaded");
        Ok(())
    }
}
```

`GraphService` has TWO distinct operations: `invalidate` (cheap, drop
handle, used by `clear_cache`) and `reload` (rebuild snapshot, used by
schema-change paths). `clear_cache` calls `invalidate`, NOT `reload`:

```rust
impl GraphService {
    /// Drop the cached `OpenedSnapshot`. The next query will rebuild via
    /// the existing fingerprint-mismatch path. No on-disk work here.
    pub fn invalidate(&self, workspace: &Path) {
        self.snapshot.swap(Arc::new(OpenedSnapshot::Closed));
        tracing::info!(workspace = %workspace.display(), "graph service invalidated");
    }

    /// Rebuild the snapshot eagerly (used by `SyncManager` schema change).
    pub async fn reload(&self, workspace: &Path) -> Result<(), QueryError> {
        let snap = tokio::task::spawn_blocking({
            let paths = self.paths.clone();
            let factory = Arc::clone(&self.ra_host_factory);
            move || rebuild_snapshot(&paths, &*factory, workspace)
        })
        .await
        .map_err(|e| QueryError::Join(e.to_string()))??;
        let old = self.current_snapshot.swap(Arc::new(snap));
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            drop(old);
        });
        Ok(())
    }
}
```

`IdeService::evict(workspace)` simply removes the workspace's `RaHost` from the `im::HashMap` snapshot and stores the new map.

**Files touched.** Same files as Step 2.

**Acceptance.** Calling `reload` while a long search is in flight does not crash; the long search returns successfully against the old reader.

**Reversal.** Each `reload` is independent; revert by removing the method and the `clear_cache` callsite.

### Step 4 — Wire invalidation triggers

**What to do.** Implement the trigger table from DECISIONS verbatim:

| Trigger | Action |
|---|---|
| `clear_cache(workspace, scope)` | (1) Refuse with `IndexBusy` if a writer holds the lock. (2) `rm -rf` the on-disk paths for the scope. (3) Call `SearchService::invalidate`, `GraphService::invalidate`, `IdeService::evict` (NOT reload — handles drop, next op rebuilds via fingerprint mismatch). |
| Workspace fingerprint mismatch on graph query | Transparent rebuild via `build_and_persist` (already in `rcm-graph`). |
| `SyncManager` schema change | `SearchService::reload(workspace)`. |
| `track_directory(new)` | No reload; `SearchService` opens lazily on first `search`. |

`clear_cache` handler pseudocode (lives in `rcm-server`):

```rust
// crates/rcm-server/src/tools/clear_cache.rs
async fn clear_cache(
    state: &AppState,
    p: ClearCacheParams,
) -> Result<CallToolResult, McpError> {
    let workspace = validate_workspace(&p.directory)?;
    let paths = ProjectPaths::resolve(&workspace, &state.storage_root)?;

    // 1. Refuse if a writer is mid-batch.
    state.search.guard_writer_idle(&workspace)
        .map_err(|_| McpError::invalid_params("indexing in progress; retry shortly", None))?;

    // 2. Delete on-disk artifacts (scope-aware; default is the whole workspace).
    delete_scope(&paths, p.scope.unwrap_or(ClearScope::Workspace))?;

    // 3. Drop in-memory handles. Next access rebuilds via fingerprint mismatch.
    state.search.invalidate(&workspace);
    state.graph.invalidate(&workspace);
    state.ide.evict(&workspace);

    Ok(CallToolResult::success(Content::text("cache cleared")))
}
```

The `SyncManager` schema-change branch: when `IncrementalIndexer` reports a Tantivy schema-version bump, the worker calls `state.search.reload(dir)` *after* its commit completes. Per the §13 placement decision, the worker is `IncrementalSyncJob` in `rcm-search`, the shell is `SyncManager` in `rcm-server`; the call goes through an `Arc<SearchService>` that the worker holds.

**Files touched.** `crates/rcm-server/src/tools/clear_cache.rs`, `crates/rcm-server/src/sync.rs` (the shell), `crates/rcm-search/src/sync_job.rs`.

**Acceptance.** Smoke checklist `clear_cache(workspace)` followed by `search` succeeds without restart.

**Reversal.** `clear_cache` falls back to its legacy rm-rf-only behavior in one revert.

### Step 5 — Graceful shutdown

**What to do.** `rcm-server` owns a `CancellationToken`. The runtime structure:

```rust
// crates/rcm-server/src/main.rs
use tokio_util::sync::CancellationToken;
use tokio::sync::Semaphore;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cfg = Config::load()?;
    let shutdown = CancellationToken::new();
    let in_flight = Arc::new(Semaphore::new(/* large */ 1024));

    // Drop order = reverse of field declaration order in AppState.
    // Declare leaves last so they drop last.
    let state = Arc::new(AppState {
        server:   ServerHandle::new(),                       // dropped first
        search:   Arc::new(SearchService::open(&cfg).await?),
        graph:    Arc::new(GraphService::open(&cfg).await?),
        ide:      Arc::new(IdeService::open(&cfg).await?),
        embedder: Embedder::production(&cfg)?,               // dropped last
    });

    let sync = SyncManager::spawn(
        state.search.clone(),
        shutdown.child_token(),
    );

    let serve = tokio::spawn(serve_stdio(
        state.clone(),
        in_flight.clone(),
        shutdown.child_token(),
    ));

    // SIGINT or stdin EOF → cancel.
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("SIGINT"),
        _ = wait_stdin_eof()         => tracing::info!("stdin EOF"),
    }
    shutdown.cancel();

    // Drain: wait for in-flight tools, 30s budget.
    let drain = async {
        let _ = serve.await;
        let _ = sync.await;
        // Acquire all permits ⇒ no tool handler is running.
        let _all = in_flight.acquire_many(1024).await.ok();
    };
    if tokio::time::timeout(Duration::from_secs(30), drain).await.is_err() {
        tracing::error!("shutdown drain exceeded 30s; aborting");
    }
    Ok(())
}
```

Each tool handler acquires a permit:

```rust
let _permit = state.in_flight.clone().acquire_owned().await
    .map_err(|_| McpError::internal_error("server shutting down", None))?;
```

`SyncManager::run` exits cleanly on cancel:

```rust
loop {
    tokio::select! {
        _ = interval.tick() => self.handle_sync_all().await,
        _ = self.shutdown.cancelled() => break,
    }
}
```

Drop order is achieved by Rust's "fields drop in declaration order": when `Arc<AppState>` reaches refcount zero at end of `main`, fields drop top-to-bottom (`server` first, `embedder` last). This matches the topological order from DECISIONS: server → capabilities → leaves.

**Files touched.** `crates/rcm-server/src/main.rs`, `crates/rcm-server/src/sync.rs`, every tool handler (one-line permit acquire).

**Acceptance.** Manual SIGINT against a running server: log shows "stdin EOF" / "SIGINT", drain completes, process exits with code 0; `lsof` shows no orphaned LMDB / sled lock files held by zombie tasks.

**Reversal.** Removing the cancel branch reverts to the current unbounded loop; permits become no-ops.

### Step 6 — Test the reload path

**What to do.** Add an integration test under `crates/rcm-server/tests/reload.rs`:

```rust
#[tokio::test(flavor = "multi_thread")]
async fn clear_cache_then_search_succeeds() {
    let fixture = TestWorkspace::with_rust_files(&[
        ("src/lib.rs", "pub fn alpha() {}"),
    ]).await;
    let state = AppState::test_with(&fixture).await;

    // Index, search, clear, search.
    state.search.index_now(fixture.path()).await.unwrap();
    let hits1 = state.search.search("alpha", 5).await.unwrap();
    assert!(!hits1.is_empty());

    clear_cache_tool(&state, fixture.path()).await.unwrap();
    // After clear_cache, on-disk data is gone and handles are invalidated.
    // The next `search` must trigger a transparent rebuild via the
    // fingerprint-mismatch path — the existing UnifiedIndexer.ensure_indexed
    // recovery in query_tools::search. This is NOT auto-reindex on clear;
    // it's lazy rebuild on the next read.
    let hits2 = state.search.search("alpha", 5).await.unwrap();
    assert!(!hits2.is_empty(), "search must lazily rebuild after clear_cache");
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_cache_during_index_returns_busy() {
    let fixture = TestWorkspace::large().await;
    let state = AppState::test_with(&fixture).await;
    let writer = tokio::spawn({
        let s = state.search.clone();
        let p = fixture.path().to_owned();
        async move { s.index_now(&p).await }
    });
    // Race the writer.
    let err = state.search.reload(fixture.path()).await
        .expect_err("reload mid-batch must error");
    assert!(matches!(err, SearchError::IndexBusy));
    writer.await.unwrap().unwrap();
}
```

**Files touched.** `crates/rcm-server/tests/reload.rs`.

**Acceptance.** Both tests pass under `cargo test -p rcm-server -- --test-threads=1`.

**Reversal.** Drop the test file.

### Phase 4 acceptance (gate to Phase 5)

- All capability services constructed once in `main`. `git grep -nE 'VectorStore::(open|new_embedded)|Bm25Search::new|OpenedSnapshot::open'` returns hits only inside the `*Service::open` and `*Service::reload` constructors, and inside `xtask`.
- Reload semantics tested (Step 6 tests green).
- Shutdown drain works on SIGINT (manual test). No orphaned lockfiles.
- Smoke checklist (`index_codebase` … `clear_cache` + re-`search`) passes.

---

## Phase 5: Embedding sealed trait + feature gate

Goal: replace the Phase-1 façade (`Embedder` newtyping `legacy::EmbeddingGenerator`) with a sealed-trait, dyn-dispatched `Embedder`. Add `test-fakes`. Make `rcm-graph`'s embedding dep feature-gated.

### Step 1 — Implement the sealed trait

**What to do.** Replace the façade with a sealed trait. Production impl is `FastEmbedEmbedder`; test impl is `DeterministicEmbedder`.

```rust
// crates/rcm-embedding/src/lib.rs
#![warn(missing_docs)]

mod sealed { pub trait Sealed {} }

/// Sealed embedding contract. All concrete impls live in this crate.
pub trait Embed: sealed::Sealed + Send + Sync {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
    fn dimensions(&self) -> usize;
}

/// Type alias: every consumer holds embeddings via this Arc.
pub type Embedder = std::sync::Arc<dyn Embed>;

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("model init failed: {0}")] ModelInit(String),
    #[error("embed failed: {0}")]      EmbedFailed(String),
    #[error("no embedding produced")]  Empty,
    #[error("blocking task join failed: {0}")] Join(String),
    #[error("embedder not enabled at build time")] Unsupported,
}

#[cfg(feature = "embeddings")]
mod fastembed_embedder;
#[cfg(feature = "embeddings")]
pub use fastembed_embedder::FastEmbedEmbedder;

#[cfg(feature = "test-fakes")]
mod deterministic_embedder;
#[cfg(feature = "test-fakes")]
pub use deterministic_embedder::DeterministicEmbedder;

/// Async wrapper — the only place embedding hits tokio.
pub async fn embed_batch_async(
    e: &Embedder,
    texts: Vec<String>,
) -> Result<Vec<Vec<f32>>, EmbedError> {
    let e = e.clone();
    tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        e.embed_batch(&refs)
    })
    .await
    .map_err(|err| EmbedError::Join(err.to_string()))?
}
```

```rust
// crates/rcm-embedding/src/fastembed_embedder.rs
use std::sync::{Arc, Mutex};
use crate::{Embed, EmbedError, sealed};

pub struct FastEmbedEmbedder {
    inner: Arc<Mutex<fastembed::TextEmbedding>>,
    dim: usize,
}

impl FastEmbedEmbedder {
    pub fn new(cfg: crate::EmbedderConfig) -> Result<crate::Embedder, EmbedError> {
        let model = fastembed::TextEmbedding::try_new(cfg.into_options())
            .map_err(|e| EmbedError::ModelInit(e.to_string()))?;
        Ok(Arc::new(Self { inner: Arc::new(Mutex::new(model)), dim: 384 }))
    }
}

impl sealed::Sealed for FastEmbedEmbedder {}
impl Embed for FastEmbedEmbedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let mut guard = self.inner.lock()
            .map_err(|_| EmbedError::EmbedFailed("mutex poisoned".into()))?;
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        guard.embed(owned, None)
            .map_err(|e| EmbedError::EmbedFailed(e.to_string()))
    }
    fn dimensions(&self) -> usize { self.dim }
}
```

```rust
// crates/rcm-embedding/src/deterministic_embedder.rs
use crate::{Embed, EmbedError, sealed};

/// Hash-based embedder. Same input → same output, no model load.
pub struct DeterministicEmbedder { dim: usize }

impl DeterministicEmbedder {
    pub fn new(dim: usize) -> crate::Embedder {
        std::sync::Arc::new(Self { dim })
    }
    fn one(&self, text: &str) -> Vec<f32> {
        let mut out = vec![0f32; self.dim];
        for (i, b) in text.bytes().enumerate() {
            out[i % self.dim] += (b as f32) / 255.0;
        }
        let n: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
        out.iter_mut().for_each(|x| *x /= n);
        out
    }
}

impl sealed::Sealed for DeterministicEmbedder {}
impl Embed for DeterministicEmbedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if texts.is_empty() { return Err(EmbedError::Empty); }
        Ok(texts.iter().map(|t| self.one(t)).collect())
    }
    fn dimensions(&self) -> usize { self.dim }
}
```

**Files touched.** `crates/rcm-embedding/src/{lib.rs,fastembed_embedder.rs,deterministic_embedder.rs}`. Delete the Phase-1 façade.

**Acceptance.** `cargo build -p rcm-embedding --no-default-features` builds the trait alone (no fastembed). `cargo build -p rcm-embedding --features embeddings` builds production. `cargo build -p rcm-embedding --features test-fakes` builds the deterministic impl.

**Reversal.** Re-introduce the façade; the trait is additive.

### Step 2 — Cargo wiring

```toml
# crates/rcm-embedding/Cargo.toml
[package]
name = "rcm-embedding"
edition = "2024"

[features]
default = ["embeddings"]
embeddings = ["dep:fastembed", "dep:ort"]
test-fakes = []

[dependencies]
thiserror   = { workspace = true }
tokio       = { workspace = true, features = ["rt"] }
fastembed   = { workspace = true, optional = true }
ort         = { workspace = true, optional = true }
```

```toml
# crates/rcm-search/Cargo.toml
[dependencies]
rcm-embedding = { path = "../rcm-embedding", default-features = false }
```

```toml
# crates/rcm-server/Cargo.toml
[dependencies]
rcm-embedding = { path = "../rcm-embedding", features = ["embeddings"] }
rcm-search    = { path = "../rcm-search" }
rcm-graph     = { path = "../rcm-graph", features = ["semantic-overlaps"] }
rcm-ide       = { path = "../rcm-ide" }

[dev-dependencies]
rcm-embedding = { path = "../rcm-embedding", features = ["test-fakes"] }
```

Capability crates declaring `default-features = false` keeps them honest — they cannot accidentally call into `FastEmbedEmbedder`. They only see the trait, the `Embedder` alias, and `EmbedError`. The binary is the only crate that turns `embeddings` on; the binary's `[dev-dependencies]` is the only place `test-fakes` is enabled in the runtime tree.

**Files touched.** All four `Cargo.toml`s.

**Acceptance.** `cargo tree -p rcm-search | grep -E '(fastembed|ort)'` returns empty.

**Reversal.** Single revert of the four Cargo.toml diffs.

### Step 3 — Make `rcm-graph`'s embedding dep optional

```toml
# crates/rcm-graph/Cargo.toml
[features]
default = []
semantic-overlaps = ["dep:rcm-embedding"]

[dependencies]
rcm-embedding = { path = "../rcm-embedding", default-features = false, optional = true }
```

```rust
// crates/rcm-graph/src/service.rs
pub struct GraphServiceBuilder {
    paths: ProjectPaths,
    ra_host: Arc<RaHost>,
    #[cfg(feature = "semantic-overlaps")]
    embedder: Option<rcm_embedding::Embedder>,
}

impl GraphServiceBuilder {
    #[cfg(feature = "semantic-overlaps")]
    pub fn with_embedder(mut self, e: rcm_embedding::Embedder) -> Self {
        self.embedder = Some(e); self
    }
    pub fn build(self) -> Result<GraphService, BuildError> { /* ... */ }
}

impl GraphService {
    pub async fn semantic_overlaps(&self, q: SemanticOverlapsRequest)
        -> Result<SemanticOverlapsResponse, QueryError>
    {
        #[cfg(feature = "semantic-overlaps")]
        {
            let Some(e) = &self.embedder else {
                return Err(QueryError::EmbedderUnavailable);
            };
            // ... use e via rcm_embedding::embed_batch_async ...
            todo!()
        }
        #[cfg(not(feature = "semantic-overlaps"))]
        { Err(QueryError::EmbedderUnavailable) }
    }
}
```

`QueryError::EmbedderUnavailable` is already specced in DECISIONS §`rcm-graph`. Adding the variant is a one-line `thiserror` addition; downstream `rcm-server` maps it to a clear MCP error string.

**Files touched.** `crates/rcm-graph/Cargo.toml`, `crates/rcm-graph/src/{service.rs,error.rs,semantic_overlaps.rs}`.

**Acceptance.** `cargo build -p rcm-graph --no-default-features` passes; `cargo tree -p rcm-graph` shows no `rcm-embedding`. `cargo build -p rcm-graph --features semantic-overlaps` passes and pulls `rcm-embedding`.

**Reversal.** Drop the `cfg` gate; declare `rcm-embedding` non-optional. One PR revert.

### Step 4 — Compile-time `Send + Sync` assertion

```rust
// crates/rcm-embedding/src/fastembed_embedder.rs
#[cfg(feature = "embeddings")]
const _ASSERT_SEND_SYNC: fn() = || {
    fn assert<T: Send + Sync>() {}
    assert::<fastembed::TextEmbedding>();
};
```

If a future fastembed/ort version drops `Send` or `Sync`, the build fails here, *before* we accidentally ship a service that races in `embed_batch`. The contingency (DECISIONS §`rcm-embedding`) is to switch the inner from `Arc<Mutex<TextEmbedding>>` to a single-thread worker actor and keep `Embed: Send + Sync` by virtue of holding only an `mpsc::Sender`.

**Files touched.** `crates/rcm-embedding/src/fastembed_embedder.rs`.

**Acceptance.** Build passes today. If the assertion ever fails, the build error names the line.

**Reversal.** Remove the `const _` block; not load-bearing for runtime.

### Step 5 — Test seam migration

**What to do.** Find every existing test that mocks embedding via ad-hoc means (e.g. dummy `Vec<f32>` fixtures, `LazyLock` overrides, or feature-flagged stubs) and migrate them to `DeterministicEmbedder`.

Example migration in `crates/rcm-search/tests/hybrid_search.rs`:

```rust
// Before:
// fn fake_embed(_t: &str) -> Vec<f32> { vec![0.0_f32; 384] }
// let store = build_store_with_vectors(/* by hand */);

// After:
use rcm_embedding::{DeterministicEmbedder, Embedder};

#[tokio::test(flavor = "multi_thread")]
async fn rrf_combines_bm25_and_vectors() {
    let embedder: Embedder = DeterministicEmbedder::new(384);
    let svc = SearchService::open_with(test_paths(), embedder.clone()).await.unwrap();
    svc.index_now(fixture_dir()).await.unwrap();
    let hits = svc.search("hello", 5).await.unwrap();
    assert!(!hits.is_empty());
}
```

`Cargo.toml` for `rcm-search`:

```toml
[dev-dependencies]
rcm-embedding = { path = "../rcm-embedding", features = ["test-fakes"] }
tokio = { workspace = true, features = ["macros", "rt-multi-thread"] }
```

**Files touched.** `crates/rcm-search/tests/*.rs`, `crates/rcm-graph/tests/*.rs`, the `[dev-dependencies]` of both crates.

**Acceptance.** `cargo test -p rcm-search` runs without loading any ONNX model (verify with `RUST_LOG=fastembed=trace cargo test -p rcm-search` — no model-init line).

**Reversal.** Tests can revert to ad-hoc fakes one at a time.

### Step 6 — Verify SDK feature work

**What to do.** Add three CI matrix cells (and document them as the local checks before merging Phase 5):

```sh
# 1. Graph alone, no semantic feature, no fastembed in tree.
cargo build -p rcm-graph --no-default-features
cargo tree -p rcm-graph | grep -E '(fastembed|ort|rcm-embedding)' && exit 1 || true

# 2. Graph with semantic-overlaps, fastembed pulled.
cargo build -p rcm-graph --features semantic-overlaps
cargo tree -p rcm-graph --features semantic-overlaps | grep rcm-embedding

# 3. Full binary, default features.
cargo build -p rcm-server
```

Run via the project devshell wrapper (`nix develop ../nix-devshells#code --command cargo ...`).

**Files touched.** `.github/workflows/ci.yml` (or equivalent), or a `Justfile` recipe.

**Acceptance.** All three commands exit 0; the negative grep in (1) finds nothing.

**Reversal.** Remove the matrix cells.

### Phase 5 acceptance (gate to Phase 6)

- Sealed `Embed` trait is the only way to embed; no `pub` constructor exposes `fastembed`/`ort` types.
- `cargo tree -p rcm-graph` (default features) shows zero `fastembed` / `ort` edges.
- `semantic_overlaps` and `similar_to_item` work in the full binary against fixtures.
- `DeterministicEmbedder` is the only embedder used in unit tests; no real ONNX load (verified by tracing).
- Smoke checklist passes including `semantic_overlaps`.

---

## Cross-phase checklist (run at end of Phase 5)

- `cargo build --workspace` green.
- `cargo build -p rcm-graph --no-default-features` green and embedding-free.
- `cargo test -p rcm-search` and `cargo test -p rcm-graph` green; no ONNX loads.
- `cargo test -p rcm-server -- reload` green (Step 6 of Phase 4).
- Manual SIGINT against `rcm-server` exits cleanly; no stale lockfiles in the XDG data dir.
- DECISIONS §"Smoke checklist" passes end-to-end including `clear_cache` + re-`search` and `semantic_overlaps`.

If any item fails, revert that phase's last step and re-run before proceeding to Phase 6 (parser scope reduction), which is the next high-risk phase.

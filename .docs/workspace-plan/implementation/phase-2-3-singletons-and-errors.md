# Phases 2 & 3 — Singleton Removal and Operation-Scoped Errors

This plan implements two low-risk, independent phases on top of the Phase 1 adapter crates. Either may land first; both must keep `cargo build --workspace` green and the smoke checklist passing (DECISIONS §"Smoke checklist"). The authoritative baseline is `.docs/workspace-plan/DECISIONS.md`.

---

## Phase 2 — Hidden Singleton Removal

**Goal:** delete every `LazyLock<Mutex<...>>` / `static .*Mutex` runtime singleton. Services are constructed exactly once in `rcm-server::main` and shared via `Arc`, matching DECISIONS §"Hidden singletons" and §"Service lifetime + invalidation".

### Step 1 — Audit current statics

**What to do.** Enumerate every static that holds runtime state. Run from the workspace root:

```bash
nix develop ../nix-devshells#code --command bash -c \
  "rg -n 'LazyLock|OnceLock|lazy_static|^static .*Mutex|^static .*RwLock' src/ crates/"
```

The known offender per DECISIONS is `static SEMANTIC: LazyLock<Mutex<SemanticService>>` in `src/legacy/semantic/mod.rs` (architecture doc `.docs/architecture/semantic.md` confirms it). Record each hit in a one-line table: file, symbol, what it caches, lock type. Anything that is purely compile-time (`static SCHEMA: LazyLock<Schema>` for a Tantivy schema with no runtime mutation) is allowed to remain — flag only those that gate runtime state behind a `Mutex`/`RwLock`.

**Files touched.** None (audit only); produce `.docs/workspace-plan/implementation/phase-2-statics-audit.md` if the list is long enough to warrant tracking.

**Acceptance criterion.** Every hit is classified as `runtime-state` or `compile-time-constant`. The plan below covers every `runtime-state` row.

**Reversal.** N/A (read-only).

### Step 2 — Replace `SEMANTIC` with an `IdeService` instance

**What to do.** Move `SemanticService`'s `HashMap<PathBuf, ProjectContext>` cache onto `rcm_ide::IdeService` as a private field, drop the global `Mutex` wrapper, and wire a single `Arc<IdeService>` through the rmcp router state.

The replacement type sketch (matching DECISIONS §"`rcm-ide`"):

```rust
// crates/rcm-ide/src/service.rs
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rcm_paths::ProjectPaths;
use rcm_ra_host::RaHost;

use crate::error::IdeError;

pub struct IdeService {
    paths: ProjectPaths,
    // Per-instance, NOT global. AnalysisHost is !Sync so Mutex (not RwLock).
    cache: Mutex<HashMap<PathBuf, Arc<RaHost>>>,
}

impl IdeService {
    #[must_use]
    pub fn open(paths: ProjectPaths) -> Result<Self, IdeError> {
        Ok(Self { paths, cache: Mutex::new(HashMap::new()) })
    }

    pub async fn find_definition(
        &self,
        req: DefinitionRequest,
    ) -> Result<NavigationResponse, IdeError> {
        let host = self.host_for(&req.project_path)?;
        // spawn_blocking around the sync ra_ap_ide call
        tokio::task::spawn_blocking(move || /* ... */)
            .await
            .map_err(IdeError::join)?
    }

    fn host_for(&self, project: &Path) -> Result<Arc<RaHost>, IdeError> {
        let key = project.canonicalize().map_err(IdeError::canonicalize)?;
        let mut cache = self.cache.lock().expect("ide cache poisoned");
        if let Some(existing) = cache.get(&key) {
            return Ok(Arc::clone(existing));
        }
        let host = Arc::new(RaHost::open_ide(&key)?);
        cache.insert(key.clone(), Arc::clone(&host));
        Ok(host)
    }
}
```

In `rcm-server::main`, construct once and stash on the router state:

```rust
// crates/rcm-server/src/main.rs
let ide = Arc::new(IdeService::open(paths.clone())?);
let search = Arc::new(SearchService::open(&paths, embedder.clone())?);
let graph = GraphService::builder(paths.clone(), ra_host.clone())
    .with_embedder(embedder.clone())
    .build()?;
let state = Arc::new(ServerState { ide, search, graph: Arc::new(graph), embedder });
serve_stdio(state).await?;
```

Tool handler diff (pseudocode, single tool — `find_definition`):

```rust
// BEFORE — src/legacy/handlers/find_definition.rs
let mut svc = legacy::semantic::SEMANTIC.lock().expect("semantic poisoned");
let ctx = svc.get_or_load(&params.project_path)?;
let locs = ctx.goto_definition(&params.file_path, params.line, params.column)?;
Ok(serde_json::to_value(locs)?)

// AFTER — crates/rcm-server/src/tools/find_definition.rs
let req = DefinitionRequest::new(params.project_path, params.file_path, params.line, params.column);
let resp = state.ide.find_definition(req).await
    .with_context(|| format!("find_definition {}:{}:{}", params.file_path.display(), params.line, params.column))?;
Ok(serde_json::to_value(resp)?)
```

Notes:
- The cache lock is held only while inserting; the actual `goto_definition` call runs without the cache lock (the `Arc<RaHost>` is cloned out first). This removes the coarse process-wide serialization that `static SEMANTIC` enforced.
- `RaHost::with_db` is the only inner sync path; it stays under `RaHost`'s internal `Mutex<AnalysisHost>` (per-instance, allowed by DECISIONS §"Hidden singletons").

**Files touched.** `crates/rcm-ide/src/service.rs`, `crates/rcm-ide/src/error.rs`, `crates/rcm-server/src/main.rs`, `crates/rcm-server/src/state.rs`, every `crates/rcm-server/src/tools/*.rs` that used `SEMANTIC` (find_definition, find_references, symbol_search, find_references_by_name). Delete `src/legacy/semantic/mod.rs::SEMANTIC` once the legacy shim no longer imports it.

**Acceptance criterion.** `rg 'SEMANTIC\.lock\(\)' src/ crates/` returns nothing. The four IDE tools route through `state.ide`. `cargo check --workspace` passes; `find_definition`/`find_references` pass the smoke checklist.

**Reversal.** Revert the `IdeService` field and tool diffs; the `static SEMANTIC` lives behind a feature flag during the transition (`#[cfg(feature = "legacy-semantic-static")]`) so the rollback is a one-commit revert.

### Step 3 — Replace scattered `EmbeddingGenerator::new()` calls with a shared `Embedder`

**What to do.** Today `EmbeddingGenerator::new()` is invoked from indexing, search, and graph paths independently — each one re-initializes the ONNX session. Centralize: `rcm-server::main` constructs one `Embedder = Arc<dyn Embed>` and threads it through service constructors. Capability crates accept `Embedder` (a type alias for `Arc<dyn Embed>` from `rcm-embedding`, see DECISIONS §"`rcm-embedding`") and clone the `Arc` cheaply when needed.

Construction site:

```rust
// crates/rcm-server/src/main.rs
use rcm_embedding::{Embedder, EmbedderConfig, FastEmbedEmbedder};

let embed_cfg = EmbedderConfig::default();              // model + cuda policy
let embedder: Embedder = Arc::new(FastEmbedEmbedder::new(embed_cfg)?);
```

Consumers take `Embedder` by value (it's already an `Arc`, so cloning is a refcount bump):

```rust
// crates/rcm-search/src/service.rs
impl SearchService {
    pub fn open(paths: &ProjectPaths, embedder: Embedder) -> Result<Self, SearchError> { /* ... */ }
}

// crates/rcm-graph/src/builder.rs
impl GraphServiceBuilder {
    pub fn with_embedder(mut self, e: Embedder) -> Self { self.embedder = Some(e); self }
}
```

The `embed_async` / `embed_batch_async` wrappers stay inside `rcm-embedding` (DECISIONS §"Async boundary": "the only place embedding hits tokio"). Capability code calls them through the trait object:

```rust
let vectors = self.embedder.embed_batch_async(texts).await?;
```

**Files touched.** `crates/rcm-embedding/src/lib.rs` (trait + alias), `crates/rcm-embedding/src/fastembed.rs`, every call site of `EmbeddingGenerator::new()` (indexing pipeline, search vector arm, graph `semantic_overlaps`), `crates/rcm-server/src/main.rs`.

**Acceptance criterion.** `rg 'EmbeddingGenerator::new\(' crates/ src/` returns one hit (inside `FastEmbedEmbedder`'s constructor) or zero (if the type is fully renamed). Cold-start initializes the ONNX session exactly once per process.

**Reversal.** Re-introduce `EmbeddingGenerator::new()` call sites as fallbacks behind `#[cfg(feature = "legacy-embedder-init")]`; revert is a single commit.

### Step 4 — Document the explicit `Arc<SyncManager>` sharing path

**What to do.** `SyncManager` is already constructed in `main`; the change here is to remove any incidental clones in tool handlers and document that the canonical sharing path is `Arc<SyncManager>` on the router state. The `CancellationToken` (DECISIONS §"Service lifetime + invalidation": "`CancellationToken`-driven shutdown") is owned by `main` and a clone is given to `SyncManager::run`. Tools never construct a `SyncManager`.

```rust
// crates/rcm-server/src/state.rs
pub struct ServerState {
    pub ide: Arc<IdeService>,
    pub search: Arc<SearchService>,
    pub graph: Arc<GraphService>,
    pub embedder: Embedder,                  // already Arc<dyn Embed>
    pub sync: Arc<SyncManager>,
    pub cancel: CancellationToken,           // for clean shutdown
}
```

**Files touched.** `crates/rcm-server/src/state.rs`, `crates/rcm-server/src/sync.rs`, `crates/rcm-server/src/main.rs`.

**Acceptance criterion.** Exactly one `SyncManager::new(...)` call in the workspace, in `main`. SIGINT triggers `state.cancel.cancel()` and the in-flight tools drain within the 30s budget specified in DECISIONS.

**Reversal.** Trivial — the change is structural, not behavioral.

### Step 5 — CI guard against regressions

**What to do.** Add an `xtask` check (or a plain shell step in CI) that fails the build if a runtime singleton reappears in `crates/`:

```bash
# crates/xtask/src/checks/no_singletons.rs (sketch)
let forbidden = ["LazyLock<Mutex", "LazyLock<RwLock", "lazy_static!", "OnceLock<Mutex"];
for pat in forbidden {
    let hits = ripgrep("crates/", pat)?;
    if !hits.is_empty() { bail!("singleton regression: {pat}\n{hits}"); }
}
```

Compile-time-constant `LazyLock<Schema>` or `LazyLock<Regex>` (no `Mutex`/`RwLock` inside) is allowed; the patterns above only match the runtime-state shape.

**Files touched.** `crates/xtask/src/checks/no_singletons.rs`, the CI workflow YAML.

**Acceptance criterion.** The CI job fails if anyone re-introduces `static FOO: LazyLock<Mutex<_>>` in `crates/`. `xtask` is excluded from the check on itself.

**Reversal.** Delete the check and the CI step.

### Phase 2 acceptance (gate)

- `rg 'LazyLock<Mutex|LazyLock<RwLock|lazy_static!|OnceLock<Mutex' crates/` returns nothing.
- All capability services are constructed exactly once in `rcm-server::main` and shared via `Arc`; their lifetimes match the MCP session (DECISIONS §"Service lifetime + invalidation").
- The smoke checklist (DECISIONS §"Smoke checklist") passes end-to-end against the fixture workspace.

---

## Phase 3 — Operation-Scoped Error Split

**Goal:** replace crate-level "god enums" with operation-scoped `thiserror` enums per DECISIONS §"Errors". Source error chains are preserved with `#[from]`, but adapter error types are kept out of the public API by hiding them behind a `pub(crate)` internal enum. `anyhow` is confined to `rcm-server` and tests, per `rust-guidelines-final.md` §9.

### Step 1 — Inventory current error enums

**What to do.** Walk the legacy crate (`src/`) and list each error enum with its variants:

| Module | Enum | Variants | Notes |
|---|---|---|---|
| `src/legacy/indexing/error.rs` | `IndexingError` | `Io`, `Embedding`, `VectorStore`, `Parser`, `Cache` | god-enum, mixes write + read concerns |
| `src/legacy/search/error.rs` | `SearchError` | `Embedding`, `VectorStore`, `Bm25`, `NoResults` | mixes vector + bm25 + result-shape errors |
| `src/legacy/embeddings/error.rs` | `EmbeddingError` | `ModelInit`, `EmbedFailed`, `NoEmbeddingGenerated`, `TaskJoin` | already roughly operation-scoped; rename + keep |
| `src/legacy/vector_store/error.rs` | `VectorStoreError` | varies | becomes internal-only, surfaces to `SearchError`/`IndexError` via `#[from]` |
| `src/legacy/semantic/...` | (returns `anyhow`) | — | promote to `IdeError` |

`EmbeddingError` is fine as an operation-scoped enum — it just moves to `rcm-embedding::EmbedError` with the variant set DECISIONS specifies (`ModelInit | EmbedFailed | Empty | Join | Unsupported`). The two genuine god-enums to split are `IndexingError` and `SearchError`.

**Files touched.** None (audit only).

**Acceptance criterion.** Every public error type in `src/legacy/**/*error*.rs` is mapped to a target enum in §Step 2.

**Reversal.** N/A.

### Step 2 — Define the new error taxonomy

**What to do.** Per DECISIONS, each crate gets one or more operation-scoped enums. Below are full worked `thiserror` definitions; adjust variant payloads as concrete `?`-call sites are reached.

```rust
// crates/rcm-paths/src/error.rs
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PathError {
    #[error("workspace path is not absolute: {path}")]
    NotAbsolute { path: std::path::PathBuf },
    #[error("failed to canonicalize workspace path {path}: {message}")]
    Canonicalize { path: std::path::PathBuf, message: String },
    #[error("storage root unavailable: {message}")]
    StorageRoot { message: String },
}
```

```rust
// crates/rcm-ra-host/src/error.rs
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RaError {
    #[error("workspace not found at {path}")]
    NotFound { path: std::path::PathBuf },
    #[error("cargo workspace load failed: {message}")]
    LoadFailed { message: String },
    #[error("vfs lookup failed for {path}")]
    VfsMiss { path: std::path::PathBuf },
    #[error("ra-host operation cancelled")]
    Cancelled,
}
```

```rust
// crates/rcm-embedding/src/error.rs
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EmbedError {
    #[error("model init failed: {message}")]
    ModelInit { message: String },
    #[error("embed failed: {message}")]
    EmbedFailed { message: String },
    #[error("empty input")]
    Empty,
    #[error("blocking task join failed: {message}")]
    Join { message: String },
    #[error("unsupported feature: {message}")]
    Unsupported { message: String },
}
```

```rust
// crates/rcm-search/src/error.rs
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndexError {
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[error("index unavailable: {message}")]
    IndexUnavailable { message: String },
    #[error("index busy (writer in use)")]
    IndexBusy,
    #[error("index corrupt: {message}")]
    IndexCorrupt { message: String },
    #[error("embed failure during index: {0}")]
    Embed(#[from] rcm_embedding::EmbedError),
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalIndexError),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SearchError {
    #[error("invalid query: {message}")]
    InvalidQuery { message: String },
    #[error("index unavailable: {message}")]
    IndexUnavailable { message: String },
    #[error("no results")]
    NoResults,
    #[error("embed failure during search: {0}")]
    Embed(#[from] rcm_embedding::EmbedError),
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalSearchError),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CorpusError {
    #[error("walk failed at {path}: {message}")]
    Walk { path: std::path::PathBuf, message: String },
    #[error("metadata cache failure: {message}")]
    Metadata { message: String },
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalCorpusError),
}
```

```rust
// crates/rcm-graph/src/error.rs
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BuildError {
    #[error("ra-host failed: {0}")]
    RaHost(#[from] rcm_ra_host::RaError),
    #[error("snapshot write failed: {message}")]
    Snapshot { message: String },
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalGraphError),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum QueryError {
    #[error("snapshot unavailable: {message}")]
    SnapshotUnavailable { message: String },
    #[error("workspace fingerprint mismatch")]
    FingerprintMismatch,
    #[error("embedder unavailable")]
    EmbedderUnavailable,
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalGraphError),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuditError {
    #[error("audit input invalid: {message}")]
    InvalidInput { message: String },
    #[error("snapshot unavailable: {message}")]
    SnapshotUnavailable { message: String },
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalGraphError),
}
```

```rust
// crates/rcm-ide/src/error.rs
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IdeError {
    #[error("invalid input: {message}")]
    InvalidInput { message: String },
    #[error("workspace load failed: {0}")]
    Load(#[from] rcm_ra_host::RaError),
    #[error("blocking task join failed: {message}")]
    Join { message: String },
    #[error("symbol not found")]
    NotFound,
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalIdeError),
}
```

**Files touched.** New `error.rs` per crate as listed.

**Acceptance criterion.** Each crate compiles standalone with its own error module; no enum has more than one operation surface (e.g., `SearchError` is read-side only; corpus building is `CorpusError`; index writing is `IndexError`).

**Reversal.** Revert the new files; the legacy enums remain alongside during the transition (Phase 1 adapter shims keep both surfaces compiling).

### Step 3 — Public vs internal variant pattern

**What to do.** Adapter error types (`tantivy::TantivyError`, `lancedb::Error`, `heed::Error`, `sled::Error`, `tokio::task::JoinError`) must NOT appear in the public signature of capability crates (DECISIONS §"Two-tier API leak rule"). The pattern is:

```rust
// crates/rcm-search/src/error.rs (continued)

// PUBLIC: stable, string-shaped, suitable for `cargo public-api`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SearchError {
    #[error("invalid query: {message}")]
    InvalidQuery { message: String },
    #[error("index unavailable: {message}")]
    IndexUnavailable { message: String },
    #[error("no results")]
    NoResults,
    #[error("embed failure during search: {0}")]
    Embed(#[from] rcm_embedding::EmbedError),
    // INTERNAL bridge — `#[doc(hidden)]` keeps it out of rendered docs.
    // The variant is `pub` (Rust has no friend visibility for enum variants),
    // but the `From` impls flow through `pub(crate) InternalSearchError`,
    // which the public-api check whitelists (variant is opaque/transparent).
    #[doc(hidden)]
    #[error(transparent)]
    Internal(#[from] InternalSearchError),
}

// INTERNAL: holds adapter types, never named in a public signature.
#[derive(Debug, thiserror::Error)]
pub(crate) enum InternalSearchError {
    #[error(transparent)]
    Tantivy(#[from] tantivy::TantivyError),
    #[error(transparent)]
    LanceDb(#[from] lancedb::Error),
    #[error(transparent)]
    Join(#[from] tokio::task::JoinError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
```

Rationale:

1. **Source chains preserved.** `?` over a Tantivy call inside `rcm-search` produces an `InternalSearchError::Tantivy`, which lifts to `SearchError::Internal(InternalSearchError::Tantivy(...))`. `std::error::Error::source()` walks the full chain (`SearchError → InternalSearchError → tantivy::TantivyError`). Callers that want the underlying adapter can downcast inside the crate; outside callers see only the public string variants and the chain via `Display` / `source()`.
2. **Public API is stable.** `cargo public-api -p rcm-search` reports `SearchError::Internal(_)` as a transparent variant; no `tantivy::` symbol is in the public surface. Bumping `tantivy` does not break semver.
3. **Mapping at the boundary.** When something genuinely is a public concern (e.g., the index file is corrupt), explicit code maps it to a public variant rather than letting `#[from]` silently embed it:

```rust
fn open_reader(path: &Path) -> Result<IndexReader, SearchError> {
    let idx = match tantivy::Index::open_in_dir(path) {
        Ok(idx) => idx,
        Err(tantivy::TantivyError::CorruptedFile(p)) => {
            return Err(SearchError::IndexUnavailable {
                message: format!("corrupt segment at {}", p.display()),
            });
        }
        Err(other) => return Err(InternalSearchError::from(other).into()),
    };
    Ok(idx.reader()?)
}
```

The same pattern applies to `IndexError`, `CorpusError`, `BuildError`, `QueryError`, `AuditError`, `IdeError`.

**Files touched.** Each crate's `error.rs` plus internal counterparts.

**Acceptance criterion.** `cargo public-api -p rcm-search` (and per-crate equivalents) shows only the public variants. Grep for `pub.*tantivy::|pub.*lancedb::|pub.*heed::|pub.*ra_ap_` in capability-crate `error.rs` returns nothing.

**Reversal.** Inline the internal enum back into the public one; trivially reversible.

### Step 4 — Confine `anyhow` to `rcm-server`

**What to do.** The `anyhow` crate is allowed in `rcm-server` (and `xtask`, and dev-deps for tests/doctests). It is FORBIDDEN in `rcm-paths`, `rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding`, `rcm-search`, `rcm-graph`, `rcm-ide` per DECISIONS §"Errors".

Capability code propagates typed errors with `?`. `rcm-server` lifts them into `anyhow::Result` at the rmcp tool boundary using `.with_context()`:

```rust
// crates/rcm-server/src/tools/find_definition.rs
use anyhow::Context as _;

pub async fn find_definition(state: Arc<ServerState>, params: FindDefinitionParams) -> anyhow::Result<Value> {
    let req = DefinitionRequest::from(params.clone());
    let resp = state.ide.find_definition(req).await
        .with_context(|| format!("find_definition {}", params.summary()))?;
    Ok(serde_json::to_value(resp)?)
}
```

CI guard (per `rust-guidelines-final.md` §9 + DECISIONS): a workspace-level grep test rejects `anyhow` appearing in any capability `Cargo.toml`:

```bash
# crates/xtask/src/checks/no_anyhow_in_capabilities.rs (sketch)
let allowed = ["rcm-server", "xtask"];
for member in workspace.members() {
    if allowed.contains(&member.name.as_str()) { continue; }
    let manifest = std::fs::read_to_string(member.manifest_path)?;
    if manifest.contains("anyhow") {
        bail!("anyhow forbidden in capability crate: {}", member.name);
    }
}
```

**Files touched.** `crates/xtask/src/checks/no_anyhow_in_capabilities.rs`, capability `Cargo.toml`s (audit pass to remove any incidental `anyhow` deps), `crates/rcm-server/src/tools/*.rs` (insert `.with_context(...)` at boundaries).

**Acceptance criterion.** `grep -l '^anyhow' crates/*/Cargo.toml` yields only `crates/rcm-server/Cargo.toml` and `crates/xtask/Cargo.toml`. The CI check fails on any new violation.

**Reversal.** Drop the CI check; revert any `.with_context()` insertions.

### Step 5 — `Drop` never panics

**What to do.** Per DECISIONS / `rust-guidelines-final.md` §9, no panics in `Drop::drop`. The audit target is `legacy::indexing::TantivyAdapter::drop`, which currently rolls back the writer to release the index lock (per `.docs/architecture/indexing.md`). On rollback failure it must log-then-swallow:

```rust
impl Drop for TantivyAdapter {
    fn drop(&mut self) {
        // Acquire the writer lock without panicking on poison.
        let mut guard = match self.writer.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("tantivy writer mutex poisoned during drop");
                poisoned.into_inner()
            }
        };
        if let Err(err) = guard.rollback() {
            // log the chain; never propagate
            tracing::error!(error = ?err, "tantivy rollback failed during drop; lockfile may persist");
        }
    }
}
```

Same audit pattern for `VectorStore::drop`, `MetadataCache::drop`, `RaHost::drop` if they exist. Any `unwrap()` / `expect()` inside a `Drop` body is a defect.

**Files touched.** Each `impl Drop` site identified by `rg 'impl Drop for' crates/ src/`.

**Acceptance criterion.** `rg -A 20 'impl Drop for' crates/ | rg 'unwrap\(\)|expect\(' | wc -l` returns 0 (or a documented allow-list of compile-time-safe `expect("static; cannot fail")` cases).

**Reversal.** Restore the panicking `Drop` impls — though there is no good reason to.

### Phase 3 acceptance (gate)

- Each capability crate exposes operation-scoped errors only; no `RcmError` god-enum exists, no enum mixes index-write + index-read concerns.
- `cargo public-api -p rcm-search` (and per capability crate) shows only public variants; internal adapter variants are `#[doc(hidden)]` and transparent.
- Adapter source chains preserved via `#[from]` on the `pub(crate)` internal enum; `std::error::Error::source()` walks the full chain at runtime.
- `anyhow` appears only in `crates/rcm-server/Cargo.toml`, `crates/xtask/Cargo.toml`, and `[dev-dependencies]`.
- No `Drop::drop` panics; rollback failures log-and-swallow.
- Smoke checklist (DECISIONS §"Smoke checklist") passes.

---

## Independence and ordering

Phases 2 and 3 touch disjoint files (Phase 2 touches service construction and tool plumbing; Phase 3 touches `error.rs` modules and `Cargo.toml` deps), so either may land first. Recommended order: Phase 3 first because typed-error propagation makes Phase 2's `IdeService` boundary easier to write without `anyhow` leaking into capability crates. But landing Phase 2 first is acceptable — capability crates can briefly depend on `anyhow` during the transition and have it removed when Phase 3 lands.

After both phases land:
- `rg 'LazyLock<Mutex|lazy_static!|OnceLock<Mutex' crates/` → empty.
- `grep -l '^anyhow' crates/*/Cargo.toml` → only `rcm-server`, `xtask`.
- `cargo public-api` per capability crate → no adapter types in public surface.
- Smoke checklist green.

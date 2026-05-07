# Phase 1 — Facade APIs over Legacy

**Goal.** By the end of this phase, the eight target crates (`rcm-paths`, `rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding`, `rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-server`) exist with their full public APIs as frozen in `DECISIONS.md`. **No behavior changes.** Internally every capability and infra crate delegates to the unchanged legacy crate (renamed `file-search-mcp-legacy`). The binary `rcm-server` depends only on the new crates. Every MCP tool in the smoke checklist passes because the new public surfaces internally route through legacy.

This is the largest refactor in the plan in terms of surface area, but the smallest in terms of risk: no algorithm moves, no storage layout changes, no feature flag flips.

## Strategy overview

- **Legacy as private path dep.** Each capability crate adds `file-search-mcp-legacy = { path = "../../legacy", package = "file-search-mcp" }` to `[dependencies]`. The dep is private — capability crates do **not** re-export legacy types.
- **Public API == DECISIONS.** Each capability crate's `lib.rs` exposes exactly the items frozen in `DECISIONS.md` §"Crates — frozen contracts". Nothing more.
- **Adapter modules.** Conversions `legacy::Foo ↔ rcm_x::Foo` live in a `pub(crate) mod legacy_adapter` per crate. Always `From`/`TryFrom`, never bare `fn convert`.
- **Server-only dependency edges.** `rcm-server` depends on `rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-paths`, `rcm-embedding` — and **not** `file-search-mcp-legacy`. The `legacy` crate becomes lib-only; its `bin/server.rs` is removed.
- **Forbidden-dep check tightens.** `forbidden_dependency_check` (the MCP tool we expose to users) now reads the workspace-policy file: capability-to-capability edges (`rcm-search → rcm-graph` etc.) are forbidden; everyone may transitively pull `legacy`; `xtask` is excluded.
- **One temporary back-edge.** `legacy` is allowed to depend on `rcm-paths` during Phase 1 (only). This lets the legacy code stop hashing workspace paths in two places. Documented in `legacy/Cargo.toml` with a `# PHASE-1-ONLY` comment; removed in Phase 2 along with the `LazyLock`.

## Per-crate facade implementation order

The order below is bottom-up over the DAG: leaves first, then capability crates, then the binary. Each crate compiles green against legacy before the next one starts.

### 1. `rcm-paths`

**What to do.** Lift `ProjectPaths` and friends out of legacy and into the new leaf crate. This is the only crate where legacy code is allowed to depend back onto a new crate during Phase 1, because path resolution must be the single function that hashes a workspace path. Move (not copy) the recipe so there is exactly one implementation.

**Files touched.**
- `crates/rcm-paths/Cargo.toml` (new)
- `crates/rcm-paths/src/lib.rs` (new)
- `crates/rcm-paths/src/legacy_adapter.rs` (new, empty initially — kept for symmetry)
- `legacy/src/<original-paths-module>.rs` → re-export shim only

**`Cargo.toml`.**

```toml
[package]
name = "rcm-paths"
edition.workspace = true
license.workspace = true

[dependencies]
directories = { workspace = true }
sha2 = { workspace = true }
serde = { workspace = true, features = ["derive"] }
thiserror = { workspace = true }
```

**`lib.rs` (full).**

```rust
#![warn(missing_docs, unreachable_pub)]
//! Storage path resolution for rcm workspaces.
//!
//! All workspace-path hashing in the workspace flows through
//! [`ProjectPaths::resolve`]. Other crates must not reimplement the recipe.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// Errors produced by path resolution.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PathError {
    /// `canonicalize` failed for the workspace path.
    #[error("canonicalize {path}: {source}")]
    Canonicalize {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// XDG directories could not be resolved.
    #[error("xdg base directory unavailable")]
    Xdg,
    /// Storage root creation failed.
    #[error("create dir {path}: {source}")]
    Create {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Storage root strategy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub enum StorageRoot {
    /// Use `directories::ProjectDirs` (default).
    Xdg,
    /// Explicit path (tests, `RCM_STORAGE_ROOT`).
    Explicit(PathBuf),
}

/// Resolved per-workspace paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ProjectPaths {
    workspace: PathBuf,
    storage: PathBuf,
    fingerprint: String,
}

impl ProjectPaths {
    /// Resolve paths for a workspace.
    ///
    /// Recipe (frozen): `sha256(canonicalize(workspace).as_encoded_bytes())`,
    /// lower-hex.
    pub fn resolve(workspace: &Path, root: &StorageRoot) -> Result<Self, PathError> {
        let canonical = workspace
            .canonicalize()
            .map_err(|source| PathError::Canonicalize {
                path: workspace.to_path_buf(),
                source,
            })?;
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_os_str().as_encoded_bytes());
        let fingerprint = hex_lower(&hasher.finalize());
        let storage = match root {
            StorageRoot::Xdg => xdg_storage(&fingerprint)?,
            StorageRoot::Explicit(p) => p.join(&fingerprint),
        };
        std::fs::create_dir_all(&storage).map_err(|source| PathError::Create {
            path: storage.clone(),
            source,
        })?;
        Ok(Self { workspace: canonical, storage, fingerprint })
    }

    /// Workspace root (canonicalized).
    pub fn workspace(&self) -> &Path { &self.workspace }
    /// Per-workspace storage directory.
    pub fn storage(&self) -> &Path { &self.storage }
    /// Lower-hex sha256 fingerprint.
    pub fn fingerprint(&self) -> &str { &self.fingerprint }
    /// Tantivy index dir (`<storage>/index`).
    pub fn tantivy_dir(&self) -> PathBuf { self.storage.join("index") }
    /// LanceDB dir (`<storage>/vectors`).
    pub fn lance_dir(&self) -> PathBuf { self.storage.join("vectors") }
    /// Sled metadata dir.
    pub fn sled_dir(&self) -> PathBuf { self.storage.join("metadata") }
    /// Heed graph snapshot dir.
    pub fn graph_dir(&self) -> PathBuf { self.storage.join("graph") }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { use std::fmt::Write; let _ = write!(s, "{b:02x}"); }
    s
}

fn xdg_storage(fingerprint: &str) -> Result<PathBuf, PathError> {
    let dirs = directories::ProjectDirs::from("dev", "rcm", "rust-code-mcp")
        .ok_or(PathError::Xdg)?;
    Ok(dirs.data_dir().join(fingerprint))
}
```

**Acceptance.** `cargo check -p rcm-paths`; legacy's old path module is a 3-line `pub use rcm_paths::*;` shim; legacy's tests still pass.

**Reversal.** Drop the new crate; restore legacy module body from git.

### 2. `rcm-ra-syntax`

**What to do.** Create the version-pin / scope-narrowing leaf. Trivial — no logic.

**Files.** `crates/rcm-ra-syntax/Cargo.toml`, `crates/rcm-ra-syntax/src/lib.rs`.

**`Cargo.toml`.**

```toml
[package]
name = "rcm-ra-syntax"
edition.workspace = true

[dependencies]
ra_ap_syntax = { workspace = true }
```

**`lib.rs` (full).**

```rust
#![allow(unreachable_pub)]
//! Narrow re-exports of `ra_ap_syntax`.
//!
//! # Dual purpose
//! 1. Pin `ra_ap_syntax`'s version in one place for the workspace.
//! 2. Keep `ra_ap_ide` and `ra_ap_hir` out of `rcm-search`'s compile graph.
//!
//! # API leak exemption
//! Re-exporting `ra_ap_syntax` items is documented and intentional; this leaf
//! is exempt from the workspace's API leak rule. Adding items to the
//! whitelist below requires code review.

pub use ra_ap_syntax::{
    AstNode, AstToken, Edition, Parse, SourceFile, SyntaxKind, SyntaxNode, SyntaxToken,
    TextRange, TextSize,
};

pub use ra_ap_syntax::ast::{
    Enum, Fn, Impl, Module, NameRef, Path, Struct, Trait, Use, UseTree,
};
```

**Acceptance.** `cargo check -p rcm-ra-syntax`; downstream crates can `use rcm_ra_syntax::SourceFile`. Confirm no other `ra_ap_*` symbol leaks.

**Reversal.** Drop the crate; downstream uses revert to direct `ra_ap_syntax`.

### 3. `rcm-ra-host`

**What to do.** Wrap `legacy::graph::loader::load` (HIR-mode) and `legacy::semantic::loader::load_project` (IDE-mode) behind the closure-based `RaHost`. The two presets in `DECISIONS.md` correspond exactly to the two existing legacy loaders. The `RootDatabase` and `Vfs` are constructed by legacy and stored opaque in `RaHost`.

**Files.** `crates/rcm-ra-host/Cargo.toml`, `crates/rcm-ra-host/src/lib.rs`, `crates/rcm-ra-host/src/legacy_adapter.rs`.

**`Cargo.toml` fragment.**

```toml
[dependencies]
file-search-mcp-legacy = { path = "../../legacy" }
rcm-ra-syntax = { path = "../rcm-ra-syntax" }
ra_ap_ide = { workspace = true }
ra_ap_hir = { workspace = true }
ra_ap_vfs = { workspace = true }
thiserror = { workspace = true }
```

**Partial `lib.rs`.**

```rust
#![allow(unreachable_pub)]
//! `RootDatabase` + `Vfs` lifecycle wrapper.
//!
//! Boundary discipline: `with_db` / `with_semantics` are technically `pub`,
//! but a workspace `clippy::disallowed_methods` rule allow-lists only
//! `rcm-graph` and `rcm-ide` as callers. `rcm-search` must not call them.

use std::path::Path;
use std::sync::Arc;

use ra_ap_hir::Semantics;
use ra_ap_ide::RootDatabase;
use thiserror::Error;

mod legacy_adapter;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum RaError {
    #[error("load workspace {path}: {message}")]
    Load { path: std::path::PathBuf, message: String },
}

/// Opaque host wrapping a `RootDatabase` + `Vfs`.
pub struct RaHost {
    inner: Arc<legacy_adapter::HostHandle>,
}

impl RaHost {
    /// Open in IDE preset (no_deps=true, no sysroot, prefill_caches=true).
    pub fn open_ide(path: &Path) -> Result<Self, RaError> {
        let inner = legacy_adapter::open_ide(path)?;
        Ok(Self { inner: Arc::new(inner) })
    }

    /// Open in HIR preset (no_deps=false, sysroot=Discover, all features, set_test=true).
    pub fn open_hir(path: &Path) -> Result<Self, RaError> {
        let inner = legacy_adapter::open_hir(path)?;
        Ok(Self { inner: Arc::new(inner) })
    }

    pub fn with_db<R>(&self, f: impl FnOnce(&RootDatabase) -> R) -> R {
        self.inner.with_db(f)
    }

    pub fn with_semantics<R>(&self, f: impl FnOnce(&Semantics<'_, RootDatabase>) -> R) -> R {
        self.inner.with_semantics(f)
    }

    pub fn workspace_fingerprint(&self) -> Fingerprint { self.inner.fingerprint() }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Fingerprint(pub(crate) String);
```

`legacy_adapter::HostHandle` owns a `legacy::semantic::loader::LoadedProject` (IDE preset) or `legacy::graph::loader::LoadedWorkspace` (HIR preset) inside an enum. `with_db` / `with_semantics` borrow into whichever variant is present.

**Boundary discipline.** Document in the crate-root rustdoc that callers must not move data extracted by the closure across `.await` if it borrows from `RootDatabase`. The closure return value is what crosses the boundary.

**Acceptance.** `cargo check -p rcm-ra-host`. Smoke: a downstream test calls `RaHost::open_ide(&fixture).with_db(|_| 1)` and gets `1`.

**Reversal.** Drop the crate; restore graph and ide direct uses of legacy loaders.

### 4. `rcm-embedding`

**What to do.** Define the sealed `Embed` trait. `FastEmbedEmbedder` wraps `legacy::embeddings::EmbeddingGenerator` (which already owns the fastembed handle, batching, dim accessor). `DeterministicEmbedder` is a fresh hash-based test fake. The `embed_batch_async` wrapper here is the only `tokio::task::spawn_blocking` call site for embeddings.

**Files.** `crates/rcm-embedding/Cargo.toml`, `crates/rcm-embedding/src/lib.rs`, `crates/rcm-embedding/src/fastembed_impl.rs`, `crates/rcm-embedding/src/test_fakes.rs`, `crates/rcm-embedding/src/legacy_adapter.rs`.

**`Cargo.toml`.**

```toml
[package]
name = "rcm-embedding"
edition.workspace = true

[features]
default = ["embeddings"]
embeddings = ["dep:file-search-mcp-legacy"]
test-fakes = []

[dependencies]
file-search-mcp-legacy = { path = "../../legacy", optional = true }
thiserror = { workspace = true }
tokio = { workspace = true, features = ["rt"] }
```

**Partial `lib.rs`.**

```rust
#![allow(unreachable_pub)]
//! Sealed `Embed` trait + production / test embedders.
//!
//! Documented exemption from the API leak rule for `Embed` and `Embedder`
//! (the trait itself is the boundary).

use std::sync::Arc;
use thiserror::Error;

mod sealed { pub trait Sealed {} }
mod legacy_adapter;
#[cfg(feature = "embeddings")] mod fastembed_impl;
#[cfg(feature = "test-fakes")] mod test_fakes;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum EmbedError {
    #[error("model init: {0}")]   ModelInit(String),
    #[error("embed failed: {0}")] EmbedFailed(String),
    #[error("empty input")]       Empty,
    #[error("join: {0}")]         Join(String),
    #[error("unsupported")]       Unsupported,
}

pub trait Embed: sealed::Sealed + Send + Sync {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
    fn dimensions(&self) -> usize;
}

pub type Embedder = Arc<dyn Embed>;

pub async fn embed_batch_async(
    e: Embedder,
    texts: Vec<String>,
) -> Result<Vec<Vec<f32>>, EmbedError> {
    tokio::task::spawn_blocking(move || {
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        e.embed_batch(&refs)
    })
    .await
    .map_err(|j| EmbedError::Join(j.to_string()))?
}

#[cfg(feature = "embeddings")]
pub use fastembed_impl::{FastEmbedEmbedder, EmbedderConfig, ModelKind, CudaPolicy};

#[cfg(feature = "test-fakes")]
pub use test_fakes::DeterministicEmbedder;
```

`fastembed_impl::FastEmbedEmbedder` holds a `legacy::embeddings::EmbeddingGenerator` and forwards `embed_batch` / `dimensions`. `EmbedError` derives `From<legacy::embeddings::EmbeddingError>` in `legacy_adapter.rs`.

**Acceptance.** `cargo check -p rcm-embedding --no-default-features --features test-fakes`; with default features against legacy. `static_assertions::assert_impl_all!(legacy::embeddings::TextEmbedding: Send, Sync);` confirms the contingency.

**Reversal.** Drop the crate; restore legacy direct `EmbeddingGenerator` calls.

### 5. `rcm-search`

**What to do.** Largest facade. `SearchService` wraps `legacy::search::HybridSearch` + `legacy::search::ResilientHybridSearch`. `CorpusWriter` wraps `legacy::indexing::UnifiedIndexer` (write side). The `chunker` and `rrf` sans-I/O cores are re-exported from legacy modules of the same names. Operation-scoped errors are introduced now: `IndexError`, `SearchError`, `CorpusError`. They have `#[from] legacy::IndexingError` etc., so conversion is `?`.

**Files.** `crates/rcm-search/Cargo.toml`, `crates/rcm-search/src/lib.rs`, `crates/rcm-search/src/service.rs`, `crates/rcm-search/src/writer.rs`, `crates/rcm-search/src/dto.rs`, `crates/rcm-search/src/error.rs`, `crates/rcm-search/src/legacy_adapter.rs`.

**`Cargo.toml` fragment.**

```toml
[dependencies]
file-search-mcp-legacy = { path = "../../legacy" }
rcm-ra-syntax            = { path = "../rcm-ra-syntax" }
rcm-paths                = { path = "../rcm-paths" }
rcm-embedding            = { path = "../rcm-embedding", optional = true }
thiserror                = { workspace = true }
tokio                    = { workspace = true, features = ["rt", "macros"] }

[features]
default     = ["embeddings"]
embeddings  = ["dep:rcm-embedding"]
```

**Partial service.**

```rust
use std::path::Path;
use rcm_paths::ProjectPaths;
use rcm_embedding::Embedder;
use file_search_mcp_legacy as legacy;
use crate::dto::{SearchRequest, SearchHit, SimilarRequest, SimilarHit};
use crate::error::SearchError;

pub struct SearchService {
    inner: legacy::search::ResilientHybridSearch,
    paths: ProjectPaths,
}

impl SearchService {
    pub async fn open(paths: &ProjectPaths, embedder: Embedder) -> Result<Self, SearchError> {
        let inner = crate::legacy_adapter::open_hybrid(paths, embedder).await?;
        Ok(Self { inner, paths: paths.clone() })
    }

    pub async fn search(&self, req: SearchRequest) -> Result<Vec<SearchHit>, SearchError> {
        let legacy_results = self.inner.search(&req.query(), req.limit()).await?;
        Ok(legacy_results.into_iter().map(SearchHit::from).collect())
    }

    pub async fn get_similar_code(&self, req: SimilarRequest) -> Result<Vec<SimilarHit>, SearchError> {
        let legacy_results = self.inner.similar(&req.query(), req.limit()).await?;
        Ok(legacy_results.into_iter().map(SimilarHit::from).collect())
    }

    pub async fn reload(&self, paths: &ProjectPaths) -> Result<(), SearchError> { /* ArcSwap, deferred to Phase 4 */ Ok(()) }
}
```

**Adapter conversion sketch (`legacy_adapter.rs`).**

```rust
use file_search_mcp_legacy as legacy;
use crate::dto::SearchHit;

impl From<legacy::search::SearchResult> for SearchHit {
    fn from(r: legacy::search::SearchResult) -> Self {
        SearchHit::new(
            ChunkId::from(r.chunk_id),  // see Phase 1 risk #2 — DISTINCT type
            r.file_path,
            r.score,
            r.snippet,
            r.line_start,
            r.line_end,
        )
    }
}
```

**Errors.**

```rust
#[derive(Debug, thiserror::Error)] #[non_exhaustive]
pub enum SearchError {
    #[error(transparent)] Backend(#[from] file_search_mcp_legacy::search::SearchError),
    #[error(transparent)] Index(#[from] file_search_mcp_legacy::indexing::IndexingError),
    #[error("workspace not indexed")] NotIndexed,
}
```

`IndexError` and `CorpusError` follow the same pattern but each scopes only the legacy errors that can flow out of their service surface.

**Acceptance.** `cargo public-api --simplified -p rcm-search` shows only items in DECISIONS. No `tantivy::`, `lancedb::`, `arrow::` strings. Smoke checklist `search`, `get_similar_code` pass.

**Reversal.** Drop the crate; binary points back at legacy.

### 6. `rcm-graph`

**What to do.** `GraphService` builder pattern. The builder holds an `Arc<RaHost>` and an optional `Embedder`. `build()` opens an `OpenedSnapshot` via `legacy::graph::loader::load` + `legacy::graph::storage::open_or_build`. Audits route to `legacy::graph::*_audit` modules. The `with_embedder` setter takes `rcm_embedding::Embedder`. All audit / query DTOs are new structs in `crates/rcm-graph/src/dto.rs`, converted via `legacy_adapter::From` impls.

**Files.** `crates/rcm-graph/{Cargo.toml, src/lib.rs, src/service.rs, src/builder.rs, src/dto.rs, src/error.rs, src/legacy_adapter.rs}`.

**Cargo.**

```toml
[features]
default            = []
semantic-overlaps  = ["dep:rcm-embedding"]

[dependencies]
file-search-mcp-legacy = { path = "../../legacy" }
rcm-ra-host             = { path = "../rcm-ra-host" }
rcm-paths               = { path = "../rcm-paths" }
rcm-embedding           = { path = "../rcm-embedding", optional = true }
thiserror               = { workspace = true }
tokio                   = { workspace = true, features = ["rt"] }
```

**Builder.**

```rust
use std::sync::Arc;
use rcm_ra_host::RaHost;
use rcm_paths::ProjectPaths;
#[cfg(feature = "semantic-overlaps")] use rcm_embedding::Embedder;

pub struct GraphService { /* private */ }

pub struct GraphServiceBuilder {
    paths: ProjectPaths,
    ra_host: Arc<RaHost>,
    #[cfg(feature = "semantic-overlaps")] embedder: Option<Embedder>,
}

impl GraphService {
    pub fn builder(paths: ProjectPaths, ra_host: Arc<RaHost>) -> GraphServiceBuilder {
        GraphServiceBuilder {
            paths, ra_host,
            #[cfg(feature = "semantic-overlaps")] embedder: None,
        }
    }
}

impl GraphServiceBuilder {
    #[cfg(feature = "semantic-overlaps")]
    pub fn with_embedder(mut self, e: Embedder) -> Self { self.embedder = Some(e); self }
    pub fn build(self) -> Result<GraphService, BuildError> {
        let snapshot = file_search_mcp_legacy::graph::storage::open_or_build(
            self.paths.graph_dir(),
            &self.ra_host,
        )?;
        Ok(GraphService::from_parts(snapshot, /* embedder = */ /* … */))
    }
}
```

The `semantic_overlaps` query returns `Err(QueryError::EmbedderUnavailable)` when the field is `None` (or when the feature is off, via a stub method behind `#[cfg(not(feature = "semantic-overlaps"))]`).

**Acceptance.** `cargo check -p rcm-graph --no-default-features` and `--features semantic-overlaps`. The 37 audit/query tools all dispatch through `GraphService` methods that wrap the legacy module functions. `cargo public-api` shows no `heed::`, `ra_ap_*` leaks.

**Reversal.** Drop the crate.

### 7. `rcm-ide`

**What to do.** `IdeService` wraps `legacy::semantic::SemanticService`. **The legacy `LazyLock<SemanticService>` continues to exist during Phase 1**: removing it is Phase 2's job. `IdeService::open` constructs a fresh `legacy::semantic::SemanticService` instance directly (legacy's `SemanticService::new()` is already public), so the `IdeService` does not touch the static. The static stays only for legacy's bin (which we are deleting in step 8 below) — so by the end of Phase 1 the static has zero callers but is not yet removed. That two-step process is intentional: the dead static is easy to delete in Phase 2 and gives us a small reversible change to land separately.

**Files.** `crates/rcm-ide/{Cargo.toml, src/lib.rs, src/service.rs, src/dto.rs, src/error.rs, src/legacy_adapter.rs}`.

**Cargo.**

```toml
[dependencies]
file-search-mcp-legacy = { path = "../../legacy" }
rcm-ra-host             = { path = "../rcm-ra-host" }
rcm-paths               = { path = "../rcm-paths" }
thiserror               = { workspace = true }
tokio                   = { workspace = true, features = ["rt", "sync"] }
```

**Partial service.**

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use rcm_paths::ProjectPaths;
use rcm_ra_host::RaHost;
use file_search_mcp_legacy as legacy;

pub struct IdeService {
    paths: ProjectPaths,
    inner: legacy::semantic::SemanticService,
    // per-instance cache, replacing the legacy LazyLock in Phase 2
    hosts: Mutex<HashMap<PathBuf, Arc<RaHost>>>,
}

impl IdeService {
    pub fn open(paths: ProjectPaths) -> Result<Self, IdeError> {
        Ok(Self {
            paths,
            inner: legacy::semantic::SemanticService::new(),
            hosts: Mutex::new(HashMap::new()),
        })
    }
    pub async fn find_definition(&self, req: DefinitionRequest)
        -> Result<NavigationResponse, IdeError> { /* delegate to inner.symbol_search/find_references */ }
    pub async fn find_references(&self, req: ReferenceRequest)
        -> Result<NavigationResponse, IdeError> { /* delegate */ }
}
```

**Acceptance.** `find_definition` and `find_references` smoke tests pass. The legacy `LazyLock` static now has zero callers (verified via `cargo geiger`/`who_calls`); we leave it in place for Phase 2.

**Reversal.** Drop crate; revert `IdeService` callers in `rcm-server` to direct legacy.

### 8. `rcm-server` (binary)

**What to do.** Replace legacy's `main.rs` with a new one that:
1. Parses CLI / config (logic moves verbatim from legacy).
2. Constructs the four services in this order: `Embedder`, `IdeService`, `SearchService`, `GraphService` (latter two need the embedder).
3. Hosts `#[tool_router]` and all `*Params` structs.
4. Drives `SyncManager` with `CancellationToken` (shell stays here verbatim, no behavior change).
5. The legacy crate's `[[bin]]` target is removed; `legacy/Cargo.toml` becomes lib-only.

**Files.** `crates/rcm-server/{Cargo.toml, src/main.rs, src/lib.rs, src/router.rs, src/params.rs, src/sync.rs}`. The `legacy/Cargo.toml` loses its `[[bin]]` block; `legacy/src/main.rs` and `legacy/src/bin/` are deleted.

**Cargo (binary).**

```toml
[package]
name = "rcm-server"
edition.workspace = true

[[bin]]
name = "rcm-server"
path = "src/main.rs"

[dependencies]
rcm-search    = { path = "../rcm-search" }
rcm-graph     = { path = "../rcm-graph", features = ["semantic-overlaps"] }
rcm-ide       = { path = "../rcm-ide" }
rcm-paths     = { path = "../rcm-paths" }
rcm-embedding = { path = "../rcm-embedding" }
rmcp          = { workspace = true }
tokio         = { workspace = true, features = ["full"] }
anyhow        = { workspace = true }
tracing       = { workspace = true }
serde         = { workspace = true, features = ["derive"] }
serde_json    = { workspace = true }
clap          = { workspace = true, features = ["derive"] }
```

The binary has **no** `file-search-mcp-legacy` dep. Verified post-build with `cargo tree -i file-search-mcp-legacy -p rcm-server`: legacy must show only as a transitive dep through `rcm-search` / `rcm-graph` / `rcm-ide` / `rcm-embedding`, never as a direct dep.

**Acceptance.** `cargo build -p rcm-server` produces the binary. The full smoke checklist passes.

**Reversal.** Restore `legacy`'s `[[bin]]` and `main.rs`; remove `crates/rcm-server`. The binary then points at legacy directly.

## Tool routing

Every MCP tool's `#[tool]` handler in `crates/rcm-server/src/router.rs` is a thin shim: convert `Params` → service request, call the right service, convert response → `serde_json::Value`. No business logic in the router.

Dispatch examples:

```rust
#[tool] async fn search(&self, p: SearchParams) -> rmcp::Result<CallToolResult> {
    let hits = self.search.search(SearchRequest::from(p)).await?;
    Ok(json_result(&hits))
}

#[tool] async fn find_definition(&self, p: FindDefinitionParams) -> rmcp::Result<CallToolResult> {
    let nav = self.ide.find_definition(DefinitionRequest::from(p)).await?;
    Ok(json_result(&nav))
}

#[tool] async fn who_calls(&self, p: WhoCallsParams) -> rmcp::Result<CallToolResult> {
    let edges = self.graph.who_calls(WhoCallsRequest::from(p)).await?;
    Ok(json_result(&edges))
}

#[tool] async fn index_codebase(&self, p: IndexCodebaseParams) -> rmcp::Result<CallToolResult> {
    let report = self.search.index(IndexRequest::from(p)).await?;
    Ok(json_result(&report))
}
```

The composition tools (`similar_to_item`, `health_check`) call multiple services, but the orchestration logic itself moves verbatim from legacy with imports rewritten.

## Adapter module convention

Every capability and infra crate has exactly one `pub(crate) mod legacy_adapter;` containing nothing but `From` / `TryFrom` impls between `file_search_mcp_legacy::*` types and the crate's own DTOs. **No business logic, no async, no `Result`-shaping** beyond what `From`/`TryFrom` allow.

Example for `rcm-search`:

```rust
// crates/rcm-search/src/legacy_adapter.rs
use file_search_mcp_legacy as legacy;
use crate::dto::{SearchHit, ChunkId};

impl From<legacy::search::SearchResult> for SearchHit {
    fn from(r: legacy::search::SearchResult) -> Self {
        SearchHit::new(
            ChunkId::new(r.chunk_id),  // distinct type — see risk #2
            r.file_path,
            r.score,
            r.snippet,
            r.line_start,
            r.line_end,
        )
    }
}

impl From<legacy::search::SearchError> for crate::error::SearchError {
    fn from(e: legacy::search::SearchError) -> Self { Self::Backend(e) }
}
```

Same convention applied to `rcm-graph` (`legacy::graph::Node` → `rcm_graph::Node`, `legacy::graph::CallEdge` → `rcm_graph::CallEdge`, etc.) and `rcm-ide` (`legacy::semantic::Location` → `rcm_ide::SourceLocation`).

## Acceptance criteria (Phase 1 exit)

1. **Smoke checklist green.** Every MCP tool listed in DECISIONS' smoke checklist returns the same shape and content it does today, called against the new `rcm-server` binary on the standard fixture workspace.
2. **`cargo public-api --simplified -p rcm-search` (and `-p rcm-graph`, `-p rcm-ide`, `-p rcm-paths`, `-p rcm-embedding`) lists ONLY** the items frozen in DECISIONS. No `tantivy::`, `lancedb::`, `arrow::`, `fastembed::`, `ort::`, `ra_ap_ide::`, `ra_ap_hir::`, `heed::`, `sled::`, `rmcp::` strings appear in capability or `rcm-paths` output. The `rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding` exemptions are limited to their documented whitelists.
3. **`cargo deny check`** passes (advisories, license, duplicates).
4. **`forbidden_dependency_check` MCP tool** reports zero violations: capability-to-capability edges absent, every crate may pull `legacy` transitively, `xtask` excluded.
5. **`cargo tree -i file-search-mcp-legacy -p rcm-server`** shows legacy only via transitive paths through capability and infra crates — never as a direct dep of the binary.
6. **`cargo build --workspace`** green with default features and `--no-default-features --features rcm-graph/semantic-overlaps`.
7. **Reversibility.** A single `git revert` of the merge commit restores: legacy's `[[bin]]` target, the binary's direct dependency on `legacy`, deletion of the eight new crates. No data migration is involved because storage layouts are unchanged.

## Phase 1 risks and mitigations

1. **Adapter conversion bloat.** Risk: `legacy_adapter.rs` modules become a parallel domain layer. Mitigation: each crate's `legacy_adapter` is a single file, contains only `From` / `TryFrom`, and **MUST NOT** be promoted into a shared `rcm-adapters` crate — the adapter is a phase-bound transition, not a new layer. A workspace lint or PR-template checkbox enforces "no `legacy_adapter`-to-`legacy_adapter` cross-imports".
2. **Type identity.** `ChunkId` in `rcm-search` is a **distinct Rust type** from `legacy::ChunkId`, even though both wrap the same `u64`. Same for `NodeId`, `BindingId`, `UsageId` in `rcm-graph`. Conversion happens at every call site. Rationale: leaking legacy types through capability APIs would force `rcm-server` to import legacy directly, defeating the whole boundary. Cost: a few hundred extra `.into()` calls; this is the point of Phase 1.
3. **Performance regression from extra conversion.** Risk: per-result allocation in `From<legacy::SearchResult> for SearchHit`. Mitigation: measure with the existing `IndexingMetrics` and any bench in `xtask`. Acceptable Phase 1 budget: < 5% wall-clock regression on `index_codebase` and `search` against the fixture workspace. Do **not** optimize until Phase 5; if regression exceeds budget, mitigate by moving fields by value (the legacy types own their strings) rather than by introducing borrowed DTOs prematurely.
4. **Legacy → `rcm-paths` back-edge confusion.** Risk: future readers see a back-edge and assume the graph is broken. Mitigation: explicit `# PHASE-1-ONLY` comment in `legacy/Cargo.toml` next to the `rcm-paths` dep, plus a checklist item in Phase 2 to remove it.
5. **Dead `LazyLock` left in legacy.** Risk: confusion about which path is live. Mitigation: `cargo geiger` / `who_calls` audit at end of Phase 1 confirms zero callers; Phase 2 deletes it as its first commit.

## 02 — Capability Crate Public APIs

This document specifies the frozen public API surface of the three capability crates: `rcm-search`, `rcm-graph`, `rcm-ide`. It is normative; deviations require updating `DECISIONS.md` first.

All three crates follow the strict-tier API leak rule: no `tantivy::`, `lancedb::`, `arrow::`, `fastembed::`, `ort::`, `ra_ap_*`, `heed::`, `sled::`, or `rmcp::` types appear in any public signature. Capability DTOs are NOT `Serialize`; only `rcm-server` serializes (rust-analyzer's pattern).

---

## `rcm-search`

### Crate-root doc

```rust
//! `rcm-search` — corpus and retrieval capability.
//!
//! Owns: file walk, change detection (Merkle), metadata cache (sled), Tantivy
//! schema/writer/reader, LanceDB connection, hybrid search, RRF fusion, corpus
//! security (sensitive-file filter, secrets scanner), and chunking-context-only
//! AST extraction via `rcm-ra-syntax`.
//!
//! Does NOT own: HIR, resolved structure, navigation, persisted graph, or any
//! `ra_ap_ide`/`ra_ap_hir` work — those live in `rcm-graph` and `rcm-ide`.
//!
//! Public-API leak rule: strict tier. No `tantivy::`, `lancedb::`, `arrow::`,
//! `fastembed::`, `ort::`, `ra_ap_*`, `heed::`, `sled::`, or `rmcp::` types in
//! public signatures. Public DTOs are not `Serialize` — `rcm-server` serializes.
#![warn(missing_docs)]
#![warn(unreachable_pub)]
```

### Cargo features

```toml
[features]
default = []
# rcm-search itself is trait-only against rcm-embedding. The production
# fastembed/ort backend is enabled exclusively by the binary (`rcm-server`),
# which depends on `rcm-embedding` with `features = ["embeddings"]`.
# Capability crates depending on `rcm-embedding` use `default-features = false`.
test-fakes = ["rcm-embedding/test-fakes"]   # default-off; deterministic embedder for unit tests
```

`rcm-search` never pulls fastembed/ort directly. Any production embedder is
threaded through `Arc<dyn Embed>` constructed by the binary; tests construct a
`DeterministicEmbedder` via the `test-fakes` feature on `[dev-dependencies]`.
This is what makes `cargo tree -p rcm-search | grep -E '(fastembed|ort)'`
return empty — the asserted invariant in Phase 5.

### Public types

```rust
use std::path::PathBuf;
use std::sync::Arc;
use rcm_embedding::Embedder;
use rcm_paths::ProjectPaths;

#[non_exhaustive]
pub struct SearchService {
    paths: Arc<ProjectPaths>,
    embedder: Embedder,
    bm25: arc_swap::ArcSwap<crate::bm25::Bm25Reader>,
    vector: arc_swap::ArcSwap<crate::vector::VectorReader>,
    cache: Arc<crate::cache::MetadataCache>,
}

#[non_exhaustive]
pub struct CorpusWriter {
    paths: Arc<ProjectPaths>,
    embedder: Embedder,
    tantivy_writer: parking_lot::Mutex<crate::bm25::TantivyWriter>,
    vector_writer: crate::vector::VectorWriter,
    cache: Arc<crate::cache::MetadataCache>,
}

/// Stable per-chunk identifier (SHA-256 over file path + byte range + content).
#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct ChunkId(String);

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CodeChunk {
    file_path: PathBuf,
    byte_start: u32,
    byte_end: u32,
    symbol_name: Option<String>,
    symbol_kind: SymbolKind,
    docstring: Option<String>,
    content: String,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum SymbolKind { Fn, Struct, Enum, Trait, Impl, Module, Use, Other }

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum ForceReindex { Force, Allow }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SearchRequest {
    query: String,
    limit: u32,
    candidate_count: u32,
}

impl SearchRequest {
    #[must_use]
    pub fn new(query: impl Into<String>) -> Self;
    #[must_use]
    pub fn with_limit(self, limit: u32) -> Self;
    #[must_use]
    pub fn with_candidate_count(self, n: u32) -> Self;
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SearchResponse {
    hits: Vec<SearchHit>,
    fallback_mode: bool,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SearchHit {
    chunk_id: ChunkId,
    chunk: CodeChunk,
    bm25_score: Option<f32>,
    vector_score: Option<f32>,
    bm25_rank: Option<u32>,
    vector_rank: Option<u32>,
    fused_score: f32,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SimilarRequest {
    seed: String,
    limit: u32,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SimilarResponse {
    hits: Vec<SearchHit>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct IndexRequest {
    workspace: PathBuf,
    force: ForceReindex,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct IndexStats {
    files_seen: u64,
    files_indexed: u64,
    files_skipped: u64,
    chunks_written: u64,
    duration_ms: u64,
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum SearchError {
    #[error("embedding failed")]
    Embedding(#[from] rcm_embedding::EmbedError),
    #[error("vector store I/O failed: {0}")]
    VectorStore(String),
    #[error("BM25 query failed: {0}")]
    Bm25(String),
    #[error("no results")]
    NoResults,
    #[error("index busy: writer in progress")]
    IndexBusy,
    #[error("path: {0}")]
    Path(#[from] rcm_paths::PathError),
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum IndexError {
    #[error("walk error in {path}: {source}")]
    Walk { path: PathBuf, #[source] source: std::io::Error },
    #[error("parse failed for {path}")]
    Parse { path: PathBuf },
    #[error("embedding failed")]
    Embedding(#[from] rcm_embedding::EmbedError),
    #[error("BM25 commit failed: {0}")]
    Bm25Commit(String),
    #[error("vector upsert failed: {0}")]
    VectorUpsert(String),
    #[error("metadata cache: {0}")]
    Cache(String),
    #[error("path: {0}")]
    Path(#[from] rcm_paths::PathError),
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum CorpusError {
    #[error("sensitive file rejected: {0}")]
    Sensitive(PathBuf),
    #[error("secret detected in {0}")]
    SecretDetected(PathBuf),
    #[error("memory pressure: {used_mb} MB above {limit_mb} MB cap")]
    Memory { used_mb: u64, limit_mb: u64 },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
```

### Public methods

```rust
impl SearchService {
    /// Open the search service against an existing project's storage layout.
    /// # Errors
    /// Returns `SearchError::Path` if storage paths cannot be resolved, or
    /// backend-specific variants if Tantivy/LanceDB readers fail to open.
    #[must_use = "Result must be handled"]
    pub fn open(paths: Arc<ProjectPaths>, embedder: Embedder)
        -> Result<Self, SearchError>;

    /// Run a hybrid BM25+vector search and fuse via RRF.
    /// # Errors
    /// Returns `SearchError::Embedding` if the query vector cannot be
    /// produced, `Bm25` if Tantivy fails, `VectorStore` if LanceDB fails,
    /// or `NoResults` if both arms returned empty.
    pub async fn search(&self, req: SearchRequest)
        -> Result<SearchResponse, SearchError>;

    /// Return chunks semantically similar to `req.seed` (vector-only).
    /// # Errors
    /// Returns `SearchError::Embedding` or `SearchError::VectorStore`.
    pub async fn similar(&self, req: SimilarRequest)
        -> Result<SimilarResponse, SearchError>;

    /// Drop in-memory `IndexReader` / `Connection` `ArcSwap` slots for
    /// `workspace`. The next operation on this service rebuilds via the
    /// existing fingerprint-mismatch path. **Does not** delete on-disk
    /// data (that is `clear_cache`'s job, in `rcm-server`) and **does
    /// not** auto-reindex.
    /// # Errors
    /// Returns `SearchError::IndexBusy` if a writer is mid-batch on
    /// `workspace`; in that case no handles are dropped.
    pub fn invalidate(&self, workspace: &Path) -> Result<(), SearchError>;

    /// Open new readers, atomically swap them in via `ArcSwap`, drop the
    /// old after a grace period. Used by `SyncManager` after a Tantivy
    /// schema-version bump. **Does not** delete on-disk data.
    /// # Errors
    /// Returns `SearchError::IndexBusy` if a writer is mid-batch.
    pub async fn reload(&self, workspace: &Path) -> Result<(), SearchError>;
}

impl CorpusWriter {
    /// Open a writer rooted at the given project paths.
    /// # Errors
    /// Returns `IndexError::Path` or backend-specific I/O errors.
    #[must_use]
    pub fn open(paths: Arc<ProjectPaths>, embedder: Embedder)
        -> Result<Self, IndexError>;

    /// Walk, change-detect, parse, embed, and write the codebase.
    /// # Errors
    /// Any per-file failure may surface; partial results commit per batch.
    pub async fn index(&self, req: IndexRequest)
        -> Result<IndexStats, IndexError>;

    /// Drop the on-disk corpus for this workspace.
    /// # Errors
    /// Returns `IndexError::Cache` or filesystem errors.
    pub async fn clear(&self) -> Result<(), IndexError>;
}
```

### Sealed / private items

- `crate::bm25::TantivyWriter` and `crate::vector::VectorWriter` are private; they hold `tantivy::IndexWriter` and `lancedb::Connection` respectively. The strict-tier leak rule requires they never appear in public signatures.
- `parking_lot::Mutex<TantivyWriter>` is the writer-side singleton. `tantivy::IndexWriter` is `Send` but single-writer; the mutex enforces "one outstanding batch" and lets `reload` return `SearchError::IndexBusy` cleanly.
- The `chunker` module's `RaSyntaxView` adapter is private. Externally only the resulting `Vec<CodeChunk>` is observable.

### Sans-I/O cores

- `crate::chunker` — `pub(crate) fn chunk(source: &[u8], parsed: rcm_ra_syntax::SourceFile) -> Vec<CodeChunk>`. Pure, sync, no tokio. Uses `rcm-ra-syntax` for chunking-context-only extraction (last-segment names, raw `use` paths, file-scoped `SymbolKind`). Does NOT resolve structure.
- `crate::rrf` — `pub(crate) fn fuse(arms: &[RankedList<'_>], k: f32) -> Vec<(ChunkId, f32)>`. Pure RRF math.
- `crate::secrets` — pattern matchers; sync, byte-slice in / boolean out.

---

## `rcm-graph`

### Crate-root doc

```rust
//! `rcm-graph` — persisted hypergraph and audits capability.
//!
//! Owns: HIR extraction via `rcm-ra-host`, the heed/LMDB snapshot lifecycle
//! (`build_and_persist`, `OpenedSnapshot`), all snapshot-only queries
//! (imports/exports/call-graph/dead-pub/overlaps/module-tree/stats), AST-driven
//! audits (`unsafe`, `channel`, `fn_body`), and snapshot-only audits
//! (`derive`, `docs`, `recursion`, `mut_static`).
//!
//! Does NOT own: corpus/text search, navigation queries (live IDE),
//! tool routing, or process composition.
//!
//! Public-API leak rule: strict tier. DTOs are not `Serialize`. Embedder
//! is feature-gated under `semantic-overlaps`.
#![warn(missing_docs)]
#![warn(unreachable_pub)]
```

### Cargo features

```toml
[features]
default = []
semantic-overlaps = ["dep:rcm-embedding"]   # default-OFF here; rcm-server enables it
test-fakes = ["rcm-embedding/test-fakes"]
```

### Public types

```rust
use std::path::PathBuf;
use std::sync::Arc;
use rcm_paths::ProjectPaths;
use rcm_ra_host::RaHost;

#[non_exhaustive]
pub struct GraphService {
    paths: Arc<ProjectPaths>,
    ra_host: Arc<RaHost>,
    current: arc_swap::ArcSwap<OpenedSnapshot>,
    #[cfg(feature = "semantic-overlaps")]
    embedder: Option<rcm_embedding::Embedder>,
}

/// Builder for `GraphService`. Optional embedder enables `semantic_overlaps`.
#[non_exhaustive]
pub struct GraphServiceBuilder {
    paths: Arc<ProjectPaths>,
    ra_host: Arc<RaHost>,
    #[cfg(feature = "semantic-overlaps")]
    embedder: Option<rcm_embedding::Embedder>,
}

/// Opaque, ID-shaped handle to a published heed snapshot.
#[non_exhaustive]
pub struct OpenedSnapshot {
    graph_id: GraphId,
    fingerprint: Fingerprint,
    env: heed::Env,                 // private
    dbs: crate::storage::GraphDatabases, // private
}

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct GraphId(String);

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct Fingerprint([u8; 32]);

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct NodeId(String);

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct BindingId(String);

#[derive(Clone, Eq, PartialEq, Hash, Debug)]
pub struct UsageId(String);

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Node {
    id: NodeId,
    qualified_name: String,
    kind: NodeKind,
    file: Option<PathBuf>,
    span: Option<(u32, u32)>,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum NodeKind { Workspace, Crate, Module, Fn, Struct, Enum, Trait, Impl, Method, AssocConst, AssocType, Variant, Static, Const, TypeAlias, Use }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Binding {
    id: BindingId,
    importer: NodeId,
    target: NodeId,
    kind: BindingKind,
    visibility: BindingVisibility,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum BindingKind { Declared, Named, Glob, Extern }

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum BindingVisibility { Public, Crate, Module, Private }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct Usage {
    id: UsageId,
    referrer: NodeId,
    target: NodeId,
    category: UsageCategory,
    file: PathBuf,
    byte_range: (u32, u32),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
#[non_exhaustive]
pub enum UsageCategory { Call, TypeRef, FieldAccess, MacroInvoke, Other }

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CallEdge {
    caller: NodeId,
    callee: NodeId,
    file: PathBuf,
    byte_range: (u32, u32),
}

#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct AuditOpts {
    crate_filter: Option<String>,
    severity_min: Severity,
}

#[derive(Copy, Clone, Eq, PartialEq, Debug, Default)]
#[non_exhaustive]
pub enum Severity {
    Info, #[default] Warn, Error,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct AuditReport<F> {
    findings: Vec<F>,
    elapsed_ms: u32,
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum BuildError {
    #[error("workspace load failed: {0}")]
    Load(#[from] rcm_ra_host::RaError),
    #[error("snapshot persist failed: {0}")]
    Persist(String),
    #[error("path: {0}")]
    Path(#[from] rcm_paths::PathError),
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum QueryError {
    #[error("snapshot not opened")]
    NotOpened,
    #[error("node not found: {0}")]
    NodeNotFound(String),
    #[error("snapshot read failed: {0}")]
    Read(String),
    #[error("embedder unavailable: enable `semantic-overlaps` feature")]
    EmbedderUnavailable,
    #[cfg(feature = "semantic-overlaps")]
    #[error("embedding failed")]
    Embedding(#[from] rcm_embedding::EmbedError),
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum AuditError {
    #[error("query failed")]
    Query(#[from] QueryError),
    #[error("re-load for AST audit failed: {0}")]
    Reload(#[from] rcm_ra_host::RaError),
}
```

### Public methods — builder

```rust
impl GraphService {
    /// Begin building a `GraphService`. The embedder is optional and only
    /// required for `semantic_overlaps`.
    #[must_use]
    pub fn builder(paths: Arc<ProjectPaths>, ra_host: Arc<RaHost>)
        -> GraphServiceBuilder;
}

impl GraphServiceBuilder {
    /// Attach an embedder to enable `semantic_overlaps`. No-op without
    /// the `semantic-overlaps` feature.
    #[must_use]
    #[cfg(feature = "semantic-overlaps")]
    pub fn with_embedder(self, embedder: rcm_embedding::Embedder) -> Self;

    /// Finalize and open the current snapshot (if any).
    /// # Errors
    /// Returns `BuildError` if storage paths or snapshot open fails.
    #[must_use]
    pub fn build(self) -> Result<GraphService, BuildError>;
}
```

### Public methods — service

```rust
impl GraphService {
    /// Build (or reuse via fingerprint) and publish a fresh snapshot.
    /// # Errors
    /// Returns `BuildError` for HIR-load or persist failure.
    pub async fn build_and_persist(&self, workspace: &std::path::Path)
        -> Result<GraphId, BuildError>;

    /// Drop the in-memory `OpenedSnapshot` for `workspace`. The next
    /// graph query rebuilds via the fingerprint-mismatch path in
    /// `build_and_persist`. **Does not** delete on-disk LMDB data and
    /// **does not** rebuild eagerly. This is the cheap operation called
    /// by `clear_cache`.
    pub fn invalidate(&self, workspace: &Path);

    /// Eagerly rebuild the snapshot for `workspace` (used by
    /// `SyncManager` schema-change paths, NOT by `clear_cache`).
    /// # Errors
    /// Returns `QueryError::Build` on snapshot build failure.
    pub async fn reload(&self, workspace: &Path) -> Result<(), QueryError>;

    /// Resolve a qualified name to a `NodeId`.
    /// # Errors
    /// Returns `QueryError::NotOpened` or `NodeNotFound`.
    pub fn lookup_by_qualified_name(&self, qname: &str)
        -> Result<NodeId, QueryError>;

    /// Imports of a module / item.
    /// # Errors
    /// `QueryError::Read` on heed failure.
    pub fn imports_of(&self, node: &NodeId) -> Result<Vec<Binding>, QueryError>;
    /// # Errors
    /// `QueryError::Read` on heed failure.
    pub fn exports_of(&self, node: &NodeId) -> Result<Vec<Binding>, QueryError>;
    /// # Errors
    /// `QueryError::Read` on heed failure.
    pub fn who_imports(&self, node: &NodeId) -> Result<Vec<Binding>, QueryError>;
    /// # Errors
    /// `QueryError::Read` on heed failure.
    pub fn usages_of(&self, node: &NodeId) -> Result<Vec<Usage>, QueryError>;

    /// Direct callers of `node`.
    /// # Errors
    /// `QueryError::Read` on heed failure.
    pub fn who_calls(&self, node: &NodeId) -> Result<Vec<CallEdge>, QueryError>;
    /// # Errors
    /// `QueryError::Read` on heed failure.
    pub fn calls_from(&self, node: &NodeId) -> Result<Vec<CallEdge>, QueryError>;
    /// Bounded recursive call graph (default depth 3, max 8).
    /// # Errors
    /// `QueryError::Read` on heed failure.
    pub fn call_graph(&self, root: &NodeId, depth: u8)
        -> Result<Vec<CallEdge>, QueryError>;

    /// Returns `Err(QueryError::EmbedderUnavailable)` if no embedder was
    /// configured at build time.
    /// # Errors
    /// `EmbedderUnavailable`, `Embedding`, or `Read`.
    #[cfg(feature = "semantic-overlaps")]
    pub async fn semantic_overlaps(&self, node: &NodeId)
        -> Result<Vec<NodeId>, QueryError>;

    /// Run the unsafe-block audit (AST-driven).
    /// # Errors
    /// `AuditError::Reload` if HIR re-load fails.
    pub async fn unsafe_audit(&self, opts: &AuditOpts)
        -> Result<AuditReport<crate::audits::UnsafeFinding>, AuditError>;

    /// # Errors
    /// `AuditError::Query` on snapshot read failure.
    pub fn derive_audit(&self, opts: &AuditOpts)
        -> Result<AuditReport<crate::audits::DeriveFinding>, AuditError>;

    /// # Errors
    /// `AuditError::Query` on snapshot read failure.
    pub fn missing_docs_audit(&self, opts: &AuditOpts)
        -> Result<AuditReport<crate::audits::MissingDocsFinding>, AuditError>;

    /// # Errors
    /// `AuditError::Reload` if HIR re-load fails.
    pub async fn fn_body_audit(&self, opts: &AuditOpts)
        -> Result<AuditReport<crate::audits::FnBodyFinding>, AuditError>;
}
```

### Sealed / private items

- `crate::storage::GraphDatabases` (typed sub-DB handles), `crate::storage::GraphPaths`, and `heed::Env` are private fields of `OpenedSnapshot`. The leak rule forbids exposing them.
- `crate::sealed::Sealed` is implemented privately for the audit-finding type parameters of `AuditReport<F>` to prevent third parties from constructing report variants the snapshot can't validate.
- No `Mutex<!Sync>` is required: heed read txns are MVCC-safe and per-call. The `arc_swap::ArcSwap<OpenedSnapshot>` is the only mutability primitive on the service.

### Sans-I/O cores

- `crate::audits::derive` — `pub(crate) fn audit(snapshot_view: SnapshotView<'_>, opts: &AuditOpts) -> AuditReport<DeriveFinding>`. Pure over a borrowed snapshot view.
- `crate::audits::docs` — same shape, pure.
- `crate::audits::recursion` — DFS over an in-memory edge slice.
- `crate::audits::mut_static` — pure over snapshot rows.
- `crate::audits::dead_pub` — pure over binding/usage iterators.
- `crate::hir_trim` — pure string transform.
- `crate::ast_resolve::resolve_call_to_function` — sync; takes borrowed AST/HIR views. The async I/O wrapper that opens the workspace is separate.
- `crate::ids` — pure SHA-256 NodeId/BindingId/UsageId builders.

---

## `rcm-ide`

### Crate-root doc

```rust
//! `rcm-ide` — live navigation capability.
//!
//! Owns: per-workspace `RaHost` cache (IDE preset: `no_deps=true`,
//! `prefill_caches=true`, no sysroot), goto-definition, find-references,
//! and name-based symbol search routed through `rcm-ra-host` closures.
//!
//! Does NOT own: corpus/text search, persisted graph, or audits.
//!
//! Public-API leak rule: strict tier. The service is constructed in
//! `rcm-server::main`, NOT a `LazyLock`. Internal cache is per-instance
//! (`Mutex<HashMap<PathBuf, Arc<RaHost>>>`), no global statics.
#![warn(missing_docs)]
#![warn(unreachable_pub)]
```

### Cargo features

```toml
[features]
default = []
# (no optional dependencies; rcm-ide is a thin live-navigation wrapper.)
```

### Public types

```rust
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use rcm_paths::ProjectPaths;
use rcm_ra_host::RaHost;

#[non_exhaustive]
pub struct IdeService {
    paths: Arc<ProjectPaths>,
    cache: Mutex<HashMap<PathBuf, Arc<RaHost>>>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct DefinitionRequest {
    workspace: PathBuf,
    file: PathBuf,
    line: u32,    // 1-based
    column: u32,  // 1-based
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ReferenceRequest {
    workspace: PathBuf,
    file: PathBuf,
    line: u32,
    column: u32,
    include_imports: bool,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SymbolSearchRequest {
    workspace: PathBuf,
    query: String,
    limit: u32,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct NavigationResponse {
    locations: Vec<SourceLocation>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct SourceLocation {
    file_path: PathBuf,
    line: u32,    // 1-based
    column: u32,  // 1-based
    name: String,
}

#[derive(thiserror::Error, Debug)]
#[non_exhaustive]
pub enum IdeError {
    #[error("path: {0}")]
    Path(#[from] rcm_paths::PathError),
    #[error("ra-host: {0}")]
    RaHost(#[from] rcm_ra_host::RaError),
    #[error("workspace not loaded: {0}")]
    WorkspaceNotLoaded(PathBuf),
    #[error("file not in vfs: {0}")]
    FileNotInVfs(PathBuf),
    #[error("position out of range")]
    PositionOutOfRange,
    #[error("query failed: {0}")]
    Query(String),
    #[error("cache poisoned")]
    Poisoned,
}
```

### Public methods

```rust
impl IdeService {
    /// Open the IDE service. Constructed once in `rcm-server::main` and
    /// passed via `Arc<IdeService>` to handlers — never a `LazyLock`.
    /// # Errors
    /// Returns `IdeError::Path` if storage paths cannot be resolved.
    #[must_use = "Result must be handled"]
    pub fn open(paths: Arc<ProjectPaths>) -> Result<Self, IdeError>;

    /// Resolve the symbol at `(file, line, column)` to its definition(s).
    /// # Errors
    /// `WorkspaceNotLoaded`, `FileNotInVfs`, `PositionOutOfRange`, `RaHost`,
    /// or `Query` on goto-definition failure.
    pub async fn find_definition(&self, req: DefinitionRequest)
        -> Result<NavigationResponse, IdeError>;

    /// Find all references to the symbol at the cursor.
    /// # Errors
    /// Same as `find_definition`.
    pub async fn find_references(&self, req: ReferenceRequest)
        -> Result<NavigationResponse, IdeError>;

    /// Name-based symbol search across the workspace.
    /// # Errors
    /// `WorkspaceNotLoaded`, `RaHost`, or `Query`.
    pub async fn symbol_search(&self, req: SymbolSearchRequest)
        -> Result<NavigationResponse, IdeError>;

    /// Drop the cached `RaHost` for `workspace`. Safe to call concurrently
    /// with queries against other workspaces.
    /// # Errors
    /// `Poisoned` if the cache mutex was poisoned.
    pub async fn evict(&self, workspace: &std::path::Path)
        -> Result<(), IdeError>;
}
```

### Sealed / private items

- The cache value type `Arc<RaHost>` is exposed only through closures internally; the `RaHost` struct stays inside `rcm-ra-host` and is wrapped here.
- `Mutex<HashMap<PathBuf, Arc<RaHost>>>` is required because `RaHost` (wrapping rust-analyzer's `AnalysisHost`) is not `Sync`; queries serialize per workspace. The mutex is **per `IdeService` instance** — there is no global static. Multiple workspaces still serialize on insertion/eviction; per-workspace queries take a clone of `Arc<RaHost>` and release the cache lock before invoking `with_db` / `with_semantics`.
- No traits are sealed; the surface is concrete structs and methods.

### Sans-I/O cores

- `crate::position` — `pub(crate) fn translate(line_index: &LineIndexView<'_>, line: u32, column: u32) -> Option<TextOffset>` and `pub(crate) fn nav_to_location(target: NavView<'_>) -> SourceLocation`. Both are pure: borrowed views in, owned values out, no async, no I/O. The async wrappers that acquire `Analysis` snapshots live in the public service methods.
- `crate::cache_key` — pure path canonicalization helper.

---

## Cross-cutting notes

- All capability DTOs derive `Clone` + `Debug` only. `Serialize`/`Deserialize` impls live in `rcm-server`'s adapter layer, which mirrors rust-analyzer's "domain types are not transport types" pattern.
- All `Result`-returning service methods are `#[must_use]` (enforced workspace-wide via the `must_use_candidate` clippy lint).
- All public structs and enums where future fields/variants are likely carry `#[non_exhaustive]`. Exhaustive types are the ID newtypes (`ChunkId`, `NodeId`, `GraphId`, `Fingerprint`, `BindingId`, `UsageId`) and unit-shaped enums whose membership is intrinsically closed.
- No boolean parameters in public APIs — `ForceReindex` is the search-side example; `include_imports` on `ReferenceRequest` is internal-only and may be replaced by an enum before stabilization.
- Service constructors are domain-named: `SearchService::open`, `CorpusWriter::open`, `IdeService::open`, `GraphService::builder`. None use `new`.

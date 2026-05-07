# 03 — Infrastructure leaf APIs + server composition

Authoritative source: `.docs/workspace-plan/DECISIONS.md`. Every signature here is normative.

---

## `rcm-paths`

### Crate-root doc

```rust
//! Storage path resolution. The **only** place that hashes a workspace path.
//! Recipe is frozen: `sha256(canonicalize(workspace).as_os_str().as_encoded_bytes())`,
//! lower-hex, 64 chars. Other crates accept `&ProjectPaths`.
//!
//! Sync only. No tokio. Strict API leak rule.
#![warn(missing_docs)]
#![warn(unreachable_pub)]
```

### Public types

```rust
#[non_exhaustive]
pub struct ProjectPaths {
    workspace_dir: PathBuf,   // canonicalized
    dir_hash: String,         // hex sha-256, 64 chars
    data_root: PathBuf,
    tantivy_path: PathBuf,    // <root>/search/tantivy/<hash>
    vector_path: PathBuf,     // <root>/search/vectors/<hash>
    cache_path: PathBuf,      // <root>/search/cache/<hash>     (sled)
    graph_path: PathBuf,      // <root>/graph/<hash>            (LMDB)
    merkle_path: PathBuf,     // <root>/snapshots/<hash>.snapshot
    collection_name: String,  // "code_chunks_<hash[..8]>"
}

pub enum StorageRoot {
    Xdg,                  // ProjectDirs("dev","rust-code-mcp","search")
    Explicit(PathBuf),    // CLI / env / config override
}

#[derive(Debug, thiserror::Error)]
pub enum PathError {
    #[error("workspace path does not exist: {0}")] Missing(PathBuf),
    #[error("canonicalize failed for {0}: {1}")]   Canonicalize(PathBuf, std::io::Error),
    #[error("XDG project dirs unavailable")]       NoXdgHome,
    #[error("io under storage root {root:?}: {source}")]
    Io { root: PathBuf, #[source] source: std::io::Error },
}
```

### Public methods

```rust
impl ProjectPaths {
    /// Frozen recipe — the only hashing call.
    pub fn resolve(workspace: &Path, root: &StorageRoot) -> Result<Self, PathError> { /* ... */ }

    pub fn workspace_dir(&self) -> &Path { /* ... */ }
    pub fn dir_hash(&self) -> &str { /* ... */ }
    pub fn data_root(&self) -> &Path { /* ... */ }
    pub fn tantivy_path(&self) -> &Path { /* ... */ }
    pub fn vector_path(&self) -> &Path { /* ... */ }
    pub fn cache_path(&self) -> &Path { /* ... */ }
    pub fn graph_path(&self) -> &Path { /* ... */ }
    pub fn merkle_path(&self) -> &Path { /* ... */ }
    pub fn collection_name(&self) -> &str { /* ... */ }
}

impl StorageRoot {
    /// Reads `RUST_CODE_MCP_DATA_DIR`; else `Xdg`.
    pub fn from_env() -> Self { /* ... */ }
}
```

### Frozen hash recipe

```rust
fn hash_workspace(canonical: &Path) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(canonical.as_os_str().as_encoded_bytes());
    let bytes = h.finalize();
    let mut s = String::with_capacity(64);
    for b in bytes { use std::fmt::Write; write!(&mut s, "{b:02x}").unwrap(); }
    s
}
```

Fixed-input fixture test pins this; any change fails CI.

### Private internals

`hash_workspace`, XDG resolver, `mkdir_p_safe`. Not exported.

### API leak status

**Strict.** Public signatures contain only `std`, `String`, `PathBuf`, and `thiserror`-derived items. No re-exports of `directories`, `sha2`, or `serde`.

### `Cargo.toml`

```toml
[package]
name = "rcm-paths"
version.workspace = true
edition.workspace = true

[dependencies]
directories = { workspace = true }
sha2        = { workspace = true }
thiserror   = { workspace = true }
serde       = { workspace = true, features = ["derive"], optional = true }

[features]
default = []
serde   = ["dep:serde"]
```

---

## `rcm-ra-syntax`

### Crate-root doc

```rust
//! Narrow re-export shim for `ra_ap_syntax`. Dual purpose:
//! (1) version pinning — only place declaring `ra_ap_syntax`;
//! (2) compile-graph isolation — keeps `ra_ap_ide`/`ra_ap_hir` out of the
//! chunker's build.
//!
//! **Exempt** from the strict API leak rule. The whitelist below IS the
//! boundary; expanding it requires code review. Sync only. No tokio.
```

### Whitelist (entire public surface)

```rust
pub use ra_ap_syntax::{
    SourceFile, AstNode, AstToken,
    SyntaxKind, SyntaxNode, SyntaxToken,
    Edition, Parse, TextRange, TextSize,
};
pub mod ast {
    pub use ra_ap_syntax::ast::{
        Fn, Struct, Enum, Trait, Impl, Module, Use, UseTree, Path, NameRef,
    };
}
```

15 items. CI grep enforces; expansion needs review.

### Private internals

None — re-exports only.

### API leak status

**Exempt** (documented).

### `Cargo.toml`

```toml
[package]
name = "rcm-ra-syntax"
version.workspace = true
edition.workspace = true

[dependencies]
ra_ap_syntax = { workspace = true }
```

No features. No tokio.

---

## `rcm-ra-host`

### Crate-root doc

```rust
//! Owning lifecycle wrapper around `RootDatabase` + `Vfs`. Two preset
//! constructors name the configuration divergence:
//!   - `open_ide`: `no_deps=true`, no sysroot, `prefill_caches=true` (~120 ms).
//!   - `open_hir`: `no_deps=false`, sysroot `Discover`, `set_test=true`,
//!      all features (seconds).
//!
//! Closure-based access (`with_db`, `with_semantics`) keeps `RootDatabase` /
//! `Semantics` out of consumer signatures.
//!
//! ## Boundary discipline
//!
//! `with_db`/`with_semantics` are technically `pub` (Rust has no friend
//! crates). External misuse is policed by `clippy::disallowed_methods`
//! allow-listing **only** `rcm-graph` and `rcm-ide`. Other consumers must
//! use the typed views.
//!
//! **Exempt** from strict API leak rule for the closure arguments only.
//! Sync only. Async callers wrap in `tokio::task::spawn_blocking`.
```

### Public types

```rust
/// Opaque host: `RootDatabase` + `Vfs` + workspace metadata.
pub struct RaHost { /* db, vfs, fingerprint, local_crates */ }

#[derive(Copy, Clone, Eq, PartialEq, Hash, Debug)]
pub struct Fingerprint([u8; 32]);

#[non_exhaustive] pub struct VfsView<'h> { /* &'h Vfs */ }
#[non_exhaustive] pub struct CrateView { /* opaque */ }

#[derive(Debug, thiserror::Error)]
pub enum RaError {
    #[error("workspace load failed: {0}")] LoadWorkspace(String),
    #[error("vfs sync failed: {0}")]       VfsSync(String),
    #[error("file not in vfs: {0}")]       UnknownFile(PathBuf),
    #[error("io: {0}")]                    Io(#[from] std::io::Error),
}
```

### Public methods

```rust
impl RaHost {
    pub fn open_ide(path: &Path) -> Result<Self, RaError> { /* ... */ }
    pub fn open_hir(path: &Path) -> Result<Self, RaError> { /* ... */ }

    /// Allow-listed: `rcm-graph`, `rcm-ide` only.
    pub fn with_db<R>(&self, f: impl FnOnce(&ra_ap_ide::RootDatabase) -> R) -> R { /* ... */ }

    /// Allow-listed: `rcm-graph`, `rcm-ide` only.
    pub fn with_semantics<R>(
        &self,
        f: impl FnOnce(&ra_ap_hir::Semantics<'_, ra_ap_ide::RootDatabase>) -> R,
    ) -> R { /* ... */ }

    pub fn vfs(&self) -> VfsView<'_> { /* ... */ }
    pub fn local_crates(&self) -> &[CrateView] { /* ... */ }
    pub fn workspace_fingerprint(&self) -> Fingerprint { /* ... */ }
}

impl<'h> VfsView<'h> {
    pub fn path_for_file(&self, file: ra_ap_vfs::FileId) -> Option<PathBuf> { /* ... */ }
    pub fn file_for_path(&self, p: &Path) -> Option<ra_ap_vfs::FileId> { /* ... */ }
}

impl CrateView {
    pub fn name(&self) -> &str { /* ... */ }
    pub fn root_file(&self) -> &Path { /* ... */ }
    pub fn is_local(&self) -> bool { /* ... */ }
}
```

### Private internals

`load_workspace_at`, `Vfs` mutation, `AnalysisHost` wiring, `Semantics::new`, fingerprint hashing (Cargo.lock + workspace metadata), `prefill_caches`.

### API leak status

**Exempt.** Closure args name `ra_ap_ide::RootDatabase` / `ra_ap_hir::Semantics`; policed by `clippy::disallowed_methods`. Typed views are leak-clean.

### `Cargo.toml`

```toml
[package]
name = "rcm-ra-host"
version.workspace = true
edition.workspace = true

[dependencies]
rcm-ra-syntax    = { path = "../rcm-ra-syntax" }
ra_ap_ide        = { workspace = true }
ra_ap_hir        = { workspace = true }
ra_ap_load_cargo = { workspace = true }
ra_ap_vfs        = { workspace = true }
ra_ap_paths      = { workspace = true }
thiserror        = { workspace = true }
sha2             = { workspace = true }
```

No tokio. No features.

---

## `rcm-embedding`

### Crate-root doc

```rust
//! Sealed embedding boundary. `Embed` IS the abstraction; consumers receive
//! `Embedder = Arc<dyn Embed>`. One ONNX session process-wide, constructed
//! once in `rcm-server::main`.
//!
//! **Exempt** from strict API leak rule — `Embed` is the boundary. No
//! `fastembed`/`ort`/`ndarray`/CUDA types in the public surface.
//!
//! Sync core; async wrappers (`embed_batch_async`) are the only tokio entry.
#![warn(missing_docs)]
```

### Public types

```rust
mod sealed { pub trait Sealed {} }

pub trait Embed: sealed::Sealed + Send + Sync {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
    fn dimensions(&self) -> usize;
}

pub type Embedder = std::sync::Arc<dyn Embed>;

#[non_exhaustive] #[derive(Debug, Clone)]
pub enum ModelKind { AllMiniLmL6V2 }

#[non_exhaustive] #[derive(Debug, Clone, Copy)]
pub enum CudaPolicy { Auto, Force, Disabled }

#[derive(Debug, Clone)]
pub struct EmbedderConfig {
    model: ModelKind,
    cuda: CudaPolicy,
    cuda_mem_limit_bytes: u64, // default 5.5 GiB
}

impl EmbedderConfig {
    pub fn new(model: ModelKind) -> Self { /* ... */ }
    pub fn with_cuda(self, cuda: CudaPolicy) -> Self { /* ... */ }
    pub fn with_cuda_mem_limit_bytes(self, n: u64) -> Self { /* ... */ }
    pub fn model(&self) -> &ModelKind { /* ... */ }
    pub fn cuda(&self) -> CudaPolicy { /* ... */ }
    pub fn cuda_mem_limit_bytes(&self) -> u64 { /* ... */ }
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedError {
    #[error("model init failed: {0}")]         ModelInit(String),
    #[error("embed failed: {0}")]              EmbedFailed(String),
    #[error("no embedding produced")]          Empty,
    #[error("blocking task join failed: {0}")] Join(String),
    #[error("unsupported configuration: {0}")] Unsupported(String),
}
```

### Concrete embedders

```rust
#[cfg(feature = "embeddings")]
pub struct FastEmbedEmbedder { /* Mutex<TextEmbedding>, dim */ }
#[cfg(feature = "embeddings")]
impl FastEmbedEmbedder {
    pub fn new(cfg: EmbedderConfig) -> Result<Self, EmbedError> { /* ... */ }
}
#[cfg(feature = "embeddings")] impl sealed::Sealed for FastEmbedEmbedder {}
#[cfg(feature = "embeddings")] impl Embed for FastEmbedEmbedder { /* under Mutex */ }

// Compile-time guarantee.
#[cfg(feature = "embeddings")]
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<fastembed::TextEmbedding>();
};

#[cfg(feature = "test-fakes")]
pub struct DeterministicEmbedder { dim: usize }
#[cfg(feature = "test-fakes")]
impl DeterministicEmbedder { pub fn new(dim: usize) -> Self { /* ... */ } }
#[cfg(feature = "test-fakes")] impl sealed::Sealed for DeterministicEmbedder {}
#[cfg(feature = "test-fakes")]
impl Embed for DeterministicEmbedder {
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError> { /* hash->vec */ }
    fn dimensions(&self) -> usize { self.dim }
}
```

### Async wrappers (sole tokio entry point)

```rust
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
    .map_err(|j| EmbedError::Join(j.to_string()))?
}
```

### API leak status

**Exempt.** `Embed` + `Embedder` ARE the boundary.

### `Cargo.toml`

```toml
[package]
name = "rcm-embedding"
version.workspace = true
edition.workspace = true

[dependencies]
thiserror = { workspace = true }
tokio     = { workspace = true, features = ["rt", "macros"] }
fastembed = { workspace = true, optional = true }
ort       = { workspace = true, optional = true }

[features]
default    = ["embeddings"]
embeddings = ["dep:fastembed", "dep:ort"]
test-fakes = []

[dev-dependencies]
rcm-embedding = { path = ".", features = ["test-fakes"] }
```

---

## `rcm-server`

### Crate-root doc

```rust
//! Composition root + MCP binary. **Not** an SDK. Public library surface is
//! intentionally minimal (`Config` for integration tests; rest `pub(crate)`).
//!
//! Owns: rmcp `#[tool_router]` + `*Params` structs; `Config` parsing
//! (CLI > env > file > defaults); `SyncManager` shell with
//! `CancellationToken`; service construction order in `main`;
//! `similar_to_item` composition; `anyhow::Error -> rmcp::ErrorCode`
//! mapping; tracing setup; user-input safety (`read_file_content`
//! path-traversal guard).
```

### Public surface

```rust
pub use crate::config::Config; // integration tests only.
```

Everything else is `pub(crate)`. `*Params` structs (one per tool):

```rust
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct SearchParams { /* ... */ }
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct ReadFileContentParams { path: String, /* ... */ }
// ... one per tool, all pub(crate).
```

### `main()` pseudocode

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rcm_server::tracing::init()?;                       // 1. tracing
    let cfg = Config::load()?;                           // 2. config (CLI>env>file>defaults)
    let cancel = tokio_util::sync::CancellationToken::new(); // 3. SIGINT / stdin-EOF
    spawn_signal_handler(cancel.clone());

    let storage_root = cfg.storage_root();               // 4. one StorageRoot

    // 5. embedder — one ONNX session, process-wide
    let embedder: rcm_embedding::Embedder = std::sync::Arc::new(
        rcm_embedding::FastEmbedEmbedder::new(cfg.embedder_config())?
    );

    // 6. capability services (paths -> embedder -> services)
    let search = rcm_search::SearchService::open_lazy(storage_root.clone(), embedder.clone())?;
    let graph  = rcm_graph::GraphService::builder_lazy(storage_root.clone())
        .with_embedder(embedder.clone()).build()?;
    let ide    = rcm_ide::IdeService::open_lazy(storage_root.clone())?;

    // 7-8. SyncManager + AppState + rmcp service
    let sync_mgr = SyncManager::new(search.clone(), graph.clone(), ide.clone(), cancel.clone());
    let state = std::sync::Arc::new(AppState {
        cfg, search, graph, ide, sync_mgr, embedder, cancel: cancel.clone(),
    });
    let service = rmcp_router::build(state.clone());

    // 9. serve until cancellation, drain on 30s budget
    tokio::select! {
        r = service.serve_stdio() => { r? }
        _ = cancel.cancelled()    => { /* graceful */ }
    }

    // 10. drop order: state (drops sync_mgr) -> services -> embedder -> paths
    drop(state);
    Ok(())
}
```

### Error mapping, health, similar_to_item

```rust
pub(crate) fn map_err(e: anyhow::Error) -> rmcp::ErrorCode { /* ... */ }
pub(crate) async fn health(state: &AppState) -> HealthReport { /* ... */ }
pub(crate) async fn similar_to_item(
    state: &AppState, item: ItemRef,
) -> Result<Vec<Hit>, anyhow::Error> { /* ... */ }
```

`map_err` rules: `IndexBusy -> ResourceBusy`, `EmbedderUnavailable -> InvalidRequest`, downcast typed errors first; else `InternalError` with redacted message. `health` fans out to each service plus embedder presence (`Embedder = "absent"` when `embeddings` feature off). `similar_to_item` fetches item text via `IdeService`/graph node, embeds via `embed_batch_async`, fans out to `SearchService::knn` + `GraphService::semantic_overlaps`, RRF-fuses locally.

### `read_file_content` user-input safety

```rust
fn safe_path(workspace: &Path, requested: &str) -> Result<PathBuf, anyhow::Error> {
    let joined = workspace.join(requested);
    let canonical = joined.canonicalize()?;
    anyhow::ensure!(
        canonical.starts_with(workspace),
        "path traversal: requested path escapes the workspace"
    );
    Ok(canonical)
}
```

### Private internals

`AppState`, `SyncManager`, `rmcp_router`, `params`, `tracing`, `signal`, `health`.

### API leak status

**N/A — binary crate.** `#![warn(missing_docs)]` not enforced.

### `Cargo.toml`

```toml
[package]
name = "rcm-server"
version.workspace = true
edition.workspace = true

[[bin]]
name = "rust-code-mcp"
path = "src/main.rs"

[dependencies]
rcm-paths     = { path = "../rcm-paths" }
rcm-embedding = { path = "../rcm-embedding" }
rcm-search    = { path = "../rcm-search" }
rcm-graph     = { path = "../rcm-graph", features = ["semantic-overlaps"] }
rcm-ide       = { path = "../rcm-ide" }

rmcp               = { workspace = true }
schemars           = { workspace = true }
serde              = { workspace = true, features = ["derive"] }
serde_json         = { workspace = true }
tokio              = { workspace = true, features = ["macros", "rt-multi-thread", "signal"] }
tokio-util         = { workspace = true, features = ["rt"] }
tracing            = { workspace = true }
tracing-subscriber = { workspace = true, features = ["env-filter"] }
anyhow             = { workspace = true }
clap               = { workspace = true, features = ["derive", "env"] }
```

### Feature flags

```toml
[features]
default = []
# Always pulls `semantic-overlaps` from `rcm-graph` for a consistent
# capability set in MCP tooling.
```

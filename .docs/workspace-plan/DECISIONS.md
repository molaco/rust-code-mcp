# Workspace Plan — Decisions Baseline

This file is the single source of truth for the converged workspace design.
Every doc under `.docs/workspace-plan/target/` and `.docs/workspace-plan/implementation/` must align with these decisions. If a doc disagrees, the doc is wrong.

## Project context

- **Current state:** single-crate `file-search-mcp` (crate `rust-code-mcp-final`), Rust 2024 edition, MCP server over stdio (rmcp), indexes/analyzes Rust codebases.
- **Backends:** Tantivy (BM25), LanceDB (vector), fastembed/ort (ONNX embeddings, CUDA-capable), `ra_ap_*` (rust-analyzer crates), heed/LMDB (graph snapshots), sled (metadata cache).
- **Target:** 8 runtime crates + 1 tooling crate (xtask), capability-keyed split with infra leaves.
- **Reference docs:**
  - `/home/molaco/Documents/rust-code-mcp-final/.docs/ARCHITECTURE.md` — current single-crate architecture (read for context).
  - `/home/molaco/Documents/rust-code-mcp-final/.docs/workspace-investigation/` — 20-agent investigation that produced this design.
  - `/home/molaco/Documents/chart-refactor/.docs/rust-guidelines-final.md` — engineering guidelines this plan must follow.

## Workspace shape

```
rust-code-mcp/
├── Cargo.toml                    # virtual workspace manifest
├── Cargo.lock                    # committed (binary workspace)
├── rust-toolchain.toml           # pinned
├── deny.toml                     # cargo-deny config
└── crates/
    ├── rcm-paths/                # infra leaf — storage path resolution
    ├── rcm-ra-syntax/            # infra leaf — ra_ap_syntax re-exports (narrow whitelist)
    ├── rcm-ra-host/              # infra leaf — RootDatabase + Vfs lifecycle
    ├── rcm-embedding/            # infra leaf — Embed trait + FastEmbedEmbedder + DeterministicEmbedder
    ├── rcm-search/               # capability — corpus + retrieval
    ├── rcm-graph/                # capability — persisted hypergraph + audits
    ├── rcm-ide/                  # capability — live navigation
    ├── rcm-server/               # bin + lib — rmcp router, composition root
    └── xtask/                    # tooling — workspace automation, NOT runtime
```

## Dependency graph (DAG)

```
rcm-server  -> { rcm-search, rcm-graph, rcm-ide, rcm-paths, rcm-embedding }
rcm-search  -> { rcm-ra-syntax, rcm-embedding (feature-gated), rcm-paths }
rcm-graph   -> { rcm-ra-host, rcm-embedding (feature: semantic-overlaps), rcm-paths }
rcm-ide     -> { rcm-ra-host, rcm-paths }
rcm-ra-host -> { rcm-ra-syntax }
rcm-paths, rcm-ra-syntax, rcm-embedding -> {} (no workspace deps)

FORBIDDEN: rcm-search ↔ rcm-graph ↔ rcm-ide (capability crates never depend on each other)
xtask is excluded from runtime architecture policy checks.
```

## Crates — frozen contracts

### `rcm-paths` (infra leaf, ~200 LoC)

- `ProjectPaths` (private fields, `#[non_exhaustive]`)
- `StorageRoot { Xdg, Explicit(PathBuf) }`
- `PathError` (`thiserror`)
- `ProjectPaths::resolve(workspace: &Path, root: &StorageRoot) -> Result<Self, PathError>` is the **only** function that hashes a workspace path. Recipe is frozen as `sha256(canonicalize(workspace).as_encoded_bytes())`, lower-hex.
- Sync only. Workspace deps: none.
- Public-API leak rule: strict. Only deps allowed: `directories`, `sha2`, `thiserror`, `serde`.

### `rcm-ra-syntax` (infra leaf, ~50 LoC)

- Crate-root doc states the dual purpose: (1) version pinning for `ra_ap_syntax`; (2) keep `ra_ap_ide`/`ra_ap_hir` out of the chunker compile graph.
- Re-exports a **narrow whitelist** of `ra_ap_syntax` items only:
  `SourceFile, AstNode, AstToken, SyntaxKind, SyntaxNode, SyntaxToken, Edition, Parse, TextRange, TextSize, ast::{Fn, Struct, Enum, Trait, Impl, Module, Use, UseTree, Path, NameRef}`
- Adding to the whitelist requires a code-review touchpoint.
- **Documented exemption from the API leak rule.**
- Sync only. Workspace deps: none.

### `rcm-ra-host` (infra leaf, ~400 LoC)

- `RaHost` (opaque; wraps `RootDatabase` + `Vfs` + workspace metadata).
- Two preset constructors: `RaHost::open_ide(path: &Path) -> Result<Self, RaError>` (`no_deps=true`, no sysroot, prefill_caches=true), `RaHost::open_hir(path: &Path) -> Result<Self, RaError>` (`no_deps=false`, sysroot=Discover, all features, set_test=true).
- Public closure-based API: `with_db<R>(&self, f: impl FnOnce(&RootDatabase) -> R) -> R`, `with_semantics<R>(&self, f: impl FnOnce(&Semantics<'_, RootDatabase>) -> R) -> R`.
- Typed views: `vfs(&self) -> VfsView<'_>`, `local_crates(&self) -> &[CrateView]`, `workspace_fingerprint(&self) -> Fingerprint`.
- `RaError` (`thiserror`).
- **Boundary discipline:** `with_db`/`with_semantics` are technically `pub` (Rust has no friend crates). Clippy's `disallowed_methods` does NOT support caller-crate-specific allow-lists; the real enforcement is: the lint fires globally for these methods → `rcm-graph` and `rcm-ide` annotate their call sites with `#[allow(clippy::disallowed_methods)]` + justification → a CI grep rejects the same `#[allow]` outside those two crates. Discipline + lint + grep, not a clippy feature.
- **Documented exemption** from API leak rule for the closure-receivable types.
- Sync only. Async callers use `spawn_blocking`.

### `rcm-embedding` (infra leaf, ~500 LoC)

- Sealed `Embed` trait:
  ```rust
  mod sealed { pub trait Sealed {} }
  pub trait Embed: sealed::Sealed + Send + Sync {
      fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedError>;
      fn dimensions(&self) -> usize;
  }
  pub type Embedder = std::sync::Arc<dyn Embed>;
  ```
- `FastEmbedEmbedder` — production impl, gated behind feature `embeddings` (default-on).
- `DeterministicEmbedder { dim: usize }` — test impl (deterministic hash → vec), gated behind feature `test-fakes` (default-off, enabled in `dev-dependencies`).
- `EmbedderConfig { model: ModelKind, cuda: CudaPolicy, cuda_mem_limit_bytes: u64 }` (all fields private; constructor + `with_*` setters).
- Public sync API: `Embed::embed_batch`, `Embed::dimensions`. Async wrappers `embed_batch_async` at this layer (one place to spawn_blocking).
- `EmbedError` (`thiserror`): `ModelInit | EmbedFailed | Empty | Join | Unsupported`.
- Cargo wiring:
  ```toml
  [features]
  default = ["embeddings"]
  embeddings = ["dep:fastembed", "dep:ort"]
  test-fakes = []
  ```
- **Documented exemption** from API leak rule for `Embedder` / `Embed` (the boundary itself).
- Compile-time `Send + Sync` assertion on `TextEmbedding` (contingency: switch to single-thread worker if upstream changes).

### `rcm-search` (capability)

- Public API: `SearchService`, `CorpusWriter`, request/response DTOs (`#[non_exhaustive]`).
- Domain-named constructors: `SearchService::open(paths: &ProjectPaths, embedder: Embedder) -> Result<Self, SearchError>`, `CorpusWriter::open(...)`.
- Operation-scoped errors: `IndexError`, `SearchError`, `CorpusError` (separate enums; not a god-enum).
- Sans-I/O cores (sync, no tokio):
  - `chunker` — `(source: &[u8], ast: SourceFile) -> Vec<CodeChunk>`
  - `rrf` — pure RRF fusion over ranked lists
- Async only at service-method boundary.
- Owns: file walk, change detection (Merkle), metadata cache (sled), Tantivy schema + writer + reader, LanceDB connection, hybrid search, RRF, **chunking-context-only** AST extraction via `rcm-ra-syntax` (last-segment names, raw `use` paths, file-scoped symbol kinds — *not* resolved structure).
- Owns corpus security: `SensitiveFileFilter`, `SecretsScanner`, `IndexingMetrics`, `MemoryMonitor`.
- `#![warn(missing_docs)]`, strict-tier API leak rule.

### `rcm-graph` (capability)

- Public API: `GraphService` via builder pattern.
  ```rust
  impl GraphService {
      pub fn builder(paths: ProjectPaths, ra_host: Arc<RaHost>) -> GraphServiceBuilder;
  }
  pub struct GraphServiceBuilder { /* private */ }
  impl GraphServiceBuilder {
      pub fn with_embedder(mut self, e: Embedder) -> Self;
      pub fn build(self) -> Result<GraphService, BuildError>;
  }
  ```
- `semantic_overlaps` returns `Err(QueryError::EmbedderUnavailable)` if no embedder configured. All other queries work without one.
- Operation-scoped errors: `BuildError`, `QueryError`, `AuditError`.
- DTOs: `Node`, `Binding`, `Usage`, `CallEdge`, `OpenedSnapshot` (handle/ID-shaped), all `#[non_exhaustive]`. **Not `Serialize`** — only `rcm-server` serializes, mirroring rust-analyzer's pattern.
- Sans-I/O audit cores: each audit is `fn audit(snapshot: &OpenedSnapshot, opts: &AuditOpts) -> AuditReport`. The I/O wrapper opening the snapshot is separate.
- Owns: HIR extraction, heed/LMDB snapshot, all `OpenedSnapshot::*` queries, snapshot-only audits, AST-driven audits (`unsafe`, `channel`, `fn_body`), file-scoped structural tools (`get_dependencies`, `get_call_graph`, `analyze_complexity`) — these MOVE here from the parser-based implementations and route through HIR.
- `rcm-graph`'s dep on `rcm-embedding` is **feature-gated**: feature `semantic-overlaps` (default-off in `rcm-graph`'s own Cargo.toml; default-on in `rcm-server`'s dep on `rcm-graph`).
- `#![warn(missing_docs)]`, strict-tier API leak rule.

### `rcm-ide` (capability)

- Public API: `IdeService`, `DefinitionRequest`, `ReferenceRequest`, `SymbolSearchRequest`, `NavigationResponse`, `SourceLocation` (all `#[non_exhaustive]`).
- `IdeService::open(paths: ProjectPaths) -> Result<Self, IdeError>` — constructed in `main`, **not** `LazyLock`.
- Internal cache: `Mutex<HashMap<PathBuf, Arc<RaHost>>>` per-service-instance (no global statics).
- `IdeError` (`thiserror`).
- `#![warn(missing_docs)]`, strict-tier API leak rule.

### `rcm-server` (bin + thin lib)

- Owns: rmcp `#[tool_router]`, all `*Params` structs, `Config`, `SyncManager` shell with `CancellationToken`, `health` aggregator, `similar_to_item` composition, server-only tools (`read_file_content`, `clear_cache`, `index_codebase`, `build_hypergraph`, `health_check`).
- The only crate that:
  - Depends on `rmcp`.
  - Serializes domain types to JSON-RPC.
  - Constructs services (one place).
  - Uses `anyhow` for error-context-and-give-up paths.
- User-input safety lives here (e.g., `read_file_content` path-traversal check — distinct from corpus security in `rcm-search`).

### `xtask` (tooling, not runtime)

- Workspace automation: storage layout v2 migration (`xtask migrate-storage --dry-run` / `xtask migrate-storage`), bench harnesses, layout-version checks.
- May depend on any workspace crate.
- **Excluded** from `forbidden_dependency_check`, `cargo public-api` leak check, `missing_docs` warnings, `unreachable_pub` warnings.

## Cross-cutting policies

### Two-tier API leak rule

- **Strict tier (capability crates `rcm-search`, `rcm-graph`, `rcm-ide`, plus `rcm-paths`):** public signatures must NOT contain `tantivy::`, `lancedb::`, `arrow::`, `fastembed::`, `ort::`, `ra_ap_*`, `heed::`, `sled::`, `rmcp::`. Enforced via `cargo public-api` CI grep.
- **Exempt tier (named infra leaves `rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding`):** documented exemptions in crate-root docs; new exemptions require code review.

### Forbidden dependency rules (Phase 0, frozen)

```text
server      -> { search, graph, ide, paths, embedding }
search      -> { ra-syntax, embedding, paths }
graph       -> { ra-host, embedding (optional), paths }
ide         -> { ra-host, paths }
ra-host     -> { ra-syntax }
search ⊥ graph;  search ⊥ ide;  graph ⊥ ide;  graph ⊥ search
xtask -> excluded
```

### Async boundary (§12)

- `rcm-paths`, `rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding` (sync core), all sans-I/O cores → **no tokio dep**.
- `rcm-embedding`'s async wrappers (`embed_batch_async`) are the only place embedding hits tokio.
- Capability service methods are async; their domain cores are sync.
- `rcm-server` owns the runtime; `#[tokio::main]` lives there.

### Hidden singletons (§11 anti-pattern)

- No `LazyLock<Mutex<...>>` statics. The current `static SEMANTIC` and any equivalent are removed.
- Internal `Mutex<!Sync>` over `AnalysisHost`/`TextEmbedding` is allowed, but **per-service-instance**, never global.
- Services are constructed once in `rcm-server::main` and passed via `Arc`.

### Errors (§9)

- Operation-scoped `thiserror` enums per service.
- `#[from]` for adapter errors, preserving source chains.
- `anyhow` confined to `rcm-server` (and tests, doctests).
- No god-enum like `RcmError`.

### Service lifetime + invalidation

| Resource | Lifetime | Reload mechanism |
|---|---|---|
| `tantivy::IndexReader` | long-lived | `ArcSwap<IndexReader>` — open new, swap, drop old after grace |
| `tantivy::IndexWriter` | long-lived | `Mutex<IndexWriter>` — single-writer; reload returns `IndexBusy` if writing |
| `lancedb::Connection` | long-lived | `ArcSwap<Connection>` |
| `sled::Db` | construction-time | rebuild service to reload |
| `heed::Env` (per `OpenedSnapshot`) | per-snapshot | `ArcSwap<OpenedSnapshot>` — atomic `CURRENT` swap |
| `RaHost` (IDE cache) | long-lived | `ArcSwap<HashMap<PathBuf, Arc<RaHost>>>` |
| `RaHost` (graph build) | per-call | scoped to `build_and_persist` |
| `Embedder` | long-lived | constructed once in `main`, `Arc<dyn Embed>` everywhere |
| `SyncManager` task | long-lived | `CancellationToken`-driven shutdown |

### Invalidation triggers

`clear_cache` is **delete-then-invalidate**, not reload-then-rebuild. The
service handle methods are named accordingly:

- `SearchService::invalidate(workspace)` — drop in-memory `IndexReader` /
  `Connection` `ArcSwap` slots (so the next op reopens or rebuilds via
  fingerprint mismatch); if a writer is mid-batch, return `IndexBusy` and
  do nothing.
- `GraphService::invalidate(workspace)` — drop the in-memory
  `OpenedSnapshot`; the next graph query rebuilds via `build_and_persist`'s
  existing fingerprint-mismatch path.
- `IdeService::evict(workspace)` — remove the cached `RaHost` entry.

Reload (without delete) is a separate operation reserved for `SyncManager`
schema change detection — same code path, but no on-disk delete.

| Trigger | Action |
|---|---|
| `clear_cache(workspace, scope)` | (1) Refuse with `IndexBusy` if a writer is mid-batch. (2) Delete on-disk artifacts for the scope (`workspaces/<hash>/{keyword,vector,metadata,merkle,graph}/`). (3) Call `SearchService::invalidate(workspace)`, `GraphService::invalidate(workspace)`, `IdeService::evict(workspace)`. (4) Do NOT auto-reindex; the next operation that needs data triggers a transparent rebuild via existing fingerprint logic. |
| Workspace fingerprint mismatch on graph query | Transparent rebuild via `build_and_persist`. |
| `SyncManager::sync_now` reports schema change | `SearchService::reload(workspace)` — opens new handles, swaps via `ArcSwap`, drops old after grace period. No on-disk delete. |
| SIGINT / stdin EOF | `CancellationToken::cancel()` → `SyncManager::run` exits → drain in-flight tools (30s budget) → drop in topological order: `server` → capability crates → leaves. Tantivy/LanceDB writes complete; loop does not interrupt them. After timeout, log + abort. |
| `track_directory(new)` | No reload; `SearchService` opens lazily on first `search`. |

### Observability (§13)

- `tracing` for everything cross-crate.
- `IndexingMetrics` is the ONLY typed metric struct (per-batch summary in `rcm-search`); everything else is `tracing::Span` fields with `#[tracing::instrument]`.
- No long span guards across `.await`; use `.instrument(span)`.

### Workspace policy (§10, §15)

- `[workspace.package]` for shared metadata (edition `2024`, license, repo).
- `[workspace.dependencies]` pins every external dep in one place.
- `[workspace.lints.rust]`: `unsafe_op_in_unsafe_fn = "deny"`, `unreachable_pub = "warn"` (NOT `missing_docs` workspace-wide), `rust_2024_idioms = "warn"`.
- `[workspace.lints.clippy]`: `disallowed_methods = "warn"` (lists `RaHost::with_db` / `with_semantics`; clippy fires globally; only `rcm-graph`/`rcm-ide` annotate call sites with `#[allow(...)]`; CI grep enforces no other crate does), `pedantic = { level = "warn", priority = -1 }`.
- `cargo-deny` for advisories (RustSec), license allow-list, duplicate-crate detection.
- `Cargo.lock` committed.
- `rust-toolchain.toml` pinned.
- File-based modules (`parser.rs` + `parser/lexer.rs`); no `mod.rs` in new code.
- Per-crate `#![warn(missing_docs)]` on `rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-paths`, `rcm-embedding`. NOT on `rcm-server`, `rcm-ra-syntax`, `rcm-ra-host`, `xtask`.

### Public DTO discipline (§7)

- `#[non_exhaustive]` on every public struct and enum where future fields/variants are likely.
- All public struct fields private; constructors expose intent.
- No boolean parameters in public APIs; use small enums (e.g., `ForceReindex { Force, Allow }`).
- `#[must_use]` on `Result`-returning service methods.
- No `Deref` for ordinary newtypes.
- Public capability-crate DTOs are NOT `Serialize` — only `rcm-server` serializes (rust-analyzer's pattern).

### Tool ownership (final)

- **rcm-server (composition):** `read_file_content`, `clear_cache`, `index_codebase`, `build_hypergraph`, `health_check`, `similar_to_item`.
- **rcm-search:** `search`, `get_similar_code`.
- **rcm-ide:** `find_definition`, `find_references`.
- **rcm-graph:** every other tool — including `get_dependencies`, `get_call_graph`, `analyze_complexity` (snapshot-backed, NOT parser-based), `semantic_overlaps` (heed-local embedding cache), and ~37 audit/query tools.

## Implementation phases (high-level)

| Phase | Goal | Risk | Reversible? |
|---|---|---|---|
| 0 | Workspace skeleton, policy enforcement | low | yes |
| 1 | 8 crates as adapters over current modules | medium | yes |
| 2 | Hidden singleton removal | low | yes |
| 3 | Operation-scoped error split | low | yes |
| 4 | Service lifetime + invalidation contract | medium | partial |
| 5 | Embedding sealed trait + feature gate | low | yes |
| 6 | Parser scope reduction (chunking-context only) | **HIGH** — cold-start ordering changes | partial |
| 7 | Storage layout v2 migration via xtask | high (operational) | partial |

Each phase keeps `cargo build --workspace` green and the smoke checklist passing. A phase is "implementable" only after its predecessor's invariants are in CI.

## Smoke checklist (every phase)

After each phase, the following MCP tool calls must succeed against a fixture workspace:
- `index_codebase`
- `search` with a known keyword
- `find_definition` on a known symbol
- `find_references` on a known symbol
- `build_hypergraph`
- `who_calls`, `who_imports`, `workspace_stats`
- `get_dependencies`, `get_call_graph`, `analyze_complexity`
- `semantic_overlaps` (when `embeddings` feature is on)
- `similar_to_item`
- `clear_cache(workspace)` followed by `search` (verifies delete-then-invalidate, then lazy rebuild on next read via fingerprint mismatch — NOT reload, NOT auto-reindex)

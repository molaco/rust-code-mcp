# 01 — Architecture

## 1. Overview

`rust-code-mcp` is a Rust 2024 MCP server (rmcp over stdio) that indexes and analyzes Rust codebases via Tantivy BM25, LanceDB vectors, fastembed/ort embeddings, `ra_ap_*` rust-analyzer crates, heed/LMDB graph snapshots, and a sled metadata cache. The current single-crate `file-search-mcp` is being split into a virtual workspace of **8 runtime crates plus `xtask`**, capability-keyed (search / graph / ide) with infra leaves (paths / ra-syntax / ra-host / embedding) and a thin `rcm-server` composition root. **Not changing:** the on-disk storage layout (until phase 7), the MCP tool surface and JSON wire format, the `ra_ap_*` HIR-driven analysis, the `fastembed` AllMiniLML6V2 model, or the `#[tokio::main]` runtime model. The split is structural: every runtime behavior the binary already exhibits remains observable through the same MCP tools.

## 2. Workspace shape

| Crate | Type | Purpose | Owns | Does NOT own | Leak rule tier |
|---|---|---|---|---|---|
| `rcm-paths` | infra leaf | Storage path resolution + workspace-hash recipe | `ProjectPaths`, `StorageRoot`, `PathError`, `dir_hash` | I/O, async, anything ra/tantivy/lance | strict |
| `rcm-ra-syntax` | infra leaf | Pinned `ra_ap_syntax` whitelist re-exports for chunking-context AST | Whitelisted `SourceFile`/`AstNode`/`SyntaxKind`/etc. re-exports | `ra_ap_ide`, `ra_ap_hir`, semantic resolution | exempt |
| `rcm-ra-host` | infra leaf | `RootDatabase`+`Vfs` lifecycle behind closure API | `RaHost`, `open_ide`/`open_hir` presets, `with_db`/`with_semantics`, `VfsView`, `Fingerprint` | Queries, audits, IDE state caches | exempt |
| `rcm-embedding` | infra leaf | Sealed `Embed` trait + `FastEmbedEmbedder` + `DeterministicEmbedder` | `Embed`, `Embedder`, `EmbedderConfig`, async wrapper, ONNX/CUDA policy | Indexing, retrieval, chunk shapes | exempt |
| `rcm-search` | capability | Corpus + retrieval (BM25 + vector + RRF) | Walk, Merkle, sled cache, Tantivy schema/writer/reader, LanceDB conn, hybrid+RRF, `SensitiveFileFilter`, `SecretsScanner`, `IndexingMetrics`, `MemoryMonitor`, chunker | HIR, IDE navigation, structural facts | strict |
| `rcm-graph` | capability | Persisted hypergraph + audits + structural tools | HIR extraction, heed/LMDB snapshot + `CURRENT` swap, all `OpenedSnapshot::*` queries, AST-driven audits (`unsafe`/`channel`/`fn_body`), structural tools (`get_dependencies`/`get_call_graph`/`analyze_complexity`) | Tantivy, LanceDB, IDE handles, rmcp | strict |
| `rcm-ide` | capability | Live navigation (definition / references / symbol search) | `IdeService`, per-instance `Mutex<HashMap<PathBuf, Arc<RaHost>>>` | Persisted graph state, indexing pipelines | strict |
| `rcm-server` | bin + thin lib | rmcp router, `*Params`, `Config`, `SyncManager`, composition root | rmcp dep, JSON-RPC serialization, service construction, `anyhow`, server-only tools, user-input safety | Domain logic, retrieval, analysis algorithms | n/a (binary) |
| `xtask` | tooling | Workspace automation (storage migration, benches, layout-version checks) | `xtask migrate-storage`, bench harnesses | Anything runtime | excluded |

## 3. Dependency graph

```mermaid
graph TD
    server[rcm-server<br/>bin + thin lib]
    search[rcm-search]
    graph[rcm-graph]
    ide[rcm-ide]
    paths[rcm-paths]
    embedding[rcm-embedding]
    rahost[rcm-ra-host]
    rasyntax[rcm-ra-syntax]
    xtask[xtask<br/>excluded from policy]

    server --> search
    server --> graph
    server --> ide
    server --> paths
    server --> embedding

    search --> rasyntax
    search -. feature: embeddings .-> embedding
    search --> paths

    graph --> rahost
    graph -. feature: semantic-overlaps .-> embedding
    graph --> paths

    ide --> rahost
    ide --> paths

    rahost --> rasyntax

    xtask -.-> server
    xtask -.-> search
    xtask -.-> graph
    xtask -.-> ide

    search -. NO .- graph
    graph -. NO .- ide
    search -. NO .- ide
```

`rcm-paths`, `rcm-ra-syntax`, and `rcm-embedding` have **no workspace dependencies**. Capability crates **never depend on each other**; `rcm-server` is the only place they meet.

## 4. Two-tier API leak rule

**Strict tier** — `rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-paths`. Public signatures must NOT reference `tantivy::`, `lancedb::`, `arrow::`, `fastembed::`, `ort::`, `ra_ap_*`, `heed::`, `sled::`, or `rmcp::`. Enforcement: `cargo public-api` snapshot per crate, plus a CI grep job that fails if any forbidden prefix appears in the dumped public surface. New strict-tier types are `#[non_exhaustive]` with private fields.

**Exempt tier** — `rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding`. These crates *exist* to expose a controlled subset of an external dependency: their entire job is to leak `ra_ap_syntax::SyntaxKind`, `ra_ap_ide_db::RootDatabase`, or a sealed trait that hides ONNX behind a Rust signature. Banning leaks here would defeat the purpose. Exemption is documented in each crate's root docs and gated by code review for additions.

| Crate | Exempt symbols / surface |
|---|---|
| `rcm-ra-syntax` | Whitelisted `ra_ap_syntax` re-exports only (`SourceFile`, `SyntaxKind`, `ast::Fn`, …) |
| `rcm-ra-host` | Closure receiver types: `&RootDatabase`, `&Semantics<'_, RootDatabase>` |
| `rcm-embedding` | `Embed` sealed trait + `Embedder = Arc<dyn Embed>` (the boundary) |

## 5. Forbidden dependency policy

Frozen edge list (Phase 0):

| Source | Allowed targets | Forbidden targets |
|---|---|---|
| `rcm-server` | `rcm-search`, `rcm-graph`, `rcm-ide`, `rcm-paths`, `rcm-embedding` | none beyond runtime crates |
| `rcm-search` | `rcm-ra-syntax`, `rcm-embedding` (feature), `rcm-paths` | `rcm-graph`, `rcm-ide`, `rcm-ra-host` |
| `rcm-graph` | `rcm-ra-host`, `rcm-embedding` (feature `semantic-overlaps`), `rcm-paths` | `rcm-search`, `rcm-ide`, `rcm-ra-syntax` (direct) |
| `rcm-ide` | `rcm-ra-host`, `rcm-paths` | `rcm-search`, `rcm-graph`, `rcm-embedding`, `rcm-ra-syntax` (direct) |
| `rcm-ra-host` | `rcm-ra-syntax` | everything else |
| `rcm-paths`, `rcm-ra-syntax`, `rcm-embedding` | (none) | (any workspace dep) |
| `xtask` | any | (excluded from check) |

Enforcement: the existing `forbidden_dependency_check` MCP tool runs in CI against `Cargo.toml` manifests, and `cargo public-api` snapshots are diffed per crate. Both must be green before a phase advances. `xtask` is explicitly excluded from `forbidden_dependency_check`, `cargo public-api`, `missing_docs`, and `unreachable_pub`.

## 6. Anti-goals

This plan **rejects**, with reason:

- **No `core` / `common` / `shared` / `util` / `model` crate.** Capability-keyed splits (search / graph / ide) keep cohesion; a junk-drawer "common" crate becomes a hidden coupling channel that defeats the dependency policy.
- **No provider SDK types in capability crate public signatures.** The strict tier exists so swapping Tantivy/LanceDB/ort doesn't ripple through `rcm-server` or downstream consumers.
- **No `LazyLock<Mutex<...>>` global singletons.** The current `static SEMANTIC` is removed: globals hide ownership, defeat tests, and serialize unrelated callers through one mutex. Per-service-instance `Mutex<!Sync>` is allowed.
- **No async in domain cores.** `rcm-paths`, `rcm-ra-syntax`, `rcm-ra-host`, `rcm-embedding`'s sync API, chunker, RRF, audit cores: zero tokio. Async lives at service-method boundaries; `rcm-server` owns `#[tokio::main]`.
- **No boolean parameters in public APIs.** `index_codebase(force: bool)` becomes `ForceReindex { Force, Allow }`. Booleans at call sites are unreadable and silently flip behavior under refactor.
- **No `Serialize` on capability DTOs.** `Node`, `Binding`, `Usage`, etc. are not `Serialize`. Only `rcm-server` serializes (rust-analyzer's pattern); this prevents wire-format leakage into domain types and lets `rcm-server` own the JSON-RPC contract.
- **No parser duplication.** The chunker uses `rcm-ra-syntax` for *chunking-context-only* AST (last-segment names, raw use paths, file-scoped symbol kinds). All resolved structural facts (`get_dependencies`, `get_call_graph`, `analyze_complexity`) route through `rcm-graph`/HIR; we do not maintain two parsers.

## 7. Decision log

| # | Decision | Rationale |
|---|---|---|
| 1 | 8 runtime crates, not 5 (too coupled) or 11 (over-fragmented) | Capability boundaries match the three things the binary actually does (corpus, graph, IDE); leaves isolate the three external toolchains (`ra_ap_syntax`, `ra_ap_ide`/`ra_ap_hir`, `fastembed`/`ort`). |
| 2 | Capability-keyed split, not pipeline-keyed | Pipeline crates (parse / chunk / embed / index) would force every capability to depend on every stage, recreating the monolith via dependency edges. |
| 3 | `Embed` is a sealed trait, not a concrete type or open trait | Sealed lets `rcm-embedding` evolve impls without breaking downstreams; `Arc<dyn Embed>` at the boundary lets tests inject `DeterministicEmbedder` without a feature flip. |
| 4 | Closure-based `RaHost::with_db` boundary, not friend crates (Rust has none) | Closures keep `&RootDatabase` lifetime-scoped and prevent capability crates from stashing handles. Misuse policed by: `disallowed_methods` lint fires globally → `rcm-graph`/`rcm-ide` annotate call sites with `#[allow(...)]` + justification → CI grep rejects the same allow elsewhere. Clippy has no caller-crate-specific allow-list. |
| 5 | `rcm-ra-syntax` separate from `rcm-ra-host` | Keeps `ra_ap_ide`/`ra_ap_hir`'s heavy compile graph out of `rcm-search`; chunker only needs syntax. |
| 6 | Two `RaHost` presets (`open_ide` / `open_hir`), not flag soup | Presets encode the only two configurations we actually use (`no_deps=true` for IDE, `no_deps=false`+sysroot for graph build); booleans-in-public-API is an anti-goal. |
| 7 | `rcm-graph`'s embedder dep is feature-gated (`semantic-overlaps`) | Semantic overlap is the only graph query needing embeddings; gating keeps graph builds light when embeddings are off and makes `EmbedderUnavailable` an explicit error path. |
| 8 | DTOs are NOT `Serialize`; `rcm-server` serializes | Mirrors rust-analyzer; prevents JSON-RPC wire format from constraining domain types. |
| 9 | Operation-scoped `thiserror` enums, not a god-`RcmError` | Each error enum carries only the variants its operation can produce; `#[from]` preserves chains; `anyhow` is confined to `rcm-server`. |
| 10 | `IdeService` constructed in `main`, not via `LazyLock` | Removes hidden global state; cache is `Mutex<HashMap<_, Arc<RaHost>>>` per service instance. |
| 11 | Structural tools (`get_dependencies`, `get_call_graph`, `analyze_complexity`) move to `rcm-graph` | These are HIR-resolved facts; routing them through the parser was a duplication of work and source of drift. |
| 12 | `xtask` excluded from runtime policy checks | Tooling can legitimately depend on every crate and use the full SDK surface; subjecting it to the leak rule would forbid useful automation without making the runtime safer. |

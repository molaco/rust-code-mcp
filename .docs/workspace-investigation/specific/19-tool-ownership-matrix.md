# 19 — Tool Ownership Matrix (Clean-Slate)

## Proposed crate split (6 crates)

A pragmatic split that follows the data-store boundary (each crate owns one persistent backend) plus thin orchestration on top. Tools live in the lowest crate that holds their primary data source; cross-store tools compose at the server.

| # | Crate | Owns | Backend |
|---|-------|------|---------|
| 1 | `rcm-core` | `Config`, `ProjectPaths`, error types, security filter, metrics, XDG path resolution. | none |
| 2 | `rcm-keyword` | Tantivy BM25 + chunker + Merkle change detection + `MetadataCache` (sled). Pure keyword index. | Tantivy + sled |
| 3 | `rcm-vector` | LanceDB store + `EmbeddingGenerator` (fastembed) + hybrid RRF fusion logic. | LanceDB + ONNX |
| 4 | `rcm-semantic` | rust-analyzer-driven `SemanticService` (`AnalysisHost`+`Vfs`), `RustParser`, position-based queries (goto/refs/symbol-search), `CallGraph` (per-file). | in-memory RA |
| 5 | `rcm-graph` | HIR extractor, LMDB hypergraph snapshot, all snapshot-only queries + audits, AST-driven audits. | heed/LMDB + RA |
| 6 | `rcm-server` | rmcp `ToolRouter`, all `*Params` structs, `SyncManager`, `HealthMonitor`, `BackupManager`, server-level composition. | none (orchestrator) |

`rcm-keyword` owns the indexing pipeline (file walk → chunk → Tantivy + LanceDB upsert) because the Merkle/cache state lives there; `rcm-vector` exposes the upsert API and search backend, and `rcm-keyword` depends on it. Hybrid search composes BM25 + vector inside `rcm-vector` (it owns the embedding generator); `rcm-server` calls into it.

## Tools grouped by owning crate

### `rcm-server` (router-only, 5 tools)
Pure operational tools or composition primitives — no data of their own.

| Tool | Why server |
|---|---|
| `health_check` | Probes BM25 + vector + Merkle across crates. Composition. |
| `clear_cache` | Deletes paths owned by 3 different crates (Tantivy, vector, cache, snapshot). |
| `index_codebase` | Drives keyword-indexer, registers with `SyncManager`, may delete Merkle. |
| `read_file_content` | Trivial fs read with binary/size guards; no backend. |
| `build_hypergraph` | Triggers `rcm-graph` build but is the only "manual ingest" command, mirrors `index_codebase`. |

### `rcm-keyword` (4 tools)
| Tool | Notes |
|---|---|
| `search` | BM25 leg lives here; hybrid composition done in-crate via `rcm-vector` dep. |
| (indexing internals consumed by `index_codebase`) | not user-facing. |

### `rcm-vector` (1 tool)
| Tool | Notes |
|---|---|
| `get_similar_code` | Pure LanceDB cosine search over chunks. No BM25 leg. |

### `rcm-semantic` (5 tools)
File-and-position driven, backed by the live `AnalysisHost` (no snapshot).

- `find_definition`
- `find_references`
- `get_call_graph` *(per-file `RustParser` call graph — distinct from snapshot `call_graph`)*
- `get_dependencies` *(per-file imports via `RustParser`)*
- `analyze_complexity` *(per-file syntactic metrics)*

### `rcm-graph` (37 tools)
All snapshot-backed queries and audits. Grouped for readability:

**Lookup / structure (8)**
`module_tree`, `workspace_stats`, `function_signature`, `enum_variants`, `item_attributes`, `items_with_attribute`, `functions_with_filter`, `overlaps`

**Imports / exports / re-exports (6)**
`get_imports`, `get_exports`, `get_reexports`, `get_declared_reexports`, `who_imports`, `re_export_chain`

**Usages (3)**
`who_uses`, `who_uses_summary`, `crate_edges`

**Call graph (snapshot, 6)**
`who_calls`, `calls_from`, `call_graph` *(snapshot-recursive)*, `callers_in_crate`, `recursive_callers_count`, `recursion_check`

**Dead-pub / dependency hygiene (4)**
`dead_pub_in_crate`, `dead_pub_report`, `crate_dependency_metric`, `forbidden_dependency_check`

**Audits — snapshot-only (4)**
`derive_audit`, `missing_docs_audit`, `mut_static_audit`, `pub_use_pub_type_audit`

**Audits — AST-driven (3)**
`unsafe_audit`, `channel_capacity_audit`, `fn_body_audit`

**Semantic similarity over snapshot items (3)**
`similar_to_item`, `semantic_overlaps` — *see Composition section.*

Total: 5 + 4 + 1 + 5 + 37 = **52 tools** (matches "~50" in `tools.md`).

## Controversial assignments

These tools have a defensible home in two or more crates. The assignment plus rationale:

1. **`get_call_graph` vs `call_graph` (snapshot)** — both produce call-graph output. `get_call_graph` is the *file-scoped* tree-sitter / `RustParser` version (lives in `rcm-semantic`); `call_graph` is the *workspace-scoped* hypergraph traversal (lives in `rcm-graph`). They share a name prefix but no code. Keeping them in different crates is correct because `get_call_graph` does not require the snapshot to exist; merge candidate — see below.

2. **`get_dependencies`** — could go to `rcm-graph` (which already has `get_imports`) but currently it is a per-file syntactic import dump that does not need the workspace HIR to be loaded. Putting it in `rcm-semantic` keeps it usable on un-indexed projects. Strong candidate for deletion (see below).

3. **`search`** — owns the *hybrid* response shape (BM25 + vector via RRF). Could live in `rcm-vector` (which owns embeddings and RRF fusion logic) or `rcm-keyword` (which owns BM25 and the rebuild-on-stale recovery path). Assigning it to `rcm-keyword` because the stale-index recovery loop is the load-bearing complexity and it is keyword-index state. `rcm-keyword` depends on `rcm-vector` for the dense leg.

4. **`get_similar_code`** — a thin wrapper on `VectorStore::search`. Could be folded into `search` (with a `dense_only: bool` flag) but kept separate because it serves a distinct UX (semantic-only, no keyword) and skips RRF. Stays in `rcm-vector`.

5. **`semantic_overlaps` / `similar_to_item`** — these blend snapshot identity (`NodeId` → qualified name → file/span enrichment) with embedding-cosine ranking against the LanceDB chunk vectors. Two-store tool. Assigned to `rcm-graph` because the *input* and *enrichment* are snapshot-side; it depends on `rcm-vector` for the cosine call. Alternative: server-level composition (see below) — defensible but creates a third layer of `*Params` plumbing.

6. **`build_hypergraph`** — naturally lives in `rcm-graph`, but is invoked the same way as `index_codebase` (a manual ingest tool). Placed in `rcm-server` to keep all "ingest" verbs in one place; it forwards to `rcm-graph::build_and_persist` via `spawn_blocking`. Defensible either way.

7. **`forbidden_dependency_check`** vs **`crate_dependency_metric`** — both compute over `crate_edges`. They are policy/metric variants of the same underlying query. Both stay in `rcm-graph`; merge candidate (below).

8. **`pub_use_pub_type_audit`** — a re-export hygiene audit. It is snapshot-only but conceptually overlaps with the `re_export_chain` query. Stays in `rcm-graph` audits cluster.

## Delete or merge candidates

- **`get_dependencies` — DELETE.** Effectively `get_imports` restricted to a single file via the parser path. Once the snapshot exists, `get_imports` is superior (it knows resolved targets). Keep `get_imports`.
- **`get_call_graph` (per-file) — MERGE into `call_graph`.** Add an optional `file: Option<PathBuf>` parameter to the snapshot `call_graph` and let it short-circuit to a file-scoped traversal when the snapshot is present. The parser-only version is a fallback that complicates docs.
- **`dead_pub_in_crate` + `dead_pub_report` — KEEP BOTH** but split clearly: one is `crate=Some`, the other is `crate=None`. Could be unified behind a single tool with optional crate filter; mild merge candidate.
- **`get_reexports` + `get_declared_reexports` — MERGE.** Add a `declared_only: bool` flag to `get_reexports`. Two tools for one boolean is API noise.
- **`crate_dependency_metric` + `crate_edges` — KEEP SEPARATE** but consider that `crate_dependency_metric` is just a reduction over `crate_edges`. Document the relationship; do not merge (different response shapes are useful).
- **`who_uses_summary` — MERGE into `who_uses`** with a `group_by_file: bool` flag. Same data, different aggregation.
- **`recursive_callers_count` — KEEP** (cheap counter that avoids paging the full BFS).
- **`similar_to_item` + `semantic_overlaps` — KEEP BOTH** (distinct semantics: one-vs-all vs all-vs-all).

Net: ~5 deletions/merges drop the surface from 52 to ~47, with no capability loss.

## Server-level composition required

These tools genuinely span two backends and the *router* must orchestrate them — a single owner crate would force a circular dep:

| Tool | Composition |
|---|---|
| `health_check` | Probes BM25 (`rcm-keyword`) + vector (`rcm-vector`) + Merkle (`rcm-keyword`) + snapshot health (`rcm-graph`). Aggregates one JSON. |
| `clear_cache` | Deletes Tantivy + vector + sled cache + Merkle file + LMDB graph dir. Touches every store. |
| `index_codebase` | Drives keyword-indexer (`rcm-keyword`), upserts vectors (`rcm-vector`), registers with `SyncManager`, optionally clears Merkle. |
| `build_hypergraph` | Calls `rcm-graph::build_and_persist` via `spawn_blocking`; trivially composable but lives at server for symmetry with `index_codebase`. |
| `search` | Hybrid BM25 + vector + RRF. Composition done inside `rcm-keyword` (which depends on `rcm-vector`); router only deserializes params. |
| `semantic_overlaps`, `similar_to_item` | Snapshot enrichment (`rcm-graph`) + LanceDB cosine (`rcm-vector`). Done inside `rcm-graph` via dep on `rcm-vector`. |

The first four require true server-level orchestration (multiple crate APIs called from the handler). The last three compose *inside* a domain crate via direct dependency.

## Crate dependency DAG

```
rcm-core ──► rcm-keyword ──► rcm-vector
       └──► rcm-semantic
       └──► rcm-graph ──► rcm-vector
                     └──► rcm-semantic (for AST audits)
       └──► rcm-server ──► {keyword, vector, semantic, graph}
```

No cycles. `rcm-vector` is a leaf for hybrid composition; `rcm-semantic` is shared between the file-position tools and the AST-driven audits in `rcm-graph`.

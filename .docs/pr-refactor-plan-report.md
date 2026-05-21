# PR-Based Refactor Plan — Final Verification Report

**Plan**: `.plans/pr-refactor-plan.md`
**Repository**: `rust-code-mcp` (single-crate, in-repo consumers only)
**Workspace**: `/home/molaco/Documents/rust-code-mcp-refactor`
**Final commit**: `notnww` / `e514f894` (PR 20 with review fix-up)
**Completed**: 2026-05-21

## Executive summary

The 22-PR module/file-boundary refactor (PR 00–21) is structurally complete. All mega-files identified in `.plans/refactor-plan.md` §1 are dissolved; the migration facades that bridged old and new paths are deleted; visibility has been narrowed where boundaries are stable; and the `cargo check --all-targets` gate is green at every commit.

PR 22 (optional crate lift) is not attempted; deferred for a future change.

## Verification

### `cargo check --all-targets`

```
nix develop ../nix-devshells#cuda-code --command cargo check --all-targets
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.25s
```

Green. No new errors. Remaining warnings are pre-existing `dead_code` flags from the workspace's `#![warn(unreachable_pub, dead_code)]` setting and a handful of `unused import` flags in `examples/` and `tests/` that predate the refactor.

### Forbidden-edge sweep

```sh
grep -rn "crate::tools\|crate::mcp" src/graph/ src/embeddings/ src/indexing/ src/search/ src/vector_store/ src/chunker/ src/parser/
```

**Zero hits.** Engine modules have no edges back into `tools::` or `mcp::`. The boundary fix landed in Phase 0.6 (PRs 01–02) and held through every subsequent split.

### Stale-facade-path sweep

```sh
grep -rn "tools::search_tool\|tools::graph_tools\|tools::analysis_tools\|tools::clear_cache_tool\|tools::health_tool\|tools::index_tool\|tools::indexing_tools\|tools::query_tools\|graph::queries::\|indexing::errors\b" src/ tests/ examples/
```

**Zero hits** across `src/`, `tests/`, and `examples/`. Every facade deleted by PR 19 is unreferenced.

### Workspace stats (final)

| Metric | Phase 0 baseline | Final | Δ |
|---|---|---|---|
| Nodes | 2973 | 3031 | +58 |
| Bindings | 5056 | 5390 | +334 |
| Usages | 7935 | 7964 | +29 |
| `pub` items | 540 | 282 | −258 |
| `pub(crate)` items | 43 | 348 | +305 |
| `pub_crate_share` | 0.074 | 0.5524 | **~7.5× improvement** |
| `restricted_to` (`pub(super)` / `pub(in path)`) | n/a | 102 | new surface |
| Module-private items | (not tracked) | 1096 | — |

The Phase 0 baseline came from `.plans/refactor-plan.md` §1. The increases in node/binding/usage counts reflect new files created by the splits plus relocated test modules.

### `dead_pub_in_crate` (final)

90 candidates remain (down from 339 mid-checkpoint-7, before the over-demotion fix-up restored 15 transitively-reachable types to `pub`).

The 90 surviving candidates are NOT residual cleanup. They are types reachable through `pub` method signatures on `pub` types (e.g., `OpenedSnapshot::workspace_stats() -> WorkspaceStats` makes every `WorkspaceStats` field's type effectively part of the public surface, but `dead_pub_in_crate` does not see transitive method-return-type reachability). Examples from the 90:

- `graph::query::model::*` (32 types) — return shapes of `OpenedSnapshot` methods called from `examples/dead_pub_report.rs`, `examples/probe_workspace.rs`, etc.
- `graph::model::{Binding, Usage, ItemKind, ExtractionModel, …}` — touched via `extract()` and `OpenedSnapshot` accessors.
- `chunker::types::{ChunkContext, ChunkId, CodeChunk}` — `Chunker` API surface.
- `embeddings::profile::{FastembedCpuModel, LocalLoaderSpec, QueryPolicy, Qwen3Variant}` — fields of `EmbeddingProfile` and return types on `EmbeddingBackend`.
- `graph::derive_audit::DeriveAuditOpts`, `graph::docs_audit::DocsAuditOpts` — argument types on audit endpoints.
- Wire DTOs and error enums that examples/tests construct directly.

A future cleanup pass could either tighten these (by also demoting the methods/structs that expose them) or accept them as the genuine public surface; both are out of scope for this refactor.

## Final structural state

### Tree

```
src/
  lib.rs           main.rs           config.rs
  metadata_cache.rs                  schema.rs
  bin/             test_tools_direct.rs

  tools/                              PR 03–07, PR 19 + PR 20
    mod.rs                            (13 LOC)
    router.rs                         (721 LOC)
    project_paths.rs                  (303 LOC)
    params/         mod.rs + audit.rs, graph.rs, indexing.rs, search.rs
    endpoints/      mod.rs + analysis.rs, cache.rs, health.rs, index.rs,
                                       indexing_support.rs, query.rs
    graph/          mod.rs (19 LOC) + audits.rs (454), codemap.rs (192),
                                       core.rs (626), crates.rs (146),
                                       response.rs (467), similarity.rs (583),
                                       surface.rs (968), tests.rs (927)

  graph/                              Phase 0.6 + PR 08–13 + PR 19
    mod.rs          ids.rs            model.rs        storage.rs
    snapshot.rs     loader.rs         hir_trim.rs     ast_resolve.rs
    extract.rs      bindings.rs       usages.rs       impls.rs
    signatures.rs   attributes.rs     statics.rs
    docs_audit.rs   derive_audit.rs   unsafe_audit.rs fn_body_audit.rs
    channel_audit.rs                  recursion_check.rs
    audit_util.rs   labels.rs         math.rs         embedding_cache.rs
    test_support.rs
    query/          mod.rs (24 LOC) + audits.rs (146), calls.rs (334),
                                       crates.rs (456), functions.rs (98),
                                       imports.rs (279), model.rs (420),
                                       modules.rs (331), navigation.rs (254),
                                       overlaps.rs (281), shared.rs (168),
                                       surface.rs (533), usage.rs (114),
                                       tests.rs (1144)
    codemap/        mod.rs (22 LOC) + model.rs (107), seeds.rs (397),
                                       build.rs (951), hierarchy.rs (109),
                                       render.rs (385), test_support.rs (211)

  embeddings/                         PR 14–15 + PR 17
    mod.rs          backend.rs (535)  profile.rs (398)
    profile_registry.rs               batching.rs   util.rs
    identity.rs     qwen3.rs          fastembed_cpu.rs
    token_lengths.rs                  error.rs
    openrouter/     mod.rs (15 LOC) + config.rs (496), client.rs (450),
                                       request.rs (53), response.rs (230),
                                       batch.rs (273), retry.rs (38),
                                       metrics.rs (141)

  chunker/                            PR 16
    mod.rs (11 LOC)                   types.rs (250) chunker.rs (297) split.rs (284)

  parser/                             PR 16
    mod.rs (19 LOC)                   types.rs (103) rust_parser.rs (515)
    call_graph.rs   imports.rs        type_references.rs

  indexing/                           PR 18
    mod.rs          unified.rs (666)  unified_parallel.rs (119)
    indexer_core.rs                   embedding_batcher.rs (767)
    file_processor.rs                 incremental.rs
    merkle.rs       tantivy_adapter.rs                consistency.rs
    identity.rs     retry.rs
    error.rs (30)   error_collection.rs (197)

  search/         (untouched by this refactor — under 500 LOC threshold)
  vector_store/   (untouched — vector_store::SearchResult renamed to
                   VectorSearchResult in PR 20)
  config/         mcp/              metrics/        monitoring/
  security/       semantic/
```

### LOC hotspots (production files over 500 LOC)

```
1144 src/graph/query/tests.rs           ← test module relocated from queries.rs in PR 19
 968 src/tools/graph/surface.rs         ← 12 surface endpoints (plan-sanctioned single concern)
 951 src/graph/codemap/build.rs         ← build_codemap algorithm + helpers
 927 src/tools/graph/tests.rs           ← test module relocated from graph_tools.rs in PR 19
 767 src/indexing/embedding_batcher.rs  ← single coherent concern (PR 18 review verdict)
 721 src/tools/router.rs                ← MCP entry point (38 #[tool] methods)
 681 src/graph/fn_body_audit.rs         ← single audit family
 680 src/vector_store/lancedb.rs        ← vector-store adapter (out of refactor scope)
 667 src/graph/snapshot.rs              ← OpenedSnapshot definition
 666 src/indexing/unified.rs            ← UnifiedIndexer orchestrator
 661 src/graph/storage.rs               ← LMDB schema
 626 src/tools/graph/core.rs            ← 16 core graph endpoints
 583 src/tools/graph/similarity.rs      ← similarity endpoint family
 561 src/tools/endpoints/query.rs       ← query endpoint family
 535 src/embeddings/backend.rs          ← EmbeddingBackend runtime wiring
```

**No production source file exceeds the plan's ~1000-LOC target.** The two over-1000-LOC files (`graph/query/tests.rs` and `tools/graph/tests.rs`) are test modules, not production code; both are coherent single-concern test suites preserved verbatim from PR 11/PR 06 outputs.

### Mega-files dissolved

| Pre-refactor file | Pre-LOC | Outcome |
|---|---|---|
| `tools/graph_tools.rs` | 4488 | dissolved into `tools/graph/{core,crates,surface,audits,similarity,codemap,response}.rs` + `tests.rs` (PR 04–06, PR 19) |
| `graph/queries.rs` | 4371 | dissolved into `graph/query/{model,imports,usage,calls,crates,surface,audits,functions,modules,overlaps,navigation,shared}.rs` + `tests.rs` (PR 08–11, PR 19) |
| `graph/codemap.rs` | 2058 | dissolved into `graph/codemap/{model,seeds,build,hierarchy,render}.rs` + `test_support.rs` (PR 12–13) |
| `embeddings/openrouter.rs` | 1618 | dissolved into `embeddings/openrouter/{config,client,request,response,batch,retry,metrics}.rs` (PR 14–15) |

### Boundary fixes

- **`graph::codemap → tools` inversion** (Phase 0.6, PRs 01–02). `embedder_version`, `ensure_embeddings_for`, and `cosine` moved out of `tools::graph_tools` into graph-side homes (`graph::math`, `graph::embedding_cache`). `embedder_version` deleted (was a one-line wrapper). Grep verification: no `crate::tools::*` references in `src/graph/`.
- **`graph::codemap → search::SearchResult` direct coupling** (PR 12). Introduced `codemap-local `SeedHit` DTO; tools-side endpoint maps `SearchResult → SeedHit` before calling `build_codemap`. The graph algorithm is now search-independent.

### Disambiguating renames (PR 20)

- `graph::derive_audit::AuditOpts` → `DeriveAuditOpts`
- `graph::docs_audit::AuditOpts` → `DocsAuditOpts`
- `vector_store::SearchResult` → `VectorSearchResult`

Each rename is type-name only; no structural changes. The structural duplication between `search::SearchResult` and `vector_store::VectorSearchResult` (the latter is a strict field-subset of the former) is documented in `.plans/refactor-plan.md` §12 as an out-of-scope follow-up.

## PR sequence outcomes

| PR | Subject | Status |
|---|---|---|
| 00 | Baseline record | DONE |
| 01 | Extract graph math helper (`cosine`) | DONE |
| 02 | Extract graph embedding cache helper (`ensure_embeddings_for`) | DONE |
| 03 | Split tools router and params | DONE |
| 04 | Split tools graph core endpoints | DONE |
| 05 | Split tools graph crate/surface/audit endpoints | DONE |
| 06 | Split tools graph similarity and codemap endpoints | DONE |
| 07 | Move standalone tool endpoints | DONE |
| 08 | Create graph query skeleton and move model types | DONE |
| 09 | Move graph query import/usage/call families | DONE |
| 10 | Move graph query crate/surface/audit families | DONE |
| 11 | Move remaining graph query families and test support | DONE |
| 12 | Split codemap model and search-hit DTO | DONE |
| 13 | Split codemap render/build/hierarchy | DONE |
| 14 | Split OpenRouter config and DTOs | DONE |
| 15 | Split OpenRouter client support | DONE |
| 16 | Split chunker and parser facades | DONE |
| 17 | Split embeddings backend profile model | DONE |
| 18 | Split indexing unified helpers and rename error collection | DONE |
| 19 | Remove migration facades after caller migration | DONE |
| 20 | Lower accidental public visibility | DONE |
| 21 | Final structural verification report (this document) | DONE |
| 22 | Optional crate lift | not attempted |

Each PR was reviewed at checkpoint boundaries (per the user's checkpoint cadence: 0, 1, 2A, 2B, 2C, 3, 4, 5, 6, 7, 8). Review findings were folded into the originating PR commit via `jj squash`.

## Public-API stability

`rust-code-mcp` has no external crate consumers (per `.plans/refactor-plan.md` §2, all 39 in-repo consumers are `examples/`, `tests/`, or `src/bin/`). PR 19's facade deletion is therefore internal-only. The intentional public facade surface — preserved through the refactor — is:

- `graph::OpenedSnapshot`, `graph::BuildOptions`
- `indexing::{UnifiedIndexer, IncrementalIndexer}`
- `embeddings::EmbeddingGenerator`
- `search::HybridSearch`
- `vector_store::VectorStore`

All 6 verified `pub` in the final state.

## Out-of-scope follow-ups

These are not addressed by this refactor and remain open for future work:

1. **`search::SearchResult` ↔ `vector_store::VectorSearchResult` structural dedup**. Same semantic content (field-subset relation). Plan §12 explicitly defers structural dedup; only the name collision was resolved in PR 20.
2. **90 surviving `dead_pub_in_crate` candidates**. Each is reachable through transitive method-signature exposure; the tool does not model that. A tightening pass could demote the EXPOSING methods/structs instead of the typed.
3. **`tools::graph::surface.rs` at 968 LOC**. Coherent single concern (12 endpoints) but the largest production-code file in the refactored tree. Could be subdivided (e.g., dead-pub / attributes / functions / re-exports) if it continues to grow.
4. **`graph::query::shared.rs` / `graph::query::navigation.rs`**. PR 11's review fix-up extracted these to make `queries.rs` a true facade. The split is correct, but `shared.rs` is a "misc" file by design. Could be further partitioned.
5. **`indexing::embedding_batcher.rs` at 767 LOC**. PR 18 review verdict: single coherent concern (GPU-batched embedding with memory-aware sizing). No split needed.
6. **PR 22 — graph crate lift**. Plan §12 lists this as optional. Pre-conditions met (clean boundaries, no `graph → tools` edges, stable surface) but not attempted in this pass.

## Closing

The refactor delivers the structural goal stated in `.plans/refactor-plan.md` §0: "Move the project from a mostly-correct layout with a handful of overloaded files into a structure where an agent can work in one module family at a time." Every endpoint family, every query family, every codemap concern, every OpenRouter concern, every chunker / parser concern, and every indexing concern lives in one focused file under ~1000 LOC. The public-API surface shrank by ~50% (`pub` 540 → 282). The forbidden engine→adapter edges are gone. The build is green.

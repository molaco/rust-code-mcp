# Refactor Plan 56: Directory, File, and Boundary Restructure

Status: draft
Scope: read-only plan; no code changes implied by this file
Basis: rust-code-mcp boundary/import/export analysis on 2026-05-16

## 0. Goal

Move the project from a mostly-correct shallow layout with several overloaded
files into a clearer module structure that agents can work in locally.

The target is not a full rewrite. The current top-level layout is mostly good:

```text
src/<module>/<file>.rs
```

The problem is concentrated in a few large mixed files and broad facade modules:

- `src/tools/graph_tools.rs`
- `src/graph/queries.rs`
- `src/graph/codemap.rs`
- `src/chunker/mod.rs`
- `src/search/mod.rs`
- `src/parser/mod.rs`
- `src/embeddings/mod.rs`

This plan follows the `THEORY_3.md` workflow:

- Category theory decides where code lives: objects, morphisms, boundaries,
  quotients, projections, lifts.
- HoTT/type theory decides what shape code has: contracts, equivalences,
  products, sums, adapters.
- Primitive operations are `Move`, `Split/Merge`, and `Lift/Lower`.

## 1. Evidence Summary

rust-code-mcp snapshot:

```text
node_count: 2579
binding_count: 4390
usage_count: 6559
```

Workspace stats:

```text
pub: 508
pub(crate): 25
pub_crate_share: ~0.047
```

Interpretation:

- Encapsulation is weak. Many items are `pub` where `pub(crate)` is likely
  enough.
- Public surface cleanup should happen after structural splits, not before.

Largest files:

```text
3937 src/tools/graph_tools.rs
3604 src/graph/queries.rs
2051 src/graph/codemap.rs
 805 src/chunker/mod.rs
 741 src/indexing/unified.rs
 726 src/tools/search_tool_router.rs
 680 src/vector_store/lancedb.rs
 667 src/graph/snapshot.rs
 657 src/indexing/embedding_batcher.rs
 654 src/graph/storage.rs
 621 src/parser/mod.rs
 536 src/tools/search_tool.rs
```

Complexity hotspots:

```text
src/tools/graph_tools.rs: 3937 lines, 81 fns, total cyclomatic 748
src/graph/queries.rs:    3604 lines, 92 fns, total cyclomatic 801
src/graph/codemap.rs:    2051 lines, 46 fns, total cyclomatic 326
```

Boundary read:

- `graph` is a real subsystem and later crate candidate.
- `tools` is the MCP adapter layer. It should stay thin.
- `indexing`, `search`, `embeddings`, `vector_store`, `chunker`, and `parser`
  are a dense engine cluster. Keep them in one crate for now.
- `graph_tools.rs` is an adapter mega-file crossing into graph/query/audit
  internals.
- `queries.rs` is a query-family mega-file.
- `codemap.rs` mixes model, graph building, and rendering.

## 2. Non-Goals

- Do not run formatting.
- Do not split into workspace crates in the first pass.
- Do not rename public APIs unless an adapter/re-export preserves the old path.
- Do not widen visibility to make moves compile.
- Do not refactor function internals before file/module boundaries are stable.
- Do not edit vendored `vendor/fastembed` as part of this plan.

## 3. Phase 0: Baseline and Guardrails

Purpose:

Freeze the current behavior and establish mechanical checks before moving code.

Theory meaning:

- HoTT: record what behavioral equivalence means before changing shapes.
- Category theory: record the current graph before applying quotient/refinement
  operations.

Steps:

1. Check VCS state.
   - Run `jj status`.
   - If `jj` is unavailable, use `git status`.
   - Exit condition: unrelated dirty files are known and left untouched.

2. Record current graph evidence.
   - `build_hypergraph(directory=...)`
   - `workspace_stats(directory=...)`
   - `module_tree(directory=..., krate="file_search_mcp", depth=2)`
   - `dead_pub_in_crate(directory=..., krate="file_search_mcp")`
   - Exit condition: before-state metrics are saved in the work log or PR notes.

3. Establish verification commands.
   - Compile/check command selected by the maintainer.
   - Existing tests selected by risk area.
   - Do not run formatting.
   - Exit condition: agent knows the checks it is allowed to run.

4. Mark public compatibility surfaces.
   - `src/lib.rs`
   - `src/graph/mod.rs`
   - `src/indexing/mod.rs`
   - `src/search/mod.rs`
   - `src/embeddings/mod.rs`
   - `src/vector_store/mod.rs`
   - `src/parser/mod.rs`
   - Exit condition: facade modules are treated as compatibility surfaces.

Common failure:

- Starting with the largest file split before knowing which public paths must be
  preserved.

## 4. Phase 1: Split `tools` Adapter Layer

Purpose:

Make `tools` a thin MCP adapter layer instead of a mixed implementation and
graph-query mega-module.

Primary operation:

- `Split/Merge`
- small `Move` operations inside `src/tools`

Theory meaning:

- Category theory: split one overloaded adapter object into smaller endpoint
  families. Keep the same external MCP surface while reducing internal boundary
  cost.
- HoTT: request/response param types remain equivalent; endpoint behavior is
  preserved.

Current problem:

- `src/tools/graph_tools.rs` is 3937 lines and handles many unrelated graph tool
  families.
- `src/tools/search_tool.rs` is mostly param schema structs.
- `src/tools/search_tool_router.rs` is router/registration, not business logic.
- `tools` module has no declared facade re-exports, so most coupling is through
  direct module paths and router registration.

Target layout:

```text
src/tools/
  mod.rs
  router.rs                  # from search_tool_router.rs
  params.rs                  # from search_tool.rs
  project_paths.rs
  tool_analysis.rs           # from analysis_tools.rs
  tool_cache.rs              # from clear_cache_tool.rs
  tool_health.rs             # from health_tool.rs
  tool_index.rs              # from index_tool.rs
  tool_indexing_support.rs   # from indexing_tools.rs
  tool_query.rs              # from query_tools.rs
  graph_core.rs              # imports/exports/who_uses/who_calls/module_tree
  graph_crates.rs            # crate_edges, dependency metric, forbidden deps
  graph_surface.rs           # docs, derive, dead pub, reexports, attributes
  graph_audits.rs            # unsafe, recursion, fn_body, channel audits
  graph_similarity.rs        # similar_to_item, semantic_overlaps, codemap
  graph_response.rs          # enrich/json response helpers
```

Step-by-step:

1. Rename router and schema files conceptually.
   - Move `search_tool_router.rs` to `router.rs`.
   - Move `search_tool.rs` to `params.rs`.
   - Update `mod.rs` and imports.
   - Exit condition: no behavior change; old module paths are either updated or
     re-exported during migration.

2. Split `graph_tools.rs` by endpoint family.
   - Move import/export/use/call APIs to `graph_core.rs`.
   - Move crate-level APIs to `graph_crates.rs`.
   - Move public-surface APIs to `graph_surface.rs`.
   - Move audit APIs to `graph_audits.rs`.
   - Move similarity/codemap APIs to `graph_similarity.rs`.
   - Move helper response/enrichment structs to `graph_response.rs`.
   - Exit condition: each new file has one sentence of purpose.

3. Keep router behavior stable.
   - Router should import endpoint functions from the new files.
   - MCP tool names must not change.
   - Param schema names must not change unless old aliases are retained.
   - Exit condition: tool registration compiles and exported MCP names are
     unchanged.

4. Verify.
   - `analyze_complexity` on all new tools files.
   - `get_imports(directory=..., module="file_search_mcp::tools")`
   - `get_imports` on each new `tools::*` module.
   - Existing MCP/tool tests.

Risk:

- Medium. This is mostly moving code, but `graph_tools.rs` has many helper
  structs and response types that may have hidden local coupling.

Rollback strategy:

- Keep each split as a small move batch.
- If a helper is shared by too many new files, keep it in `graph_response.rs`
  instead of duplicating.

## 5. Phase 2: Split `graph::queries`

Purpose:

Turn one broad query mega-file into query-family modules.

Primary operation:

- `Split`

Theory meaning:

- Category theory: refine one overloaded object into query-family objects.
- HoTT: query result types are contracts; moving them must preserve type names
  or provide facade re-exports.

Current problem:

- `src/graph/queries.rs` is 3604 lines with 92 functions.
- It imports core graph model types, snapshot, ids, storage transaction types,
  and defines many unrelated query outputs.

Target layout:

```text
src/graph/query/
  mod.rs
  model.rs          # shared query result structs/enums
  imports.rs        # imports_of, exports, reexports, declared reexports
  usage.rs          # who_imports, who_uses, who_calls, calls_from
  crates.rs         # crate_edges, crate_dependency_metric, forbidden deps
  surface.rs        # dead_pub, attrs, pub-use/type audit
  functions.rs      # function_signature, functions_with_filter, call graph
  modules.rs        # module_tree
  overlaps.rs       # overlaps report
```

Compatibility facade:

```text
src/graph/queries.rs
```

should temporarily remain as a facade:

```rust
pub use query::model::*;
pub use query::imports::*;
pub use query::usage::*;
pub use query::crates::*;
pub use query::surface::*;
pub use query::functions::*;
pub use query::modules::*;
pub use query::overlaps::*;
```

Step-by-step:

1. Create `src/graph/query/mod.rs`.
   - Add submodules without moving logic yet.
   - Exit condition: empty/new modules compile when included.

2. Move result types first.
   - Move structs like `DeadPubFinding`, `CrateEdge`, `UsageSummaryRow`,
     `FunctionWithSignature`, `ModuleTreeNode`, `OverlapsReport`, etc. into
     `query/model.rs`.
   - Keep `graph::queries::*` re-export compatibility.
   - Exit condition: type paths still work through `graph::queries`.

3. Move import/export query functions.
   - `get_imports`-related snapshot methods and helper types.
   - `get_exports`, `get_reexports`, `get_declared_reexports`.
   - Exit condition: tools graph core endpoints compile.

4. Move usage/call query functions.
   - `who_imports`, `who_uses`, `who_uses_summary`.
   - `who_calls`, `calls_from`, `call_graph`, `recursive_callers_count`.
   - Exit condition: caller/callee tools compile.

5. Move crate-level query functions.
   - `crate_edges`.
   - `crate_dependency_metric`.
   - `forbidden_dependency_check`.
   - Exit condition: crate-level tool tests compile.

6. Move surface/public API queries.
   - `dead_pub_in_crate`, `dead_pub_report`.
   - item attributes.
   - re-export chain.
   - pub-use/pub-type audit.
   - Exit condition: dead-pub and surface tools compile.

7. Move function/module/overlap queries.
   - `module_tree`.
   - function signature/filter.
   - overlaps.
   - Exit condition: query-family files have coherent surfaces.

8. Verify.
   - `analyze_complexity` on `src/graph/query/*.rs`.
   - `get_declared_reexports(directory=..., module="file_search_mcp::graph")`
   - `who_imports` for key public query result types.
   - Existing graph query tests.

Risk:

- High. `queries.rs` is central and many tools import its result types.

Rollback strategy:

- Keep `queries.rs` as a facade until all users migrate.
- Move one family per commit/change.

## 6. Phase 3: Split `graph::codemap`

Purpose:

Separate codemap data model, build algorithm, search seeding, and rendering.

Primary operation:

- `Split`

Theory meaning:

- Category theory: codemap currently combines several morphism families in one
  object. Split by data model, graph construction, and presentation.
- HoTT: codemap structs are contracts; keep their public shape stable.

Current problem:

- `src/graph/codemap.rs` is 2051 lines.
- It imports `OpenedSnapshot`, `ModuleTreeNode`, ids/model kinds, storage txn
  types, and render/search-related logic.

Target layout:

```text
src/graph/codemap/
  mod.rs
  model.rs          # Codemap, CodemapNode, CodemapEdge, EdgeKind, stats/options
  seeds.rs          # seed resolution and search-hit normalization
  build.rs          # BFS/subgraph construction
  hierarchy.rs      # filtered module hierarchy construction
  render.rs         # mermaid/outline/json formatting helpers
```

Step-by-step:

1. Create `src/graph/codemap/mod.rs` and move public types to `model.rs`.
   - Keep `pub use model::*`.
   - Exit condition: existing public type paths still work.

2. Move rendering functions to `render.rs`.
   - Mermaid and outline formatting should not depend on seed search logic.
   - Exit condition: render code imports only codemap model types.

3. Move seed/search-hit logic to `seeds.rs`.
   - Keep embedding/search policy handling here.
   - Exit condition: build path calls seed resolver through a narrow function.

4. Move graph construction to `build.rs`.
   - BFS, incoming/outgoing expansion, edge construction.
   - Exit condition: build logic has no rendering concerns.

5. Move hierarchy projection to `hierarchy.rs`.
   - Module tree filtering/projection.
   - Exit condition: hierarchy code only handles tree shape.

6. Verify.
   - `analyze_complexity` on codemap subfiles.
   - Existing codemap tests.
   - `build_codemap` when CUDA/embedding environment is valid.

Risk:

- Medium-high. The current `build_codemap` path may touch embeddings and CUDA.
  Do not use CUDA-dependent verification as the only check.

Rollback strategy:

- Keep module-level facade at `graph::codemap`.
- Split types first, behavior last.

## 7. Phase 4: Split Facade/Implementation Files

Purpose:

Make `mod.rs` files mostly facades and move implementation into named files.

Primary operation:

- `Split`

Theory meaning:

- Category theory: `mod.rs` should project the public surface, not contain the
  whole implementation graph.
- HoTT: public types remain equivalent through re-exports while internals move.

Targets:

```text
src/chunker/
  mod.rs
  types.rs
  chunker.rs
  split.rs
```

```text
src/search/
  mod.rs
  types.rs
  vector.rs
  hybrid.rs
  rrf.rs
  bm25.rs
  resilient.rs
  rrf_tuner.rs
  error.rs
```

```text
src/parser/
  mod.rs
  types.rs
  rust_parser.rs
  imports.rs
  call_graph.rs
  type_references.rs
```

```text
src/embeddings/
  mod.rs
  types.rs
  generator.rs
  pipeline.rs
  backend.rs
  qwen3.rs
  token_lengths.rs
  error.rs
```

Step-by-step:

1. Split `chunker`.
   - Move `ChunkId`, `CodeChunk`, `ChunkContext`, config structs to `types.rs`.
   - Move `Chunker` implementation to `chunker.rs`.
   - Move oversized chunk/token split logic to `split.rs`.
   - Keep `chunker::CodeChunk`, `chunker::ChunkId`, etc. re-exported.
   - Exit condition: indexing/search/vector_store still import chunk types.

2. Split `search`.
   - Move `HybridSearchConfig` and `SearchResult` to `types.rs`.
   - Move `VectorSearch` to `vector.rs`.
   - Move `HybridSearch` to `hybrid.rs`.
   - Move reciprocal rank fusion core to `rrf.rs`.
   - Keep `search::HybridSearch` and `search::SearchResult` re-exported.
   - Exit condition: `tools::query_tools`, tests, and `indexing` compile.

3. Split `parser`.
   - Move shared parse result/symbol types to `types.rs`.
   - Move `RustParser` implementation to `rust_parser.rs`.
   - Keep `parser::RustParser` re-exported.
   - Exit condition: `analysis_tools`, `indexer_core`, and examples compile.

4. Split `embeddings`.
   - Move `Embedding` and `ChunkWithEmbedding` to `types.rs`.
   - Move `EmbeddingGenerator` to `generator.rs`.
   - Move `EmbeddingPipeline` to `pipeline.rs`.
   - Keep `embeddings::EmbeddingGenerator` re-exported.
   - Exit condition: indexing/search/vector_store imports remain stable.

5. Verify each split independently.
   - `get_imports` for affected modules.
   - Targeted tests/checks.
   - `dead_pub_in_crate` to identify newly dead facade exports.

Risk:

- Medium. These are public facade modules with many internal and test imports.

Rollback strategy:

- Keep old public paths via `pub use`.
- Avoid renaming types/functions during this phase.

## 8. Phase 5: Indexing Boundary Cleanup

Purpose:

Clarify indexing orchestration without moving the dense engine cluster into
separate crates yet.

Primary operation:

- `Split`
- narrow `Move`

Theory meaning:

- Category theory: indexing is a real container, but `unified.rs` is an
  orchestration object with several concerns.
- HoTT: indexing state/stat result types are contracts; keep them stable.

Current evidence:

- `indexing::unified` imports `embeddings`, `vector_store`, `config`,
  `metrics`, `chunker`, `tantivy_adapter`, and `indexer_core`.
- `indexing::incremental` depends on `unified` and `merkle`.
- `indexing::indexer_core` depends on `file_processor`, `embedding_batcher`,
  `chunker`, `parser`, `embeddings`, and `metadata_cache`.

Target layout:

```text
src/indexing/
  mod.rs
  types.rs              # IndexStats, IndexFileResult if stable
  unified.rs            # public orchestrator remains
  unified_parallel.rs   # parallel directory/file traversal helpers
  unified_search.rs     # create_bm25_search/vector_store helpers if separable
  indexer_core.rs
  embedding_batcher.rs
  file_processor.rs
  incremental.rs
  merkle.rs
  tantivy_adapter.rs
  consistency.rs
  retry.rs
  error.rs
  errors.rs
```

Step-by-step:

1. Move stable public result types out of `unified.rs`.
   - `IndexStats`
   - `IndexFileResult`
   - Exit condition: `indexing::IndexStats` and `indexing::IndexFileResult`
     paths still work.

2. Split parallel traversal helpers from `unified.rs`.
   - Move pure traversal/batching helpers to `unified_parallel.rs`.
   - Exit condition: `UnifiedIndexer` remains the public orchestrator.

3. Split search construction helpers if clearly separable.
   - BM25/vector-store helper creation can move to `unified_search.rs`.
   - Exit condition: no new public API unless needed.

4. Keep the engine cluster together.
   - Do not create crates for `embeddings`, `search`, `vector_store`, etc. yet.
   - Exit condition: boundaries are cleaner inside the crate first.

5. Verify.
   - Existing indexing tests.
   - `who_imports` for `UnifiedIndexer`, `IncrementalIndexer`, `IndexStats`.
   - `analyze_complexity` on `unified*.rs`.

Risk:

- Medium. `IncrementalIndexer` and examples/tests import `UnifiedIndexer` and
  `IndexStats`.

Rollback strategy:

- Keep `unified.rs` as the public owner of `UnifiedIndexer`.
- Split helpers only when signatures stay private.

## 9. Phase 6: Visibility and Public Surface Cleanup

Purpose:

Reduce accidental public API after structural moves stabilize.

Primary operation:

- `Lower` public visibility
- `Move` facade projection to intended modules

Theory meaning:

- Category theory: visibility is projection. Public symbols are external
  morphism targets.
- HoTT: public signatures are contracts. Do not expose obligations callers do
  not need.

Current evidence:

```text
pub: 508
pub(crate): 25
pub_crate_share: ~0.047
```

Step-by-step:

1. Run dead-public report after phases 1-5.
   - `dead_pub_in_crate(directory=..., krate="file_search_mcp")`
   - `dead_pub_report(directory=...)`
   - Exit condition: report is reviewed with examples/vendor noise excluded.

2. Demote obvious internal items.
   - Extraction helpers.
   - Audit helper functions.
   - Internal query helpers.
   - Internal tool response helpers.
   - Exit condition: external examples/tests still compile.

3. Keep facade exports intentional.
   - `graph::OpenedSnapshot`
   - `graph::BuildOptions`
   - `indexing::IncrementalIndexer`
   - `indexing::UnifiedIndexer`
   - `embeddings::EmbeddingGenerator`
   - `search::HybridSearch`
   - `vector_store::VectorStore`
   - Exit condition: facade modules have one-sentence public surfaces.

4. Resolve duplicate type names where useful.
   - `search::SearchResult` vs `vector_store::SearchResult`.
   - `graph::derive_audit::AuditOpts` vs `graph::docs_audit::AuditOpts`.
   - Exit condition: naming or module paths make the distinction clear.

5. Verify.
   - `workspace_stats` should show higher `pub_crate_share`.
   - `dead_pub_in_crate` should shrink for non-facade modules.

Risk:

- Medium. Examples and test crates import public library APIs.

Rollback strategy:

- Demote in small batches.
- Use `who_imports`, `who_uses`, and `find_references` before demotion/deletion.

## 10. Phase 7: Optional Crate Lift

Purpose:

Lift stable modules into crates only after internal boundaries prove stable.

Primary operation:

- `Lift`

Theory meaning:

- Category theory: module-to-crate maps a stable module object into a higher
  category with stronger dependency rules.
- HoTT: public API becomes a stronger contract; compatibility cost increases.

Do not start here.

Candidate 1: `graph`

Reason:

- Real subsystem.
- Strong named surface: persisted workspace hypergraph.
- Many examples/tests consume graph snapshot/query types.

Possible target:

```text
crates/rmc-graph/
  src/ids.rs
  src/model.rs
  src/storage.rs
  src/snapshot.rs
  src/loader.rs
  src/extract/
  src/query/
  src/audit/
  src/codemap/
```

Candidate 2: engine cluster

Reason:

- `indexing`, `search`, `embeddings`, `vector_store`, `chunker`, and `parser`
  are dense and mutually useful.

Possible target:

```text
crates/rmc-engine/
  src/chunker/
  src/parser/
  src/embeddings/
  src/vector_store/
  src/indexing/
  src/search/
```

Candidate 3: MCP adapter

Reason:

- `tools`, `mcp`, router, and project path handling are adapter-layer code.

Possible target:

```text
crates/rmc-mcp/
  src/router.rs
  src/tools/
  src/sync.rs
  src/project_paths.rs
```

Step-by-step:

1. Verify module surfaces after phases 1-6.
   - `get_declared_reexports`
   - `who_imports`
   - `crate_edges`

2. Lift `graph` first if any crate split is needed.
   - Keep compatibility re-exports in main crate temporarily.
   - Exit condition: main crate depends on `rmc-graph`; graph does not depend on
     main crate.

3. Lift engine only as a cluster.
   - Do not split `indexing/search/embeddings/vector_store/chunker/parser` into
     independent crates yet.
   - Exit condition: engine crate has no dependency back into MCP adapter.

4. Lift MCP adapter last.
   - It should depend on graph and engine, not the reverse.
   - Exit condition: dependency direction is acyclic.

5. Verify.
   - `crate_edges(directory=...)`
   - `forbidden_dependency_check(directory=..., rules=[...])`
   - workspace tests/checks.

Risk:

- High. Crate lift makes public API and dependency ordering much stricter.

Rollback strategy:

- Keep old re-export paths during migration.
- Lift one crate at a time.

## 11. Suggested Execution Order

Recommended order:

```text
Phase 0: Baseline and guardrails
Phase 1: Split tools adapter layer
Phase 2: Split graph::queries
Phase 3: Split graph::codemap
Phase 4: Split facade/implementation files
Phase 5: Indexing boundary cleanup
Phase 6: Visibility/public surface cleanup
Phase 7: Optional crate lift
```

Do not reorder Phase 6 before Phases 1-5. Visibility cleanup before movement
will create churn and likely need to be repeated.

Do not start Phase 7 until the module-level boundaries compile and remain stable
for at least one full verification pass.

## 12. Per-Phase Agent Output Template

Each implementation pass should report:

```text
Phase:
Primitive operation:
Files touched:
Boundary reason:
Type/contract reason:
Compatibility paths preserved:
Verification run:
New risks:
Next step:
```

## 13. Verification Checklist

After each phase:

- `jj status`
- compile/check command approved for this repo
- targeted tests for touched module family
- `analyze_complexity` for split files
- `get_imports` for touched modules
- no formatting command was run

After all module splits:

- `build_hypergraph(directory=...)`
- `workspace_stats(directory=...)`
- `dead_pub_in_crate(directory=..., krate="file_search_mcp")`
- `get_declared_reexports` for main facade modules
- compare public surface against Phase 0 baseline

## 14. Success Criteria

Structural:

- No file over 1500 lines except generated/vendor/test fixtures.
- `src/tools/graph_tools.rs` no longer exists as a mega-file.
- `src/graph/queries.rs` is a facade or removed after migration.
- `src/graph/codemap.rs` is split by model/build/render responsibilities.
- `mod.rs` files are mostly facades.

Boundary:

- `tools` depends inward on graph/engine; graph/engine do not depend on tools.
- `graph` has coherent submodules: model, storage, snapshot, extract, query,
  audit, codemap.
- engine cluster remains together until proven stable enough to lift.

Visibility:

- `pub_crate_share` increases meaningfully from ~0.047.
- Dead-public findings in `file_search_mcp` decrease after examples/vendor are
  excluded.
- Public facade exports are intentional and documented by one-sentence surfaces.

Agent ergonomics:

- Future feature work can target one module family at a time.
- Graph tools can be modified without reading a 3937-line file.
- Query changes can be made in a specific query-family file.
- Search/chunker/parser/embeddings implementation can be changed without editing
  facade-heavy `mod.rs` files.

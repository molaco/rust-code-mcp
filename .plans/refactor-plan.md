# Refactor Plan: Module & File Boundary Restructure

Status: ready to execute
Basis: `rust-code-mcp` workspace analysis, current checkout (post package
rename, post multi-provider work).
Snapshot: 2892 nodes, 4863 bindings, 7569 usages.

## 0. Goal

Move the project from a mostly-correct layout with a handful of overloaded
files into a structure where an agent can work in one module family at a time.
This is **not** a rewrite and **not** a crate split (that is optional Phase 7).

The top-level layout (`src/<module>/<file>.rs`) is kept. The work is
concentrated in the files that the size/complexity evidence actually flags.

## 1. Evidence

Non-generated source files over 500 lines:

```text
3976  src/tools/graph_tools.rs        <- mega-file, mixed endpoint families
3604  src/graph/queries.rs            <- mega-file, 92 query fns
2058  src/graph/codemap.rs            <- mega-file, model+build+seed+render
1654  src/embeddings/openrouter.rs    <- hotspot, config+http+batch+retry+parse
 898  src/embeddings/backend.rs       <- borderline, profile/runtime data model
 805  src/chunker/mod.rs              <- facade-heavy mod.rs
 802  src/indexing/embedding_batcher.rs  <- borderline, review only
 792  src/graph/fn_body_audit.rs      <- borderline, review only
 743  src/tools/search_tool_router.rs <- router (rename, not split)
 742  src/indexing/unified.rs         <- orchestrator with mixed helpers
 680  src/vector_store/lancedb.rs     <- under threshold, leave
 667  src/graph/snapshot.rs           <- under threshold, leave
 654  src/graph/storage.rs            <- under threshold, leave
 621  src/parser/mod.rs               <- facade-heavy mod.rs
 592  src/tools/query_tools.rs        <- under threshold, leave
 550  src/tools/search_tool.rs        <- param schemas (rename, not split)
 534  src/config/indexer.rs           <- under threshold, leave
```

Public surface: `530 pub` vs `30 pub(crate)`, `pub_crate_share ≈ 0.054`.
Encapsulation is weak; visibility cleanup happens **after** splits (Phase 6),
never before — moving code first invalidates any earlier visibility work.

### 1.1 What this plan deliberately does NOT touch — and why

A 10/10 plan proposes only work the evidence justifies. The following are
explicitly out of scope; touching them would be churn, not improvement:

- **`src/graph/` extract & audit files.** Already decomposed into flat files,
  all comfortably sized: `extract.rs` 460, `bindings.rs` 485, `usages.rs` 481,
  `impls.rs` 362, `signatures.rs` 341, `attributes.rs` 350, `ast_resolve.rs`
  28; audits `channel_audit.rs` 454, `derive_audit.rs` 403, `unsafe_audit.rs`
  340, `recursion_check.rs` 336, `docs_audit.rs` 268. Do **not** wrap these in
  `extract/` or `audit/` subdirectories — it is pure relocation with no
  decomposition value.
- **`src/search/`** — `mod.rs` is under 500 lines and the module is already
  split (`bm25.rs`, `resilient.rs`, `rrf_tuner.rs`, `error.rs`). Leave it.
- **`src/embeddings/mod.rs`** — 243 lines. Small. It stays the facade; do not
  fan it out into `types.rs`/`generator.rs`/`pipeline.rs`.
- **`src/vector_store/lancedb.rs`** (680), **`src/graph/snapshot.rs`** (667),
  **`src/graph/storage.rs`** (654), **`src/config/indexer.rs`** (534),
  **`src/tools/query_tools.rs`** (592) — each is one coherent concern under
  the split threshold. Leave.
- **`vendor/fastembed/`** — vendored third party. Never edited by this plan.

## 2. Boundary Read

`rust-code-mcp` stays one crate for this plan. The library has ~31 incoming
consumers across `benches/`, `tests/`, and `examples/`; a crate split now
would generate large API churn before the internal boundaries are even stable.

Intended dependency direction (acyclic):

```text
tools / mcp ─> graph, indexing, search, embeddings, vector_store
indexing    ─> parser, chunker, embeddings, vector_store, search
search      ─> embeddings, vector_store, chunker
chunker     ─> parser
graph       ─> graph internals only
```

Forbidden edges: `graph -> tools`, `graph -> mcp`, `engine -> tools`,
`embeddings -> indexing`. Phase 7 verification enforces these with
`forbidden_dependency_check`.

## 3. Guardrails

These hold for **every** phase:

1. **No formatting.** Do not run `cargo fmt` or any formatter.
2. **No visibility widening to make a move compile.** If a move needs a
   wider `pub`, the move is wrong — add a facade `pub use` instead.
3. **No public-path renames.** A symbol's external path is preserved by a
   facade `pub use`. Renaming is Phase 6's job, behind facades.
4. **No internal refactor before boundaries are stable.** Do not rewrite
   function bodies while moving them; move first, refactor never (in this plan).
5. **No crate split** before Phase 7, and Phase 7 is optional.
6. **`vendor/fastembed/` is never edited.**
7. **One concern per commit.** Each commit is a single coherent move; it must
   compile and pass verification before the next.
8. **Examples and tests are first-class.** `examples/` and `tests/` import many
   public library paths (and doc-tests import them too). Every phase keeps
   `cargo check --examples` green — not just at the end of the phase, but
   after each commit. This is the most likely thing to break; treat it as the
   primary regression signal.

### 3.1 Verification command

All checks run through the project Nix devshell, from the repo root:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --lib --tests --examples
```

`cargo fmt` is never run. Targeted unit tests for a touched module family are
run with `cargo test <module>:: --lib` through the same devshell.

## 4. Phase 0: Baseline & Guardrails

Purpose: freeze the current behavior and record the before-state so each later
phase can be checked against it.

Steps:

1. Confirm VCS state: `jj status` (or `git status`). Record any unrelated dirty
   files and leave them untouched.
2. Record the before-state baseline:
   - `cargo check --lib --tests --examples` is green (record it).
   - `workspace_stats(directory=…)` — save `pub` / `pub(crate)` counts.
   - `build_hypergraph(directory=…)` — save node/binding/usage counts.
   - `dead_pub_in_crate(directory=…, krate="rust_code_mcp")` — save the
     finding count (examples/tests excluded from interpretation).
   - `analyze_complexity` on each file listed in §1 — save LOC + cyclomatic.
3. Mark the compatibility surfaces — these `mod.rs` / facade files must keep
   their public paths intact for the whole plan:
   `src/lib.rs`, `src/graph/mod.rs`, `src/indexing/mod.rs`,
   `src/search/mod.rs`, `src/embeddings/mod.rs`, `src/vector_store/mod.rs`,
   `src/parser/mod.rs`, `src/chunker/mod.rs`, `src/tools/mod.rs`.

Exit condition: baseline metrics saved; verification command confirmed green.

## 5. Phase 1: Split the `tools` Adapter Layer

Purpose: `tools` is the MCP adapter layer and must be thin. Today
`graph_tools.rs` (3976) carries many unrelated endpoint families.

Operation: `Split` + two `Rename`s. Lowest-risk phase (adapter code, mechanical).

Target layout:

```text
src/tools/
  mod.rs
  router.rs                  # was search_tool_router.rs
  project_paths.rs

  params/                    # was search_tool.rs (param/schema structs)
    mod.rs
    search.rs
    graph.rs
    audit.rs
    indexing.rs

  endpoints/
    mod.rs
    analysis.rs              # was analysis_tools.rs
    cache.rs                 # was clear_cache_tool.rs
    health.rs                # was health_tool.rs
    index.rs                 # was index_tool.rs
    indexing_support.rs      # was indexing_tools.rs
    query.rs                 # was query_tools.rs

  graph/                     # was graph_tools.rs, split by endpoint family
    mod.rs
    core.rs                  # imports/exports/reexports, who_imports/uses/calls,
                             #   calls_from, call_graph, module_tree
    crates.rs                # crate_edges, crate_dependency_metric,
                             #   forbidden_dependency_check
    surface.rs               # dead_pub*, item/items attributes, missing_docs,
                             #   derive, pub_use/pub_type, re_export_chain
    audits.rs                # unsafe, mut_static, recursion, channel, fn_body
    similarity.rs            # similar_to_item, semantic_overlaps,
                             #   ensure_embeddings_for, resolve_graph_tool_backend,
                             #   embedder_version, cosine
    codemap.rs               # build_codemap endpoint bridge
    response.rs              # shared JSON/enrichment/render response helpers
```

Compatibility facades kept for the migration (deleted in Phase 6 once no
external path depends on them):

```text
src/tools/search_tool.rs         ->  pub use crate::tools::params::*;
src/tools/search_tool_router.rs  ->  pub use crate::tools::router::*;
src/tools/graph_tools.rs         ->  pub use crate::tools::graph::*;
```

Steps:

1. Rename `search_tool_router.rs` -> `router.rs`, `search_tool.rs` ->
   `params/` (split param structs by family). Leave the old files as
   one-line `pub use` facades. Commit.
2. Split `graph_tools.rs` one endpoint family at a time into `tools/graph/*`,
   each move its own commit. Shared helper/response structs that >1 family
   needs go to `graph/response.rs` rather than being duplicated.
3. Move the standalone endpoint files into `tools/endpoints/` (rename only).
4. Keep `router.rs` importing endpoint fns from the new paths. **MCP tool
   names and param-struct names must not change.**

Risk: Medium. Mostly moves, but `graph_tools.rs` has helper structs with
hidden local coupling.

Rollback: each family is its own commit; if a helper is shared too widely,
park it in `graph/response.rs` instead of duplicating.

Verification: `cargo check --lib --tests --examples`; `analyze_complexity` on
each new `tools/graph/*` file; `get_imports` on `rust_code_mcp::tools`.

Exit: `graph_tools.rs` is a facade; every new file has a one-sentence purpose;
all MCP tool names unchanged.

## 6. Phase 2: Split `graph::queries`

Purpose: `queries.rs` (3604, 92 fns) is the central query mega-file.

Operation: `Split`. Highest-risk phase — many tools and types consume it.

Target layout:

```text
src/graph/query/
  mod.rs
  model.rs       # shared query result structs/enums
  imports.rs     # imports_of, exports, reexports, declared reexports
  usage.rs       # who_imports, who_uses, who_uses_summary
  calls.rs       # who_calls, calls_from, call_graph, recursive_callers_count
  crates.rs      # crate_edges, crate_dependency_metric, forbidden deps
  surface.rs     # dead_pub*, item attributes, pub-use/pub-type, re_export_chain
  functions.rs   # function_signature, functions_with_filter
  modules.rs     # module_tree
  overlaps.rs    # overlaps report
```

Compatibility facade — `src/graph/queries.rs` stays until Phase 6:

```rust
pub use query::{model::*, imports::*, usage::*, calls::*, crates::*,
                surface::*, functions::*, modules::*, overlaps::*};
```

Steps (each a commit):

1. Create `graph/query/mod.rs` with empty submodules; confirm it compiles.
2. **Move result types first** into `query/model.rs` (`DeadPubFinding`,
   `CrateEdge`, `UsageSummaryRow`, `FunctionWithSignature`, `ModuleTreeNode`,
   `OverlapsReport`, …). Keep `graph::queries::*` re-exporting them.
3. Move query functions one family at a time: imports -> usage -> calls ->
   crates -> surface -> functions/modules/overlaps. Compile after each.

Risk: High. `queries.rs` result types are imported across `tools` and tests.

Rollback: keep `queries.rs` as a facade until all consumers migrate; one
family per commit; never widen visibility to compile (Guardrail 2).

Verification: `cargo check --lib --tests --examples` after every family;
`analyze_complexity` on `graph/query/*`; targeted `cargo test graph:: --lib`.

Exit: `queries.rs` is a pure facade; each `query/*` file is one family.

## 7. Phase 3: Split `graph::codemap`

Purpose: `codemap.rs` (2058) mixes data model, graph construction, seed
resolution, and rendering.

Operation: `Split`.

Target layout:

```text
src/graph/codemap/
  mod.rs
  model.rs       # Codemap, CodemapNode, CodemapEdge, EdgeKind, options, stats
  seeds.rs       # seed resolution + search-hit normalization (embedding policy)
  build.rs       # BFS / subgraph construction
  hierarchy.rs   # filtered module-hierarchy projection
  render.rs      # mermaid / outline / json formatting
```

`src/graph/codemap.rs` becomes `mod.rs` of the new directory with
`pub use model::*` so external type paths are unchanged.

Steps (each a commit): model types -> render -> seeds -> build -> hierarchy.

Risk: Medium-high. `build_codemap` touches embeddings; do not rely on a
CUDA/embedding live run as the only check — `cargo check` is the gate.

Rollback: split types first, behavior last; keep the `graph::codemap` facade.

Verification: `cargo check --lib --tests --examples`; `analyze_complexity` on
`codemap/*`; codemap tests.

Exit: codemap is split by model / build / seeds / hierarchy / render.

## 8. Phase 4: Split `embeddings::openrouter`

Purpose: `openrouter.rs` (1654) mixes runtime config, env parsing, the HTTP
client, request/response DTOs, batch planning, retry policy, and metrics.

Operation: `Split`.

Target layout:

```text
src/embeddings/openrouter/
  mod.rs
  config.rs      # runtime config, env vars, provider-routing preferences
  client.rs      # OpenRouterEmbedder, HTTP client, embed_documents/queries
  request.rs     # request DTOs
  response.rs    # response parsing, float + base64 decoding
  batching.rs    # remote batch planner, input ordering / restore
  retry.rs       # retryability classification, payload-too-large split
  metrics.rs     # OpenRouterRequestMetrics, record_request
                 #   (fold into client.rs if it stays under ~150 lines)
```

`src/embeddings/mod.rs` keeps `mod openrouter;` and its existing re-exports;
external paths (`OpenRouterRuntimeConfig`, `openrouter_runtime_config`, …)
unchanged.

Steps (each a commit): config -> request/response DTOs -> batching -> retry ->
metrics -> client (the remaining `OpenRouterEmbedder` orchestration).

Risk: Medium. Self-contained module; few external consumers beyond
`embeddings::mod` re-exports and `indexing::embedding_batcher`.

Rollback: per-concern commits; keep `embeddings::openrouter::*` re-exports.

Verification: `cargo check --lib --tests --examples`;
`cargo test embeddings::openrouter --lib`.

Exit: no `openrouter/` file over ~500 lines; concerns cleanly separated.

## 9. Phase 5: Facade & Borderline Splits

Purpose: thin out the remaining facade-heavy and borderline files.

Operation: `Split` + one `Merge`.

Targets and intent (split by concern; **do not** pre-commit to a fixed file
count — merge a target file away if its concern is small):

- **`src/chunker/mod.rs` (805)** -> `mod.rs` (facade) + `types.rs`
  (`ChunkId`, `CodeChunk`, `ChunkContext`, config) + `chunker.rs` (the
  `Chunker` impl) + `split.rs` (oversized-chunk / token-split logic).
- **`src/parser/mod.rs` (621)** -> `mod.rs` (facade) + `types.rs` (parse
  result / symbol types) + `rust_parser.rs` (the `RustParser` impl) + further
  files (`imports.rs`, `call_graph.rs`, `type_references.rs`) **only** for
  concerns that exceed ~250 lines; otherwise keep them in `rust_parser.rs`.
- **`src/embeddings/backend.rs` (898)** -> split the profile data model
  (`EmbeddingProfile`, the built-in registry, `QueryPolicy`,
  `LocalLoaderSpec`, `FastembedCpuModel`, `Qwen3Variant`) into `profile.rs`,
  leaving `backend.rs` with `EmbeddingBackend` + identity wiring. `identity.rs`
  already exists and is unchanged.
- **`src/indexing/unified.rs` (742)** -> keep `UnifiedIndexer` and the public
  `IndexStats` / `IndexFileResult` types in place (or in `indexing/types.rs`
  if cleanly separable); move pure parallel-traversal helpers to
  `unified_parallel.rs`. No new public API.
- **Merge `src/indexing/error.rs` + `src/indexing/errors.rs`** into one
  `error.rs`. Having both is an accident; consolidate and re-export so no
  external path breaks.

Borderline files — **review, do not split unless they mix concerns**:
`indexing/embedding_batcher.rs` (802) and `graph/fn_body_audit.rs` (792). If
each is one coherent concern, leave it and record that decision.

Risk: Medium. These are facade modules with many internal + test imports.

Rollback: keep old public paths via `pub use`; no type/fn renames here.

Verification: `cargo check --lib --tests --examples`; `get_imports` for each
touched module; `dead_pub_in_crate` to spot newly-dead facade exports.

Exit: no non-generated source file over ~1000 lines (target ~800); the four
mega-files of §1 are gone; the duplicate indexing error file is resolved.

## 10. Phase 6: Visibility & Public-Surface Cleanup

Purpose: reduce accidental public API now that boundaries are stable.

Operation: `Lower` visibility; delete the migration facades.

Steps:

1. Run `dead_pub_in_crate(krate="rust_code_mcp")` and `dead_pub_report`;
   review with `examples/` and `tests/` consumers in mind (a symbol used only
   by an example is not dead — it is an example API).
2. Demote internal-only items (`pub` -> `pub(crate)`): extraction helpers,
   audit helpers, query helpers, tool response helpers. Use `who_imports` /
   `who_uses` / `find_references` before each demotion.
3. Delete the Phase 1/2 migration facades (`search_tool.rs`,
   `search_tool_router.rs`, `graph_tools.rs`, `graph/queries.rs`) **only**
   once no external path (incl. examples/tests) still imports through them —
   verified with `who_imports`.
4. Resolve duplicate type names where it removes ambiguity:
   `search::SearchResult` vs `vector_store::SearchResult`;
   `graph::derive_audit::AuditOpts` vs `graph::docs_audit::AuditOpts`.
5. Keep intentional facade exports: `graph::OpenedSnapshot`,
   `graph::BuildOptions`, `indexing::{UnifiedIndexer, IncrementalIndexer}`,
   `embeddings::EmbeddingGenerator`, `search::HybridSearch`,
   `vector_store::VectorStore`.

Risk: Medium. Examples/tests import public APIs — Guardrail 8 applies hard.

Rollback: demote in small batches; one batch per commit.

Verification: `workspace_stats` shows a higher `pub_crate_share`;
`dead_pub_in_crate` shrinks for non-facade modules; `cargo check --examples`
stays green.

Exit: `pub_crate_share` meaningfully above the 0.054 baseline; facade modules
have one-sentence public surfaces.

## 11. Phase 7: Optional Crate Lift

Do not start here. Only after Phases 1-6 compile and survive one full
verification pass unchanged.

First (and likely only) candidate: **`graph`** — a real subsystem with a
strong named surface (the persisted workspace hypergraph) and no dependency
back into `tools`/`mcp`.

Steps:

1. Re-check surfaces: `get_declared_reexports`, `who_imports`, `crate_edges`.
2. Lift `graph` into `crates/rmc-graph/`; keep compatibility re-exports in the
   main crate. Exit: main crate depends on `rmc-graph`; `rmc-graph` depends on
   nothing in the main crate.
3. The engine cluster (`indexing`, `search`, `embeddings`, `vector_store`,
   `chunker`, `parser`) is lifted only as a single cluster, and only if a
   later need arises — not in this plan.

Risk: High. Crate lift hardens the public API and dependency ordering.

Verification: `crate_edges`; `forbidden_dependency_check` with the §2 rules;
full workspace `cargo check --lib --tests --examples`.

## 12. Execution Order

```text
Phase 0  Baseline & guardrails
Phase 1  Split tools adapter layer        (lowest risk — start here)
Phase 2  Split graph::queries             (highest risk)
Phase 3  Split graph::codemap
Phase 4  Split embeddings::openrouter
Phase 5  Facade & borderline splits
Phase 6  Visibility & public-surface cleanup
Phase 7  Optional crate lift
```

Phase 6 must not move earlier than Phase 5 — visibility cleanup before the
moves are done just has to be redone. Phase 7 must not start until module
boundaries are stable across one clean verification pass.

## 13. Per-Phase Output Template

Each implementation pass reports:

```text
Phase:
Operation (Split / Move / Rename / Merge / Lower):
Files touched:
Boundary reason (why this split):
Compatibility paths preserved (which facades / re-exports):
Verification run (command + result):
cargo check --lib --tests --examples: pass/fail
New risks:
Next step:
```

## 14. Verification Checklist

After every commit:

- `jj status` reviewed
- `nix develop ../nix-devshells#cuda-code --command cargo check --lib --tests --examples` green
- no formatting command was run

After each phase:

- targeted tests for the touched module family
- `analyze_complexity` on every new/split file
- `get_imports` on touched modules

After all splits (end of Phase 5):

- `build_hypergraph`, `workspace_stats`, `dead_pub_in_crate`
- `get_declared_reexports` on the facade modules
- compare the public surface against the Phase 0 baseline

## 15. Success Criteria

Structural:

- No non-generated source file over ~1000 lines (target ~800).
- `tools/graph_tools.rs`, `graph/queries.rs`, `graph/codemap.rs`,
  `embeddings/openrouter.rs` no longer exist as mega-files.
- `mod.rs` files are facades, not implementations.
- `indexing/error.rs` and `indexing/errors.rs` consolidated to one.

Boundary:

- `tools` depends inward only; `graph` and the engine cluster never depend on
  `tools`/`mcp`. `forbidden_dependency_check` passes the §2 rules.

Visibility:

- `pub_crate_share` meaningfully above 0.054.
- Dead-public findings shrink for non-facade modules (examples/tests excluded).

Regression:

- `cargo check --lib --tests --examples` green at every commit — examples and
  tests never break.
- MCP tool names and param-struct external paths unchanged throughout.

Agent ergonomics:

- A change to one tool family, query family, audit, or the OpenRouter client
  can be made in one focused file without reading a 3000+-line file.

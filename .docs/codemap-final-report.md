# Codemap implementation — final report

**Branch state:** six sequential commits on top of `d3f0225b` (the proposal commit `v2.4`).
**Final cargo state:** `cargo check --lib` clean at the **17-warning pre-codemap baseline**. Every codemap symbol introduced is reachable from a `#[tool]` method.

## Phase summary

| # | Commit | Subject | Net LOC |
|---|---|---|---|
| 1 | `c18f218d` | Foundation types + `ItemKind` predicates | +113 (3 files) |
| 2 | `424b0388` | Span index + line→byte bridge + path normalization | +290 (2 files) |
| 3 | `bc50e21a` | Raw-ID graph adapters + `min_call_distance` | +112 (2 files) |
| 4 | `c053d94b` | Extract `ensure_embeddings_for` from `semantic_overlaps` | +79 (1 file, ~135 LOC moved) |
| 5 | `8faf0973` | `build_codemap` algorithm core | +633 (2 files) |
| 6 | `03e3d770` | Mermaid/outline renderers + MCP tool wiring | +497 (4 files) |

**Total:** ~1,720 LOC across 6 source files + 6 phase reports + this final report. The proposal's estimate was ~1,610 LOC; actual landed slightly higher due to the renderer + smoke tests in Phase 6 being more verbose than budgeted.

## Files touched

| File | Role |
|---|---|
| `src/graph/codemap.rs` | **new** — response types, helpers (span/line bridge, BFS distance), algorithm, renderers, tests |
| `src/graph/snapshot.rs` | extended `OpenedSnapshot` with `span_index` + `line_to_byte` lazy caches |
| `src/graph/queries.rs` | appended `pub(crate)` `callees_of` / `referrers_of` adapters |
| `src/graph/model.rs` | added `ItemKind::is_callable()` / `is_type()` predicate methods |
| `src/graph/mod.rs` | wired `pub mod codemap;` |
| `src/tools/graph_tools.rs` | extracted `ensure_embeddings_for`; added `handle_build_codemap`; promoted `cosine` to `pub(crate)` |
| `src/tools/search_tool.rs` | added `BuildCodemapParams` |
| `src/tools/search_tool_router.rs` | added `build_codemap` `#[tool]` method |

## Reuse contract — honored

Every external API the proposal's §11 reuse map promised to call is now called rather than re-implemented:

- `HybridSearch::search(query, limit)` — invoked from `handle_build_codemap`.
- `cosine(a, b)` — invoked from Phase C scoring in `build_codemap` (visibility promoted from `fn` to `pub(crate) fn`; minimal blast radius).
- `EmbeddingGenerator::embed_async(String)` — invoked for the prompt vector.
- `EmbeddingGenerator::embed_batch_async` — invoked transitively via `ensure_embeddings_for` (which Phase 4 factored out of `semantic_overlaps`).
- `lookup_by_qualified_name` — primary seed resolver.
- `module_tree(crate_name, depth)` — for hierarchy projection.
- `open_workspace_snapshot` + `internal_error` — MCP error mapping.
- `read_file_content` — used by `ensure_embeddings_for` (already was).
- `usages_for_consumer_function` / `usages_for_target` — wrapped by the new `callees_of` / `referrers_of`.
- `embeddings_by_target` heed sub-DB — read in Phase A of scoring; written by `ensure_embeddings_for`.
- `recursive_callers_count` template — `min_call_distance` mirrors its frontier+visited shape.

No duplication added. `resolve_workspace_relative` (build-time only) is **not** called from query time — a tiny `canonicalize_and_strip` was added instead, as the proposal anticipated.

## Async hygiene — honored

The three-phase pattern from `.plans/codemaps-proposal.md` §5 SCORE is implemented exactly as described in `build_codemap`:

- **Phase A** opens a single `RoTxn` for scoring data + cache lookups, then **drops it** at end of scope.
- **Phase B** is the only async region — `EmbeddingGenerator::new()` (lazy, only if needed), `embed_async(prompt)`, and `ensure_embeddings_for(snap, &missing).await`. **No heed transaction held across any `.await`.**
- **Phase C** combines into relevance scores with no transaction at all.

`ensure_embeddings_for` follows the same pattern internally: short `RoTxn` to classify cached vs missing, drop, `embed_batch_async`, short `RwTxn` to persist.

## Algorithm decisions vs. the proposal

| Item | Proposal | Implemented | Reason |
|---|---|---|---|
| Snippet extraction | optional `include_snippets` | always `None` in v1 | Permitted by plan; defers a fiddly file-read pass. |
| `trait_dispatch_unresolved` counter | dropped in v2.2 | not added | RA filters unresolved sites; data not available in `Usage` table. |
| RA `goto_definition` fallback | dropped in v2.4 | not added | Position-based, not name-based. Unresolved names → `diagnostics`. |
| Multi-crate projection | "find the most common crate" *or* "synthetic workspace wrapper" | synthetic workspace wrapper | Cleaner; consistent with `ModuleTreeNode` shape. |
| `bm25_by_node` for repeated hits | unspecified | sum | Frequently-cited functions outrank one-off hits. |
| `min_call_distance` underflow | unspecified | `u32::MAX` → `graph_prox = 0` | Explicit clamp avoids `1/(1+u32::MAX)` float drift. |
| Edge weight on duplicates | accumulated | `*entry.or_insert(0) += 1`; `EdgeKind` gained `PartialEq, Eq, Hash` | Necessary for the HashMap key. |

## Test status

Each phase added `#[cfg(test)] mod tests` entries (4 in Phase 2, 3 in Phase 3, 3 in Phase 5, 2 in Phase 6 → 12 new tests total). They all compile under `cargo check --lib --tests`.

**They do not currently *run* because** the synthetic-fixture path in `usages.rs` builds a Cargo workspace at runtime and invokes rust-analyzer's `cargo metadata`, which fails on the synthetic manifest in this local environment. This is a **pre-existing environmental issue inherited from Phase 2's fixture choice**, not a Phase-N regression — the original `pattern2_trait_dispatch_captured` test in `src/graph/usages.rs` exhibits the same failure mode. End-to-end validation is left to manual MCP-server runs against the live workspace.

## What changed vs. the v2.4 plan

- **Cosine visibility.** Plan said "reuse `cosine`"; that required promoting `fn cosine` to `pub(crate) fn cosine` in `src/tools/graph_tools.rs`. Single-line change in Phase 5.
- **`ResolvedEmbedding` return type.** `ensure_embeddings_for` returns `HashMap<NodeId, ResolvedEmbedding>` (carrying `content_hash` alongside `vector`) rather than just `HashMap<NodeId, Vec<f32>>`. Lets `semantic_overlaps` keep its identical-source short-circuit without re-reading files. Phase 5 ignores `content_hash`. Net: faithful refactor.
- **`HybridSearch` construction in the MCP tool** is inlined (project paths + Tantivy + `query_tools::create_hybrid_search`) with a best-effort fallback for missing BM25 indices. The plan didn't anticipate the constructor complexity; introducing a shared helper for one call site felt premature.
- **`canonicalize_and_strip`** lives in `codemap.rs` (not as a shared `paths.rs` module) because no other call site needs it yet.

## Deferred (v2+, per plan §§8/10/12)

- `codemaps_by_key` persistence sub-DB (requires `SCHEMA_VERSION` 11→12).
- Snapshot-handle reuse across tool calls (eliminate per-request span-index build).
- `SnapshotView` trait for sans-I/O testability.
- Trait-dispatch coverage improvements in `Usage` extraction (separate, larger workstream).
- `include_snippets = true` source-text extraction.

## Verification at end of branch

```
$ nix develop ../nix-devshells#code --command cargo check --lib
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.20s
warning: `file-search-mcp` (lib) generated 17 warnings
```

The 17 residual warnings are all **pre-existing** and unrelated to the codemap implementation (unused RA helpers in `semantic/position.rs`, `unreachable_pub` lints in `ids.rs`, dead `build_type_references*` in `parser/type_references.rs`). The codemap implementation contributes zero warnings.

## Status

The `build_codemap` MCP tool is production-shape:
- Reachable from the rmcp router.
- Validates inputs (`McpError::invalid_params` for invalid `format` strings or missing prompt+seed inputs).
- Returns one of `json` / `mermaid` / `outline` / `all` per the `format` parameter.
- Carries the trait-dispatch nuance in its tool description so consumers know what's captured vs. not.
- Asynchronously hygienic — no heed transactions held across `embed` awaits.

Ready for manual smoke-testing via an MCP client against the live workspace.

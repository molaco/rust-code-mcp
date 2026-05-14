# Force 15 — Compile Time / Build Graph

Constraint analyzed: how each candidate workspace layout affects cold build,
incremental rebuild, and `cargo check`/`cargo test` latency given the heavy
deps in `Cargo.toml` (rmcp, tantivy, lancedb + arrow, fastembed + ort,
ra_ap_* x9, heed, sled, tokio).

## Heavy-dep cost map (status quo)

- **ra_ap_* (9 crates @ 0.0.330)** — by far the largest single contributor;
  pulls salsa, chalk, hir, hir_def, base_db, ide, ide_db, vfs, project_model,
  load-cargo. Cold build dominates. Used by `parser`, `semantic`, `graph`.
- **fastembed + ort + ndarray** — heavy native + generic code, ONNX bindings.
  Used by `embeddings`, indirectly by `search` (query embed) and `indexing`.
- **lancedb + arrow-array + arrow-schema** — large Arrow generics; long
  monomorphization. Used only by `vector_store`.
- **tantivy** — moderate. Used by `schema`, `search::bm25`, `indexing::tantivy_adapter`.
- **rmcp (git)** — proc-macro `#[tool_router]` heavy. Used by `tools` + `main`.
- **heed** — small/medium. Used by `graph::storage`.
- **tokio (multi-thread + macros)** — pervasive.

## 1. Single crate (status quo)

- **Layout:** all heavy deps compile into one rlib.
- **Parallelism:** rustc parallelizes per-codegen-unit only; no crate-level
  fan-out. ra_ap_* + ort + lancedb compile sequentially in dep order, then
  the giant `file-search-mcp` rlib monomorphizes everything.
- **Incremental footprint:** edit one line in `tools/graph_tools.rs` →
  recompile the entire crate's codegen units that touch the changed module
  + the final binary link. `cargo check` is fast for non-API edits thanks to
  incremental, but `cargo test` rebuilds the test harness against the whole
  rlib (~115 s as recorded in MEMORY).
- **Wins:** zero duplicated dep compiles, zero crate-graph overhead, simplest
  feature gating.
- **Losses:** no parallel crate compilation; every edit re-typechecks the
  whole crate; test binary is monolithic.

## 2. 5-crate split (server, code-search, graph, ide, search-eval)

- **Heavy-dep placement:**
  - `server` → rmcp, tokio, tools, mcp::SyncManager (depends on the others).
  - `code-search` → tantivy, lancedb/arrow, fastembed/ort, indexing, search,
    chunker, embeddings, vector_store, schema, metadata_cache (sled).
  - `graph` → ra_ap_hir/hir_def/base_db, heed, parser pieces.
  - `ide` → ra_ap_ide/ide_db/load-cargo/vfs/project_model (semantic).
  - `search-eval` → RRFTuner + offline tooling (light).
- **Parallelism:** cargo can build `code-search`, `graph`, `ide` in parallel
  once their disjoint dep stacks resolve. ra_ap_* still serializes within
  `graph`+`ide`, but `code-search` (lancedb/ort/tantivy) runs concurrently
  on a free core. Realistic cold-build wall-clock: **~30–40% faster** on a
  multi-core box vs status quo.
- **Incremental footprint:** edit `graph/queries.rs` → recompile `graph` +
  `server`. `code-search` and `ide` untouched. This is the biggest win.
- **`cargo check`:** per-crate; touching one crate avoids retypechecking
  the others. **Major win** for everyday loops.
- **`cargo test`:** each crate gets its own test binary; running `-p graph`
  skips relinking against tantivy/lancedb/ort entirely. **Largest practical
  win** given the 115 s snapshot-build cost.
- **Losses:** ra_ap_* deps shared by `graph`+`ide` compile twice unless you
  factor a shared `ra-common` crate. Workspace-wide `cargo build` adds some
  metadata/link overhead (~5–10 s).

## 3. 10+ crate split (every subsystem its own crate)

- Splits embeddings, vector_store, chunker, parser, semantic, graph,
  indexing, search, tools, monitoring, metrics, security, config…
- **Parallelism:** more crates buildable in parallel, but the heavy deps
  (ra_ap_*, ort, lancedb) still pin the critical path — they live in one
  leaf crate each. Marginal cold-build improvement over (2).
- **Incremental win:** smaller blast radius per edit (e.g., `chunker` edit
  only rebuilds chunker + indexing + tools).
- **Losses:** explosion of `Cargo.toml`s, version drift risk, more
  proc-macro re-runs (serde derives compile per crate), workspace metadata
  resolution slows, and many edits cross 3–4 crate boundaries causing
  cascading rebuilds. Net incremental gain over (2) is small; cold build
  gain near zero. **Not worth the maintenance tax** for this codebase size.

## 4. 3-crate minimal split (server, code-search, graph; ide+semantic in graph)

- **Heavy-dep placement:** `code-search` owns tantivy/lancedb/ort/fastembed;
  `graph` owns all ra_ap_* + heed; `server` owns rmcp.
- **Parallelism:** two heavy leaves (`code-search`, `graph`) build in
  parallel — captures most of the cold-build win of layout (2).
- **Incremental:** edits to ra_ap_* glue or graph queries don't touch
  tantivy/ort side and vice versa. Editing in `server` (tools wiring) still
  rebuilds only the thin top layer.
- **`cargo test -p graph`** skips ort/lancedb/tantivy entirely — the
  single biggest dev-loop saving.
- **Losses:** `graph` becomes a fat crate (ra_ap_* + heed + ide + semantic);
  internal edits inside it still trigger full-crate retypecheck. But it's
  still smaller than status quo.

## Recommendation

**Layout 4 (3-crate minimal)** minimizes incremental compile time per unit
of refactor effort:

- Captures the parallelism win — the two heaviest dep stacks (ra_ap_* and
  ort/lancedb) sit in *separate* crates and build concurrently.
- Lets `cargo test -p code-search` and `cargo test -p graph` skip the
  other half of the heavy deps, directly attacking the 115 s snapshot cost.
- Avoids the duplication and `Cargo.toml` sprawl of layout (3).
- Migration cost is small: cleavage planes already match module boundaries
  in `src/` (graph/semantic/parser vs indexing/search/embeddings/vector_store
  vs tools/mcp/main).

If a 4th crate is ever justified, split `ide`/`semantic` out of `graph`
into its own crate to deduplicate ra_ap_ide vs ra_ap_hir compile costs —
but only after measuring; ra_ap_* shares enough internal crates that the
duplication may be cheaper than an extra crate boundary.

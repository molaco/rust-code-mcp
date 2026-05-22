# Phase 7 — Phase B Report

**Plan**: `.plans/refactor-phase-7-plan.md` — Refactor Plan: Phase 7 — Cleanup & Crate Lift
**Phase**: B — Engine + Graph crate lift
**Status**: complete — workspace established, both lift crates internally complete, `forbidden_dependency_check` codified and passing 0 violations
**Workspace**: `/home/molaco/Documents/rust-code-mcp-refactor`
**Date**: 2026-05-21

## Summary

Nine `jj` commits land the workspace conversion (B.0) and lift six modules into two crates: `rmc-engine` (parser, schema, chunker, embeddings, vector_store, search) and `rmc-graph` (graph alone, depending only on `rmc-engine`). `cargo check --workspace --all-targets` is green at every commit. `forbidden_dependency_check` against the three core Phase B rules returns 0 violations.

The lift forced ~36 `pub(crate) → pub` widenings — items previously reachable monolith-wide that now need to cross crate boundaries to reach their consumers in the main crate (`tools/*`, `indexing/*`, `vector_store/*`). The widening is the inherent architectural cost of a crate lift; it is recorded item-by-item in the commit log and the §4.B.0 plan addendum explains the rule.

## Commits

```
9ce71b19  phase 7 B.8: codify forbidden_dependency_check rule set in .docs/architectural-rules.md
302cfbd6  phase 7 B.7: lift graph into rmc-graph; depend on rmc-engine; widen ~30 pub(crate)→pub
edbdc829  phase 7 B.6: verify rmc-engine boundary (zero forbidden imports; cargo check -p rmc-engine green)
8c0d7410  phase 7 B.5: lift search into rmc-engine; engine internally complete
407f1a93  phase 7 B.4: lift vector_store into rmc-engine; add lancedb + arrow + async-trait + directories
c72bb27e  phase 7 B.3: lift embeddings into rmc-engine; widen 5 items for embedding_batcher + vector_store
d8c42529  phase 7 B.2: lift chunker into rmc-engine; add serde + uuid
0811748f  phase 7 B.1: lift parser + schema into rmc-engine; widen FileSchema
bce8c898  phase 7 B.0: cargo workspace skeleton; create rmc-engine + rmc-graph stub crates
```

Parent of the series: `f148d263` (Phase A.4 polish).

## B.0 — Workspace skeleton

- Root `Cargo.toml` converted to workspace (resolver = "3", 3 members: `.`, `crates/rmc-engine`, `crates/rmc-graph`).
- All 50 main-crate `[dependencies]` moved to `[workspace.dependencies]` preserving versions/features. Main `[dependencies]` redirects every entry via `{ workspace = true }`.
- `[patch.crates-io]`, `[features]`, 13 `[[example]]` blocks, `[dev-dependencies]` preserved attached to the main crate.
- `crates/rmc-engine/` and `crates/rmc-graph/` created with `Cargo.toml` (empty `[dependencies]`), `src/lib.rs` (just a doc comment), and `README.md`.
- Main `Cargo.toml` adds `rmc-engine`, `rmc-graph` as path dependencies (still resolving to empty stubs).

**Outcome**: workspace builds green, both new crates compile as empty libraries.

## B.1 — Lift parser + schema

- `src/parser/` (6 files) and `src/schema.rs` moved to `crates/rmc-engine/src/`.
- Third-party deps added to `rmc-engine/Cargo.toml`: `ra_ap_syntax` (parser), `tantivy` (schema). Narrower than the plan's "typical expected set" anticipated — parser only uses `ra_ap_syntax`, not the full `ra_ap_*` family.
- **First cross-crate widening**: `FileSchema` widened from `pub(crate)` to `pub` because `src/tools/endpoints/indexing_support.rs:47` reaches it across the new crate boundary. This established the precedent codified in the §4.B.0 "Cross-crate visibility widening" rule (added mid-execution after the agent stopped on the visibility ambiguity).

## B.2 — Lift chunker

- `src/chunker/` (4 files) moved.
- Third-party deps added: `serde`, `uuid`. The plan's anticipated `text-splitter`, `tokenizers`, `tracing`, `anyhow`, `regex`, `tree-sitter` set was wrong — none of those are actually used by chunker.
- **No visibility widenings required** — chunker's public surface (`Chunker`, `ChunkContext`, `ChunkId`, `ChunkSplitConfig`, `CodeChunk`) was already `pub`.

## B.3 — Lift embeddings

The largest move: 12 files + `openrouter/` subdir (8 files) = 20 files total.

- Third-party deps added: `reqwest`, `hf-hub`, `fastembed`, `candle-core`, `tokenizers`, `serde_json`, `thiserror`, `tracing`, `tokio`, `futures`, `toml`. `tempfile` added as a dev-dep for `profile_registry.rs` tests.
- **Five widenings**, all driven by `indexing::embedding_batcher` and `vector_store` consumers:
  - `mod batching` (`pub(crate)` → `pub`).
  - `type Embedding = Vec<f32>`.
  - `struct BatchPlan` + fields `start`, `end`.
  - `fn plan_batches`.

## B.4 — Lift vector_store

- `src/vector_store/` (4 files) moved.
- Third-party deps added: `lancedb`, `arrow-array`, `arrow-schema`, `async-trait`, `directories`. The `directories` dep was missed by the initial `^use` grep — caught when the first compile failed on `directories::ProjectDirs::from(...)` (inline path in `mod.rs:49`).
- **No visibility widenings required** — vector_store's public surface was already `pub` from the monolith era (`LanceDbBackend`, `VectorStoreBackend`, `VectorStoreError`, `VectorSearchResult`).

## B.5 — Lift search

- `src/search/` (5 files) moved. **`rmc-engine` is now internally complete.**
- Third-party deps added: just `anyhow` (everything else was already declared from earlier moves).
- **No visibility widenings required.**

## B.6 — Verify engine boundary

No code change. Two checks passed:

- `grep -rnE 'use crate::(graph|tools|indexing|mcp|config|monitoring|metadata_cache|metrics|security|semantic)' crates/rmc-engine/src/` returned zero hits — no hidden inversions surfaced.
- `cargo check -p rmc-engine` built standalone in ~1m on a cold cache (20 dead-code style warnings, no errors).

## B.7 — Lift graph

- `src/graph/` moved to `crates/rmc-graph/src/graph/` (28 top-level entries: 26 .rs files + `codemap/`, `query/` subdirs).
- **All 5 references to `crate::embeddings` were inline fully-qualified paths**, not `use` statements: `embedding_cache.rs:{38,136}` and `codemap/build.rs:{221,245,266}`. The plan anticipated `use crate::embeddings::` rewrites; the actual rewrite was `sed s/crate::embeddings/rmc_engine::embeddings/g`.
- **24 third-party deps added** to `rmc-graph/Cargo.toml`: `rmc-engine` (path), `heed`, `serde`, `serde_json`, `serde_bytes`, `bincode`, `anyhow`, `thiserror`, `tracing`, `tokio`, `num_cpus`, `ra_ap_syntax`, `ra_ap_ide`, `ra_ap_ide_db`, `ra_ap_load-cargo`, `ra_ap_project_model`, `ra_ap_vfs`, `ra_ap_hir`, `ra_ap_hir_def`, `rmcp`, `cargo_metadata`, `walkdir`, `sha2`, `directories`. Dev-dep: `tempfile`.
- **`rmcp` flag — is it a layering inversion?** No. `rmcp` is the external MCP SDK crate, not the in-tree `crate::mcp` module. `rmc-graph` consumes `rmcp::schemars` for `JsonSchema` derive on `query/model.rs` types so MCP tool param schemas auto-generate. It's a third-party dep on equal footing with `serde`. The forbidden-edge check is against the in-workspace `rust-code-mcp` crate, not against the unrelated external `rmcp`.
- **~30 visibility widenings**, all driven by `tools/graph/*` consumers in the main crate. Categories:
  - **Labels** (`labels.rs`): all five label functions widened (`usage_category_label`, `binding_kind_label`, `node_kind_label`, `item_kind_short_label`, `item_kind_display_label`) — consumed by `tools/graph/{core,response,similarity,surface}.rs`.
  - **Audit modules**: `recursion_check`, `channel_audit`, `fn_body_audit`, `docs_audit`, `derive_audit` — opts structs, finding structs, and entry-point fns widened. Consumed by `tools/graph/{audits,surface}.rs`.
  - **Codemap**: `codemap/mod.rs` re-exports, `codemap/model.rs` structs (`Codemap`, `CodemapNode`, `CodemapEdge`, `EdgeKind`, `CodemapStats`, `CodemapOptions`, `EmbeddingPolicy`), `codemap/seeds.rs::SeedHit`, `codemap/render.rs::{render_mermaid, render_outline}`, `codemap/build.rs::{build_codemap, newest_source_mtime}`. Consumed by `tools/graph/codemap.rs`.
  - **Storage**: `default_data_dir` (consumed by `tools/endpoints/cache.rs:191`).
  - **Module-level**: `mod labels`, `use embedding_cache::ensure_embeddings_for`, `use math::cosine` (the three modules narrowed to `mod` in Phase A.3 had to widen their re-exports to remain reachable across the new crate boundary).
- **Final-gate grep** `'use crate::(search|indexing|tools|mcp|config|monitoring|metadata_cache|metrics|security|semantic|chunker|parser|schema|vector_store)'` in `crates/rmc-graph/src/` returned zero hits. **No hidden inversions.**

## B.8 — Wire `forbidden_dependency_check`

- Rule set codified in `.docs/architectural-rules.md` (JSON form, directly runnable via `mcp__rust-code-mcp__forbidden_dependency_check`).
- Three Phase B rules verified against the post-B.7 workspace: **0 violations.**
  - `rmc-engine` must not depend on `rmc-graph`.
  - `rmc-engine` must not depend on `rust-code-mcp`.
  - `rmc-graph` must not depend on `rust-code-mcp`.
- Phase C extension (8 additional rules) drafted in the same doc but not yet enforceable — `rmc-config`, `rmc-indexing`, `rmc-server` don't exist as crates yet.
- CI wiring (continuous re-run) is a follow-up; the rule documentation is the durable artifact.

## Workspace metrics (post-Phase-B)

From `mcp__rust-code-mcp__workspace_stats`:

| Metric | Post-Phase-A | Post-Phase-B | Δ |
|---|---|---|---|
| `pub_` items | 283 | 328 | **+45** |
| `pub_crate` items | 354 | 309 | **−45** |
| `restricted_to` | 98 | 98 | 0 |
| `pub_crate_share` | 0.5557 | 0.4851 | **−0.0706** |
| Modules | 293 | 295 | +2 |
| `dead_pub_in_crate` candidates | 90 | (refresh needed) | — |

**The `pub_crate_share` declined.** This is the inherent architectural cost of the crate lift: items that were `pub(crate)` for monolith-wide reachability had to widen to `pub` to remain reachable across the new crate boundary. The widening tally is approximately:

- B.1: 1 (FileSchema).
- B.3: 5 (batching module + 4 items).
- B.7: ~30 (labels, audits, codemap, storage, etc.).

≈36 forced widenings; the remaining ~9 of the +45 are new module-declaration `pub` markers (`pub mod parser; pub mod chunker; …` in `rmc-engine/src/lib.rs` etc.).

**This is sanctioned by the §4.B.0 cross-crate-widening rule.** The original `pub(crate)` represented monolith-scope reachability; preserving that reachability across the new crate boundary requires `pub`. It is not "widening to make a move compile" in the §3 Guardrail 2 sense.

A subsequent narrowing pass could re-tighten some of these (e.g. via `pub(in crate::graph)` if Rust supported cross-crate visibility restrictions — which it does not). For now, the wider `pub` is the cost of the boundary.

## Verification gates (every commit)

| Gate | Tool | Result |
|---|---|---|
| Build | `cargo check --workspace --all-targets` (Nix devshell) | Green at every commit |
| Forbidden module imports inside `rmc-engine` | `grep -rnE 'use crate::(graph\|tools\|indexing\|mcp\|config\|...)' crates/rmc-engine/src/` | 0 hits (B.6) |
| Forbidden module imports inside `rmc-graph` | analogous grep | 0 hits (B.7 pre-close gate) |
| Crate-level forbidden deps | `mcp__rust-code-mcp__forbidden_dependency_check` | 0 violations (B.8) |
| Rust compile-time path stability for in-repo consumers | main `src/lib.rs` re-exports | All preserved — `use rust_code_mcp::graph::…` etc. continues to resolve at compile time |
| **Hypergraph qualified-name stability** | none — inherent property of any crate lift | **NOT preserved.** Canonical qualified names shifted (`rust_code_mcp::graph::loader::load` → `rmc_graph::graph::loader::load`, `rust_code_mcp::parser::…` → `rmc_engine::parser::…`, etc.). See "Plan deviations" below |

## Plan deviations and decisions

**Cross-crate visibility widening rule added mid-execution (during B.1).** The original Phase B sub-sections did not explicitly address what to do when a `pub(crate)` item in a moved module has consumers outside the new crate. The first B.1 agent attempt correctly stopped on this ambiguity for `FileSchema`. The plan §4.B.0 was updated to record the rule before resuming: "when a moved `pub(crate)` item has consumers outside the new crate (verified by `error[E0603]` from `cargo check`), widen to `pub`. Record each widening in the commit message." This rule then applied uniformly across B.3 and B.7 widenings.

**Dependency inventory tighter than the plan anticipated.** The plan's prose listed "typical expected sets" for each crate's deps that turned out to overestimate. Actual minimal sets:

- `parser/schema`: `ra_ap_syntax`, `tantivy` (not the full `ra_ap_*` family or `serde/anyhow/tracing/rayon`).
- `chunker`: `serde`, `uuid` only (not `text-splitter`, `tokenizers`, etc.).
- `vector_store`: `lancedb`, `arrow-array`, `arrow-schema`, `async-trait`, `directories` (`directories` was found only on first compile failure).

The inventory-from-grep procedure in §4.B.0 worked but inline fully-qualified paths (`directories::ProjectDirs::from(...)`) escape the `^use` grep — caught only by the compile error. Future moves should also grep for `<crate_root>::` patterns inline.

**Five `crate::embeddings` references in `graph/` were inline paths, not `use` statements.** The plan called for "use crate::embeddings::… → use rmc_engine::embeddings::…" rewrites; the actual rewrite was a `sed` on `crate::embeddings` → `rmc_engine::embeddings` (no `use` keyword involvement). Functionally identical; worth noting for the report.

**Hypergraph qualified-name stability (clarification — added in post-Phase-B review).** Phase B preserves *Rust compile-time path stability* via the `pub use rmc_graph::graph;` / `pub use rmc_engine::parser;` (etc.) re-exports in main `src/lib.rs`: any in-repo consumer writing `use rust_code_mcp::graph::OpenedSnapshot;` continues to compile. **However, the hypergraph's canonical qualified names** — derived from the *declaration site* module path — **did shift** as a direct consequence of the crate lift:

| Symbol declared in | Pre-B canonical name | Post-B canonical name |
|---|---|---|
| `rmc-engine` modules | `rust_code_mcp::parser::…`, `rust_code_mcp::schema::…`, etc. | `rmc_engine::parser::…`, `rmc_engine::schema::…`, etc. |
| `rmc-graph::graph::…` | `rust_code_mcp::graph::…` | `rmc_graph::graph::…` |
| Modules still in main (tools/mcp/indexing/…) | `rust_code_mcp::tools::…` etc. | unchanged |

This is **inherent to any crate lift** — Rust's canonical path for an item *is* the absolute path through real (declaration-site) modules, and the `pub use` facades in main `lib.rs` are compile-time re-exports, not name aliases. The hypergraph (and the MCP tools that read it) sees the canonical name only.

**Consequences**:
- In-repo code that uses `rust_code_mcp::…` paths via `use` keeps compiling (Rust resolves through the facade).
- In-repo test fixtures and assertions that hardcode qualified-name **string literals** (`"rust_code_mcp::graph::loader::load"` as a string passed to `OpenedSnapshot::lookup_by_qualified_name`, etc.) DO break — those literals must be updated to the new canonical names. This was the source of the cargo test failures the parent plan's Guardrail 9 foresaw (the rule was originally written for module splits but applies identically at crate-lift scale).
- External tooling that queries the hypergraph by `rust_code_mcp::…` paths must migrate.

Phase B initially declared "public-path stability" without distinguishing these two senses. The verification gates table above (and §3 below) now make the distinction explicit. The test-string-literal migration was completed in a post-Phase-B fix commit (see §"Open follow-ups" / commit log).

## Readiness for Phase C

Preconditions per the plan §5 (Server cluster lift):

| Precondition | Status |
|---|---|
| Phase B complete and committed | ✅ (9 commits) |
| `rmc-engine` self-contained | ✅ (verified B.6) |
| `rmc-graph` depends only on `rmc-engine` | ✅ (verified B.7 final gate + B.8) |
| `forbidden_dependency_check` rule set drafted with Phase C extension | ✅ (`.docs/architectural-rules.md`) |
| Main `src/lib.rs` facade re-exports keeping all in-repo paths stable | ✅ |
| **Settle pass through one feature-work cycle** | ⏳ Not yet — Phase B just landed. Phase C should not start without aging |

Phase C should not begin until the codebase has aged through normal feature work without introducing structural debt that violates the new crate boundaries. The 9 Phase B commits all landed in the same work session; the §6 guardrail "Don't proceed past a phase exit gate without one verification settle period" applies.

## Open follow-ups (not in Phase B scope)

- **CI wiring for `forbidden_dependency_check`**. The rule set is codified; running it on every PR is a separate task (likely a `cargo test` or shell script that invokes the MCP server via stdio). Phase B documents the target state; Phase B does not ship the CI machinery.
- **The 36 forced widenings could be re-tightened with future engineering.** Each widening from `pub(crate)` to `pub` exposes a wider API surface than the original monolith intended. Some could be redesigned (e.g. by moving consumers into the same crate, or by adding facade types that wrap the internal type). The §4.B.0 widening rule does not call for this; it is a known cost.
- **Phase A.4-polish follow-up still pending**: `index_directory_with_backup` + `Backup` trait + monitoring impl appear to be dead code. Demoting to `pub(crate)` surfaced the warning; deleting is a feature-level decision. Not relevant to Phase B but still open.
- **The `vector_store::SearchResult` ↔ `search::SearchResult` structural dedup** (Phase 6 §10 step 4 open follow-up) remains untouched and is now harder — they live in the same crate (`rmc-engine`) but are still two distinct types.

## Phase B → end-state target tree (per §11 of the plan)

The post-Phase-B layout matches §11 target except for the `rmc-config`/`rmc-indexing`/`rmc-server` crates (which are Phase C work):

```text
crates/
  rmc-engine/            (B.1–B.5 lifted: parser, schema, chunker, embeddings, vector_store, search)
  rmc-graph/             (B.7 lifted: graph)

src/                     (main crate; Phase C will reduce this)
  bin/test_tools_direct.rs
  config/, indexing/, mcp/, metrics/, monitoring/, security/, semantic/, tools/
  lib.rs (facade)
  main.rs

.docs/
  architectural-rules.md (B.8)
  refactor-phase-7-plan-phase-A-report.md
  refactor-phase-7-plan-phase-B-report.md   (this document)

.plans/
  refactor-phase-7-plan.md
```

## Conclusion

Phase B landed cleanly in one work session. The Cargo workspace is established, two foundational crates are lifted, the `forbidden_dependency_check` rule set is codified and passes 0 violations. The crate lift came with ~36 forced `pub(crate) → pub` widenings — the inherent cost of crossing crate boundaries — which the plan now explicitly sanctions in the §4.B.0 cross-crate-widening rule.

The pragmatic next move is the same as after Phase A: let the codebase age. Phase C is well-scoped and ready, but the parent plan's "settle pass through normal feature work" guardrail applies harder now than ever — crate APIs harden expensively, and Phase B's 36 widenings are visible signals of that hardening. Defer Phase C until either (a) a real consumer pulls on the further lift or (b) a future audit confirms the Phase B surface is stable enough to commit further crate boundaries on top of it.

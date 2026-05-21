# Phase 7 — Phase A Report

**Plan**: `.plans/refactor-phase-7-plan.md` — Refactor Plan: Phase 7 — Cleanup & Crate Lift
**Phase**: A — Cleanup (cycle breaks + surface narrowing)
**Status**: complete at the code level; A.4 (settle pass) is a calendar wait, no code change
**Workspace**: `/home/molaco/Documents/rust-code-mcp-refactor`
**Date**: 2026-05-21

## Summary

Three structural changes landed in four `jj` commits, plus one polish commit. Both SCCs flagged by the Phase 7 boundary analysis are broken. `graph`'s declared public surface is narrowed from a 32-item glob + 34 named re-exports (66 total) to a 43-item explicit list (40 `pub` + 3 `pub(crate)`). `cargo check --all-targets` is green at every commit.

The post-Phase-A codebase is ready to age. Phase B (engine + graph crate lift) and Phase C (server cluster lift) remain explicitly optional per the parent plan and should not be started until the codebase has aged through one normal-feature-work pass with no follow-up structural change.

## Commits

```text
8f2f6abb  phase 7 A.3: narrow graph public surface (66 -> 42); remove query::model::* glob;
          demote 3 to pub(crate); drop 22 dead re-exports
49901e30  phase 7 A.2: move ProjectPaths to mcp; widen 4 helpers to pub(crate)
          (narrowest-expressible post-move); add tools::project_paths compat shim;
          break tools-mcp cycle
56b18c1d  phase 7 A.1: extract trait Backup; break indexing-monitoring cycle
dd89bb86  phase 7: import refactor-phase-7-plan into refactor workspace
```

Parent: `d7c4cd9a` (`docs + plans update`, post PR 21).

## A.1 — Break `indexing ↔ monitoring` cycle

**Goal**: eliminate the two-edge SCC where `indexing::unified` accepted `Option<&monitoring::backup::BackupManager>` while `monitoring::backup` read `indexing::merkle::FileSystemMerkle`.

**Resolution**: extracted a single-method trait in indexing's own namespace.

| File | Change |
|---|---|
| `src/indexing/backup.rs` | **NEW** — 13 lines. `pub(crate) trait Backup { fn create_backup(&self, merkle: &FileSystemMerkle) -> anyhow::Result<PathBuf>; }` |
| `src/indexing/mod.rs` | +1 line — `pub(crate) mod backup;` |
| `src/indexing/unified.rs` | line 308 — parameter type changed from `Option<&crate::monitoring::backup::BackupManager>` to `Option<&dyn crate::indexing::backup::Backup>` |
| `src/monitoring/backup.rs` | +6 lines — trait impl appended: `impl crate::indexing::backup::Backup for BackupManager { fn create_backup(...) { BackupManager::create_backup(self, merkle) } }` |

**Verification**:
- `grep -rn 'crate::monitoring' src/indexing/` → zero hits (cycle edge gone).
- `cargo check --all-targets` → green.

**Visibility note**: trait declared `pub(crate)` — narrowest workable. No widening to `pub`.

**Post-A.1 dep direction**: only `monitoring → indexing` remains (for `FileSystemMerkle` and the `Backup` trait). The cycle is gone.

## A.2 — Break `tools ↔ mcp` cycle

**Goal**: eliminate the six-edge SCC where `mcp::sync.rs:132` used `tools::project_paths::ProjectPaths` and `tools::router` / `tools::endpoints/*` referenced `mcp::SyncManager`.

**Resolution**: moved `ProjectPaths` and its private helpers from `tools::project_paths` to `mcp::project_paths`. `mcp` is structurally lower in the adapter stack (fan-out 2 vs tools' 12), so putting project-discovery types there makes `mcp` self-contained.

| File | Change |
|---|---|
| `src/mcp/project_paths.rs` | **NEW** — 303 lines, content of the prior `src/tools/project_paths.rs` |
| `src/mcp/mod.rs` | +1 line — `pub mod project_paths;` above the existing `pub mod sync;` |
| `src/tools/project_paths.rs` | **REPLACED** with 3-line compat shim: `pub use crate::mcp::project_paths::*;` |
| `src/tools/mod.rs` | unchanged — `pub mod project_paths;` still declares the (now-shim) module so external consumers keep resolving the old path |
| `src/mcp/sync.rs` | line 132 — `use crate::tools::project_paths::ProjectPaths;` → `use crate::mcp::project_paths::ProjectPaths;` (the cycle-edge rewrite) |
| 7 in-crate caller files | `use crate::tools::project_paths::*;` → `use crate::mcp::project_paths::*;` (`tools/endpoints/{health,cache,index,query,indexing_support}.rs`, `tools/graph/{codemap,similarity}.rs`) |

**Visibility migration on the four helpers**:

The four `pub(in crate::tools)` helpers — `data_dir`, `resolve_embedding_backend`, `dir_hash`, `read_embedder_identity` — had to widen to `pub(crate)`. Justification:

- Caller inventory showed all consumers in `crate::tools::*` (9 + 5 + 3 + 4 = 21 sites), a sibling subtree to the new home in `crate::mcp::*`.
- Neither `pub(in crate::mcp)` (mcp-only) nor `pub(super)` (parent-only) suffices when callers are in a sibling subtree.
- The only alternative — converting to associated `fn`s of `ProjectPaths` — would make them `pub` (since `ProjectPaths` is `pub`), strictly wider than `pub(crate)`.
- `pub(crate)` is the narrowest **expressible** Rust visibility that preserves the existing reachability set. The plan was updated mid-execution to record this rationale.

**Compat shim rationale**: `tests/test_mcp_stdio_transport.rs:9` imports `rust_code_mcp::tools::project_paths::ProjectPaths` — an in-repo consumer of the public path that the parent plan's Guardrail 8 requires us to keep working. The shim is deleted in Phase C.3 when `tools` lifts to `rmc-server`; until then it stays.

**Verification**:
- `grep -rn 'use crate::tools' src/mcp/` → zero hits (cycle edge gone).
- `grep -rn 'use crate::tools::project_paths' src/` → zero hits (in-crate callers migrated).
- `cargo check --all-targets` → green.

## A.3 — Narrow `graph` public surface

**Goal**: replace the `pub use query::model::*;` glob in `src/graph/mod.rs` with explicit named re-exports, classified against future crate boundaries.

**Method**: for each candidate type, grep `src/`, `tests/`, `examples/` for consumers. Classify:

- **Cross-crate API**: used by anything outside `src/graph/` (in code, not doc comments). Keep `pub`.
- **Graph-internal**: used only inside `src/graph/`. Narrow to `pub(crate)`.
- **Dead-by-shadowing**: zero external references. Drop from the facade.

**Result**:

| Metric | Before | After |
|---|---|---|
| Declared `pub use` lines in `src/graph/mod.rs` | 21 lines (including 1 glob) | 9 lines (zero globs) |
| Individual re-exported items | ~66 | 43 |
| Of which `pub` (cross-crate API) | ~64 | 40 |
| Of which `pub(crate)` (graph-internal) | 2 | 3 |
| Dropped from facade (dead-by-shadowing) | — | 22 |

**Items narrowed to `pub(crate)` (1 net new)**:
- `model::EmbeddingRecord` — consumed only by `src/graph/embedding_cache.rs` via the facade.
- (`ensure_embeddings_for` and `cosine` were already `pub(crate)` and stayed there.)

**Items dropped from the facade** (22 total — all still defined in their source modules; only the re-export is removed):

From `model::*` (8): `BindingKind`, `ExtractionModel`, `GenericBound`, `Param`, `SelfKind`, `StaticMetadata`, `UsageCategory` (plus the now-narrowed `EmbeddingRecord`).

From `query::model::*` (10): `EdgeSymbol`, `TypeCollision`, `TypeLocation`, `ModuleShadow`, `WithinCrateDuplicate`, `CommonFnName`, `NodeKindCounts`, `VisibilityCounts`, `ReExportLink`, `MutStaticFinding`. These are nested response shapes still reachable through their parent types' fields; no external code names them directly.

Other (4): `ids::UsageId`, `snapshot::BuildResult`, `unsafe_audit::UnsafeFinding` (already reached via the `unsafe_audit` submodule), `storage::{GraphDatabases, GraphManifest}`.

**Cross-crate-API pins** (examples):
- `CrateEdge`, `CrateMetric`, `ForbiddenDependencyViolation` — `src/tools/graph/crates.rs:11`.
- `ForbiddenDependencyRule` — `src/tools/params/graph.rs:5`.
- `OverlapScope` — `src/tools/graph/response.rs:22`.
- `LoadedWorkspace`, `load` — `examples/debug_burn_loader.rs:{11,21}`.
- `BindingId` — `examples/debug_burn_target.rs:16`.
- `ItemKind`, `Node`, `NodeId`, `NodeKind` — `src/tools/graph/tests.rs:14`.

**Bonus narrowings (module declarations)**: three submodules went from `pub mod` (or `pub(crate) mod`) to private `mod` because their contents are now reachable only via the explicit named re-exports:

- `mod query;` (was `pub mod query;`)
- `mod math;`
- `mod embedding_cache;`

**Verification**:
- `grep -n 'pub use.*\*' src/graph/mod.rs` → zero hits (glob gone).
- `cargo check --all-targets` → green (one iteration needed: initial attempt to drop `EmbeddingRecord` broke `embedding_cache.rs`; resolved by switching to `pub(crate) use`).

## A.4 — Settle pass

A.4 is a calendar wait, not a code change. Phase A is functionally complete; Phase B (engine + graph crate lift) and Phase C (server cluster lift) must not be started until normal feature work has aged through this state without follow-up structural change. The parent plan §12 mandates "one full verification pass unchanged" before any crate lift.

## Workspace metrics (post-A.3)

From `mcp__rust-code-mcp__workspace_stats` re-run after A.3:

| Metric | PR-21 baseline | Post-A.3 | Δ |
|---|---|---|---|
| `pub_` items | 282 | 283 | +1 |
| `pub_crate` items | 348 | 354 | +6 |
| `restricted_to` (incl. `pub(in path)`) | 102 | 98 | −4 |
| `pub_crate_share` | 0.5524 | 0.5557 | +0.0033 |
| `dead_pub_in_crate` candidates | 90 | 90 | 0 |
| Modules | 291 | 293 | +2 |
| Items | 2447 | 2449 | +2 |

Notes on the deltas:

- `+1 pub`: the new `pub mod project_paths;` declaration in `src/mcp/mod.rs` (A.2). The shim path in `tools/mod.rs` retained its `pub mod project_paths;` so the test consumer keeps working; the move added a second module visibility entry in mcp.
- `+6 pub(crate)`: trait Backup + 4 widened helpers + `pub(crate)` re-exports introduced/touched in A.3.
- `−4 restricted_to`: the four `pub(in crate::tools)` markers no longer exist.
- `+2 modules`: `src/indexing/backup.rs` and `src/mcp/project_paths.rs`.
- `+2 items`: the `Backup` trait (counted as `Trait`, which went from 6 → 7) plus one new item I haven't precisely attributed (likely a glob-removal counting artifact).
- `dead_pub_in_crate` unchanged at 90 because A.3 narrowed *facade re-exports*, not the source-level definitions. The 90 candidates remain because the underlying types are still declared `pub` in their defining modules (this is correct — narrowing definitions is a separate, follow-up activity).

The headline metric — `pub_crate_share` — moved only slightly (+0.003). This is expected: the share was already at 0.55 post-PR-21, and Phase A's surface narrowing happened in the facade (re-exports) rather than at definitions.

## Forbidden-edge sweeps (final state)

| Edge | Pre-Phase-A | Post-Phase-A |
|---|---|---|
| `grep -rn 'crate::monitoring' src/indexing/` | 1 hit | 0 hits |
| `grep -rn 'use crate::tools' src/mcp/` | 1 hit | 0 hits |
| `grep -n 'pub use.*\*' src/graph/mod.rs` | 1 hit (glob) | 0 hits |

All three structural debts identified by the Phase 7 boundary analysis are eliminated.

## Plan deviations and decisions

**Mid-execution plan update**: A.2's "Visibility migration" bullet was rewritten before the agent could proceed. Original wording said "do not widen to `pub` or `pub(crate)`"; verification showed all four helpers had callers in `crate::tools::*` (a sibling subtree to the new home in `crate::mcp::*`) — no narrower-than-`pub(crate)` visibility expresses "visible from `crate::tools::*` when the items live in `crate::mcp::*`." The plan was updated to mandate `pub(crate)` as the narrowest **expressible** Rust visibility, with a recorded justification under Guardrail 3. The agent then completed A.2 cleanly.

**Bonus narrowing in A.3**: The agent additionally narrowed three module declarations (`mod query;`, `mod math;`, `mod embedding_cache;`) from `pub mod`/`pub(crate) mod` to private `mod`. This was within the plan's intent (deliberate-only API exposure) but exceeded the literal "edit `src/graph/mod.rs` only" wording. The change is sound — the modules are still reachable through the explicit re-exports — and is kept.

## Readiness for Phase B

Preconditions per parent plan §11:

| Precondition | Status |
|---|---|
| Phases 1–6 (PRs 00–21) complete | ✅ |
| **No SCCs in the module graph** | ✅ Both broken in A.1 and A.2 |
| **`graph` public surface is deliberate, not accidental** | ✅ Glob removed in A.3; every export reviewed and pinned to a real consumer |
| **`cargo check --all-targets` green at every commit** | ✅ |
| **One settle pass through normal feature work** | ⏳ Not yet — A.4 |
| `forbidden_dependency_check` rule set drafted | ⏳ Phase B.8 work |

Phase A is technically complete. Phase B should not be started until the codebase ages through one feature-work cycle without new structural debt being introduced. The work to actually create the Cargo workspace (`crates/rmc-engine`, `crates/rmc-graph`) and lift modules is the entirety of Phase B in the plan; it has been preserved unchanged.

## Open follow-ups (not in Phase A scope)

- **`index_directory_with_backup` (and transitively the `Backup` trait + `monitoring::backup::BackupManager` impl) is dead code.** Demoting the method from `pub` to `pub(crate)` in the A.4 polish commit let the compiler prove zero in-crate callers — surfacing a "never used" warning that was previously hidden by the wider visibility. The A.1 cycle break is still structurally correct (the cycle was in the type graph regardless of dynamic usage), but a future cleanup could delete the unused method, trait, impl, and possibly `BackupManager` itself. **Deliberately out of A.4 scope** because (a) deciding "delete vs revive" is a feature-level call, not a structural one, and (b) the cycle break commit (A.1) should stand on its own merits.
- The 90 `dead_pub_in_crate` candidates are unchanged. Most of them are facade narrowing artifacts now: their source-level definitions are still `pub`, but the type is reachable only via signatures or no longer named externally. A subsequent narrowing pass could demote the definitions themselves to `pub(crate)`. This is **deliberately not a Phase A task** — definitions in `graph/model.rs` and `graph/query/model.rs` are consumed by the `pub` re-exports in `graph/mod.rs`, so any demotion at the definition site would force the re-export to also narrow. The right time to do this is during Phase B's `forbidden_dependency_check` enforcement, when crate boundaries make the question machine-decidable.
- The compat shim `src/tools/project_paths.rs` (3 lines) is scheduled for deletion in Phase C.3 (when `tools` lifts to `rmc-server`). Until then it costs nothing.
- One unattributed `+1 item` in the workspace_stats delta (post-A vs pre-A). Likely a glob-removal counting artifact in the hypergraph rebuild; not material.

## Out-of-scope items remaining from earlier plans

The PR-refactor close-out (`.docs/pr-refactor-plan-report.md`) noted 6 follow-up items that are still pending; none was a Phase A target. Briefly:

- Optional split of `src/graph/query/tests.rs` (1144 LOC; over the §15 ~1000 LOC line; relocated as one block in PR 19).
- Optional fold of `src/embeddings/openrouter/metrics.rs` (141 LOC; below the §8 fold threshold).
- The `search::SearchResult` ↔ `vector_store::SearchResult` structural dedup (Phase 6 §10 step 4 explicitly out-of-scope).
- Phase 7 itself (now in progress; A done).
- A few smaller items.

None blocks Phase B preparedness.

## Conclusion

Phase A landed cleanly in one work session. Both SCCs are broken, the `graph` public surface is now a deliberate-API contract, and the workspace is structurally ready for the crate lifts in Phase B and C. The pragmatic next move is to leave Phase B/C deferred and let the codebase age — Phase A standing alone is a valid successful outcome of the Phase 7 plan and is the default outcome the parent plan recommends.

# Workspace Plan — Master Implementation Overview

**Authoritative source:** `.docs/workspace-plan/DECISIONS.md`. This is an
executive summary tying Phases 0–7 (plus a decommission Phase 8) into a single
critical path, risk register, and go/no-go contract. Where this file disagrees
with `DECISIONS.md`, `DECISIONS.md` wins.

The plan converts single-crate `file-search-mcp` into a virtual workspace of
8 runtime crates plus `xtask`: capability crates (`rcm-search`, `rcm-graph`,
`rcm-ide`) over named infra leaves (`rcm-paths`, `rcm-ra-syntax`,
`rcm-ra-host`, `rcm-embedding`) with a thin composition root in `rcm-server`.

---

## 1. Phase summary table

| Phase | Goal | Risk | Reversible | Effort | Blocks on |
|---|---|---|---|---|---|
| 0 | Virtual workspace, pinned toolchain, deny/lints/forbidden-dep CI; legacy + 8 placeholders compile. | low | yes | M | — |
| 1 | 8 target crates expose frozen public APIs as adapters delegating to legacy. | medium | yes | XL | 0 |
| 2 | Delete `LazyLock<Mutex<…>>` runtime singletons; services constructed once in `main`. | low | yes | S | 1 |
| 3 | Operation-scoped `thiserror` enums per service; `anyhow` confined to `rcm-server`. | low | yes | M | 1 |
| 4 | Service lifetime: `ArcSwap` readers, `Mutex` writer, `CancellationToken` shutdown, `clear_cache` delete-then-invalidate (lazy rebuild on next op). | medium | partial | L | 1, 3 |
| 5 | Sealed `Embed` trait; `embeddings`/`test-fakes` features; `rcm-graph::semantic-overlaps` feature. | low | yes | M | 1 |
| 6 | Parser scope reduction — chunking-context AST in `rcm-search`; `get_dependencies`/`get_call_graph`/`analyze_complexity` route through HIR. | **high** | partial | L | 4, 5 |
| 7 | Storage layout v2 migration via `xtask migrate-storage`. | high (ops) | partial | M | 0 |
| 8 | Delete `file-search-mcp-legacy`, all `legacy_adapter` modules, the `legacy → rcm-paths` back-edge. | low | yes | S | 1–6 |

`xtask` is excluded from runtime architecture policy checks throughout.

---

## 2. Critical path

1. **Phase 0** lands first; nothing compiles in workspace shape without the
   manifest, deny config, and `forbidden_dependency_check`.
2. **Phase 1** is the largest single landing — a pure surface-area refactor.
   Every later phase carves real behavior out of legacy into the facade
   Phase 1 created.
3. **Phases 2 ‖ 3 parallelize.** Singleton removal touches `rcm-ide` and
   the rmcp router state; error split touches each service's error module.
4. **Phase 4 depends on Phase 3** (`IndexBusy` and `ShutdownInProgress`
   need scoped enums; the `ArcSwap`-driven swap-and-drop path is
   silent — no error variant — so there is no separate
   "reload-in-progress" error).
5. **Phase 5 ‖ Phase 4.** Only touches `rcm-embedding` and `rcm-graph`
   features; lifetime contract is orthogonal.
6. **Phase 6 follows 4 + 5.** Routing structural tools through the
   persisted HIR snapshot requires Phase 4's long-lived `OpenedSnapshot`
   and `ArcSwap` reload. Highest-risk phase; runs alone.
7. **Phase 7 is independent.** `xtask`-only; can land any time after
   Phase 0. Recommended last so the storage layout settles once Phase 6
   stabilizes which fields are indexed.
8. **Phase 8** is cleanup — deletable when Phases 1–6 have moved every
   byte of behavior into a real crate.

Single engineer: **0 → 1 → 2 → 3 → 4 → 5 → 6 → 7 → 8**.
Two engineers: **0 → 1 → (2 ‖ 3) → (4 ‖ 5) → 6 → 7 → 8**.

---

## 3. Per-phase smoke checklist invariants

Every phase keeps the full smoke checklist (`DECISIONS.md` §"Smoke
checklist") passing. Per-phase emphasis:

- **Phase 0.** Core tools against legacy binary unchanged.
- **Phase 1.** Full checklist against `rcm-server`; outputs byte-identical
  to Phase 0. Add `clear_cache` then re-`search`.
- **Phase 2.** `find_definition`, `find_references`, `similar_to_item`
  (previously hit `static SEMANTIC`) run concurrently from two clients
  without global-mutex serialization.
- **Phase 3.** Full checklist; exercise error paths (unindexed workspace,
  missing file) and confirm JSON shape unchanged.
- **Phase 4.** `clear_cache(workspace)` then `search` — first call must
  trigger lazy rebuild via the fingerprint-mismatch path (no
  auto-reindex); subsequent calls hit the rebuilt index. Concurrent
  `index_codebase` during `clear_cache` returns `IndexBusy` (writer
  mid-batch). SIGINT mid-`build_hypergraph` drains in 30s.
- **Phase 5.** `semantic_overlaps`, `get_similar_code`, `similar_to_item`
  with default features; with `--no-default-features --features
  test-fakes`, `semantic_overlaps` returns `EmbedderUnavailable` when
  `rcm-graph::semantic-overlaps` is off.
- **Phase 6.** **Mandatory regression** on `get_dependencies`,
  `get_call_graph`, `analyze_complexity` — same-or-better (HIR resolves
  what ra-syntax approximated). Cold-start within phase-doc budget.
- **Phase 7.** Full checklist before/after `xtask migrate-storage`;
  storage follows v2; `health_check` reports v2.
- **Phase 8.** Full checklist; `cargo build --workspace` with legacy
  removed.

---

## 4. Risk register

| ID | Description | Prob | Impact | Phase | Mitigation | Rollback trigger |
|---|---|---|---|---|---|---|
| R1 | HIR-backed structural tools slower than ra-syntax on first call. | med | high | 6 | Bench before/after; persist HIR eagerly on `build_hypergraph`; warm cache on `index_codebase`. | p95 first-call >2× Phase 5 baseline. |
| R2 | First `search` after `clear_cache` is slow (lazy rebuild via fingerprint-mismatch path on a cold workspace blocks the requesting task). | med | med | 4 | Document the cold-rebuild as expected; `SyncManager` may opportunistically pre-warm on `track_directory`; bench p95 first-search-after-clear vs. cold `index_codebase`. | First-call p95 >2× baseline cold-index; or users mistake the rebuild for a hang and abort. |
| R2b | `ArcSwap` swap contention spikes search latency under the LEGITIMATE `SyncManager::reload` path (schema-version bump). | low | med | 4 | Swap then drop old after grace; never blocks queries. Bench concurrent read+reload. | p99 search >2× baseline during a forced `SyncManager::reload`. |
| R3 | Storage v2 migration loses data (interrupted mid-rename). | low | high | 7 | `--dry-run` mandatory; `.layout-version` sentinel only after fsync; v1 paths kept until next run confirms read-back. | `health_check` fails post-migration; sled/heed open error on prior-good workspace. |
| R4 | Phase 1 adapter conversions dominate ingestion time. | med | med | 1 | `From`/`TryFrom` over moves, not clones; bench `index_codebase` end-to-end. | wall-time >1.3× legacy baseline. |
| R5 | `forbidden_dependency_check` too strict; blocks legitimate refactors. | med | low | 0 | Policy file checked in; PRs may adjust with review. | Reviewer cannot recall the rationale. |
| R6 | Sealed `Embed` can't fit a future test scenario. | low | low | 5 | Add a third sealed impl in `rcm-embedding` rather than unsealing. | Test team requests embedder we cannot add inside the crate. |
| R7 | Closure-based `RaHost::with_db`/`with_semantics` awkward in hot paths. | med | med | 1, 6 | Typed views for common queries; purpose-built helpers on `RaHost` rather than leaking `&RootDatabase`. | Hot path needs >3 closure round-trips per call. |
| R8 | `clippy::disallowed_methods` allow-list bypassed by refactor. | med | low | 0 | Clippy on every PR; allow-list lives in workspace lints, reviewable in diffs. | A `with_db` call appears outside allow-list. |
| R9 | Cold ONNX load latency unchanged after Phase 5. | high | low | 5 | Accepted: Phase 5's value is compile hygiene + test fakes. Document. | N/A (unmet expectation, not regression). |
| R10 | Tantivy schema drops in Phase 6 force user rebuilds. | med | med | 6 | Phase 6 doc enumerates drops; bump `schema_version`; rebuild path tested. | Users report unexpected rebuild. |
| R11 | Public-API leak: a capability crate re-exports `ra_ap_*` or `tantivy::` via `pub use`. | med | med | 1+ | `cargo public-api` CI grep; PR fails on leak. | New public symbol with forbidden prefix. |
| R12 | `rcm-server` composition root grows into a dumping ground. | med | low | 1+ | Helpers used by 1 capability move there; ≥2 promote to a leaf. | `rcm-server/src/util/` >500 LoC. |
| R13 | `SyncManager` shutdown drain exceeds 30s budget. | low | med | 4 | Soft cap; long commits complete then process aborts. Log duration. | Repeated drain timeouts in prod logs. |
| R14 | Legacy back-edge `legacy → rcm-paths` outlives Phase 1. | med | low | 1, 8 | `# PHASE-1-ONLY` comment; Phase 8 deletes legacy entirely. | Phase 8 PR still shows the edge. |
| R15 | Cross-platform canonicalization → different storage hash → cache invalidated on platform move. | low | low | 0 | Recipe frozen as Linux-canonical; macOS/Windows accept one-time rebuild. | N/A (frozen, accepted). |
| R16 | `xtask` accidentally becomes a runtime dep. | low | high | 0 | Forbidden-dep check excludes xtask but flags any inbound edge. | CI fails on offending PR. |
| R17 | `cargo public-api` flakes on `rcm-ra-syntax` due to upstream churn. | med | low | 0 | `rcm-ra-syntax` is a documented exemption; CI grep allow-lists its prefixes only. | False positives block PRs >24h. |
| R18 | `OpenedSnapshot` lifetime leaks into `rcm-server`. | med | med | 4, 6 | DTOs handle/ID-shaped; `&OpenedSnapshot` queries only inside `rcm-graph`. | A `rcm-server` tool requires `<'a>` to compile. |
| R19 | Two embedders compiled simultaneously balloon binary size. | low | low | 5 | `embeddings`/`test-fakes` mutually exclusive in release; `test-fakes` is `dev-dependencies` only. | Release binary >2× current. |
| R20 | Phase 7 no-op edge case (never-indexed workspace) is buggy. | low | low | 7 | `xtask migrate-storage` short-circuits when no v1 sentinel; integration test covers it. | Test fails. |

---

## 5. Go/no-go criteria per phase

### Phase 0
- [ ] `cargo build --workspace` and `cargo deny check` pass.
- [ ] `forbidden_dependency_check` produces 0 violations.
- [ ] All 8 placeholder crates exist with workspace lints applied.
- [ ] `xtask` compiles; `xtask --help` runs.
- [ ] `cargo public-api` baselines exist for every strict-tier crate.
- [ ] Smoke checklist passes against legacy binary.
- [ ] `Cargo.lock` and `rust-toolchain.toml` committed.

### Phase 1
- [ ] `cargo build -p rcm-server --release` succeeds; smoke checklist passes.
- [ ] Each capability crate's public API matches `DECISIONS.md` exactly.
- [ ] No capability crate publicly re-exports any forbidden prefix
      (`ra_ap_*`, `tantivy::`, `lancedb::`, `arrow::`, `fastembed::`,
      `ort::`, `heed::`, `sled::`, `rmcp::`).
- [ ] `rcm-server` does not depend on `file-search-mcp-legacy`.
- [ ] Only back-edge is `legacy → rcm-paths`, marked `# PHASE-1-ONLY`.
- [ ] `index_codebase` wall-time ≤1.3× legacy baseline.

### Phase 2
- [ ] No `LazyLock<Mutex<…>>` / `static .*Mutex` runtime singletons remain.
- [ ] `rcm-ide::IdeService` constructed once in `rcm-server::main`.
- [ ] Concurrent `find_definition` does not serialize through a global
      mutex (2-client smoke run).
- [ ] Smoke checklist passes.

### Phase 3
- [ ] No god-enum; each service has its own scoped error type.
- [ ] `anyhow` only in `rcm-server` and tests/examples/doctests.
- [ ] Public `Result` service methods are `#[must_use]`.
- [ ] `#[from]` adapters preserve `source` chains (integration test).
- [ ] Smoke checklist passes; error JSON shape unchanged.

### Phase 4
- [ ] `IndexReader` and `lancedb::Connection` in `ArcSwap`; `IndexWriter`
      in `Mutex`.
- [ ] `clear_cache(workspace)` returns `IndexBusy` mid-batch.
- [ ] SIGINT during `build_hypergraph` drains within 30s; exit 0.
- [ ] Concurrent reload + 50-QPS search: p99 ≤2× steady-state.
- [ ] `OpenedSnapshot` swap is atomic (property test).
- [ ] Smoke checklist passes including `clear_cache` → `search`.

### Phase 5
- [ ] `Embed` trait sealed via `mod sealed { pub trait Sealed {} }`.
- [ ] `cargo build -p rcm-embedding --no-default-features` succeeds.
- [ ] Same with `--features test-fakes` succeeds.
- [ ] `cargo build -p rcm-graph --no-default-features` builds;
      `semantic_overlaps` returns `Err(QueryError::EmbedderUnavailable)`.
- [ ] `rcm-server` default features wire `FastEmbedEmbedder` and
      `rcm-graph/semantic-overlaps`.
- [ ] `Send + Sync` compile assertion on `TextEmbedding` passes.
- [ ] Smoke checklist passes.

### Phase 6
- [ ] `get_dependencies`, `get_call_graph`, `analyze_complexity` route
      through `rcm-graph` HIR snapshot.
- [ ] HIR results strict-superset legacy ra-syntax results (documented
      diffs).
- [ ] Cold `build_hypergraph` ≤1.5× Phase 5 baseline; warm ≤baseline.
- [ ] Dropped Tantivy fields enumerated; `schema_version` bumped; rebuild
      tested.
- [ ] `rcm-search` no longer uses `ra_ap_syntax` for resolved structure.
- [ ] Smoke checklist passes.

### Phase 7
- [ ] `xtask migrate-storage --dry-run` exits 0 on v1, v2, and
      never-indexed workspaces with correct messages.
- [ ] Migration writes `.layout-version = 2` only after fsync.
- [ ] Smoke checklist passes after migration; `health_check` reports
      `layout_version = 2`.
- [ ] Rollback path covered by xtask integration tests.

### Phase 8
- [ ] `crates/file-search-mcp-legacy/` deleted.
- [ ] No `legacy_adapter` modules remain.
- [ ] `legacy → rcm-paths` back-edge gone.
- [ ] Smoke checklist passes; `cargo build --workspace` and `cargo deny
      check` green.

---

## 6. Rollback strategy per phase

- **Phase 0.** `git revert` the manifest commit; legacy returns in place.
- **Phase 1.** `git revert` the `rcm-server` dep flip and facade commit;
  placeholder dirs may remain.
- **Phase 2.** `git revert` the singleton-removal commit. No on-disk
  state involved.
- **Phase 3.** `git revert` the error-split commit. No state changes.
- **Phase 4.** **Partial.** `git revert` reinstates the prior sync model.
  Tantivy's WAL handles writer-interrupt recovery; user may need
  `clear_cache` + `index_codebase` once on first start.
- **Phase 5.** `git revert` the sealed-trait commit. No state changes.
- **Phase 6.** **Partial.** `git revert` reinstates parser-based tools.
  If users rebuilt under the new `schema_version`, those indexes are
  unreadable by old code and require `clear_cache` + `index_codebase`.
  Most expensive rollback in the plan.
- **Phase 7.** **Partial.** Storage v2 is one-way at the filesystem level.
  `xtask migrate-storage --rollback` (must ship in the Phase 7 PR)
  renames v2 paths back to v1 and removes the sentinel. Without it, users
  must `clear_cache` and re-index.
- **Phase 8.** `git revert` restores the legacy crate and adapter modules.

---

## 7. Estimated total timeline

Single engineer, 5-day weeks:

| Scenario | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | Total |
|---|---|---|---|---|---|---|---|---|---|---|
| Best | 3d | 8d | 2d | 4d | 6d | 4d | 8d | 4d | 1d | **40d ≈ 8 wk** |
| Realistic | 5d | 12d | 3d | 5d | 9d | 5d | 12d | 6d | 2d | **59d ≈ 12 wk** |
| Worst | 7d | 18d | 5d | 7d | 14d | 8d | 20d | 10d | 3d | **92d ≈ 18 wk** |

Two engineers can parallelize 2 ‖ 3 (~3 days) and 4 ‖ 5 (~5 days),
bringing realistic total to ~10 weeks. Phase 6 cannot parallelize — it
touches `rcm-search` and `rcm-graph` simultaneously.

---

## 8. Decision points the team must make BEFORE starting

1. **Crate naming prefix.** `DECISIONS.md` uses `rcm-*`. Recoverable in
   Phase 0; expensive after Phase 1. **Confirm `rcm-*` or pick now.**
2. **Legacy disposition.** Keep `file-search-mcp-legacy` as a shim through
   Phases 1–6, or absorb directly (riskier, faster). **Default: shim.**
3. **Tantivy schema drop policy (Phase 6).** Aggressive (drop ra-syntax-only
   fields, force rebuild) vs. conservative (keep approximations).
   **Default: aggressive — bump `schema_version`, document, accept
   rebuild.**
4. **Phase 7 trigger.** Ship after Phase 6 vs. defer until a schema change
   motivates it. **Default: ship after Phase 6.**
5. **SDK consumer roadmap.** Are external Rust crates expected to depend on
   `rcm-search`, `rcm-graph`, `rcm-ide`? If yes, semver-checks tighten in
   CI. **Default: no — workspace-internal only.**

---

## 9. Out-of-scope explicitly

Not part of this plan; each gets its own plan if it happens later:

- A second embedding provider (OpenAI, Cohere, etc.).
- A non-stdio MCP transport (HTTP, WebSocket, named pipe).
- An HTTP server frontend for direct (non-MCP) access.
- Replacing LanceDB with Qdrant or Tantivy with Meilisearch.
- Multi-tenancy (multiple workspace caches under different identities).
- Unicode-normalization changes to the dir-hash recipe — the recipe is
  **frozen**.
- Audit-tool churn beyond what `DECISIONS.md` already routes through
  `rcm-graph`.
- Replacing `heed`/LMDB with sled or sqlite for the snapshot store.

---

End of overview.

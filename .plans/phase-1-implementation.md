# Phase 0 + Phase 1 — Implementation Plan

Turns `phase-1-plan-v3.md` (design) into a buildable, dependency-ordered plan,
grounded in the current code. Each step: **Goal / Builds on / Net-new / Steps /
Exit / Issues**. Issues are the point — they are collected and ranked at the
end.

Scope: Phase 0 (environment foundation) + Phase 1 (the 5-verb API). Phases 2-4
(synthetic data, RL, flywheel) depend on this and are out of scope here.

---

## Current State (grounded inventory)

Confirmed by reading the source, not assumed:

| Capability | Exists? | Where | Notes |
|---|---|---|---|
| Hypergraph build | yes | `rmc-graph` `snapshot::build_and_persist` | `loader::load` → `extract` → LMDB. **Whole-workspace, 5-18s, cold.** |
| Graph load (RA) | yes | `rmc-graph` `loader::load` → `LoadedWorkspace` | holds RA `RootDatabase`; **discarded after extract** |
| Extract | yes | `rmc-graph` `extract` (`emit_crate` per crate) | whole-workspace walk |
| Persisted store | yes | `rmc-graph` `storage::GraphDatabases` | LMDB (heed), ~15 sub-DBs, **content-addressed IDs**, DUP_SORT secondaries; `embeddings_by_target` already present |
| Query layer | yes | `rmc-graph` `query/*` on `OpenedSnapshot` | LMDB read-only; ~30 tools |
| Merkle change detection | yes | `rmc-indexing` `FileSystemMerkle` | reuse for dirty-file detection |
| Embeddings (Candle/Qwen3) | yes | `rmc-engine` `embeddings`, `EmbeddingGenerator` | reuse for clustering |
| Vector store (LanceDB) | yes | `rmc-engine` `vector_store` | search index, not graph |
| Audits | yes | `rmc-graph` `fn_body_audit`/`unsafe_audit`/`recursion_check`/`channel_audit`/`docs_audit`/`derive_audit` + `query/audits` | reuse as **write-time gates** |
| Semantic similarity / DBSCAN-ish | yes | `rmc-graph` `query/similarity`, `semantic_overlaps` | extend for Analyze |
| Symbol rename (RA) | yes (**preview only**) | `rmc-server` `semantic` `rename_by_*` | seed for propagation; does **not** apply |
| RA semantic service | yes | `rmc-server` `semantic` + `semantic/loader` | find_def / find_ref / rename preview |
| VCS | jj | per AGENTS.md | reuse for rollback |
| Codemap | yes | `rmc-graph` `codemap/*` | task-conditioned subgraph; precursor to the "vision" map |

**Net-new for Phase 0/1:** warm-host incremental writer, source-mutating CRUD
with propagation, write-time gate harness, description index, Analyze/vision
layer, counterfactual simulator, jj rollback wrapper, reward computation,
episode runner.

---

## Three architectural realizations (read before the steps)

1. **Apply == rebuild.** A CRUD op and an incremental graph update are the same
   pipeline: *source edit → RA reload of dirty files → re-extract dirty crates
   → diff-patch LMDB*. Build it once; CRUD and incremental-rebuild both ride it.

2. **Keep LMDB as the query substrate; add a warm RA host as the incremental
   writer.** Don't rip out the read layer. The change is: stop discarding
   `LoadedWorkspace`; keep its `RootDatabase` warm; on edit, `set_file_text`
   the dirty files, re-extract affected crates, diff against existing
   content-addressed keys, patch LMDB. Query layer (`OpenedSnapshot` + `query/*`)
   is untouched. This decouples the already-fast read side from the hard new
   write side.

3. **Two latency monsters, not one.** (a) incremental graph rebuild (addressed
   by warm host); (b) **the reward gate** — `cargo check`/`test` per commit is
   seconds-to-minutes and was under-weighted in the design. Both must be solved
   or rollouts are infeasible.

---

## Dependency graph / critical path

```
P0.1 determinism ─┐
P0.4 bench pool ──┼─────────────────────────────┐
                  │                              │
P0.2 warm-host incremental writer ──► P1.5 CRUD ─┼─► P1.4 simulator
   (apply==rebuild engine)            │          │
                  │                   ├─► P1.6 gates ─┐
P0.3 jj rollback ─┘                   │              ├─► P1.7 commit/reward ─► P1.8 episode runner
                                      │              │        ▲
P1.1 read view (on SLOW build) ───────┘              │        │
P1.2 descriptions ───────────────────────────────────┘        │
P1.3 Analyze/vision ──────────────────────────────────────────┘
```

**Critical path:** `P0.2 (warm-host) → P1.5 (CRUD) → P1.7 (reward) → P1.8`.
P1.1/P1.2/P1.3 (read side) can be built in parallel against the **slow**
full-rebuild (they run once per episode, cached), so P0.2 does **not** block
them — only the write/reward path needs it.

---

# Phase 0 — Foundation

### P0.1 — Determinism & snapshot hardening
- **Goal:** identical workspace state → identical snapshot; one global seed for
  any stochastic step.
- **Builds on:** existing fingerprint in `build_and_persist`.
- **Net-new:** seed plumbing; a reproducibility test.
- **Steps:** (1) audit `extract`/ID derivation for any ordering nondeterminism;
  (2) thread a `seed` through config; (3) golden test: build twice → byte-equal
  LMDB dump (or equal node/binding/usage sets).
- **Exit:** two cold builds of the same tree produce equal graph contents.
- **Issues:** HashMap iteration order in `extract` may already be a source of
  nondeterminism in secondary structures; need to confirm IDs are
  order-independent (they're content-addressed, so likely fine).

### P0.2 — Warm-host incremental writer (THE lethal item)
- **Goal:** <500ms graph update on a small edit; this engine also powers CRUD.
- **Builds on:** `loader::load`/`LoadedWorkspace` (RA `RootDatabase`), `extract`,
  `storage` (content-addressed LMDB), `FileSystemMerkle`.
- **Net-new:** a long-lived `WorkspaceHost` that owns the warm `RootDatabase`;
  a file→affected-crate map; scoped re-extract; LMDB diff-patch.
- **Steps:**
  1. Lift `LoadedWorkspace` into a persistent `WorkspaceHost` (don't discard
     after extract).
  2. Edit ingestion: `apply_edits(files)` → RA `set_file_text` on dirty files.
  3. Dirty→crate mapping: which crates' extraction is invalidated.
  4. Scoped re-extract: run `emit_crate` only for dirty crates (today `extract`
     does all; refactor to per-crate callable).
  5. Diff-patch: compute new Node/Binding/Usage ID sets for dirty crates, diff
     vs existing LMDB keys, delete-removed + insert-new, fix DUP_SORT secondary
     indices (`*_by_target`, `*_by_consumer`, `children_by_parent`, …).
  6. Bench against 100k-LOC workspace, 10-line edit.
- **Exit:** edit→queryable-graph in <500ms; result equals a cold rebuild
  (cross-check vs P0.1 golden).
- **Issues (highest risk in the whole project):**
  - **RA incrementality reach.** Salsa recomputes lazily, but a single fn-body
    edit can invalidate cross-crate inference. Worst case re-extract is not
    "one crate." Need to measure actual invalidation fan-out.
  - **Memory.** A warm `RootDatabase` per workspace is heavy (RA holds the whole
    crate graph + types). Multiple concurrent rollout workspaces multiply this.
  - **Secondary-index patching correctness.** DUP_SORT deletes are fiddly; a
    bug silently corrupts query results → corrupts training signal.
  - **Determinism vs warm host.** Incrementally-recomputed extraction may differ
    in ordering from cold; must match P0.1 invariants on the reward-bearing
    fields at least.
  - **proc-macro / build.rs.** Edits that change generated code may need a
    heavier recompute path.

### P0.3 — jj rollback primitive
- **Goal:** sub-second reset of workspace + graph to a prior state.
- **Builds on:** jj; `WorkspaceHost`.
- **Net-new:** wrapper around `jj op restore`; host re-sync after restore.
- **Steps:** snapshot id before episode; `rollback()` = `jj op restore` +
  re-point `WorkspaceHost` (ideally incremental-patch back, not cold reload).
- **Exit:** rollback returns identical pre-state (graph + files), sub-second.
- **Issues:** after `jj op restore` the warm host is stale — cheap re-sync
  requires diffing restored files vs host state (reuse P0.2 path). If it falls
  back to cold reload, rollback is slow → hurts episodes with frequent resets.

### P0.4 — Benchmark workspace pool
- **Goal:** 50-100 real Rust crates that **build in the nix devshell**.
- **Net-new:** fetch/pin script; per-crate build verification.
- **Steps:** select by size/quality spread; pin revs; verify `cargo check`
  passes for each under `nix develop ../nix-devshells#cuda-code`.
- **Exit:** corpus reproducibly fetched; all members compile.
- **Issues:** many crates won't build cleanly (system deps, nightly features);
  the cargo-gate reward (P1.7) is meaningless on a crate that doesn't compile at
  baseline. Filter aggressively.

---

# Phase 1 — The 5-Verb API

### P1.1 — Read view / Navigate (low risk, start here)
- **Goal:** location model + multi-scale view; `goto/zoom/show_body/
  show_callers/follow_trail`.
- **Builds on:** `query/*` (who_calls, calls_from, exports, re_export_chain,
  module_tree), `codemap`, `skeleton` (body hide/show).
- **Net-new:** location state, map scale levels, `ContextView` assembler +
  serialization.
- **Steps:** (1) define `Location` (crate/module/cluster/item/body); (2)
  `ContextView` composing existing queries; (3) navigation verbs; (4) costed
  `show_body` (reuse skeleton body-strip in reverse).
- **Exit:** an external agent can walk the graph and get coherent views on the
  slow snapshot.
- **Issues:** "cluster" scale needs P1.3; until then map has a hole at the city
  level — build item/module levels first, slot clusters in after P1.3.

### P1.2 — Description index (4th index)
- **Goal:** one-line description per item; merkle-keyed regen; `search_by_description`.
- **Builds on:** `FileSystemMerkle`, skeleton (item text), an LLM sub-model;
  could store in a new LMDB sub-DB beside `embeddings_by_target`.
- **Net-new:** description generator (batched), storage, retrieval (embed
  descriptions into LanceDB, or BM25).
- **Steps:** (1) per-item prompt from skeleton+signature+neighbors; (2) batch
  generate; (3) store keyed by NodeId + content_hash; (4) regen only changed
  items on edit; (5) retrieval index.
- **Exit:** descriptions exist workspace-wide, regen on change, queryable.
- **Issues:** generation cost/throughput; **staleness within an episode** —
  CRUD changes items but regen may lag (acceptable if regen rides the P0.2 dirty
  set); which model (separate small vs main agent — open decision).

### P1.3 — Analyze / vision layer
- **Goal:** `cluster/outliers/affinity/co_change`; the mesoscale "city" map +
  concept labels.
- **Builds on:** `EmbeddingGenerator` + `embeddings_by_target` (Candle),
  `query/similarity`/`semantic_overlaps` (DBSCAN-ish seed), `petgraph` (new dep)
  for graph algos, descriptions (P1.2) for labels, git for co_change.
- **Net-new:** GMM/spectral over embeddings (Candle); item feature vectors;
  LOF/Mahalanobis outliers; node2vec-ish affinity; co-change association
  (Apriori/lift); cluster→label.
- **Steps:** (1) define item feature vector + the "structure" adjacency (from
  call/usage graph); (2) clustering on both substrates, seeded; (3) outliers;
  (4) affinity; (5) co_change from `git log --name-only`; (6) attach concept
  labels; (7) cache per episode.
- **Exit:** stable (seeded) clusters/outliers/affinity; map gains city level.
- **Issues:** feature engineering is under-specified in the design — what
  exactly is an item's vector, what graph does spectral run on; cluster
  **quality is now load-bearing for perception** (bad clusters = astigmatism) →
  need soft membership + zoom-through to raw; co_change needs git history (young
  code = weak signal).

### P1.5 — Structural CRUD with auto-propagation (the heavy item; before P1.4)
- **Goal:** `add/modify_signature/modify_body/delete/move` + `extract_*/inline/
  split/merge` + module/crate ops; each applies atomically or refuses.
- **Builds on:** P0.2 apply==rebuild engine, RA rename (`rename_by_*` — but make
  it **apply**, not preview), `WorkspaceHost`.
- **Net-new:** source mutation + multi-file transaction + propagation per op.
- **Steps (in ascending difficulty):**
  1. `move(target,dest)` — relocate decl, fix `use` paths (RA-assisted), cycle
     check.
  2. `delete(target)` — ref-check; refuse or cascade.
  3. `modify_signature` — apply RA rename mechanics + **callsite synthesis** for
     added params (open decision: refuse / `todo!()` / `callsite_strategy`).
  4. `modify_body` — replace body span; gated by P1.6.
  5. `extract_function`/`extract_trait`/`inline`.
  6. `split_module`/`merge_modules`.
  7. `create_module`/`move_module`/`lift_to_crate`/`lower_to_module` — touches
     `mod` tree + **Cargo.toml**.
- **Exit:** each op atomically mutates source, re-syncs graph via P0.2, or
  refuses with structured reason.
- **Issues:**
  - RA rename is **preview-only** today; applying edits + handling RA refusals
    (keywords, foreign items, conflicts) is new and partial.
  - **Transactionality:** N file edits must apply atomically with clean rollback
    on any failure (lean on jj checkpoint per op).
  - `modify_signature` callsite synthesis — the biggest correctness call.
  - `lift_to_crate`/Cargo.toml edits — workspace manifest surgery, easy to break
    the build.
  - Silent propagation bugs corrupt training data.

### P1.4 — Counterfactual simulator (after P1.5)
- **Goal:** `simulate(op)` → predicted deltas + cascade + would-refuse, no apply.
- **Builds on:** P1.5 apply logic.
- **Net-new:** dry-run mode of the apply engine (compute effects, skip
  persist/source-write).
- **Steps:** factor apply into `compute_effects` + `persist`; `simulate` calls
  the former only.
- **Exit:** simulate predictions match real apply outcomes on a test set.
- **Issues:** **simulate must share apply's exact logic or it lies** — hence
  built after, as a mode of, P1.5; cost of effect-computation if called often.

### P1.6 — Write-time guideline gates
- **Goal:** hard refusals (complexity/params/LOC/nesting/unwrap/unsafe/cycle/
  compile/test) + soft penalties; boundary allowlist.
- **Builds on:** all existing audits (`fn_body_audit`, `unsafe_audit`,
  `recursion_check`, complexity via `analyze_complexity`, cycle via SCC), P0.2.
- **Net-new:** gate harness running audits on the dirty set pre-commit; TOML
  boundary allowlist (read-only to agent); refusal-reason struct.
- **Steps:** (1) wire audits to run on changed items only (fast); (2)
  hard/soft classification + thresholds; (3) allowlist loader; (4) structured
  refusal output.
- **Exit:** a violating op is refused (hard) or penalized (soft) with reason.
- **Issues:** audit latency at write time (must run incrementally on dirty set,
  not workspace-wide); threshold calibration; allowlist format/ownership.

### P1.7 — Commit & reward computation
- **Goal:** `commit()` → compile + tests + audit deltas + graph-metric deltas →
  reward vector; rollback on hard-fail.
- **Builds on:** P0.3 rollback, P1.6 gates, audit/query layer, `petgraph` for
  graph metrics.
- **Net-new:** cargo gate runner (via nix devshell), before/after audit diff,
  delta-computable graph metrics (modularity/conductance/clustering-coef/
  betweenness), reward scalarizer.
- **Steps:** (1) snapshot metrics pre-op; (2) on commit run `cargo check`
  (+ scoped tests); (3) compute audit + metric deltas; (4) assemble vector;
  (5) hard-fail → rollback.
- **Exit:** commit returns the full reward vector; failing commit reverts.
- **Issues (second latency monster):**
  - `cargo check`/`test` per commit is **seconds-to-minutes**. Options to
    evaluate: check-only on most steps, test only at `declare_done`, warm/
    incremental cargo, or RA-based type-check instead of cargo. **Unresolved and
    critical.**
  - graph-metric **delta** computation must be incremental (full recompute per
    commit re-creates the latency wall).
  - Goodhart on metrics — defer hardening until gaming observed.

### P1.8 — Episode runner (integration milestone)
- **Goal:** the loop (view→action→reward), step budget, termination, structured
  trajectory logging.
- **Builds on:** everything above.
- **Net-new:** action dispatch over the 5 verbs, episode lifecycle, trajectory
  recorder (the future SFT data format).
- **Steps:** (1) action router; (2) per-action reward + episode-end summary;
  (3) budget + `declare_done`; (4) trajectory log; (5) run a frontier model via
  API end-to-end on P0.4 pool.
- **Exit:** a frontier model completes real episodes (e.g. the rmc
  `project_paths` dedupe) through the API; trajectories logged.
- **Issues:** credit assignment (commit-time compile-break vs the 5 prior moves);
  reward weights; whether the env is expressive enough (abstraction leaks —
  refactors that don't fit the verb set).

---

## Issues Register (ranked by lethality)

1. **Warm-host incremental rebuild reach (P0.2).** If RA invalidation fan-out
   on a small edit is large, "sub-second" fails and the whole RL loop is
   infeasible. *Mitigation:* measure fan-out early on real edits before
   committing to the rest.
2. **Cargo gate latency (P1.7).** `cargo check`/test per commit can dominate
   wall-clock. *Mitigation:* prototype check-only + test-at-episode-end +
   warm cargo; consider RA type-check as the fast gate.
3. **CRUD propagation correctness & transactionality (P1.5).** Silent edit/
   propagation bugs corrupt training data invisibly. *Mitigation:* per-op jj
   checkpoint; differential test apply-vs-cold-rebuild on every op.
4. **Architectural shift to a stateful warm host.** Current design is
   build-snapshot-then-query (stateless). Warm host + per-workspace RA db is a
   memory- and lifecycle-heavy change, multiplied across concurrent rollouts.
5. **Secondary-index (DUP_SORT) diff-patch bugs (P0.2).** Corrupt query results
   feed corrupt rewards. *Mitigation:* P0.1 golden cross-check after every
   incremental update during dev.
6. **modify_signature callsite synthesis (P1.5).** The "how complete is the
   abstraction" call; refuse vs `todo!()` vs `callsite_strategy`. Still open.
7. **simulate/apply divergence (P1.4).** Resolved structurally (simulate = mode
   of apply) but must be enforced by shared code + tests.
8. **Description staleness within an episode (P1.2).** Mitigated if regen rides
   the P0.2 dirty set; otherwise the agent navigates on stale labels.
9. **Cluster quality as perception (P1.3).** Bad clusters distort the agent's
   vision, not just a suggestion. Needs soft membership + zoom-through.
10. **Benchmark crates not building (P0.4).** Cargo gate is meaningless on a
    crate that doesn't compile at baseline. Filter hard.
11. **Determinism vs warm host (P0.1/P0.2).** Incremental extraction ordering
    may differ from cold; only reward-bearing fields must be stable (right-sized
    rigor) — but that "only" needs verifying.

---

## Recommended milestone order

- **M0 (de-risk):** P0.1 + a *spike* of P0.2 measuring RA invalidation fan-out
  and a P1.7 spike measuring `cargo check` latency. **These two numbers decide
  whether the project is feasible as designed.** Do them first, before building
  anything large.
- **M1 (read side, parallel, on slow build):** P1.1 + P1.2 + P1.3. Independent
  of P0.2; validates the vision/observation half.
- **M2 (write engine):** finish P0.2, then P1.5, then P1.4, then P1.6.
- **M3 (loop):** P0.3 + P1.7 + P1.8 → first end-to-end episodes with a frontier
  model.
- **Gate to Phase 2:** M3 green on the rmc repo's own known refactors.

The two M0 spikes are the cheapest way to surface the two lethal issues (#1, #2)
before sinking months into the build.
```

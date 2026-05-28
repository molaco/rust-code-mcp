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

### P1.5 — Structural CRUD with auto-propagation (split per review #5)
Split into an MVP vertical slice (P1.5a) that closes the end-to-end loop, then
expansions. **P1.5a is the only CRUD on the critical path to M3**; b–e follow
after the first loop runs. All ride the P0.2 apply==rebuild engine and the D2/D3
affected-set + invalidation contracts; each op is wrapped in a D4 checkpoint.

- **P1.5a (MVP) — `modify_body` only.** Local, **no propagation** (body-only
  edit class in D2 → cheapest re-extract). Exercises the *entire* apply → gate
  (P1.6) → reward (P1.7) path with the least surface. This is what unblocks the
  first end-to-end episode (M3). *Builds on:* P0.2, skeleton body span. *Exit:*
  body replaced, graph re-synced via the body-only D2 path, gated, rewarded, or
  refused.
- **P1.5b — `move` + `delete`.** Introduces propagation: `move` fixes `use`
  paths (RA-assisted) + cycle check; `delete` ref-checks then refuses/cascades.
  First use of reverse-dep affected sets.
- **P1.5c — `modify_signature`.** Apply RA rename mechanics + **callsite
  synthesis** for added params — the open decision (refuse / `todo!()` /
  `callsite_strategy`). Highest correctness risk.
- **P1.5d — `extract_function` / `extract_trait` / `inline`.**
- **P1.5e — `split_module` / `merge_modules` / `create_module` / `move_module` /
  `lift_to_crate` / `lower_to_module`.** Touches `mod` tree + **Cargo.toml**
  (manifest surgery; Cargo edits = cold-rebuild class in D2).
- **Cross-cutting issues:** RA rename is **preview-only** today — applying edits
  and handling RA refusals (keywords, foreign items, conflicts) is new;
  multi-file **transactionality** via per-op D4 checkpoint; `modify_signature`
  synthesis is the biggest call; silent propagation bugs corrupt training data
  (mitigate with apply-vs-cold-rebuild differential tests on every op).

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

Review-raised structural hazards are now resolved by the M0 decisions: snapshot
identity → **D1**; affected-set → **D2**; full invalidation → **D3**; rollback
contract → **D4**; P1.5 scope → **split into P1.5a–e**. The two existential
*measured* unknowns (RA fan-out, cargo latency) remain — they gate everything.


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

- **M0 (decisions + de-risk):** P0.1 + the four **decisions D1–D4** (written
  contracts: working-snapshot, affected-set, invalidation matrix, checkpoint) +
  the two **feasibility spikes** (RA fan-out, cargo latency) with go/no-go
  thresholds. M0 outputs *decisions, not just numbers.* Nothing large is built
  until D1–D4 hold and both spikes pass.
- **M1 (read side, parallel, on slow build):** P1.1 + P1.2 + P1.3. Independent
  of P0.2; validates the vision/observation half.
- **M2a (write engine core):** finish P0.2 (implementing D1–D4) + **P1.5a
  `modify_body` only**.
- **M3 (first loop — early!):** P0.3 + P1.6 + P1.7 + P1.8 with `modify_body`
  alone → end-to-end episodes with a frontier model **before** the full CRUD
  surface exists. This is the point of the P1.5 split: prove the loop, then
  widen it.
- **M2b (CRUD expansion):** P1.5b → c → d → e, then P1.4 simulator (as a dry-run
  mode of the now-stable apply engine).
- **Gate to Phase 2:** M3 green on the rmc repo's own known refactors, widened
  by M2b.

The two M0 spikes are the cheapest way to surface the two lethal issues (#1, #2)
before sinking months into the build; D1–D4 are the cheapest way to surface the
correctness/identity hazards (#3–#5, #11) before they corrupt training data.

---

## M0 — Decisions to resolve before build (addresses review)

Review (2026-05-28) correctly flagged that M0 must produce **hard decisions**,
not just measurements, and that four contracts were underdefined: snapshot
identity under mutation, the affected-set algorithm, the full invalidation
matrix, and the rollback contract. Resolved below. M0 = these four decisions
**plus** the two feasibility spikes that validate them.

### D1 — Working-snapshot strategy (resolves review #1)

The published store is content-addressed: `graph_id_for(workspace_hash,
fingerprint)`, with a `CURRENT` pointer and a per-`graph_id` manifest
(`storage.rs`). Any `.rs` byte flips the fingerprint → a new `graph_id`. So
in-place patching of a published snapshot **breaks the invariant** that a
`graph_id` equals its contents.

**Decision:** Phase 1 adds a second snapshot class.

- **Published snapshots** (`snapshots/<graph_id>/`, immutable, content-addressed)
  — unchanged. Still the cold-build artifact; serve as an episode's *initial
  state*.
- **Working snapshot** (`working/<session_id>/`, **mutable, identity-decoupled**)
  — identity is `(session_id, base_graph_id, edit_seq)`, **not** a content
  fingerprint. Never published or reused by fingerprint; purely ephemeral RL
  state. The apply==rebuild engine patches this in place.
- **Init:** at episode start, copy the base published LMDB into the working dir
  (LMDB `mdb_copy`, once per episode, amortized over ~50 steps — fine under
  right-sized rigor). Open the warm RA host from the same base.
- **Publish (optional):** only if a result must persist — recompute the real
  fingerprint/`graph_id` and copy out. Training normally logs the trajectory and
  discards the working snapshot.

This is the explicit answer to "mutable live snapshot vs new snapshot per edit":
**one mutable working snapshot per session**, not a new content-addressed
snapshot per edit (which would mean an LMDB copy per step).

### D2 — Affected-set algorithm (resolves review #3)

"`emit_crate` only for dirty crates" is too optimistic. `extract` builds
crate/module maps then runs bindings, impl items, attributes, signatures,
statics, and usages over the local crate set; an exported-surface change ripples
into reverse-dependents' usages/bindings. Reverse-deps come from the existing
`crate_edges` consumer→producer graph, reversed.

**Classify the edit, then expand:**

| Edit class | Affected set |
|---|---|
| **Body-only** (fn/method body; sig, visibility, items unchanged) | editing fn's outgoing usages only; **no** reverse-deps |
| **Signature / visibility** (params/return/generics/`pub`) | editing crate (node + signature) **+ reverse-deps that reference the item** (their usages/bindings re-resolve) |
| **Item add / remove** (pub item created/deleted) | editing crate (nodes/bindings/contains) + reverse-dep usages/bindings (surface changed) |
| **Module-tree** (add/remove/rename `mod`, move file) | editing crate fully (contains, module nodes, paths) + reverse-dep import bindings (`use` paths) |
| **Macro / proc-macro / build.rs** | editing crate **fully** + all reverse-deps **fully** (generated code is opaque) |
| **Cargo.toml feature/dep** | conservative: treat as **cold rebuild** (feature unification can touch the whole workspace) |

The classifier runs on the edit the CRUD op already knows it is making — it does
not need to infer the class from a textual diff.

### D3 — Invalidation matrix (resolves review #2)

Every persisted table + cache, per edit class. Legend: **P** patch · **D**
re-derive · **C** content-hash cache (lazy regen, self-invalidating) · **—**
unchanged · **F** full rebuild.

| Table / cache | Body | Sig/vis | Item ±  | Mod-tree | Macro | Cargo |
|---|---|---|---|---|---|---|
| `nodes_by_id` | — | P | P | P | F | F |
| `bindings_by_id` + `_by_from_module` + `_by_target` | — | P | P | P | F | F |
| `children_by_parent` (**contains**) | — | — | P | P | F | F |
| `usages_by_id` + `_by_target` + `_by_consumer` + `_by_consumer_function` | P | P | P | P | F | F |
| `signatures_by_target` | — | D | D | — | F | F |
| `static_metadata_by_target` | D (if static) | D | D | — | F | F |
| `embeddings_by_target` | C | C | C | C | C | C |
| `descriptions` (new) | C | C | C | C | C | C |
| `meta_by_key` (counts) | P (usage cnt) | P | P | P | F | F |
| manifest `node_count` etc. | P | P | P | P | F | F |

`contains`, `signatures`, `statics`, embeddings, descriptions, and `meta` counts
were the records the review noted as silently stale-able — each now has an
explicit rule. Content-hash caches (embeddings/descriptions) self-invalidate;
everything else is patched or re-derived against the affected set from D2.

### D4 — Checkpoint / restore contract (resolves review #4)

`jj op restore` only restores **source**. A `Checkpoint` must capture all four
layers and `restore` must reset them atomically:

```
Checkpoint = {
  source:      jj operation id,
  graph:       working-snapshot undo-log marker (inverse of each LMDB patch),
  ra_host:     warm-host edit-sequence number,
  caches:      embeddings/descriptions are content-hash keyed → self-heal,
}
```

- **Per-op undo log** (not full LMDB copies): each patch records the prior value
  of every key it touches; `restore` replays inverses back to the marker —
  cheap, suited to frequent rollback.
- **Source:** `jj op restore` to the checkpoint op.
- **RA host:** replay inverse `set_file_text` (same incremental path as apply) to
  the marked edit-seq; if that diverges, fall back to re-open from base (slow —
  avoid).
- **Atomicity:** all-or-nothing; a failed restore re-opens the working snapshot
  from base.

This makes rollback a first-class contract across source + graph + host +
caches, not a thin `jj` wrapper.

### M0 feasibility spikes (validate the decisions)

1. **RA invalidation fan-out** — apply representative edits of each D2 class to a
   warm host; measure the real affected-set size and re-extract time. **Go/no-go:
   body-only edit re-extract < 500ms on a 100k-LOC workspace.** If fan-out is
   workspace-wide for small edits, D2 collapses and the design needs rework.
2. **Cargo gate latency** — `cargo check` (warm/incremental) and a scoped test
   run on the P0.4 pool. **Go/no-go: per-commit check < ~2s warm.** If not,
   adopt RA-type-check-as-gate and test-only-at-`declare_done`.

M0 ships D1–D4 as written contracts **and** these two numbers. Only then does
M2 (write engine) start.
```

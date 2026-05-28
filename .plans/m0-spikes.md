# M0 Feasibility Spikes — Concrete Spec

Two measurements decide whether Phase 1 is feasible as designed. Neither is
resolvable by design; both need numbers on real code. Run **Spike 1 first** —
its warm-host harness is reused by Spike 2.

All `cargo`/RA commands run through the devshell (per AGENTS.md):
`nix develop ../nix-devshells#cuda-code --command <cmd>`.

Go/no-go gates (restated):
- **Spike 1:** body-only edit re-extract **< 500ms** on a ~100k-LOC workspace.
- **Spike 2:** warm `cargo check` **< ~2s** per commit; else fall back to
  RA-diagnostics-as-gate + test-at-`declare_done`.

---

## Shared setup

**Targets (two sizes, because fan-out scales with workspace size):**
- T1 = the rmc workspace itself (~mid-size, known well).
- T2 = a ~100k-LOC Rust workspace (P0.4 isn't built yet → pick one large
  vendored/cloned crate, e.g. a big workspace from crates.io, and pin it). The
  500ms gate is judged on **T2**; T1 is the fast dev-loop target.

**Harness:** a throwaway `#[test]` or `examples/` binary in `rmc-graph` that
reuses `graph::loader::load` and `graph::extract::{extract, emit_crate}`
directly (both confirmed callable; `emit_crate` is already per-crate).

**Timing:** `std::time::Instant` around each measured region; for cargo, wrap the
whole invocation and also use `hyperfine` for distributions. Report
median + p90 over **≥10 runs**, not a single sample (salsa LRU + cargo cache
state evolve).

**Edit targets on T1 (named, real):** `rmc_indexing::dir_hash` (leaf-ish) and a
fn in `rmc_graph::graph::extract` (hub crate, many dependents) — the leaf vs hub
pair stresses reverse-dependent fan-out.

---

## Spike 1 — RA warm-host invalidation fan-out

**Question:** with a warm `RootDatabase`, how much does one edit invalidate, and
how long to re-extract the affected scope?

### Step 0 — feasibility gate (do this before anything else)
Confirm the warm host is *mutable*:
- `load()` → keep the `LoadedWorkspace` (its RA db) alive.
- Obtain a mutable handle and apply an RA `Change` (`set_file_text(file_id, new)`
  via `db.apply_change(change)` — verify exact API against the pinned
  `ra_ap_ide_db`/`ra_ap_base_db` version).
- Confirm proc-macro server + build-script outputs survive warm (they may be
  torn down after `load`).

**If the host can't be mutated warm without re-`load()`** → that *is* the
finding: incremental needs a different RA integration than the current
build-then-discard path. Stop and report.

### Step 1 — baselines
Time and record on T1 and T2:
- cold `load()` (the expensive part)
- cold full `extract()`
- single-crate `emit_crate()` for the smallest and largest local crate

(These bound the problem: if one crate's `emit_crate` already exceeds 500ms,
scoped-to-crate is insufficient and we need scoped-to-item.)

### Step 2 — per-edit-class measurement
For each D2 edit class, apply via `set_file_text`, then **force recompute** by
re-running the extraction the affected set needs, and time it. Decompose into
two numbers so we know whether the bottleneck is RA (intrinsic) or our extractor
(fixable):
- **t_ra** = time for one cheap semantic query on the edited item after the edit
  (forces salsa recompute; e.g. resolve the fn / fetch its HIR).
- **t_extract** = time to re-run `emit_crate` over the affected set (D2).
- **t_patch** = time to serialize + LMDB-`put` the affected records (may stub
  with a scratch env).

Edit classes (skip Cargo — D2 says cold rebuild):

| Class | Concrete edit |
|---|---|
| body-only | change a statement inside `dir_hash`'s body |
| signature | add a param to a `pub(crate)` fn |
| item add | add a new `pub fn` to a module |
| module-tree | add `mod x;` + a new file (needs new `FileId` + source-root change — harder API) |
| macro | edit a `macro_rules!` body or a `#[derive]`-heavy type |

Run each **on both a leaf crate and the hub crate** (`rmc_graph`) to see
reverse-dependent fan-out.

### Step 3 — instrument fan-out
Record per edit: number of crates in the affected set actually re-extracted, and
(if salsa exposes it via `salsa_event` / `RA_PROFILE`) count of recomputed
queries. This tells us whether D2's classification matches reality.

### Results template
```
Target | Class      | crate   | t_ra | t_extract | t_patch | total | affected_crates
T2     | body-only  | leaf    |  ms  |    ms     |   ms    |  ms   |       1
T2     | body-only  | hub     |      |           |         |       |
T2     | signature  | hub     |      |           |         |       |
...
```

### Go / no-go
- **GO:** T2 body-only `total < 500ms`. Secondary: sig/item < ~1-2s acceptable
  (rarer). Macro/module-tree may be slow (rare; conservative full-crate is OK).
- **NO-GO:** body-only > 500ms. Implication by which term dominates:
  - **t_ra dominates** → RA incremental recompute is intrinsically too slow;
    consider a *lightweight syntactic extractor* for body-only edits (they only
    change `usages_by_consumer_function` for one fn — may not need full RA).
  - **t_extract dominates** → our extractor needs item-level scoping, not just
    crate-level; tractable engineering.
  - **fan-out is workspace-wide for tiny edits** → the warm-host design fails;
    rethink (e.g. accept stale graph between commits, rebuild async).

---

## Spike 2 — Cargo gate latency

**Question:** can a correctness gate run per commit fast enough, and if not,
does RA-diagnostics cover enough to be the per-step gate?

### Step 1 — cargo timings (via devshell, hyperfine, ≥10 runs)
On T1 and T2:
- `t_cold_check` — `cargo check` on a clean target (episode-start cost,
  amortizable)
- **`t_warm_check`** — `cargo check` after a 1-line body edit (THE number)
- `t_warm_check_sig` — after a signature edit (more rebuild)
- `t_scoped_test` — `cargo test -p <edited_crate>` warm
- `t_full_test` — full `cargo test` warm

### Step 2 — RA-diagnostics-as-gate probe (reuses Spike 1 warm host)
- **t_ra_diag** — time `Analysis::diagnostics` (full diagnostics) on the warm
  host after an edit. Likely near-free given the host is already warm.
- **Coverage probe** — inject 5 known error kinds, record which RA flags:
  1. type mismatch (`let x: u32 = "s";`)
  2. missing struct field
  3. unresolved import / name
  4. borrow error (`use after move`)
  5. unsatisfied trait bound
  Expectation: RA catches 1-3, **misses 4** (no full borrowck) and possibly 5
  (incomplete trait solving). Document exactly what it catches.

### Results template
```
Target | t_cold_check | t_warm_check (p50/p90) | t_warm_check_sig | t_scoped_test | t_full_test | t_ra_diag
T2     |              |                        |                  |               |             |
RA coverage: type-mismatch ✓ | missing-field ✓ | unresolved ✓ | borrow ✗ | trait ?
```

### Go / no-go
- **GO (cargo per commit):** `t_warm_check < ~2s` on T2 → `cargo check` is the
  per-commit hard gate; tests run only at `declare_done`.
- **FALLBACK (likely):** `t_warm_check >> 2s` → two-tier gate:
  - per-step: **RA diagnostics** (cheap, type-level) — fast reward shaping.
  - per-episode (`declare_done`): full `cargo check` + scoped tests — the real
    correctness gate.
  - Reward consequence: per-step "type-clean" is cheap signal; per-episode
    "compiles + tests pass" is the hard gate. Borrowck errors only surface at
    episode end — accept and shape for it.
- **NO-GO (neither works):** if both cargo is slow *and* RA coverage is too thin
  to be a useful per-step filter → the per-step gate is unreliable; revisit
  whether commits can be batched or the env tolerates type errors mid-episode.

---

## Synergy, ordering, effort

- **Run Spike 1 first.** Its warm host is exactly what Spike 2's RA-diagnostics
  path measures — Step 2 of Spike 2 is ~free once Spike 1's harness exists.
- **Effort:** Spike 1 ≈ 3-4 days (the warm-mutation API + scoped re-extract
  harness is the work). Spike 2 ≈ 2 days (mostly scripting + the coverage
  probe). Both on T1 first (fast iteration), then T2 for the verdict.

## Decision tree out of M0

```
Spike1 GO  + Spike2 GO       → build as designed (cargo per-commit gate)
Spike1 GO  + Spike2 FALLBACK → build as designed, two-tier gate (RA + episode-end cargo)   [most likely]
Spike1 NO-GO (t_extract)     → add item-level scoping to extractor, re-spike
Spike1 NO-GO (t_ra)          → lightweight syntactic extractor for body edits, re-spike
Spike1 NO-GO (fan-out)       → abandon warm-in-loop graph; async-rebuild + tolerate staleness — major redesign
Spike2 NO-GO                 → revisit commit batching / mid-episode error tolerance
```

These two results, plus D1–D4, are the complete gate to M2.

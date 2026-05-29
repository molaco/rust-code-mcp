# Subset Plan — Refactor Toolset on the Warm-Host Engine (Option 5)

A scoped subset of `.plans/actual-phase-1-plan.md`: build the plan's
"Apply == Rebuild" core and expose it as **MCP tools**, dropping the entire
RL training apparatus. This document is an *overlay* — it sets scope, crate
boundaries, the tool surface, sequencing, and the two engine decisions. The
verb-/engine-level step detail lives in the existing split sections (pointers
below); don't duplicate it here.

> Devshell: every shell command runs under
> `nix develop ../nix-devshells#cuda-code --command <cmd>`.

---

## 1. Scope

**Build (from the full plan):**
- **P0.1** determinism (Section A) — reproducible extraction; the golden
  cross-check that proves incremental edits equal a cold rebuild.
- **D1–D4 contracts** (Section B, *contracts only*) — working snapshot,
  affected-set, invalidation matrix, checkpoint.
- **P0.2 + P0.3** warm host + rollback (Section C) — the one mutation engine.
- **P1.1** navigate read view (Section D).
- **P1.5a–e** the 13 CRUD verbs (Sections G + H).
- **P1.4 + P1.6** simulate (dry-run) + gates (audit/refusal) (Section I).

**Drop:**
- **RL stack:** `rmc-reward` (P1.7), `rmc-episode` (P1.8), `rmc-rl`, the
  `Policy`/`AnthropicClient` loop, trajectory/SFT, reward scalarizer.
- **Feasibility scaffolding:** `rmc-spikes`, the P0.4 bench pool. The warm
  host's sub-second re-query is a *target*, not a go/no-go gate.
- **Enrich track:** `describe` (P1.2 / Section E) and `analyze`
  (P1.3 / Section F). Stackable later; out of this subset.

**Net effect:** ~16–19k new LOC vs ~27k for the full plan; new deps shrink to
`petgraph`, `syn`, `toml_edit` (no `linfa*`, `clap`, `which`, `chrono`,
`reqwest`).

---

## 2. Guiding constraints (idiomatic Rust 2024)

These are constraints, not suggestions (carried over from the guidelines review
already applied to the split sections):

- **File-based modules** — `foo.rs` + `foo/`, never `mod.rs` (the pre-existing
  `graph/mod.rs` is grandfathered).
- **Per-operation `thiserror` errors** in every library crate; `anyhow` only in
  binaries. Preserve chains with `#[source]`/`#[from]`. No god-enums.
- **Enum-over-trait dispatch** for the closed verb set: `Crud` enum + `match`
  in `compute_effects`/`apply_effects` (no `CrudVerb` trait — DD-2).
  `#[non_exhaustive]` so verbs can grow without breaking `match` arms.
- **Sans-I/O split** — `compute_effects(&Host) -> Effects` is pure (no writes);
  `apply_effects(&mut Host, &Effects)` performs the I/O. `simulate` is
  `compute_effects` only, so it cannot diverge from `apply`.
- **No whole-file formatters** (E5) — `syn`/`ra_ap_syntax` locate byte ranges
  for *analysis only*; replacement text is string-built and spliced.
- **Private fields + accessors**; `#[non_exhaustive]` on growable public types;
  newtypes for IDs (`JjOpId`, `GraphId`, `EditSeq`, `SessionId`).
- **Async hygiene** — never hold a workspace lock across `.await`; reads take a
  shared lock, writes an exclusive lock (existing `WorkspaceLockRegistry`).
- **`thiserror` stays pinned at workspace `"1"`** (DD-5); no v2.

---

## 3. Crate inventory

### New (3)
- **`rmc-semantic`** — rename engine extracted from `rmc-server::semantic`
  (mandatory: breaks the `rmc-server` ⇄ `rmc-crud` cycle). `SemanticService`,
  `RenamePreview`, `RenameEdit`, `RenameFileMove`, `rename_by_*`, `SemanticError`.
  Deps: `rmc-graph`, `ra_ap_{ide,ide_db,syntax}`, `thiserror`, `tracing`, `serde`.
- **`rmc-crud`** — the 13 verbs as `Crud` enum + `compute_effects`/`apply_effects`.
  Public: `Crud` (`#[non_exhaustive]`), `Effects`, `CallsiteFill`, `EditOutcome`,
  `CascadePolicy`, `SignatureChange`, `EditError`/`CrudError`; helpers
  `source_edit` (splice), `syn_ast` (analysis), `preview_apply` (RA line/col→byte).
  Deps: `rmc-graph`, `rmc-config`, `rmc-semantic`, `ra_ap_{syntax,ide,ide_db}`,
  `syn = "2"`, `toml_edit = "0.22"`, `thiserror`, `tracing`, `serde`, `serde_json`.
- **`rmc-gates`** — audit/refusal harness over the dirty set. `GateHarness`,
  `GateOutcome`/`GateReport`, `RefusalReason`, `RefusalCode`, `Severity`,
  `Penalty`, `CascadeKind`, `BoundaryAllowlist`, `GatesConfig`, `GateError`.
  Wraps existing audits + SCC cycle check. Deps: `rmc-graph`, `rmc-config`,
  `petgraph = "0.6"`, `thiserror`, `tracing`, `serde`, `toml`.

### Modified (4 + workspace)
- **`rmc-graph`** (largest) — new submodules under `src/graph/`:
  `working/` (D1 snapshot, D4 undo log, D3 `patch.rs` + `patch/`),
  `host/` (D2 classifier + affected-set + `WorkspaceHost`),
  `checkpoint/` (D4 checkpoint + jj/undo restore),
  `view/` (P1.1 navigate). Plus `determinism.rs` + `snapshot_compare.rs`
  (P0.1), `extract.rs` split (`emit_crate_into`), 2 new sub-DBs
  (`undo_log_by_edit_seq` DUP_SORT, `working_meta_by_session`),
  `SCHEMA_VERSION` bump. Typed `HostError`. New dep: `syn`, `petgraph`.
  *(No `analyze/` or `descriptions/` — those are the dropped enrich track.)*
- **`rmc-config`** — slim `#[non_exhaustive] RuntimeConfig` (private fields +
  accessors): `seed: Seed`, `callsite_fill: CallsiteFill`,
  `working_snapshot_root: PathBuf`. New `src/runtime.rs`. *(No Anthropic/model
  config — no LLM in this subset.)*
- **`rmc-indexing`** — `src/indexing/seed.rs` (deterministic file ordering).
- **`rmc-server`** — new MCP tool handlers `src/mcp/handlers/{navigate,crud,
  simulate,gates}.rs`; `semantic/` **moved out** to `rmc-semantic` and
  re-pointed; promote `OpenedSnapshot::line_to_byte` to `pub`. New deps:
  `rmc-semantic` (path), `rmc-crud`/`rmc-gates` (path). No `rl` feature gate
  needed — these tools ship in the server (no RL crates to hide).

### Dropped / not built
`rmc-reward`, `rmc-episode`, `rmc-rl`, `rmc-spikes`. `rmc-host` stays optional
(default: keep `WorkspaceHost` in `rmc-graph::host`). `rust-code-mcp` unchanged.

---

## 4. MCP tool surface (~20 new tools in `rmc-server`)

- **Navigate (read):** `goto`, `zoom`, `show_body`, `show_callers`,
  `follow_trail` → `ContextView`. Ride the cold `OpenedSnapshot`; shared lock.
- **Refactor (write):** `modify_body`, `move`, `delete`, `modify_signature`,
  `extract_function`, `extract_trait`, `inline`, `split_module`,
  `merge_modules`, `create_module`, `move_module`, `lift_to_crate`,
  `lower_to_module`. Each returns a diff + `GraphDiffSummary`; exclusive lock.
- **Safety:** `simulate` (dry-run → `Effects` + would-refuse, no writes),
  `run_gates` (standalone audit of a dirty set or whole crate).

Every write tool follows the canonical pipeline (errata E1): `compute_effects`
→ `gates.evaluate` → if hard-refused, **return without writing anything** →
else checkpoint → `fs::write` + RA `set_file_text` → scoped re-extract → LMDB
diff-patch → record undo. Gate refusal or any apply error triggers rollback.

---

## 5. Engine model (warm host as a tool, not an episode)

A long-lived **per-workspace** `WorkspaceHost` (keyed in the existing
`SearchRuntimeCache`/runtime registry) holding a warm rust-analyzer
`AnalysisHost` + `Vfs` over the LMDB graph. The plan's per-episode working-copy
distinction collapses for a tool product — see decisions below.

Detail for each piece lives in the existing sections:
- Warm host + apply pipeline + rollback → **`06-section-c-warm-host-rollback.md`**
- D1–D4 type contracts → **`05-section-b-contracts-spikes.md`** (contracts only)
- Navigate verbs/ContextView → **`07-section-d-read-view.md`**
- modify_body / move / delete → **`10-section-g-modify-move-delete.md`**
- signature / extract / inline / module ops → **`11-section-h-sig-extract-module.md`**
- simulate + gates → **`12-section-i-simulator-gates.md`**
- determinism + golden cross-check → **`04-section-a-determinism-bench.md`**
- canonical types / module homes → **`02-canonical-reconciliation.md`** (authoritative)

---

## 6. Engine decisions (defaults taken; flip if needed)

**DD-A — Commit model: apply-and-publish immediately.** Without an RL
commit/reward step, write tools apply to source + incrementally patch the
*live* published graph in one exclusive-locked operation, with a checkpoint for
undo. There is **no** working-copy/`commit`/`discard` dance.
- *Rationale:* an interactively-invoked tool wants its edit visible to the next
  query immediately; the episode's deferred-publish only earned its keep under
  thousands of speculative edits.
- *Flip to (b):* if a caller needs speculative multi-edit batches, add explicit
  `begin_session` / `commit` / `discard` tools over the working snapshot
  (`mdb_copy`) — the D1 machinery already supports it.

**DD-B — Rollback substrate: undo-log primary, jj optional.** Rollback replays
the in-process `UndoLog` + `file_prior_text` (works in any directory). When the
workspace is a jj (or git) repo, also record the op id and prefer
`jj op restore` for the source side.
- *Rationale:* this environment isn't a VCS repo; the tool must still undo.
- *Flip:* require a jj workspace and make `jj op restore` the primary path if
  you want VCS-native history of every tool edit.

---

## 7. Milestones

| # | Deliverable | Sections | Exit gate |
|---|---|---|---|
| **M0** | Determinism + D1–D4 contracts | A (P0.1), B (contracts) | Two cold builds byte-equal under fixed seed; each D3 invalidation row maps to a named `patch_*` fn |
| **M1** | Warm host + rollback | C | `modify_body` differential: incremental apply == cold rebuild on 12 fixtures; rollback restores cleanly |
| **M2** | Navigate read tools | D | `ContextView` non-empty for ≥95% of items at every scale; wired as MCP tools |
| **M3** | CRUD verbs | G → H | `differential_apply_vs_cold` green for every verb (pulls in `rmc-semantic` at `move`) |
| **M4** | simulate + gates tools | I | `simulate(op) == apply(op)` per verb; gates refuse correctly and block the write |

**Acceptance (phase exit):** all verb differential tests green, `simulate ==
apply`, gates refuse + auto-rollback verified, no LMDB corruption across a
soak of repeated edit/rollback cycles.

**Risk-reduction order:** M0 (cheap, unblocks) → M1 (the engine, critical path)
→ M2 in parallel (read side, no warm-host dep) → M3 thinnest-verb-first
(`modify_body` → `move`/`delete` → H) → M4.

---

## 8. File-tree (subset; `+` new, `~` modified, `→` moved)

```
crates/
  rust-code-mcp/                          (unchanged)
  rmc-config/
    src/lib.rs                       ~    pub mod runtime;
    src/runtime.rs                   +    RuntimeConfig (seed, callsite_fill, working_snapshot_root)
  rmc-indexing/
    src/indexing/seed.rs             +    deterministic file ordering
  rmc-graph/
    Cargo.toml                       ~    + syn, petgraph (no linfa/prettyplease)
    src/lib.rs                       ~    + pub use working/host/view/checkpoint
    src/graph/
      mod.rs                         ~    + pub mod working/host/view/checkpoint
      extract.rs                     ~    split: emit_crate_into exposed
      loader.rs  model.rs  snapshot.rs  storage.rs   ~  (+ sub-DBs, SCHEMA_VERSION)
      query/audits.rs                ~    pub(crate) entry for gate harness
      determinism.rs                 +    sort_model_for_persistence
      snapshot_compare.rs            +    SnapshotDump / SnapshotDiff
      working.rs + working/          +    snapshot.rs, undo_log.rs, init.rs, patch.rs, patch/{7}
      host.rs    + host/             +    workspace_host.rs, edit_class.rs, affected_set.rs, extract_per_crate.rs
      view.rs    + view/             +    location.rs, context_view.rs, navigate.rs, scale.rs
      checkpoint.rs + checkpoint/    +    jj.rs, restore.rs
  rmc-server/
    Cargo.toml                       ~    + rmc-semantic, rmc-crud, rmc-gates (path)
    src/mcp/handlers.rs + handlers/  +    navigate.rs, crud.rs, simulate.rs, gates.rs
    src/semantic/                    →    MOVED to crates/rmc-semantic/
+ rmc-semantic/  src/{lib,service,rename,error}.rs
+ rmc-crud/      src/{lib,effects,error,source_edit,syn_ast,callsite_fill,preview_apply}.rs
+ rmc-crud/      src/verbs/{modify_body,move_,delete,modify_signature,extract_function,
+                           extract_trait,inline,split_module,merge_modules,create_module,
+                           move_module,lift_to_crate,lower_to_module}.rs
+ rmc-gates/     src/{lib,hard,soft,cycle,allowlist,config}.rs
docs/contracts/  D1.md D2.md D3.md D4.md          +   (M0 deliverables)
```

`rmc-host/` (optional, default skip). No `rmc-reward`/`-episode`/`-rl`/`-spikes`.

**Workspace `Cargo.toml`:** `members` += `rmc-semantic`, `rmc-crud`,
`rmc-gates`; `default-members` unchanged. New `[workspace.dependencies]`:
`petgraph = "0.6"`, `syn = { version = "2", features = ["full","extra-traits","visit","visit-mut"] }`,
`toml_edit = "0.22"`. `thiserror` stays `"1"`.

---

## 9. LOC estimate

| Area | ~LOC |
|---|---|
| `rmc-graph` warm-host core (working/host/checkpoint) + determinism | ~7,500 |
| `rmc-graph` view/navigate | ~1,500 |
| `rmc-crud` (13 verbs + helpers) | ~6,000 |
| `rmc-gates` | ~1,500 |
| `rmc-semantic` (mostly moved) | ~1,000 |
| `rmc-server` tool handlers | ~600 |
| `rmc-config` / `rmc-indexing` | ~450 |
| tests (differential, golden, navigate, simulate==apply, gate refusal) | ~1,500 |
| **New total** | **~16k–19k** |
| Modified (extract split, storage, mod.rs, loader, lib.rs re-exports) | ~1,200 |
| Relocated (rmc-server semantic → rmc-semantic) | ~800 |

Estimate basis: ~70 new source files at typical Rust file sizes incl. inline
tests; the plan specifies files/types/steps but not line counts, so treat ±20%
as the real band.

---

## 10. Out of scope (explicit)

RL loop (reward/episode/policy/trajectory/SFT), the Anthropic model-in-the-loop,
`rmc-rl` CLI, `rmc-spikes` + bench pool, `describe`/`analyze` enrichment tools
and their `linfa`/LLM deps. Each remains stackable onto this subset later
without rework, since the engine (`WorkspaceHost`) and the `compute_effects`/
`apply_effects` split are exactly the substrate they were designed to ride.

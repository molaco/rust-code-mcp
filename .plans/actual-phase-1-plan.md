# Phase 0 + Phase 1 — Actual Implementation Plan

Code-explicit, step-by-step, phase-by-phase plan synthesized from 10 parallel
subagent designs against the source plan at
`.plans/phase-1-implementation.md` and the existing crate surface in
`.skeleton/*.rs`.

Read **Section Z first** for the project-wide map (crate inventory, milestone
Gantt, file-tree diff, cross-slice integration tests, issue register, open
decisions, risk-reduction order). Sections A–J are the vertical slices in
critical-path order — each is self-contained and can be implemented from its
own text without further design rounds.

Devshell: every shell command runs under `nix develop ../nix-devshells#cuda-code --command <cmd>`.

---

## Table of contents

- [Errata (post-review revisions) — canonical resolutions](#errata-post-review-revisions--canonical-resolutions) — **READ FIRST**
- [Canonical Reconciliation — Single Source of Truth](#canonical-reconciliation--single-source-of-truth) — **HIGHEST PRECEDENCE**
- [Section Z — Integration, Milestones, Crate Inventory](#section-z--integration-milestones-crate-inventory)
- [Section A — P0.1 Determinism + P0.4 Benchmark Pool](#section-a--p01-determinism--p04-benchmark-pool)
- [Section B — M0 Contracts (D1–D4) + Feasibility Spikes](#section-b--m0-contracts-d1d4--feasibility-spikes)
- [Section C — P0.2 Warm-Host Incremental Writer + P0.3 jj Rollback](#section-c--p02-warm-host-incremental-writer--p03-jj-rollback)
- [Section D — P1.1 Read View / Navigate](#section-d--p11-read-view--navigate)
- [Section E — P1.2 Description Index](#section-e--p12-description-index)
- [Section F — P1.3 Analyze / Vision Layer](#section-f--p13-analyze--vision-layer)
- [Section G — P1.5a modify_body + P1.5b move / delete](#section-g--p15a-modify_body--p15b-move--delete)
- [Section H — P1.5c modify_signature + P1.5d extract/inline + P1.5e module ops](#section-h--p15c-modify_signature--p15d-extractinline--p15e-module-ops)
- [Section I — P1.4 Counterfactual Simulator + P1.6 Write-Time Gates](#section-i--p14-counterfactual-simulator--p16-write-time-gates)
- [Section J — P1.7 Commit/Reward + P1.8 Episode Runner](#section-j--p17-commitreward--p18-episode-runner)

---

# Errata (post-review revisions) — canonical resolutions

Eight blockers raised in review of the first draft. The resolutions below
are canonical. Where the body of the plan (Sections Z, A–J) contradicts
this errata, **the errata wins** — body text is as-of-draft design;
errata is what we build.

> **Precedence:** the **Canonical Reconciliation** section (immediately
> after this Errata) outranks even the Errata. It resolves the cross-slice
> type/layout/crate duplication the Errata left open (the B⇄C duplicate
> D1–D4 declarations, the 3-way module-layout split, the rmc-semantic
> cycle). Order: **Canonical Reconciliation → Errata → body.**

## E1 — Canonical apply pipeline (resolves Finding #1)

Hard gates run **before** any source write, RA mutation, or LMDB write.
The single canonical pipeline used by every CRUD verb:

```
1. compute_effects(host, op) -> Effects
   (PURE: no I/O, no fs::write, no LMDB write, no set_file_text)
2. gates.evaluate(effects.estimated_affected_items, &effects) -> GateOutcome
3. if !gate_outcome.passed:
       return Err(EditError::Refused(gate_outcome.hard))
   // NOTHING WRITTEN: no source, no LMDB, no checkpoint, no RA mutation.
4. let checkpoint = host.begin_checkpoint()?;
5. host.apply_edits(&effects.source_edits) -> ApplyOutcome
   // host owns fs::write under the same lock as RA set_file_text + LMDB write.
6. on Ok  -> checkpoint.commit()   -> return Ok(EditOutcome)
   on Err -> checkpoint.restore()  -> return Err(EditError::HostRejected)
```

Section I §15 is the canonical statement of this ordering. Section G
Steps 4–6 (which take the Checkpoint *before* the gate runs) and Section J
Step 11 (which calls `crud.apply` then `host.evaluate_gates`) are revised
to match. The router.rs `Action::Crud` arm becomes:

```rust
Action::Crud(op) => {
    // 1. simulate-style effects + gates BEFORE persist.
    let effects = compute_effects_for(&self.host, &op)?;
    let dirty = effects.estimated_affected_items.clone();
    let gate_outcome = self.gates.evaluate(&dirty, &effects)?;
    if !gate_outcome.passed {
        return Ok(DispatchOutcome {
            result: ActionResult::Refused(gate_outcome.hard),
            reward_vec: RewardVector::compile_fail(),
            scalar: -1.0,
            affected: vec![],
            new_audit_baseline: None,
        });
    }
    // 2. persist inside checkpoint.
    let checkpoint = self.host.begin_checkpoint()?;
    let edit = persist_for(op_kind, &mut self.host, effects)?;
    // 3. cargo/test gate AFTER persist, INSIDE the checkpoint window.
    let dirty_crates = self.host.crates_of(&edit.affected_items);
    let cr = commit.run(&edit, &gate_outcome, &checkpoint,
                        &dirty_crates, tokens, false).await?;
    if !cr.passed { /* commit.run already rolled back via checkpoint */ }
    else          { checkpoint.commit()?; }
    Ok(DispatchOutcome { /* ... */ })
}
```

`Crud::apply` collapses to the same shape: `compute_effects → gate → if pass
{ checkpoint → persist → commit }`. The Checkpoint never spans the gate
check, so a refusal cannot leave behind half-applied state.

## E2 — Episode / Commit ownership (resolves Finding #2)

`Commit<'a>` borrows `WorkspaceHost` + `OpenedWorkingSnapshot`; `Episode`
owns them. Storing `Commit<'static>` inside `Episode` is a self-referential
struct and will not compile in safe Rust. Resolution: **`Commit` is built
per-action from `Episode`'s fields, never stored.**

```rust
pub struct Episode<M: ModelClient> {
    pub host:          WorkspaceHost,
    pub snap:          OpenedWorkingSnapshot,
    pub crud_state:    CrudState,           // stateless config; no borrows
    pub navigator_cfg: NavigatorConfig,     // stateless config; no borrows
    pub gate_runner:   CargoGateRunner,     // owns its session_target_dir
    pub gates:         GateRunnerConfig,    // thresholds + allowlist (Arc)
    pub weights:       RewardWeights,
    pub model:         M,
    pub budget:        StepBudget,
    pub trajectory:    TrajectoryRecorder,
    pub task:          TaskSpec,
    pub metric_cache:  MetricCache,
    pub before_audits: AuditCounts,
}

impl<M: ModelClient> Episode<M> {
    async fn step(&mut self, action: &Action, tokens: u32) -> Result<StepRecord> {
        // Borrow per-action; Commit<'_> lifetime is local to step().
        let mut commit = Commit {
            host:          &mut self.host,
            snap:          &self.snap,
            thresholds:    &self.gates.thresholds,
            gate:          &self.gate_runner,
            weights:       &self.weights,
            metric_cache:  &mut self.metric_cache,
            before_audits: &mut self.before_audits,
        };
        // dispatch ... commit.run(...).await?
    }
}
```

`Navigator` and `Crud` are also built per-step from configs + borrows, not
stored. If `Crud` and `Commit` need overlapping `&mut self.host` in
practice, the fallback is `Arc<Mutex<WorkspaceHost>>` shared between
constructs — cheap clone-of-arc, no contention in a single-threaded
episode loop. Section J §9 (`Episode::new`) is revised: store only owned
or Arc-backed fields; never store a borrowing struct.

## E3 — FileEdit.path is workspace-relative (resolves Finding #3)

**Convention:** `FileEdit.path` is always workspace-relative. The CRUD
layer never passes absolute paths. The host computes the absolute path
exactly once via `self.workspace_root.join(&edit.path)` before VFS lookup
and `fs::write`.

Section G Step 6 (modify_body) is corrected:

```rust
let edit = FileEdit {
    path: rel_file.clone(),         // workspace-relative (was: abs_path)
    new_text,
    edit_class: EditClass::BodyOnly,
};
```

Same correction applies wherever Section G/H constructs a `FileEdit` with
`crud.workspace_root.join(...)` — pass the relative path instead. Enforce
via constructor + debug assertion:

```rust
impl FileEdit {
    pub fn new(path: PathBuf, new_text: String, edit_class: EditClass) -> Self {
        debug_assert!(path.is_relative(), "FileEdit.path must be workspace-relative");
        Self { path, new_text, edit_class }
    }
}
```

Test `file_edit_relative_path_invariant` asserts every emitted edit's
path is relative.

## E4 — Cross-crate usage validity is BodyOnly-only (resolves Finding #4)

Section C §6 claim — "Cross-crate usages from a clean crate to a dirty
crate remain valid in LMDB because the producer's NodeId didn't change"
— **applies only to `EditClass::BodyOnly`**, where the producer's Node,
binding, and usage IDs are unchanged by definition.

For every other class the affected-set already includes reverse-dep crates
(D2), but the diff-patch in Section C §6 must extend its scan window:

| Class | Diff-patch scan window |
|---|---|
| BodyOnly | dirty crate modules only |
| Sig/Vis | dirty crate + reverse-dep crate modules (binding visibility may change → consumer binding/usage rows re-emit) |
| ItemAddRemove | same as Sig/Vis; deletion case must DELETE all `bindings_by_target` / `usages_by_target` rows keyed by removed NodeId across the whole workspace |
| ModuleTree | same as ItemAddRemove; `qualified_name` changes → NodeId changes → all old-NodeId rows orphaned; DELETE workspace-wide then re-emit |
| Macro | full reverse-dep re-extraction; widest scan |
| Cargo | cold rebuild — diff-patch not used |

Concretely Section C Step 6's existing-IDs scan changes:

```rust
let scope_crates: HashSet<NodeId> = match partial.edit_class {
    EditClass::BodyOnly => partial.dirty_crates.iter().copied().collect(),
    _ => partial.dirty_crates.iter()
            .chain(partial.reverse_dep_crates.iter())
            .copied().collect(),
};
let existing_nodes: HashMap<NodeId, Node> = self.dbs.nodes_by_id
    .iter(&rtxn)?
    .filter_map(|r| r.ok())
    .filter(|(_, n)| n.crate_id.map_or(false, |c| scope_crates.contains(&c)))
    .map(|(k, n)| (NodeId::from_bytes_arr(k.try_into().unwrap()), n))
    .collect();
```

For ItemAddRemove and ModuleTree, additional workspace-wide sweeps of
`bindings_by_target` / `usages_by_target` keyed on the dropped NodeIds
are required to remove orphans. The `differential_apply_vs_cold` test
already covers this — it's the contract that forces correctness here.

## E5 — No formatters; preserve byte ranges (resolves Finding #5)

Per project rule: **never run a whole-file formatter.** `prettyplease` is
banned for source edits. The pattern for every AST-using verb:

- Use `syn` / `ra_ap_syntax` for **analysis only** — locate byte ranges of
  the signature, the call expression's arg list, the inherent impl's
  method block, the extracted statement range, etc.
- Construct the replacement text by string-formatting from the
  operation's input fields (the new signature string, the new arg
  expression, etc.) — **never** by re-rendering an AST.
- Splice via `source_edit::splice_bytes(file_text, start, end, replacement)`.
- All bytes outside the splice are preserved verbatim.

Section H §3 (modify_signature) is revised:

```rust
// Locate (signature_start, body_open_brace_start) via syn::ItemFn::sig analysis.
let (sig_start, sig_end) = fn_signature_byte_range(&file_text, &node, &fn_item)?;
let new_sig_text = render_signature(&op.new_sig);  // pure formatter, no AST unparse
let new_file_text = source_edit::splice_bytes(&file_text,
    sig_start as usize, sig_end as usize, &new_sig_text);
```

Section H §5 (callsite rewrite): locate `ExprCall.args` TextRange via `syn`
analysis; build the new argument list as a comma-separated string; splice.
Same pattern for §9 (extract_function — splice into captured byte_range
and append new fn text immediately after item end), §11/12 (extract_trait
— splice into the inherent impl block's item ranges and insert the new
trait + impl text), §16 (inline — splice into the call expression's
TextRange).

`toml_edit` is **kept** for `Cargo.toml` edits because it is documented as
format-preserving (preserves comments, blank lines, key ordering).
`rustfmt` is also banned in this pipeline. The host writes files exactly
as the CRUD layer produced them.

## E6 — Action enum is M3-minimum, Analyze is M2b (resolves Finding #6)

The source plan's "5 verbs" are Navigate / Analyze / CRUD / Simulate /
Commit. M3's `Action` enum intentionally omits Analyze because:

- Vision-layer queries (P1.3) don't drive state changes — they're read-only
  observations the agent gets for free inside `ContextView` (via P1.1
  cluster scale).
- The M3 loop is `modify_body`-only; the simplest viable action set is
  what we want for the first end-to-end test.
- Analyze lands in M2b alongside the rest of the CRUD widening.

Phase-1-exit `Action` (M2b-final) adds the `Analyze` variant:

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum Action {
    Navigate(NavStep),
    Analyze(AnalyzeQuery),         // NEW in M2b
    Crud(CrudOp),
    Simulate(CrudOp),
    Commit,
    DeclareDone { summary: String },
    AskNoOp,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AnalyzeQuery {
    Clusters { at_scale: f32 },
    Outliers { crate_id: NodeId },
    Affinity { a: NodeId, b: NodeId },
    CoChange { a: NodeId, b: NodeId },
    SearchByDescription { query: String, limit: usize },
}
```

Section J §10's enum and Section J Step 11's router add this variant in
M2b. The M3 ship is the smaller enum; this is by design, not omission.

## E7 — Canonical module paths under `src/graph/` (resolves Finding #7)

All new modules live under `crates/rmc-graph/src/graph/` to match the
existing `src/graph/{codemap, query, skeleton, ...}/` layout. Section Z's
file-tree diff is the canonical statement:

```
crates/rmc-graph/src/graph/working/        (D1 working snapshot, D4 undo log, D3 patch helpers)
crates/rmc-graph/src/graph/host/           (P0.2 warm host, D2 edit-class + affected-set, per-crate re-extract)
crates/rmc-graph/src/graph/checkpoint/     (D4 Checkpoint + jj + restore)
crates/rmc-graph/src/graph/view/           (P1.1)
crates/rmc-graph/src/graph/descriptions/   (P1.2)
crates/rmc-graph/src/graph/analyze/        (P1.3)
```

There is **no `affected/` directory** — the earlier draft of this entry
listed one, but Section Z's file-tree (the canonical statement) and Z's
re-export list (`pub use host::affected_set::AffectedSet`,
`pub use host::edit_class::EditClass`) home D2 under `host/` and D3's
patch helpers under `working/patch/`. D2/D3 live in `host/` + `working/`,
not a standalone `affected/`. The **Canonical Reconciliation** section
below is authoritative on the full layout and supersedes this entry where
they differ.

Section B's `crates/rmc-graph/src/working/`, `affected/`, `checkpoint/`
(no `graph/` prefix) and Section C's `crates/rmc-graph/src/host/` are
typos — treat as if prefixed with `graph/`, and re-home B's `affected/*`
into `host/{edit_class,affected_set}.rs` + `working/patch/mod.rs` per the
Canonical Reconciliation. The corresponding `pub mod` declarations belong
in `src/graph/mod.rs`, not `src/lib.rs`:

```rust
// crates/rmc-graph/src/graph/mod.rs
pub mod working;
pub mod host;
pub mod checkpoint;
pub mod view;
pub mod descriptions;
pub mod analyze;
```

Re-exports from `src/lib.rs` continue to surface the public API (e.g.
`pub use graph::host::WorkspaceHost;`).

## E8 — Spike 1 measures partial re-extract (resolves Finding #8)

M0 go/no-go gate: body-only **partial** re-extract p95 < 500ms. Spike 1
must measure exactly that, not a whole-workspace upper bound (which can
falsely reject — whole-workspace 800ms doesn't imply partial > 500ms).

Section B Step 25 is revised:

```rust
fn main() -> Result<()> {
    let fx = WorkspaceFixture::from_env()?;
    let mut report = Report::default();
    report.loc = fx.loc();
    let mut loaded = rmc_graph::graph::loader::load(&fx.root)?;

    // Cold extract recorded as SECONDARY baseline only.
    let t = Instant::now();
    let _ = rmc_graph::graph::extract::extract_full(&loaded);
    report.secondary_cold_extract_ms = t.elapsed().as_millis() as u64;

    for scenario in EditScenario::menu() {
        scenario.apply_to_disk()?;
        let dirty_crate = scenario.dirty_crate_handle(&loaded)?;
        loaded.vfs.set_file_contents(
            scenario.vfs_id(&loaded.vfs)?, Some(scenario.new_bytes()),
        );
        // PRIMARY metric: partial re-extract over the dirty crate only.
        let t = Instant::now();
        let _ = rmc_graph::graph::extract::extract_partial(&loaded, &[dirty_crate]);
        report.per_class.insert(scenario.class(), t.elapsed().as_millis() as u64);
        scenario.revert_on_disk()?;
    }

    println!("{}", serde_json::to_string_pretty(&report)?);
    // Gate: per_class[BodyOnly] p95 < 500ms.
    assert_pass(&report);
    Ok(())
}
```

This makes Section C Step 1 (the `extract::extract_partial` refactor) a
hard prerequisite of M0.2. Sequencing implication:

  M0.0 (A) → M0.1 (B contracts) + Section C Step 1 only → M0.2 (B spikes)

The rest of Section C (warm-host machinery) still gates on M0.2 passing —
only the per-crate emission refactor moves earlier. The Section Z
Milestone Gantt is updated implicitly: M0.2 depends on `extract_partial`
existing in `rmc-graph`.

---

# Canonical Reconciliation — Single Source of Truth

This plan was synthesized from independent per-slice subagent designs.
Sections B and C were authored separately and **each** declared the D1–D4
types; the Errata fixed eight semantic blockers but did not dedup those
declarations, so the same type appears two or three times with different
names, shapes, and module homes. This section picks **one** of each and
names what to delete.

**Precedence (highest first): this section → Errata (E1–E8) → body
(Sections Z, A–J).** Where they disagree, the higher authority wins. The
four scrubs in §R5 are already applied to the body.

## §R1 — Canonical module layout (resolves B ⇄ C ⇄ Z)

Three layouts existed: B's `working/ affected/ checkpoint/`, C's
everything-under-`host/`, and Z's `working/ host/ checkpoint/`. **Z's
file-tree is canonical** — it is the most complete and matches Z's own
re-export list (`pub use host::affected_set::AffectedSet`, etc.). All paths
are under `crates/rmc-graph/src/graph/`:

```
graph/
  working/                  D1 working snapshot, D4 undo log, D3 patch APPLY helpers
    snapshot.rs             WorkingSnapshot, init_from_published, publish_as_new_graph_id
    identity.rs             SessionId, WorkingSnapshotIdentity
    undo_log.rs             UndoLog, UndoBatch, UndoOp, UndoMarker        (C's in-memory design)
    patch/
      mod.rs                DiffPatch + compute_patch/apply_patch + the D3 matrix
                            (SubDb, InvalidationAction, InvalidationRule, invalidations_for, ALL_SUB_DBS)
      nodes.rs  bindings.rs  usages.rs  contains.rs  signatures.rs  statics.rs  meta.rs
  host/                     P0.2 warm host, D2 classifier + affected-set, per-crate re-extract
    workspace_host.rs       WorkspaceHost, FileEdit, EditSeq, apply_edits
    edit_class.rs           EditClass (canonical variants), classify()
    affected_set.rs         AffectedSet, ReverseDepGraph, expand()/affected_set()
    extract_per_crate.rs    extract_partial, PartialExtractionModel
  checkpoint/               D4 checkpoint contract
    mod.rs                  Checkpoint (C's fields), take()
    jj.rs                   jj op log/restore wrappers
    restore.rs              WorkspaceHost::rollback / restore replay
  view/  descriptions/  analyze/        P1.1 / P1.2 / P1.3 (unchanged)
```

There is **no `graph/affected/`**, and none of `host/edits.rs`,
`host/diff_patch.rs`, `host/re_extract.rs`, `host/rollback.rs`,
`checkpoint/checkpoint.rs`, `checkpoint/undo.rs` — those are superseded
B/C filenames (§R6).

## §R2 — Canonical core types (resolves the duplicate D1–D4 declarations)

| Concept | CANONICAL | Home | Superseded |
|---|---|---|---|
| Edit class | `EditClass { BodyOnly, SignatureOrVis, ItemAddRemove, ModuleTree, Macro, CargoManifest }` | `host/edit_class.rs` | B `{…, SigOrVis, …, Cargo}`; C `{Body, Signature, …, CargoManifest}` |
| Host edit input | `FileEdit { path: ws-rel, new_text, edit_class }` (host trusts the class) | `host/workspace_host.rs` | B's `Edit` enum + `classify(&Edit)` — the verb sets the class by construction; no diff-inference |
| Affected set | `AffectedSet { dirty_files, dirty_crates, reverse_dep_crates, full_rebuild }` (struct) | `host/affected_set.rs` | C's `affected_crates() -> Vec<NodeId>` (becomes the builder that returns `AffectedSet`) |
| Undo log | **in-memory** `UndoLog { batches: Vec<UndoBatch> }`; `UndoOp` per primary + per DUP_SORT secondary | `working/undo_log.rs` | B's on-disk `BufWriter` `UndoLog`, `UndoEntry`, byte-offset marker |
| Undo marker | `UndoMarker(EditSeq)` — pop batches with `seq > marker` | `working/undo_log.rs` | B's `UndoLogMarker { byte_offset, entry_count }` |
| Checkpoint | `Checkpoint { jj_op_id: String, file_prior_text: HashMap<PathBuf,String>, edit_seq_marker: EditSeq }` | `checkpoint/mod.rs` | B's `{ jj_op_id: JjOpId, undo_log_marker, ra_edit_seq, caches }` |
| D3 matrix | `SubDb`, `InvalidationAction`, `InvalidationRule`, `invalidations_for(class)`, `ALL_SUB_DBS` | `working/patch/mod.rs` | B's `affected/matrix.rs` (same content, new home) |
| Diff/patch | `DiffPatch { node_inserts/updates/removes, … }` + `compute_patch`/`apply_patch` | `working/patch/mod.rs` | C's `host/diff_patch.rs` (same content, new home) |

Rationale for the load-bearing picks:
- **EditClass names** follow the source plan (`phase-1-implementation.md`)
  and `m0-spikes.md` prose (`BodyOnly`, `SignatureOrVis`, `CargoManifest`).
  B's D3 `invalidations_for` and E4's table use the abbreviations
  `SigOrVis`/`Cargo` — read those as `SignatureOrVis`/`CargoManifest`.
- **In-memory UndoLog** is sufficient: crash-recovery is "drop the
  working-snapshot dir and re-`mdb_copy` from the published base" (the
  slow-path bailout already in Section C). Durability buys nothing the
  recopy doesn't, and the `Vec<UndoBatch>` is what C's apply/rollback
  (Steps 7, 11) and G/J already use.
- **Checkpoint = C's shape** because `file_prior_text` is the *mechanism*
  RA restore needs (replay `set_file_text` with prior text); B's bare
  `ra_edit_seq` can't restore RA without it. `edit_seq_marker` doubles as
  the undo-log marker (the log is keyed by `EditSeq`), so all three D4
  domains are covered: source = `jj_op_id`, graph + RA-seq =
  `edit_seq_marker`, RA-replay data = `file_prior_text`.

**Consequence for `PartialExtractionModel`** (`extract_per_crate.rs`): it
must carry the affected-set context E4's scan-window reads. Canonical:
```rust
pub struct PartialExtractionModel {
    pub edit_class: EditClass,            // NEW — E4 reads partial.edit_class
    pub dirty_crates: Vec<NodeId>,
    pub reverse_dep_crates: Vec<NodeId>,  // NEW — E4 reads partial.reverse_dep_crates
    pub nodes: BTreeMap<NodeId, Node>,
    pub bindings: Vec<Binding>,
    pub usages: Vec<Usage>,
    pub contains: Vec<(NodeId, NodeId)>,
    pub signatures: Vec<(NodeId, FunctionSignature)>,
    pub statics: Vec<(NodeId, StaticMetadata)>,
}
```
The builder copies `edit_class` / `dirty_crates` / `reverse_dep_crates`
from the `AffectedSet`; E4's `partial.edit_class` / `.reverse_dep_crates`
then compile as written.

## §R3 — One M0.1 deliverable, not two

Sections B ("type-first contracts") and C ("warm host") both define D1–D4.
**B is the home of the canonical declarations; C consumes them by `use`,
never re-declares them.** M0.1 (Section B) ships the §R2 types at the §R1
homes; M2a (Section C) implements the methods (`apply_edits`,
`compute_patch`, `apply_patch`, `rollback`, `extract_partial`) against
those exact types. Where Section C's text appears to re-declare `EditClass`
/ `UndoLog` / `Checkpoint`, read it as:
```rust
use crate::graph::{
    host::edit_class::EditClass,
    working::undo_log::UndoLog,
    checkpoint::Checkpoint,
};
```
Section B's `working::patch` helper signatures (the M0.1 exit gate) are the
`working/patch/*` files in §R1.

## §R4 — Canonical crate set

| Crate | Status | Note |
|---|---|---|
| `rmc-semantic` | **NEW, mandatory** | rename engine extracted from `rmc-server::semantic`; breaks the `rmc-server` ⇄ `rmc-crud` cycle (rmc-server deps rmc-crud via `rl`; rmc-crud needs the rename engine). M2a prereq. |
| `rmc-host` | optional, **default SKIP** | keep `WorkspaceHost` in `rmc-graph::graph::host`; extract only if a real circular dep appears. G's `pub use rmc_host::FileEdit` reads as `pub use rmc_graph::graph::host::FileEdit`. |
| `rmc-spikes`, `rmc-crud`, `rmc-gates`, `rmc-reward`, `rmc-episode`, `rmc-rl` | new | as Section Z. |

**`prettyplease` is banned** (E5) — removed from every dep list and the
workspace `Cargo.toml`. `syn` / `ra_ap_syntax` are for byte-range
**analysis only**; replacement text is string-built and spliced.
`toml_edit` is kept (format-preserving, not a whole-file formatter).

**Section H still narrates `prettyplease::unparse(&file)` in several verb
bodies** (`modify_signature`, `extract_*`, `*_module`) — those calls are
**voided by E5 + this section**. The implementer does NOT call `unparse`:
locate the byte range with `syn`/`ra_ap_syntax`, build the replacement
string from the op's fields, and `splice_bytes`. The `syn` `printing`
feature and the `quote` / `proc-macro2` codegen deps are dropped (they
exist only to support unparse). Converting Section H's per-verb bodies from
unparse to locate-and-splice is the one **open rewrite** this
reconciliation does not finish inline — E5 sketches the splice for §3/5/9/
11/12/16; the rest of Section H (file lists, step order, tests) is correct.

## §R5 — Scrubs applied to the body

1. **rmc-semantic extracted** — Z crate inventory, `members`, file-tree,
   and rmc-crud deps updated; rmc-crud deps `rmc-semantic`, not `rmc-server`.
2. **prettyplease removed** — rmc-crud deps, rmc-graph deps, workspace
   `Cargo.toml` diff.
3. **`Episode` de-self-referenced** — Section J's `Episode` no longer
   stores `Commit<'static>` / owned `Crud` / `Navigator`; it stores owned
   config + `host`/`snap`/`semantic` and builds the borrowing structs
   per-step (matches E2).
4. **E7 `affected/` removed** — D2/D3 live under `host/` + `working/patch/`
   per §R1; E7's stray `affected/` dir and `pub mod affected;` deleted.

## §R6 — Superseded names (grep map for Sections B/C)

| Body text | Read as |
|---|---|
| `EditClass::SigOrVis`, `::Signature` | `EditClass::SignatureOrVis` |
| `EditClass::Cargo` | `EditClass::CargoManifest` |
| `EditClass::Body` (Section C) | `EditClass::BodyOnly` |
| `affected/edit.rs`, `affected/set.rs`, `affected/matrix.rs` | `host/edit_class.rs`, `host/affected_set.rs`, `working/patch/mod.rs` |
| `host/edits.rs`, `host/diff_patch.rs`, `host/re_extract.rs`, `host/rollback.rs` | `host/workspace_host.rs`, `working/patch/mod.rs`, `host/extract_per_crate.rs`, `checkpoint/restore.rs` |
| `checkpoint/checkpoint.rs`, `checkpoint/undo.rs` | `checkpoint/mod.rs`, `working/undo_log.rs` |
| B's `Edit` enum + `classify(&Edit)` | build `FileEdit { edit_class }` directly in the verb |
| `Commit<'static>` stored in `Episode` (C) | per-step `Commit<'_>` (E2 / scrub #3) |
| `rmc_host::FileEdit` | `rmc_graph::graph::host::FileEdit` (rmc-host skipped) |
| `OpenedWorkingSnapshot` (Section J) | `WorkingSnapshot` (D1 — already an opened env+dbs handle) |
| `prettyplease` | (removed — string-splice, E5) |
| `OpenedSnapshot::line_to_byte` @ `snapshot.rs:629` | real loc `snapshot.rs:665` |

---

# Section Z — Integration, Milestones, Crate Inventory

## Overview

This section is the project-wide horizontal map that ties the nine vertical slices of the Phase 0 + Phase 1 plan into a single buildable sequence. Each per-slice section (P0.1+P0.4 in Section A, M0 D1–D4 + spikes in Section B, P0.2+P0.3 in Section C, P1.1 in Section D, P1.2 in Section E, P1.3 in Section F, P1.5a+b in Section G, P1.5c+d+e in Section H, P1.4+P1.6 in Section I, P1.7+P1.8 in Section J) designs a self-contained vertical: data structures, modules, tests, exit gates. This Section Z designs how those slices land in the workspace together — the new crates they introduce, the modules they add to existing crates, the new workspace dependencies, the milestone gating, the cross-slice integration tests, and the order in which the slices must execute to burn risk down quickly.

The plan is anchored to two principles taken straight from the source plan in `/home/molaco/Documents/rust-code-mcp-refactor/.plans/phase-1-implementation.md`. First, **Apply == Rebuild**: the warm-host incremental writer (Section C) is the *only* mutation engine. Every CRUD verb (Sections G/H), the simulator (Section I), and the episode runner (Section J) ride that one pipeline. Second, **Decisions before code**: M0 (Section B) ships the four written contracts D1 (working snapshot), D2 (affected-set), D3 (invalidation matrix), D4 (checkpoint) plus the two go/no-go spike numbers (RA fan-out < 500ms body-only, cargo check warm < 2s). Nothing past M0 begins until those land. The milestone order is M0 → M1 (read side, parallel, on the slow cold build) → M2a (write engine core + `modify_body` only) → M3 (the first end-to-end episode with the minimal verb set — early!) → M2b (widen CRUD). The read side does not block on the write engine — read-side slices (Sections D/E/F) run in parallel from M1 against the slow `build_and_persist` snapshot, so the lethal write/reward path (Sections C → G → J) drives the critical path alone.

## Milestone Gantt

| Milestone | Sections / Slices | Inputs | Exit Gate | Go/No-Go |
|---|---|---|---|---|
| **M0.0 — Foundation** | A (P0.1 determinism + P0.4 bench pool) | current `snapshot::build_and_persist`, nix `cuda-code` devshell | Golden test: two cold builds of `rmc` workspace produce equal `nodes_by_id`, `bindings_by_id`, `usages_by_id` byte streams under fixed seed; 50–100 crate pool fetched and all `cargo check` green | Seed in `rmc_config::RuntimeConfig`; golden test green; pool size ≥ 50 |
| **M0.1 — Contracts D1–D4** | B (decisions only) | A complete; the source-plan D1–D4 text | Four checked-in documents (`docs/contracts/D{1,2,3,4}.md`) referenced by code via doctest links; each invalidation row in D3 has a named patch helper signature | Code review sign-off; every D3 row mapped to a `pub(crate) fn patch_*` declaration in `rmc-graph::graph::working::patch` |
| **M0.2 — Spikes** | B (`crates/rmc-spikes` bin) | M0.1 contracts; A's bench pool | Spike 1: `cargo run -p rmc-spikes --bin ra_fanout` reports body-only re-extract p95 < 500ms on 5 reference workspaces of 100k+ LOC. Spike 2: `cargo run -p rmc-spikes --bin cargo_gate` reports warm `cargo check` p95 < 2s on the pool | **Hard go/no-go.** Spike-1 fail → redesign D2 (per-method invalidation rather than per-crate). Spike-2 fail → pivot to RA-type-check-as-gate (`CargoGateMode::RaOnly`) for M3 default |
| **M1 — Read side (parallel)** | D (P1.1 Navigator), E (P1.2 descriptions), F (P1.3 Analyze/vision) | M0.0 only; runs on the *slow* `build_and_persist` snapshot | `cargo test -p rmc-graph navigator::tests::contextview_all_scales` green at item/module/cluster/body scale; `descriptions_by_target` populated for the rmc workspace; cluster labels stable across two seeded runs | Coverage: ContextView assembles non-empty for ≥ 95 % of items; description coverage ≥ 90 % of public items; cluster silhouette > 0.25 |
| **M2a — Write engine core** | C (P0.2 WorkspaceHost + P0.3 rollback) and G (P1.5a `modify_body` only) | M0.1 (D1–D4), M0.2 (spikes pass) | `cargo test -p rmc-graph working::modify_body_e2e` green: edit a body → re-extract dirty crate → diff-patch LMDB → query result equals cold rebuild (golden cross-check from Section A); jj checkpoint round-trip < 1s | Body-only edit→queryable graph p95 < 500ms; differential test green on all 12 body-edit fixtures |
| **M3 — First loop** | J (P1.7 reward + P1.8 episode runner) + I (P1.6 gates, refusal mode only) | M2a | `cargo run -p rmc-rl -- --task dedupe_project_paths --model claude-opus-4-7` completes an episode with `modify_body` only, produces a trajectory, ends with either `declare_done` or budget exhaustion, and reward vector is fully populated | Reward fields non-NaN, jj rollback on hard-fail works, no LMDB corruption after 50 episodes |
| **M2b — CRUD widening** | G (P1.5b move/delete) → H (P1.5c sig, d extract, e module-tree) → I (P1.4 simulator) | M3 | `cargo test -p rmc-crud differential::apply_vs_cold` green for every verb on a held-out fixture suite; `simulate(op)` matches `apply(op)` on all verbs | Differential pass rate 100 %; modify_signature on `dedupe_project_paths`-style sig change preserves call sites under `CallsiteFill::Todo` |
| **Phase-1 exit** | All sections | All milestones | M3 episodes on the rmc repo's known refactors (dedupe_project_paths, extract `WorkspaceHost`) terminate with positive reward delta vs random baseline; phase doc handed to Phase 2 (synthetic data) | Mean reward on 20 held-out tasks > 0 |

## New crate inventory

All new crates live under `crates/` and are added to the workspace `members` list in M0/M2a/M3 (per the schedule below). Versions track the source plan's "right-sized rigor": pin minor for new direct deps, allow `workspace = true` for everything already shared.

- **`crates/rmc-spikes` — dev-only feasibility spikes (Section B)**
  - **Purpose:** house the two M0.2 binaries that decide D2/D3 viability and `CargoGateMode` default. Not shipped; not depended on by any other crate.
  - **Public surface:** two `[[bin]]` targets — `ra_fanout` and `cargo_gate`; one `lib.rs` exposing `bench_harness::{Workload, run, Report}` so both spikes share the same JSON-report format.
  - **Cargo deps:** `rmc-graph` (path), `rmc-config` (path, for `seed`), `anyhow`, `serde`, `serde_json`, `tracing`, `tracing-subscriber`, `clap = "4"` (new shared dep), `tokio` (workspace), and `criterion = "0.5"` as a dev-dependency only.
  - **Built in:** Section B. Removed from `default-members` so a `cargo build` from the root does not compile spikes.

- **`crates/rmc-host` — *optional* extracted warm host (Section C)**
  - **Purpose:** if M2a measurement reveals a circular dep between rmc-graph (which holds extract / storage) and the new working-snapshot + RA-host machinery, lift `WorkspaceHost`, `EditSeq`, and the apply==rebuild engine into their own crate. **Default recommendation: keep inside `rmc-graph::host` and skip this crate.** Listed here so the integration plan does not need a re-shuffle if the extraction does become necessary.
  - **Public surface:** `WorkspaceHost`, `WorkspaceHost::apply_edits`, `WorkspaceHost::checkpoint`, `WorkspaceHost::restore`, `EditClass`, `AffectedSet`.
  - **Cargo deps if extracted:** `rmc-graph` (path, for `storage`, `extract::per_crate`, `ids`, `model`), `ra_ap_*` (workspace), `heed` (workspace), `anyhow`, `tracing`, `serde`, `bincode`, `sha2`.
  - **Built in:** Section C, conditionally. The plan tracks both forks; the file-tree diff below shows the default (in-graph) layout.

- **`crates/rmc-semantic` — rename/refactor mechanics (extracted from `rmc-server`) — M2a prerequisite**
  - **Purpose:** the symbol-rename engine (`SemanticService` + RA `rename` preview) lifted out of `rmc-server::semantic` into its own crate so that **both** `rmc-server` (MCP handlers) **and** `rmc-crud` (CRUD verbs) can depend on it without a cycle. `rmc-server` gains a dep on `rmc-crud` (via the `rl` feature, line below); `rmc-crud` needs the rename engine — if the engine stayed in `rmc-server`, the two crates would depend on each other and **fail to compile**. Extracting `rmc-semantic` is therefore mandatory, not optional. This rejects the earlier "promote in place + depend on `rmc-server`" approach.
  - **Public surface:** `pub struct SemanticService`, `pub struct RenamePreview { edits, file_moves }`, `pub struct RenameEdit`, `pub struct RenameFileMove`, `pub fn rename_by_name(..)`, `pub fn rename_by_position(..)`.
  - **Cargo deps:** `rmc-graph` (path), `ra_ap_ide` (workspace), `ra_ap_ide_db` (workspace), `ra_ap_syntax` (workspace), `anyhow`, `thiserror`, `tracing`, `serde`.
  - **Built in:** prerequisite for Section G (M2a). Mechanical move: `crates/rmc-server/src/semantic/` → `crates/rmc-semantic/src/`; flip `pub(crate)` → `pub` on the four types + two fns (real locs `semantic/mod.rs:53`, `rename.rs:15/41/61/70/168`); `rmc-server` re-points its handlers at the new crate.

- **`crates/rmc-crud` — CRUD verbs (Sections G + H)**
  - **Purpose:** the five Phase-1 verbs (`modify_body`, `move`, `delete`, `modify_signature`, `extract_*`/`inline`, `*_module`/`lift_to_crate`/`lower_to_module`) as pure operations over the working snapshot, expressed as `compute_effects(host, op) -> Effects` and `apply_effects(host, effects) -> Result<Outcome>`. The split satisfies P1.4 (simulator) — simulate is `compute_effects` only.
  - **Public surface:** `pub enum Crud { ModifyBody{..}, Move{..}, Delete{..}, ModifySignature{..}, ExtractFunction{..}, ExtractTrait{..}, Inline{..}, SplitModule{..}, MergeModules{..}, CreateModule{..}, MoveModule{..}, LiftToCrate{..}, LowerToModule{..} }`, `pub struct Effects { source_patches, graph_patches, manifest_patches, would_refuse }`, `pub trait CrudVerb { fn compute_effects(&self, host: &WorkspaceHost, op: Self::Op) -> Result<Effects>; }`, `pub enum CallsiteFill { Todo, RefuseIfMissing, Explicit(String) }`.
  - **Cargo deps:** `rmc-graph` (path, for `WorkspaceHost`, `ids`, `model`, `extract`, `storage`), `rmc-config` (path), `rmc-semantic` (path, for `SemanticService`/`RenamePreview`/`RenameEdit`/`RenameFileMove` rename mechanics — **extracted from `rmc-server` to break the `rmc-server` ⇄ `rmc-crud` dependency cycle**; see Canonical Reconciliation §R4), `ra_ap_syntax` (workspace), `ra_ap_ide` (workspace, for `rename` preview), `ra_ap_ide_db` (workspace), `syn = "2"` (**new shared dep**, for AST *analysis only* — locate byte ranges; replacement text is string-built and spliced, never AST-unparsed, per E5), `toml_edit = "0.22"` (**new shared dep**, format-preserving `Cargo.toml` surgery in P1.5e), `anyhow`, `thiserror`, `tracing`, `serde`, `serde_json`. **No `prettyplease`** (banned by E5).
  - **Built in:** Sections G (modify_body, move, delete) and H (signature, extract/inline, module-tree).

- **`crates/rmc-gates` — write-time guideline gates (Section I)**
  - **Purpose:** wrap the existing audits (`fn_body_audit`, `unsafe_audit`, `recursion_check`, `derive_audit`, `channel_audit`, `docs_audit`, `analyze_complexity`) and the SCC cycle check from `petgraph` into a `GateHarness` that runs over the dirty set produced by D2, returns hard refusals (with `RefusalReason`) and soft penalties.
  - **Public surface:** `pub struct GateHarness`, `pub struct GateReport { hard_refusals: Vec<RefusalReason>, soft_penalties: Vec<Penalty> }`, `pub fn run_gates(host: &WorkspaceHost, dirty: &AffectedSet, allowlist: &BoundaryAllowlist) -> GateReport`, `pub struct BoundaryAllowlist` (read-only loader for `rmc.gates.toml`).
  - **Cargo deps:** `rmc-graph` (path, exposes `query/audits::*`, `fn_body_audit`, `unsafe_audit`, `recursion_check`, `derive_audit`, `channel_audit`, `docs_audit`), `rmc-config` (path), `petgraph = "0.6"` (**new shared dep**, for SCC), `anyhow`, `thiserror`, `tracing`, `serde`, `toml = "0.9"` (workspace already present).
  - **Built in:** Section I.

- **`crates/rmc-reward` — commit + reward vector (Section J first half, P1.7)**
  - **Purpose:** cargo gate runner (`CargoGateMode::{Off, RaOnly, CheckOnly, CheckAndTest, RaPlusCheckEveryK{k:5}}`), audit diff, graph-metric delta (modularity / conductance / clustering coefficient via `petgraph`), reward scalarizer.
  - **Public surface:** `pub struct RewardVector { compile_ok, test_pass_delta, audit_deltas, graph_metric_deltas, gate_penalty, refusal_count }`, `pub fn commit(host: &mut WorkspaceHost, gate_report: &GateReport, mode: CargoGateMode) -> Result<RewardVector>`, `pub enum CargoGateMode`, `pub fn rollback(host: &mut WorkspaceHost, checkpoint: &Checkpoint) -> Result<()>` (thin wrapper over `WorkspaceHost::restore` + `jj op restore`).
  - **Cargo deps:** `rmc-graph` (path), `rmc-gates` (path), `rmc-config` (path), `petgraph = "0.6"`, `linfa = "0.7"`, `linfa-clustering = "0.7"`, `anyhow`, `thiserror`, `tracing`, `serde`, `serde_json`, `tokio` (workspace), `which = "6"`.
  - **Built in:** Section J first half (P1.7).

- **`crates/rmc-episode` — episode runner + trajectory (Section J second half, P1.8)**
  - **Purpose:** the loop: `observe -> act -> reward`, action dispatch over the 5-verb API, step budget, `declare_done`, trajectory log (the future SFT dataset format), per-episode jj checkpoint.
  - **Public surface:** `pub struct EpisodeRunner`, `pub struct Trajectory { steps: Vec<Step> }`, `pub struct Step { observation: ContextView, action: Action, reward: RewardVector, refusal: Option<RefusalReason> }`, `pub enum Action { Crud(Crud), Navigate(NavAction), Simulate(Crud), DeclareDone }`, `pub trait Policy { async fn act(&mut self, obs: &ContextView) -> Result<Action>; }`, `pub struct AnthropicPolicy` (default impl wrapping the Anthropic Messages API).
  - **Cargo deps:** `rmc-graph` (path), `rmc-crud` (path), `rmc-gates` (path), `rmc-reward` (path), `rmc-config` (path), `tokio` (workspace), `serde`, `serde_json`, `anyhow`, `thiserror`, `tracing`, `reqwest = { workspace = true }` for the Anthropic API client.
  - **Built in:** Section J second half (P1.8).

- **`crates/rmc-rl` (bin) — CLI driver (Section J close)**
  - **Purpose:** thin `clap`-driven CLI: `rmc-rl episode --task <name> --model <id> --budget <n>` and `rmc-rl bench-spike` (forwards to `rmc-spikes`). Single `[[bin]]` target, no library surface.
  - **Cargo deps:** `rmc-episode` (path), `rmc-config` (path), `clap = "4"`, `anyhow`, `tokio`, `tracing`, `tracing-subscriber`.
  - **Built in:** Section J close.

## Modified existing crate inventory

For each existing crate, the changes are anchored to the section that introduces them.

- **`crates/rmc-config` (Section A → updated in B, E, I, J)**
  - **New pub APIs:** `pub struct RuntimeConfig { pub seed: u64, pub cargo_gate_mode: CargoGateMode, pub callsite_fill: CallsiteFill, pub working_snapshot_root: PathBuf, pub anthropic_model: String, pub description_model: String }`.
  - **New modules:** `src/runtime.rs` (RuntimeConfig + env loader); `src/anthropic.rs` (model id + API-key plumbing — pure config, no client).
  - **No new heavy deps:** keep `rmc-config` deps-thin (anyhow, tracing, directories already present).
  - **Schema/version bump:** the existing `Config` keeps `from_env`; the new `RuntimeConfig` adds `from_env_with_seed(default_seed: u64)`. Bumps `rmc-config` minor version 0.1 → 0.2 inside the workspace.

- **`crates/rmc-engine` (Section E mainly; small Section F changes)**
  - **New modules:** `src/embeddings/descriptions.rs` (description generator: prompt formatting, batched LLM call, `DescriptionRecord { content_hash, text, model_id }`).
  - **New feature flag:** `description-llm` gating the description generator on `reqwest` + `serde_json` (already present).
  - **Surface changes:** `EmbeddingPipeline::process_chunks` gets an overload `process_descriptions(items: &[DescribedItem])`.
  - **No new heavy deps** beyond what `embeddings` already pulls.

- **`crates/rmc-graph` (Sections C, D, E, F — *largest* change surface)**
  - **New top-level submodules under `src/graph/`** (all introduced in Section C unless noted):
    - `working/` — D1's working-snapshot. Contains `working/snapshot.rs` (`WorkingSnapshot`), `working/undo_log.rs` (D4 per-key inverse log; new sub-DB `undo_log_by_edit_seq` DUP_SORT), `working/init.rs` (`mdb_copy` from a published `graph_id`).
    - `host/` — D2's RA-warm host. `host/workspace_host.rs` (`WorkspaceHost`), `host/edit_class.rs` (`EditClass` + classifier), `host/affected_set.rs` (D2 algorithm using existing `crate_edges` reversed), `host/patch.rs` (D3 dispatcher), `host/extract_per_crate.rs` (refactor of `extract::emit_crate`).
    - `analyze/` — Section F (P1.3). `analyze/cluster.rs` (`GmmCluster`, soft membership), `analyze/outliers.rs` (LOF/Mahalanobis), `analyze/affinity.rs` (node2vec-ish over `petgraph`), `analyze/co_change.rs` (Apriori/lift over `git log --name-only`), `analyze/labels.rs`.
    - `view/` — Section D (P1.1). `view/location.rs`, `view/context_view.rs`, `view/navigate.rs`, `view/scale.rs`.
    - `descriptions/` — Section E (P1.2). `descriptions/store.rs` (new LMDB sub-DB `descriptions_by_target`), `descriptions/regen.rs`, `descriptions/search.rs`.
    - `checkpoint/` — Section C (P0.3). `checkpoint/mod.rs`, `checkpoint/jj.rs`.
  - **Refactored modules:** `extract.rs` is split — the per-workspace driver stays, the per-crate `emit_crate` body is exposed as `pub(crate) fn emit_crate_into(...)`.
  - **New sub-DBs added to `storage::GraphDatabases`** (per D3): `descriptions_by_target`, `undo_log_by_edit_seq` (DUP_SORT), `vision_cache_by_session` (DUP_SORT), `working_meta_by_session`.
  - **New pub APIs surfaced for downstream crates:** `pub use working::WorkingSnapshot`, `pub use host::WorkspaceHost`, `pub use host::affected_set::AffectedSet`, `pub use host::edit_class::EditClass`, `pub use view::ContextView`, `pub use checkpoint::Checkpoint`, `pub use descriptions::store::DescriptionRecord`.
  - **Schema/version bump:** `storage::SCHEMA_VERSION` bumps from current to next + 1.
  - **New deps in `crates/rmc-graph/Cargo.toml`:** `petgraph = { workspace = true }`, `linfa = { workspace = true }`, `linfa-clustering = { workspace = true }`, `linfa-anomaly = { workspace = true }`, `syn = { workspace = true }` (AST *analysis only*, per E5). **No `prettyplease`** (banned by E5).

- **`crates/rmc-indexing` (Section A — small)**
  - **New module:** `src/indexing/seed.rs` — single source of truth for the deterministic ordering of file walks.
  - **No new deps.**

- **`crates/rmc-server` (Sections D, E, F, G/H, J — handler additions only)**
  - **New modules:** `src/mcp/handlers/navigate.rs`, `describe.rs`, `analyze.rs`, `crud.rs`, `episode.rs`.
  - **`semantic/` module extracted, not modified in place:** `src/semantic/` moves out to the new `crates/rmc-semantic/` (see New crate inventory + cycle rationale). The four types and two fns are promoted `pub(crate)` → `pub` **in the new crate**; `rmc-server`'s handlers re-point at `rmc-semantic`. Separately, promote `OpenedSnapshot::line_to_byte` from `pub(crate)` → `pub` — this stays in `rmc-graph` (real loc `snapshot.rs:665`, not the earlier-cited `629`).
  - **Workspace dep additions:** `rmc-semantic` (path, replaces the in-tree `semantic/` module — non-feature-gated); `rmc-crud`, `rmc-gates`, `rmc-reward`, `rmc-episode` (all gated by a new `rl` feature). The `rl`-feature dep on `rmc-crud` is exactly why `semantic/` had to leave this crate.
  - **No schema bumps.**

- **`crates/rust-code-mcp` (no changes)** — top-level bin keeps using `rmc-server`; the new RL stack is driven by the new `rmc-rl` bin instead.


## Workspace Cargo.toml diff

The single source of truth is the root `Cargo.toml`. Below is the diff against the file read above. Versions follow the source plan's hints; everything not explicitly pinned uses `workspace = true` propagation.

```diff
 [workspace]
 resolver = "3"
 members = [
   "crates/rust-code-mcp",
   "crates/rmc-config",
   "crates/rmc-engine",
   "crates/rmc-graph",
   "crates/rmc-indexing",
   "crates/rmc-server",
+  # M0 (Section B)
+  "crates/rmc-spikes",
+  # M2a prerequisite (Section G) — extracted from rmc-server to break the rmc-server ⇄ rmc-crud cycle
+  "crates/rmc-semantic",
+  # M2b/M3 (Sections G + H)
+  "crates/rmc-crud",
+  # M3 (Section I)
+  "crates/rmc-gates",
+  # M3 (Section J)
+  "crates/rmc-reward",
+  "crates/rmc-episode",
+  "crates/rmc-rl",
+  # Optional — only if `WorkspaceHost` extraction is taken (default: skip).
+  # "crates/rmc-host",
 ]
 default-members = ["crates/rust-code-mcp"]
+# `rmc-spikes` and `rmc-rl` are intentionally NOT default members.

 [workspace.dependencies]
 # ... unchanged entries elided ...

+# Phase 1: graph algorithms (Sections F, I, J)
+petgraph = "0.6"
+
+# Phase 1: clustering / outliers for vision layer (Section F)
+linfa             = "0.7"
+linfa-clustering  = "0.7"
+linfa-nn          = "0.7"     # nearest-neighbour, used by LOF
+linfa-anomaly     = "0.7"
+
+# Phase 1: AST analysis for CRUD verbs (Sections C, G, H).
+# syn/ra_ap_syntax locate byte ranges ONLY; replacement text is string-built
+# and spliced. No whole-file formatter (prettyplease/rustfmt) — banned by E5.
+syn           = { version = "2", features = ["full", "extra-traits", "visit", "visit-mut"] }
+toml_edit     = "0.22"     # format-preserving Cargo.toml edits (kept; not a formatter)
+
+# Phase 1: CLI driver + spike harness (Sections B, J)
+clap          = { version = "4", features = ["derive"] }
+
+# Phase 1: locate cargo / jj binaries inside the devshell (Section J)
+which         = "6"
+
+# Phase 1: trajectory + reward logging
+chrono        = { version = "0.4", default-features = false, features = ["std", "serde"] }

 [patch.crates-io]
 fastembed = { path = "vendor/fastembed" }
```

## File-tree diff

The final layout under `crates/` after Phase 0 + Phase 1 lands. `+` marks new files/directories; `~` marks files materially modified; unmarked entries are unchanged.

```
crates/
  rust-code-mcp/                          (unchanged)
  rmc-config/
    Cargo.toml                            ~  + RuntimeConfig deps stay tiny
    src/
      lib.rs                              ~  + pub mod runtime; pub mod anthropic;
      config.rs
      config/
        errors.rs
        indexer.rs
+     runtime.rs                          (RuntimeConfig + seed + CargoGateMode etc.)
+     anthropic.rs                        (model id + API-key plumbing)
  rmc-engine/
    Cargo.toml                            ~  + description-llm feature
    src/
      embeddings/
        mod.rs                            ~  + pub mod descriptions;
        batching.rs
+       descriptions.rs                   (DescriptionRecord + LLM batch driver)
      schema.rs
      search/
      vector_store/
  rmc-graph/
    Cargo.toml                            ~  + petgraph, linfa*, syn (no prettyplease — E5)
    src/
      lib.rs                              ~  + pub use re-exports for the new modules
      graph/
        mod.rs                            ~  + pub mod working/host/view/descriptions/analyze/checkpoint
        extract.rs                        ~  split: per-crate driver exposed
        loader.rs                         ~  no-longer-drops RootDatabase: host owns it
        model.rs                          ~  + EditClass / AffectedSet types live here
        snapshot.rs                       ~  publish path unchanged; init-from-working added
        storage.rs                        ~  + SCHEMA_VERSION bump; + new sub-DBs
        query/
          audits.rs                       ~  exposed pub(crate) entry for gate harness
+       working/
+         snapshot.rs                     (WorkingSnapshot)
+         undo_log.rs                     (D4 inverse log)
+         init.rs                         (mdb_copy from base graph_id)
+         patch/
+           nodes.rs / bindings.rs / usages.rs / contains.rs / signatures.rs / statics.rs / meta.rs
+       host/
+         workspace_host.rs               (WorkspaceHost)
+         edit_class.rs                   (D2 classifier)
+         affected_set.rs                 (D2 reverse-dep expansion)
+         extract_per_crate.rs            (calls extract::emit_crate_into)
+       analyze/
+         cluster.rs / outliers.rs / affinity.rs / co_change.rs / labels.rs / feature_vector.rs
+       view/
+         location.rs / context_view.rs / navigate.rs / scale.rs
+       descriptions/
+         store.rs / regen.rs / search.rs / prompt.rs
+       checkpoint/
+         mod.rs / jj.rs / restore.rs
  rmc-indexing/
    src/indexing/
+       seed.rs                           (deterministic file ordering)
  rmc-server/
    Cargo.toml                            ~  + rmc-semantic (path) + rmc-crud/gates/reward/episode (rl feature)
    src/
      mcp/
+       handlers/
+         navigate.rs / describe.rs / analyze.rs / crud.rs / episode.rs
      semantic/                           →  MOVED to crates/rmc-semantic/ (breaks rmc-server ⇄ rmc-crud cycle)
+ rmc-semantic/                            (extracted from rmc-server/src/semantic — M2a prereq)
+   src/lib.rs                             (pub SemanticService / RenamePreview / RenameEdit / RenameFileMove)
+   src/{service,rename}.rs
+ rmc-spikes/                              (Section B — M0)
+   src/lib.rs
+   src/bin/ra_fanout.rs
+   src/bin/cargo_gate.rs
+ rmc-crud/                                (Sections G + H — M2a / M2b)
+   src/lib.rs / effects.rs
+   src/verbs/{modify_body,move_,delete,modify_signature,extract_function,
+              extract_trait,inline,split_module,merge_modules,create_module,
+              move_module,lift_to_crate,lower_to_module}.rs
+ rmc-gates/                               (Section I — M3)
+   src/{lib,hard,soft,allowlist}.rs
+ rmc-reward/                              (Section J — M3)
+   src/{lib,cargo_gate,metric_delta,audit_delta,scalarizer}.rs
+ rmc-episode/                             (Section J — M3)
+   src/lib.rs
+   src/policy/{mod,anthropic}.rs
+   src/{trajectory,budget}.rs
+ rmc-rl/                                  (Section J — M3)
+   src/main.rs
+ rmc-host/                                (OPTIONAL — only if circular dep forces it)
+   src/lib.rs
```

## Cross-slice integration tests

1. **`m0_spike_passes_thresholds` (Section B).** Both spikes within their p95 budgets on the bench pool; report checked into `.plans/m0-evidence/`.
2. **`m1_navigator_on_cold_build` (Sections D + E + F).** Drive `Navigator::goto/zoom/show_body/show_callers/follow_trail` over ≥50 random items at every scale; assert ContextView non-empty.
3. **`m2a_modify_body_warmhost` (Sections C + G + I).** 12 body-edit fixtures: re-extract dirty crate only, D3 patches applied, no checkpoint restore, golden cross-check against cold rebuild.
4. **`m3_first_episode_dedupe_project_paths` (ALL).** End-to-end `cargo run -p rmc-rl` on the `project_paths` dedup task; terminates with reward, no LMDB corruption.
5. **`differential_apply_vs_cold` (Sections C + G + H).** For every CRUD verb and every fixture, LMDB stores byte-equal to a cold rebuild on the post-edit source tree.
6. **`concurrent_episode_no_corruption` (Sections C + J).** 4 parallel `EpisodeRunner`s over disjoint working snapshots; no cross-corruption, published base byte-equal pre/post.
7. **`simulate_equals_apply` (Section I + G/H).** For every verb, `simulate(op).effects == apply(op).effects` on ≥5 fixtures per verb.
8. **`description_regen_rides_dirty_set` (Sections C + E).** `Crud::ModifySignature` only regenerates descriptions for items in the affected set.
9. **`cluster_quality_seeded_stable` (Section F).** Seeded `RuntimeConfig::seed = 42`; two runs produce identical hard membership, soft membership within ε = 1e-6.
10. **`callsite_fill_todo_compiles` (Section H).** `Crud::ModifySignature` with new required param and `CallsiteFill::Todo` → callsites have `todo!()` for the new arg, `cargo check` passes.

## Issue register cross-reference

- **Issue #1 — RA fan-out.** → Section B (spike #1) + Section C (D2 affected-set). Proven by tests 1, 3.
- **Issue #2 — Cargo gate latency.** → Section B (spike #2) + Section J (`CargoGateMode` decision). Proven by tests 1, 4.
- **Issue #3 — CRUD propagation correctness.** → Sections G + H (`compute_effects` / `apply_effects` split) + Section I (differential harness). Proven by tests 5, 7.
- **Issue #4 — Warm host lifecycle.** → Section C (per-session WorkspaceHost; disjoint LMDB envs). Proven by test 6.
- **Issue #5 — DUP_SORT diff-patch bugs.** → Section C (D4 undo log; D3 matrix 1:1 with patch helpers) + Section A (golden cross-check). Proven by tests 3, 5.
- **Issue #6 — modify_signature callsite synthesis.** → Section H (`CallsiteFill::Todo` default). Proven by test 10.
- **Issue #7 — simulate/apply divergence.** → Section I (simulate IS compute_effects). Proven by test 7.
- **Issue #8 — Description staleness.** → Section E (regen rides D2 affected set). Proven by test 8.
- **Issue #9 — Cluster quality as perception.** → Section F (soft membership + zoom_through). Proven by test 9.
- **Issue #10 — Benchmark crates not building.** → Section A (P0.4 aggressive filter).
- **Issue #11 — Determinism vs warm host.** → Section A (seed + golden) + Section C (incremental ordering match). Proven by tests 3, 9.

## Open decisions register (consolidated)

- **Devshell name:** `cuda-code`. Locked.
- **Description-generation LLM (P1.2):** Default = Claude Haiku 4.5 (`claude-haiku-4-5-20251001`). Alternative = local Qwen3-0.6B via Candle.
- **Episode policy model (P1.8):** Default = `claude-opus-4-7`. Set in `RuntimeConfig::anthropic_model`.
- **Cluster algorithm (P1.3):** Default = GMM via `linfa-clustering`. Spectral fallback for sparse-feature workspaces.
- **`CallsiteFill` default (P1.5c):** `CallsiteFill::Todo`. Exposed in `RuntimeConfig::callsite_fill`.
- **`CargoGateMode` default (P1.7):** `CargoGateMode::RaPlusCheckEveryK { k: 5 }`. Fallback if spike #2 fails: `CargoGateMode::RaOnly`.
- **Working-snapshot init (D1):** LMDB `mdb_copy` once per episode from the published `graph_id`.
- **Where `WorkspaceHost` lives:** default = inside `rmc-graph::host`. Extract to `crates/rmc-host` only if circular deps appear.
- **Description storage:** new LMDB sub-DB `descriptions_by_target` + new LanceDB table `descriptions_vec` (not extending the chunks table).
- **Anthropic SDK choice:** call Messages API directly via `reqwest` (rustls-tls). Swap to official Rust SDK if/when stable.
- **`syn` feature flags:** `["full", "extra-traits", "visit", "visit-mut"]`.
- **Schema version policy:** breaking bump on each invalidating change; no migration. Old snapshots rejected with clear error.
- **jj operation isolation (P0.3):** one jj op per `Checkpoint::source`; `restore` is `jj op restore <id>`.
- **Trajectory log format (P1.8):** JSONL one `Step` per line under `${working_snapshot_root}/trajectories/${session_id}.jsonl`.
- **`rmc-server` `rl` feature:** off in the published binary; on in `rmc-rl`.
- **Spike-1 thresholds:** p95 < 500ms body-only on 100k LOC; if exceeded → per-method invalidation in D2.
- **Spike-2 thresholds:** p95 < 2s warm `cargo check`; if exceeded → `CargoGateMode::RaOnly` default.

## Risk reduction order

1. Section A (P0.1 + P0.4) — cheapest, unblocks everything.
2. Section B contracts (M0.1) — code-free risk burn for D1–D4.
3. Section B spikes (M0.2) — burns Issues #1, #2 with hard numbers.
4. Sections C + D + E + F in parallel — write engine is critical path; read side runs in parallel on slow cold build.
5. Section G P1.5a only — thinnest CRUD verb, unblocks M3.
6. Section I gates only (refusal mode) — top of `modify_body`, no simulator yet.
7. Section J P1.7 — reward vector on gated `modify_body`.
8. Section J P1.8 — first end-to-end episode (M3).
9. Sections G+H widening (P1.5b–e) and Section I simulator (P1.4) — M2b.


---

# Section A — P0.1 Determinism + P0.4 Benchmark Pool

## Overview

This slice de-risks the two non-engine items on the M0 critical path. P0.1 makes the cold build of a workspace reproducible byte-for-byte (or content-for-content) so every downstream layer — the warm-host incremental writer (P0.2), the differential apply-vs-cold-rebuild test that gates every CRUD op (P1.5), the secondary-index diff-patch correctness check (issue #5), and the reward signal stability — has a stable ground truth to diff against. Without P0.1, the entire RL training signal can drift on iteration-order noise alone. P0.4 fetches and pins a 50-100 crate benchmark pool that builds cleanly under the nix devshell so the M0 feasibility spikes have realistic workspaces to run against.

These two items have no compile-order dependency on each other, but they share a tester. The reproducibility test (P0.1) needs at minimum one tiny crate to exercise the byte-equality check, and the pool (P0.4) needs the build pipeline to ingest each member without panicking — so the P0.1 implementation should be wired such that the pool members themselves become continuous regression fixtures for determinism. Both must land before M0's two feasibility spikes start.

## Existing nondeterminism inventory (audited)

The extract → persist pipeline has these confirmed `HashMap`/`HashSet` iteration sites whose iteration order propagates into either `Vec`-ordering on `ExtractionModel` or DUP_SORT secondary-index insert-order in LMDB:

1. `crates/rmc-graph/src/graph/bindings.rs:54` — `for (module_id, _) in def_map.modules()` (RA-internal; the outer loop iterates `local_crates` which is ordered).
2. `crates/rmc-graph/src/graph/bindings.rs:116` — `let mut seen: HashSet<(NodeId, String, NodeId, BindingKind)>` dedups `model.bindings` via `retain`. `retain` preserves Vec order — fine.
3. `crates/rmc-graph/src/graph/usages.rs:45` — `for (&def_id, &target_node_id) in def_to_node`. **The worst offender**: `model.usages` is built in HashMap-iteration order.
4. `crates/rmc-graph/src/graph/signatures.rs:47` — same pattern; `model.signatures` order is HashMap order.
5. `crates/rmc-graph/src/graph/statics.rs:33` — same; `model.statics` is HashMap order.
6. `crates/rmc-graph/src/graph/snapshot.rs:408-456` — `write_model` writes DUP_SORT secondaries. **DUP_SORT stores duplicates in value-sort-order**, so insert order does not affect on-disk byte layout for the duplicate values themselves — primary tables are content-addressed and stored by key, also fine.

**Conclusion on byte-equality:** The LMDB file content is determined by `{key, value}` set union — primary tables keyed on content-addressed IDs, secondary DUP_SORT keyed on NodeId with content-addressed values, all sorted. The on-disk layout *should* be deterministic given the same input set, except for LMDB free-list / page-allocation noise. We therefore target **content-equality** (set-equality after `mdb_dump`-style iteration) as the primary contract and **byte-equality** (after `mdb_copy --compact`) as a strict-mode bonus.

## New modules / files

- `crates/rmc-graph/src/graph/determinism.rs` — new module. Houses the canonical sort orders for `ExtractionModel.bindings`, `usages`, `contains`, `signatures`, `statics`; the public `sort_model_for_persistence(&mut ExtractionModel)` entrypoint called from `extract::extract` before `write_model`.
- `crates/rmc-graph/src/graph/snapshot_compare.rs` — new module. Public functions: `dump_snapshot(&OpenedSnapshot) -> SnapshotDump` returning a canonical in-memory representation of every sub-DB as `BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>`, and `compare_snapshots(&SnapshotDump, &SnapshotDump) -> SnapshotDiff` returning per-table set differences. Used by the golden test and later by P0.2's apply-vs-cold-rebuild differential test.
- `crates/rmc-graph/tests/determinism_golden.rs` — integration test. Builds the rmc workspace twice (two staging dirs), dumps both, asserts `compare_snapshots == empty`.
- `crates/rmc-graph/benches/determinism_bench.rs` — micro-benchmark to track the cost of `sort_model_for_persistence` (target: < 5% of total extract time).
- `bench/Cargo.toml` — new workspace **outside** the main workspace (path: `/home/molaco/Documents/rust-code-mcp-refactor/bench/Cargo.toml`). NOT a member of the rmc workspace. It is a separate Cargo workspace that vendors the 50-100 corpus crates.
- `bench/fetch_corpus.sh` — fetch / pin / verify-build script.
- `bench/corpus.toml` — declarative manifest: list of `[corpus.<slug>] git, rev, path, edition, expected_loc, tags`.
- `bench/README.md` — selection criteria, expected build time, troubleshooting.
- `crates/rmc-config/src/config.rs` (edit) — add `pub seed: u64` field to `Config`, with env-var loader `RMC_SEED` (default `0`).
- `crates/rmc-graph/src/graph/snapshot.rs` (edit) — extend `BuildOptions` with `pub seed: u64`; thread through `extract::extract`.

## Type definitions

```rust
// crates/rmc-graph/src/graph/determinism.rs

pub(crate) fn sort_model_for_persistence(model: &mut ExtractionModel) {
    sort_contains(&mut model.contains);
    sort_bindings(&mut model.bindings);
    sort_usages(&mut model.usages);
    sort_signatures(&mut model.signatures);
    sort_statics(&mut model.statics);
}

fn sort_contains(v: &mut Vec<(NodeId, NodeId)>) {
    v.sort_unstable_by(|(p1, c1), (p2, c2)| {
        p1.as_bytes().cmp(p2.as_bytes()).then_with(|| c1.as_bytes().cmp(c2.as_bytes()))
    });
    v.dedup();
}

fn sort_bindings(v: &mut Vec<Binding>) {
    v.sort_unstable_by(|a, b| {
        super::snapshot::binding_id_for(a).as_bytes().cmp(
            super::snapshot::binding_id_for(b).as_bytes(),
        )
    });
}

fn sort_usages(v: &mut Vec<Usage>) {
    v.sort_unstable_by(|a, b| {
        super::snapshot::usage_id_for(a).as_bytes().cmp(
            super::snapshot::usage_id_for(b).as_bytes(),
        )
    });
}

fn sort_signatures(v: &mut Vec<(NodeId, FunctionSignature)>) {
    v.sort_unstable_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
}

fn sort_statics(v: &mut Vec<(NodeId, StaticMetadata)>) {
    v.sort_unstable_by(|(a, _), (b, _)| a.as_bytes().cmp(b.as_bytes()));
}
```

```rust
// crates/rmc-graph/src/graph/snapshot_compare.rs

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotDump {
    pub nodes: BTreeMap<Vec<u8>, Vec<u8>>,             // bincode-encoded Node
    pub bindings: BTreeMap<Vec<u8>, Vec<u8>>,
    pub bindings_by_from_module: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    pub bindings_by_target: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    pub children_by_parent: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    pub usages: BTreeMap<Vec<u8>, Vec<u8>>,
    pub usages_by_target: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    pub usages_by_consumer: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    pub usages_by_consumer_function: BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>,
    pub signatures: BTreeMap<Vec<u8>, Vec<u8>>,
    pub statics: BTreeMap<Vec<u8>, Vec<u8>>,
    pub meta: BTreeMap<String, Vec<u8>>,               // excludes "graph_id", "created_at_unix"
}

#[derive(Debug, Clone, Default)]
pub struct SnapshotDiff { /* per-table _only_in_a / _only_in_b / _value_differs */ }

impl SnapshotDiff { pub fn is_empty(&self) -> bool { /* all empty */ } }

pub fn dump_snapshot(snap: &OpenedSnapshot) -> Result<SnapshotDump>;
pub fn compare_snapshots(a: &SnapshotDump, b: &SnapshotDump) -> SnapshotDiff;
```

```rust
// crates/rmc-config/src/config.rs — extend Config

pub struct Config {
    pub server_port: u16,
    pub data_dir: PathBuf,
    pub max_file_size: u64,
    pub num_threads: usize,
    pub debug: bool,
    pub retry_attempts: u32,
    pub retry_delay_ms: u64,
    /// Global determinism seed. Threaded through every stochastic step.
    /// Future clustering / GMM / node2vec consumers reach for this. 0 by default.
    pub seed: u64,
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            // ...existing fields...
            seed: std::env::var("RMC_SEED").ok().and_then(|s| s.parse().ok()).unwrap_or(0),
        }
    }
}
```

```rust
// crates/rmc-graph/src/graph/snapshot.rs — extend BuildOptions

pub struct BuildOptions {
    pub force_rebuild: bool,
    pub data_dir_override: Option<PathBuf>,
    pub env: GraphEnvOptions,
    pub seed: u64,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self { /* existing */ seed: 0 }
    }
}
```

```toml
# bench/corpus.toml — declarative pool (excerpt)

[meta]
total_crates_target = 75
total_loc_target_min = 200_000

[corpus.serde_json]
git = "https://github.com/serde-rs/json"
rev = "v1.0.128"           # pinned tag, NOT a branch
path = "vendor/serde_json"
edition = "2021"
expected_loc = 18_000
tags = ["small", "serde", "no-build-rs", "no-proc-macro"]
build_cmd = "cargo check --offline --frozen --all-targets"
```

## Step-by-step implementation

### Phase 1: P0.1 — sort the extraction model

1. **WHAT**: Create `crates/rmc-graph/src/graph/determinism.rs` with `sort_model_for_persistence` and its five helpers. **DEPENDS**: nothing. **VERIFY**: `cargo build -p rmc-graph` succeeds.
2. **WHAT**: Add `mod determinism;` to `crates/rmc-graph/src/graph/mod.rs` after `pub(crate) mod snapshot;`. **DEPENDS**: 1. **VERIFY**: `cargo check -p rmc-graph` succeeds.
3. **WHAT**: In `snapshot.rs`, change `binding_id_for` / `usage_id_for` visibility from `pub(crate)` to `pub(in crate::graph)`. **VERIFY**: build succeeds.
4. **WHAT**: Call `determinism::sort_model_for_persistence(&mut model)` at the end of `extract::extract` (after `extract_usages`). **VERIFY**: existing extract tests still pass.
5. **WHAT**: Add `pub seed: u64` to `BuildOptions` (with `Default::default`) and to `Config` with env-var loader `RMC_SEED`. **VERIFY**: `BuildOptions::default().seed == 0`.
6. **WHAT**: Thread `seed` from `Config` → call sites that construct `BuildOptions`. **VERIFY**: `cargo check --workspace` succeeds.
7. **WHAT**: Change `extract::extract(loaded: &LoadedWorkspace, seed: u64) -> ExtractionModel`; thread seed through `sort_model_for_persistence` (today ignored — plumbing for P1.3 clustering). **VERIFY**: build.

### Phase 2: P0.1 — snapshot comparison + golden test

8. **WHAT**: Create `snapshot_compare.rs` with `SnapshotDump`, `SnapshotDiff`, `dump_snapshot`, `compare_snapshots`. For `meta_by_key` exclude `"graph_id"` and `"created_at_unix"`. **DEPENDS**: 1. **VERIFY**: unit test on a synthetic snapshot via `persist_test_model` round-trips to a non-empty dump.
9. **WHAT**: Create `crates/rmc-graph/tests/determinism_golden.rs` with `two_cold_builds_are_content_equal`. **DEPENDS**: 4 + 8. **VERIFY**: `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --test determinism_golden`. Expected ~30-60s.
10. **WHAT**: Add `#[ignore] byte_equal` variant using `heed::EnvOpenOptions::copy_to_file` with compaction. **VERIFY**: `cargo test -p rmc-graph --test determinism_golden -- --ignored` reports equal hashes.

### Phase 3: P0.1 — fix remaining ordering escapes

11–16. Audit `usages.rs:45`, `signatures.rs:47`, `statics.rs:33`, `bindings.rs:54`, `bindings.rs:42-44`, `impls.rs:42,49`. Convert HashMaps that are iterated for emission to `BTreeMap` or `FxHashMap` (deterministic seed). Add `// HashMap-iteration: order leaks; canonicalized by graph::determinism::sort_*` comments. **VERIFY**: golden test from step 9 still passes.

### Phase 4: P0.4 — benchmark pool

17. **WHAT**: Create directory `bench/` (sibling of `crates/`) with `Cargo.toml`:
    ```toml
    [workspace]
    resolver = "3"
    members = []
    exclude = ["vendor"]
    ```
    **VERIFY**: `cargo check --manifest-path bench/Cargo.toml` succeeds.
18. **WHAT**: Add `bench/` to root `Cargo.toml`'s `[workspace] exclude`. **VERIFY**: workspace check does not visit `bench/`.
19. **WHAT**: Author `bench/corpus.toml` with 75 entries: 5 small smoke, 30 small, 25 medium, 10 large, 5 huge. Hard filter: must build with `cargo check --offline --frozen --all-targets` after `cargo fetch`; reject system-C-lib deps, build.rs network downloads, nightly-only crates. **VERIFY**: parses; entry count ≥ 50; every `rev` is a SHA or vX.Y.Z tag.
20. **WHAT**: Author `bench/fetch_corpus.sh`. Parse corpus.toml, clone-if-missing into `$VENDOR/<slug>`, checkout rev, update `bench/Cargo.toml` members, `cargo fetch --locked`, per-entry `cargo check`, record success/duration into `bench/build_report.json`. Exit 0 if ≥ 50 passed. **VERIFY**: `nix develop ../nix-devshells#cuda-code --command bash bench/fetch_corpus.sh` writes report with ≥ 50 passing.
21. **WHAT**: Author `bench/build_report.json` schema validator. **VERIFY**: known-good exits 0; passed < 50 exits 1.
22. **WHAT**: Smoke test `bench/tests/smoke.sh` that picks 3 smallest crates and runs `build_hypergraph`. **VERIFY**: produces non-empty `workspace_stats`.
23. **WHAT**: Extend `tests/determinism_golden.rs` with `#[ignore] corpus_crates_are_content_equal` over 5 smallest corpus crates. **VERIFY**: `RMC_BENCH_DETERMINISM=1 cargo test --ignored corpus_crates_are_content_equal`.

### Phase 5: integration + docs

24–26. Surface `seed` in `Config::print_summary`. Add "Determinism" and "Benchmark Pool" subsections to `AGENTS.md`.

## Tests

- **`two_cold_builds_are_content_equal`** — build rmc workspace twice into tmpdirs, dump, assert `SnapshotDiff::is_empty()`.
- **`two_cold_builds_are_byte_equal_after_compact`** (#[ignore]) — compact both LMDB envs and `sha256` `data.mdb`.
- **`corpus_crates_are_content_equal`** (#[ignore], `RMC_BENCH_DETERMINISM=1`) — over 5 smallest corpus crates.
- **`seed_field_propagates_through_build_options`** — construct `BuildOptions { seed: 42, ... }`, assert no panic.
- **`sort_bindings_is_total_and_idempotent`** — shuffled IDs, two calls same result, two shuffles same outputs.
- **`sort_usages_is_total_and_idempotent`** — same shape for Usage.
- **`sort_contains_dedups`** — `[(A,B),(A,B),(C,D)]` → `[(A,B),(C,D)]`.
- **`dump_round_trip`** — `persist_test_model`, dump, assert non-empty.
- **`compare_identical_dumps_is_empty`** — two read txns, same snapshot.
- **`compare_detects_node_diff`** — mutate one byte, assert `nodes_value_differs.len() == 1`.

## Open decisions / risks

- **Risk: `def_map.modules()` RA-internal-stable**. If RA upgrades change ordering, the sort-after-extract neutralizes it for persist, but `def_to_node` insertion order could shift which Node value wins on a duplicate. *Mitigation*: assert all duplicates produce equal Node records.
- **Risk: byte-equality after `mdb_copy --compact` may differ due to LMDB metadata pages**. *Mitigation*: accept content-equality as primary contract; document strict mode as informational.
- **Risk: 50/75 corpus crates failing**. *Mitigation*: oversample (target 100 candidates, ship 50-75).
- **Risk: 75 git repos at full history is slow** (~5 GB). *Mitigation*: `git clone --depth 1 --branch <tag>`.
- **Risk: nix devshell pinned MSRV ≠ corpus MSRV**. *Mitigation*: pick corpus by MSRV ≤ devshell's stable channel.
- **Open decision: `bench/` as git submodule vs sibling repo?** Recommend submodule.
- **Open decision: where does seed get consumed in P0.1?** Today nowhere; it's plumbing for P1.3.
- **Risk: warm-host incremental rebuilds produce extraction in different order than cold rebuilds**. *Mitigation*: warm-host merge path (P0.2) must call same `sort_model_for_persistence` on merged model. Document this now.


---

# Section B — M0 Contracts (D1–D4) + Feasibility Spikes

## Overview

This slice formalises the four written contracts that gate every subsequent build step (P0.2 warm-host writer, P1.5 CRUD, P1.6 gates, P1.7 reward). The decisions live as prose in `.plans/phase-1-implementation.md`, but until they are encoded as Rust types they cannot be referenced or unit-tested. The work here is therefore *type-first*: introduce the smallest possible set of new modules under `rmc-graph` that hold the canonical declarations of `WorkingSnapshot` (D1), `EditClass` + `AffectedSet` (D2), the invalidation matrix table (D3) and `Checkpoint` + `UndoLog` (D4). No mutation paths are wired up yet — that lands in M2a.

Alongside the contracts we ship a brand-new dev-only crate `rmc-spikes` containing two binaries: `ra_fanout` (Spike 1) and `cargo_latency` (Spike 2). Both produce JSON reports with hard go/no-go numbers — body-only re-extract < 500 ms and warm `cargo check` < ~2 s on the P0.4 pool. If either spike fails its threshold, the M2 plan must be revisited before P0.2 starts.

## New modules / files

- `crates/rmc-graph/src/working/mod.rs` — module root for D1 (`WorkingSnapshot`, session-id type, copy/publish ops).
- `crates/rmc-graph/src/working/snapshot.rs` — `WorkingSnapshot` + `init_from_published`, `working_dir_for`, `working_paths`, `publish_as_new_graph_id`. Owns the LMDB `mdb_copy` via `heed::Env::copy_to_path`.
- `crates/rmc-graph/src/working/identity.rs` — `SessionId(Uuid)`, `WorkingSnapshotIdentity { session_id, base_graph_id, edit_seq }`.
- `crates/rmc-graph/src/affected/mod.rs` — module root for D2 + D3, re-exports `EditClass`, `Edit`, `AffectedSet`, `InvalidationRule`, `SubDb`, `InvalidationAction`, `invalidations_for`, `classify`, `expand`.
- `crates/rmc-graph/src/affected/edit.rs` — `Edit` enum (typed payload a CRUD op hands the engine) and `classify(&Edit) -> EditClass`. Classification is by construction — no diff inference.
- `crates/rmc-graph/src/affected/set.rs` — `AffectedSet`, `expand(class, edit, &ReverseDepGraph) -> AffectedSet`, and `ReverseDepGraph` (in-memory reverse adjacency of `crate_edges()`).
- `crates/rmc-graph/src/affected/matrix.rs` — `SubDb` enum (one variant per field of `GraphDatabases`), `InvalidationAction`, `InvalidationRule`, the static `INVALIDATION_MATRIX` table, and `invalidations_for(class) -> Vec<InvalidationRule>`. A missing (class, sub-db) pair is a compile-time `match` exhaustiveness failure.
- `crates/rmc-graph/src/checkpoint/mod.rs` — module root for D4.
- `crates/rmc-graph/src/checkpoint/undo.rs` — `UndoEntry`, `UndoLog`, append-only on-disk format (`working/<session_id>/undo.log`), `record / restore / mark`.
- `crates/rmc-graph/src/checkpoint/checkpoint.rs` — `Checkpoint`, `JjOpId`, `UndoLogMarker`, `RaEditSeq`, plus `take_checkpoint`, `restore`.
- `crates/rmc-graph/src/lib.rs` — add `pub mod working;`, `pub mod affected;`, `pub mod checkpoint;`.
- `crates/rmc-graph/Cargo.toml` — add `uuid = { version = "1.10", features = ["v4", "serde"] }`.
- `crates/rmc-spikes/Cargo.toml` — new dev-only crate; `publish = false`; depends on `rmc-graph`, `rmc-indexing`, `anyhow`, `serde`, `serde_json`, `tracing`, `clap`.
- `crates/rmc-spikes/src/lib.rs` — shared helpers: `WorkspaceFixture`, `EditScenario`, `Report`, `measure_re_extract`.
- `crates/rmc-spikes/src/bin/ra_fanout.rs` — Spike 1.
- `crates/rmc-spikes/src/bin/cargo_latency.rs` — Spike 2.
- `Cargo.toml` (workspace) — add `"crates/rmc-spikes"` to `members`.

## Type definitions

### D1 — `crates/rmc-graph/src/working/identity.rs`

```rust
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self { Self(Uuid::new_v4()) }
    pub fn to_hex(self) -> String { self.0.simple().to_string() }
}

/// Identity tuple for a working snapshot. Decoupled from content fingerprint
/// by design — see D1. Two working snapshots with the same `(base_graph_id,
/// edit_seq)` but different `session_id` are distinct artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkingSnapshotIdentity {
    pub session_id: SessionId,
    pub base_graph_id: String,
    pub edit_seq: u64,
}
```

### D1 — `crates/rmc-graph/src/working/snapshot.rs`

```rust
use std::path::{Path, PathBuf};
use std::sync::Arc;
use anyhow::Result;
use heed::{CompactionOption, Env, WithoutTls};

use crate::graph::storage::{GraphDatabases, GraphEnvOptions, GraphManifest, GraphPaths};
use super::identity::{SessionId, WorkingSnapshotIdentity};

pub struct WorkingSnapshot {
    pub identity: WorkingSnapshotIdentity,
    pub working_dir: PathBuf,
    pub env: Arc<Env<WithoutTls>>,
    pub dbs: GraphDatabases,
    pub base_manifest: GraphManifest,
}

impl WorkingSnapshot {
    /// Copy `<workspace_hash>/snapshots/<base_graph_id>/data.mdb` into a fresh
    /// working dir using `heed::Env::copy_to_path` (LMDB `mdb_copy`).
    pub fn init_from_published(
        paths: &GraphPaths,
        base_graph_id: &str,
        env: GraphEnvOptions,
    ) -> Result<Self> { todo!() }

    pub fn publish_as_new_graph_id(&self) -> Result<String> { todo!() }
    pub fn drop_session(self) -> Result<()> { todo!() }
    pub fn bump_edit_seq(&mut self) { self.identity.edit_seq += 1 }
}

pub fn working_dir(paths: &GraphPaths, session: SessionId) -> PathBuf { todo!() }
```

### D2 — `crates/rmc-graph/src/affected/edit.rs`

```rust
use std::path::PathBuf;
use serde::{Deserialize, Serialize};
use crate::graph::ids::NodeId;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Edit {
    ModifyBody {
        target: NodeId,
        file: PathBuf,
        byte_span: (u32, u32),
        new_body: String,
    },
    ModifySignature {
        target: NodeId,
        file: PathBuf,
        new_signature_source: String,
    },
    ItemAddRemove {
        op: ItemMutation,
        parent_module: NodeId,
        target_qualified: String,
    },
    ModuleTree {
        affected_files: Vec<PathBuf>,
        affected_crate: NodeId,
    },
    Macro { affected_crate: NodeId },
    Cargo { file: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ItemMutation { Add, Remove }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EditClass {
    BodyOnly, SigOrVis, ItemAddRemove, ModuleTree, Macro, Cargo,
}

pub fn classify(edit: &Edit) -> EditClass {
    match edit {
        Edit::ModifyBody { .. } => EditClass::BodyOnly,
        Edit::ModifySignature { .. } => EditClass::SigOrVis,
        Edit::ItemAddRemove { .. } => EditClass::ItemAddRemove,
        Edit::ModuleTree { .. } => EditClass::ModuleTree,
        Edit::Macro { .. } => EditClass::Macro,
        Edit::Cargo { .. } => EditClass::Cargo,
    }
}
```

### D2 — `crates/rmc-graph/src/affected/set.rs`

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AffectedSet {
    pub dirty_files: BTreeSet<PathBuf>,
    pub dirty_crates: BTreeSet<NodeId>,
    pub reverse_dep_crates: BTreeSet<NodeId>,
    pub full_rebuild: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ReverseDepGraph {
    by_producer: BTreeMap<NodeId, BTreeSet<NodeId>>,
}

impl ReverseDepGraph {
    pub fn from_crate_edges<E: ToCrateEdge>(edges: impl IntoIterator<Item = E>) -> Self { todo!() }
    pub fn consumers_of(&self, producer: NodeId) -> impl Iterator<Item = NodeId> + '_ {
        self.by_producer.get(&producer).into_iter().flat_map(|s| s.iter().copied())
    }
}

pub trait ToCrateEdge {
    fn consumer_crate(&self) -> NodeId;
    fn producer_crate(&self) -> NodeId;
}

pub fn expand(
    class: EditClass,
    edit: &Edit,
    rdg: &ReverseDepGraph,
    crate_of_file: &dyn Fn(&std::path::Path) -> Option<NodeId>,
) -> Result<AffectedSet> { todo!() }
```

### D3 — `crates/rmc-graph/src/affected/matrix.rs`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SubDb {
    NodesById, BindingsById, BindingsByFromModule, BindingsByTarget,
    ChildrenByParent, UsagesById, UsagesByTarget, UsagesByConsumer,
    UsagesByConsumerFunction, SignaturesByTarget, StaticMetadataByTarget,
    EmbeddingsByTarget, DescriptionsByTarget, MetaByKey, Manifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InvalidationAction {
    Patch, ReDerive, ContentHashCache, Unchanged, Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct InvalidationRule { pub table: SubDb, pub action: InvalidationAction }

pub fn invalidations_for(class: EditClass) -> Vec<InvalidationRule> {
    use EditClass::*; use InvalidationAction::*; use SubDb::*;
    let mut out = Vec::with_capacity(15);
    macro_rules! r { ($t:expr, $a:expr) => { out.push(InvalidationRule { table: $t, action: $a }); }; }
    match class {
        BodyOnly => {
            r!(NodesById, Unchanged);  r!(BindingsById, Unchanged);
            r!(BindingsByFromModule, Unchanged);  r!(BindingsByTarget, Unchanged);
            r!(ChildrenByParent, Unchanged);  r!(UsagesById, Patch);
            r!(UsagesByTarget, Patch);  r!(UsagesByConsumer, Patch);
            r!(UsagesByConsumerFunction, Patch);  r!(SignaturesByTarget, Unchanged);
            r!(StaticMetadataByTarget, ReDerive);  r!(EmbeddingsByTarget, ContentHashCache);
            r!(DescriptionsByTarget, ContentHashCache);  r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        SigOrVis => {
            for t in [NodesById, BindingsById, BindingsByFromModule, BindingsByTarget] { r!(t, Patch); }
            r!(ChildrenByParent, Unchanged);
            for t in [UsagesById, UsagesByTarget, UsagesByConsumer, UsagesByConsumerFunction] { r!(t, Patch); }
            r!(SignaturesByTarget, ReDerive);  r!(StaticMetadataByTarget, ReDerive);
            r!(EmbeddingsByTarget, ContentHashCache);  r!(DescriptionsByTarget, ContentHashCache);
            r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        ItemAddRemove => {
            for t in [NodesById, BindingsById, BindingsByFromModule, BindingsByTarget, ChildrenByParent,
                      UsagesById, UsagesByTarget, UsagesByConsumer, UsagesByConsumerFunction] { r!(t, Patch); }
            r!(SignaturesByTarget, ReDerive);  r!(StaticMetadataByTarget, ReDerive);
            r!(EmbeddingsByTarget, ContentHashCache);  r!(DescriptionsByTarget, ContentHashCache);
            r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        ModuleTree => {
            for t in [NodesById, BindingsById, BindingsByFromModule, BindingsByTarget, ChildrenByParent,
                      UsagesById, UsagesByTarget, UsagesByConsumer, UsagesByConsumerFunction] { r!(t, Patch); }
            r!(SignaturesByTarget, Unchanged);  r!(StaticMetadataByTarget, Unchanged);
            r!(EmbeddingsByTarget, ContentHashCache);  r!(DescriptionsByTarget, ContentHashCache);
            r!(MetaByKey, Patch);  r!(Manifest, Patch);
        }
        Macro | Cargo => {
            for t in ALL_SUB_DBS {
                let a = match t {
                    EmbeddingsByTarget | DescriptionsByTarget => ContentHashCache,
                    _ => Full,
                };
                r!(*t, a);
            }
        }
    }
    out
}

pub(crate) const ALL_SUB_DBS: &[SubDb] = &[
    SubDb::NodesById, SubDb::BindingsById, SubDb::BindingsByFromModule,
    SubDb::BindingsByTarget, SubDb::ChildrenByParent, SubDb::UsagesById,
    SubDb::UsagesByTarget, SubDb::UsagesByConsumer, SubDb::UsagesByConsumerFunction,
    SubDb::SignaturesByTarget, SubDb::StaticMetadataByTarget, SubDb::EmbeddingsByTarget,
    SubDb::DescriptionsByTarget, SubDb::MetaByKey, SubDb::Manifest,
];
```

### D4 — `crates/rmc-graph/src/checkpoint/undo.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub sub_db: SubDb,
    pub key: Vec<u8>,
    pub prior_value: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoLogMarker { pub byte_offset: u64, pub entry_count: u64 }

pub struct UndoLog {
    pub path: PathBuf,
    writer: BufWriter<File>,
    entry_count: u64,
}

impl UndoLog {
    pub fn open(working_dir: &Path) -> Result<Self> { todo!() }
    pub fn record(&mut self, entry: UndoEntry) -> Result<()> { todo!() }
    pub fn mark(&mut self) -> Result<UndoLogMarker> { todo!() }
    pub fn restore(&mut self, marker: UndoLogMarker, wtxn: &mut RwTxn<'_>,
                   dbs: &crate::graph::storage::GraphDatabases) -> Result<()> { todo!() }
    pub fn entry_count(&self) -> u64 { self.entry_count }
}
```

### D4 — `crates/rmc-graph/src/checkpoint/checkpoint.rs`

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JjOpId(pub String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RaEditSeq(pub u64);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub jj_op_id: JjOpId,
    pub undo_log_marker: UndoLogMarker,
    pub ra_edit_seq: RaEditSeq,
    pub caches: (),  // content-hash keyed → self-heal
}

pub fn take_checkpoint(ws: &WorkingSnapshot, undo: &mut UndoLog,
                       jj_op_id: JjOpId, ra_edit_seq: RaEditSeq) -> Result<Checkpoint> { todo!() }

/// All-or-nothing restore. Caller MUST drop the working snapshot on Err.
pub fn restore(ws: &WorkingSnapshot, checkpoint: &Checkpoint,
               undo: &mut UndoLog, ra_host: &mut dyn RaHostHandle) -> Result<()> { todo!() }

pub trait RaHostHandle {
    fn current_edit_seq(&self) -> RaEditSeq;
    fn rewind_to(&mut self, target: RaEditSeq) -> Result<()>;
}
```

## Step-by-step implementation

### D1 — Working snapshot

1. **Add module skeleton.** Create `working/{mod.rs, identity.rs, snapshot.rs}`; add `pub mod working;` to `lib.rs`. `todo!()` bodies. **VERIFY**: `cargo check -p rmc-graph`.
2. **Implement `working_dir`.** `paths.root_dir.join("working").join(session.to_hex())`. **VERIFY**: `working_dir_is_under_workspace_hash`.
3. **Implement `init_from_published`.** `fs::create_dir_all(working_dir)`; open source env read-only at `paths.snapshot_dir(base_graph_id)`; `source_env.copy_to_path(working_dir.join("data.mdb"), CompactionOption::Disabled)`; open destination env; open `GraphDatabases::open`. Read `manifest.json` via `storage::read_manifest`; clone as `base_manifest`. Construct identity with fresh `SessionId::new()` and `edit_seq: 0`. **VERIFY**: `init_copies_full_state`.
4. **Implement `drop_session`.** Drop `Arc<Env>`, `fs::remove_dir_all` ignoring `NotFound`. **VERIFY**: `init_then_drop_leaves_no_residue`.
5. **Implement `publish_as_new_graph_id` (skeleton).** For M0 return `Err(anyhow!("publish not implemented in M0"))`. Signature compiles. **VERIFY**: `#[ignore]` test stub.

### D2 — Classifier + expander

6. **Add `affected` module skeleton.** **VERIFY**: `cargo check`.
7. **Implement `classify`.** Trivial `match`. **VERIFY**: `classifier_maps_every_variant`.
8. **Implement `ReverseDepGraph::from_crate_edges`.** Iterate, insert `(producer, consumer)`. **VERIFY**: synthetic A→B, A→C, B→C round-trips `consumers_of`.
9. **Implement `expand`.** Per class:
   - `BodyOnly` → `dirty_files = {edit.file}`, `dirty_crates = {crate_of_file(file)}`, no reverse-deps.
   - `SigOrVis`, `ItemAddRemove` → plus transitive reverse-dep closure via BFS.
   - `ModuleTree` → similar, with `affected_files` from `Edit::ModuleTree`.
   - `Macro` → dirty crate plus transitive consumers.
   - `Cargo` → `full_rebuild = true`.
   **VERIFY**: per-class table-driven test.
10. **Document `crate_of_file` injection.** Closure provided by working snapshot layer using `file → crate_id` map derived from `Node` records.

### D3 — Matrix in code

11. **Add `SubDb` enum + compile-time guard.**
    ```rust
    #[cfg(test)]
    fn _matches_storage_layout() {
        let _: fn(crate::graph::storage::GraphDatabases) = |dbs| {
            let crate::graph::storage::GraphDatabases {
                meta_by_key: _, nodes_by_id: _, bindings_by_id: _,
                bindings_by_from_module: _, bindings_by_target: _,
                children_by_parent: _, usages_by_id: _, usages_by_target: _,
                usages_by_consumer: _, usages_by_consumer_function: _,
                signatures_by_target: _, static_metadata_by_target: _,
                embeddings_by_target: _,
            } = dbs;
        };
    }
    ```
12. **Implement `invalidations_for`.** Write the `match` per the prose D3 table.
13. **Cite source.** Top-of-file doc-comment links to `.plans/phase-1-implementation.md`.

### D4 — Checkpoint + undo log

14. **Add `checkpoint` module skeleton.** **VERIFY**: `cargo check`.
15. **Implement `UndoLog::open`.** `OpenOptions::read+write+create+append`; track existing length + count. **VERIFY**: `open_then_reopen_preserves_count`.
16. **Implement `record`.** bincode-serialize entry, `u32` LE length prefix + bytes, increment count. Flush on `mark` and `drop`. **VERIFY**: round-trip.
17. **Implement `mark`.** Flush, return `{ byte_offset: file.stream_position(), entry_count }`. **VERIFY**: trivial.
18. **Implement `restore`.** Flush; seek to EOF; walk backwards reading length prefixes; per entry dispatch on `sub_db` (primary tables → put/delete; DUP_SORT → use `delete_one_duplicate` or `put`; `MetaByKey` → str-byte key; `Manifest` → bincoded `GraphManifest`, rewrite `manifest.json` outside txn). Truncate log to marker; reset `entry_count`. **VERIFY**: `record_then_restore_to_zero_recovers_pre_state`.
19. **Implement `take_checkpoint`.** Returns `Checkpoint { jj_op_id, undo_log_marker: undo.mark()?, ra_edit_seq, caches: () }`.
20. **Implement `restore` orchestrator.** Open `wtxn`; `undo.restore`; commit. Then `Command::new("jj").args(["op","restore",&checkpoint.jj_op_id.0]).status()` (gated `#[cfg(feature = "jj")]` for M0). Then `ra_host.rewind_to`. Any Err → caller drops working snapshot. **VERIFY**: `restore_round_trips_5_patches`.
21. **Wire `bump_edit_seq` on `WorkingSnapshot`** for the patcher (P0.2 calls it).

### Spike 1 — RA fan-out

22. **Create `rmc-spikes` crate.** `publish = false`, two `[[bin]]` entries. Add to workspace `members`. **VERIFY**: `cargo check -p rmc-spikes`.
23. **`WorkspaceFixture` helper.** `from_env()` reads `RMC_SPIKE_WORKSPACE` (default = rmc workspace itself for smoke).
24. **`EditScenario` enum.** Mirror D2 classes plus textual patches:
    - `BodyOnly`: replace leaf-fn body with `return Default::default();`.
    - `SigOrVis`: `pub fn` → `pub(crate) fn`.
    - `ItemAdd`: append `pub fn __spike_added() {}`.
    - `ModuleTree`: insert `pub mod __spike_module;` + a 1-file module.
    - `Macro`: change a `macro_rules!` body.
    - `Cargo`: bump a patch version.
    Record `(file, original_bytes)` so spike can revert.
25. **`ra_fanout.rs` binary.**
    ```rust
    fn main() -> Result<()> {
        let fx = WorkspaceFixture::from_env()?;
        let mut report = Report::default();
        report.loc = fx.loc();
        let t0 = Instant::now();
        let mut loaded = rmc_graph::graph::loader::load(&fx.root)?;
        report.cold_load_ms = t0.elapsed().as_millis() as u64;
        let t1 = Instant::now();
        let _model = rmc_graph::graph::extract::extract(&loaded);
        report.cold_extract_ms = t1.elapsed().as_millis() as u64;
        for scenario in EditScenario::menu() {
            scenario.apply_to_disk()?;
            let t = Instant::now();
            loaded.vfs.set_file_contents(
                scenario.vfs_id(&loaded.vfs)?, Some(scenario.new_bytes()),
            );
            let _ = rmc_graph::graph::extract::extract(&loaded);
            report.per_class.insert(scenario.class(), t.elapsed().as_millis() as u64);
            scenario.revert_on_disk()?;
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        assert_pass(&report);  // body-only < 500ms
        Ok(())
    }
    ```
    Pass: `report.per_class[BodyOnly] < 500`. Fail exits with code 2.
26. **Document Spike 1's caveats.** This measures whole-workspace re-extract (upper bound). If it passes, the optimised P0.2 path is guaranteed to pass.

### Spike 2 — Cargo gate latency

27. **`cargo_latency.rs`.**
    ```rust
    fn main() -> Result<()> {
        let pool_dir = std::env::var("RMC_SPIKE_POOL")?;
        let mut report = PoolReport::default();
        for crate_dir in list_crates(&pool_dir)? {
            run_cargo(&crate_dir, &["check", "--message-format=json"])?;  // warm cache
            let scoped_test = pick_one_test_target(&crate_dir)?;
            let mut warm = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                run_cargo(&crate_dir, &["check"])?;
                warm.push(t.elapsed().as_millis());
            }
            let mut test_warm = Vec::new();
            for _ in 0..3 {
                let t = Instant::now();
                run_cargo(&crate_dir, &["test", "--no-fail-fast", "--", "--quiet", &scoped_test])?;
                test_warm.push(t.elapsed().as_millis());
            }
            report.crates.push(CrateReport {
                name: crate_name(&crate_dir),
                warm_check_p50_ms: median(&warm),
                warm_test_p50_ms:  median(&test_warm),
            });
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        assert!(report.crates.iter().all(|c| c.warm_check_p50_ms < 2000));
        Ok(())
    }
    ```
28. **Document Spike 2's scoping.** `cargo check` (cheap) and `cargo test` filtered to one test. If median > 2s, design switches to RA-based type-check (P1.7).

### Wrap-up

29. **M0 status section** for human paste.
30. **Gate sign-off:** both spike binaries exit zero → M0 green. Else revise M2 plan.

## Tests

### D1
- **`init_copies_full_state`** — build tiny published snapshot via `persist_test_model`; `init_from_published`; walk both envs and assert `nodes_by_id` keys identical.
- **`init_then_drop_leaves_no_residue`** — after `drop_session`, working dir gone, `snapshot_dir(base_graph_id)` untouched.
- **`identity_edit_seq_starts_at_zero`** — invariant; bumped only by `bump_edit_seq`.

### D2 classifier
- **`classifier_maps_every_variant`** — one row per `Edit` variant.
- **`body_only_edit_classifies`** — `Edit::ModifyBody { ... }` → `EditClass::BodyOnly`.
- **`cargo_toml_edit_classifies_as_full_rebuild_class`** — `Edit::Cargo { ... }` → `Cargo` → `expand` sets `full_rebuild = true`.

### D2 expander
- **`expand_body_only_has_empty_reverse_deps`** — A→B→C, edit fn in C, `reverse_dep_crates` empty.
- **`expand_sig_or_vis_propagates_to_reverse_deps`** — edit `pub fn` in A, transitive closure ⊇ {B, C}.
- **`expand_macro_includes_all_consumers`** — macro edit in A; reverse-dep closure.
- **`expand_cargo_sets_full_rebuild`**.

### D3 matrix
- **`every_class_covers_every_sub_db`** — iterate `EditClass::ALL` × `SubDb::ALL`, assert each pair has a rule.
- **`body_only_does_not_touch_nodes`** — `BodyOnly` contains `Rule { table: NodesById, action: Unchanged }`.
- **`cargo_is_full_rebuild_except_caches`**.
- **`matrix_matches_design_doc_prose`** — golden table assertion mirroring the doc grid.
- **`_matches_storage_layout`** — compile-time guard from Step 11.

### D4
- **`open_then_reopen_preserves_count`**.
- **`record_then_restore_to_zero_recovers_pre_state`** — synthetic env, 5 mutations recorded, restore to pre-marker → byte-equal.
- **`restore_handles_dup_sort`** — `BindingsByTarget` (DUP_SORT) record/delete/restore.
- **`restore_truncates_log_file`** — post-restore file size == marker.
- **`take_then_restore_round_trips`** — 5 patches via stub patcher → restore → byte-equal.
- **`restore_failure_signals_drop_required`** — mock `RaHostHandle::rewind_to` returns `Err`.

### Spikes
- **`ra_fanout_runs_on_rmc_workspace`** (`#[ignore]`) — exits 0.
- **`cargo_latency_runs_on_one_crate`** (`#[ignore]`) — exits 0.

## Open decisions / risks

- **Source of reverse-dep graph.** `OpenedSnapshot::crate_edges()` already exists. M0 ships only `ToCrateEdge` trait + adapter; **the `ReverseDepGraph::from_opened_snapshot` constructor lands in P0.2**.
- **Macro-vs-ItemAdd detection.** No `diff_to_edit_class` function — caller picks the `Edit` variant. Construction-time choice.
- **LMDB copy cost.** With `DEFAULT_MAP_SIZE = 1 GiB`, worst-case `mdb_copy` ≈ on-disk size. Rmc workspace ~50 MB → < 200 ms on SSD. 50 steps/episode → < 5 ms amortized per step.
- **Undo log size growth.** Per `UndoEntry`: ~300B × ~10 entries for body-only = a few KB; ModuleTree may reach hundreds of KB. 50 steps × ~1 MB worst-case = 50 MB; acceptable.
- **`Manifest` table is special.** Lives on disk, not LMDB. Restore rewrites `manifest.json` after committing LMDB txn — atomicity vs LMDB is weaker. Mitigation: patcher recomputes counts from `meta_by_key` on next open.
- **`DescriptionsByTarget` doesn't exist yet.** D3 reserves the row; P1.2 adds the sub-DB and matrix row is already correct.
- **`StaticMetadataByTarget` for `BodyOnly`.** Rule is `ReDerive`; patcher elides via precondition check.
- **Spike 1 over-estimates** — passes guarantee P0.2 will too.
- **Spike 2 depends on P0.4 pool** — runs against rmc alone if pool absent (smoke).
- **No `jj` crate dep in M0.** Restore shells out to `jj op restore`.
- **Determinism.** Both affected-set and matrix are pure functions; `ReverseDepGraph::from_crate_edges` iterates BTreeMap order.


---

# Section C — P0.2 Warm-Host Incremental Writer + P0.3 jj Rollback

## Overview

This slice is the **critical path** for the whole project: P0.2 (warm host + incremental writer) feeds P1.5 (CRUD) feeds P1.7 (reward) feeds P1.8 (episode runner). Every other phase can be built against the slow cold-rebuild path, but the episode loop cannot run faster than what the warm host delivers. The lethal item is `sub-500ms re-extract + LMDB patch on a body-only edit in a 100k-LOC workspace`; if that fails, RL is infeasible.

The architectural shift is from "build snapshot → discard `RootDatabase` → query LMDB" to "open snapshot once → keep `RootDatabase` warm in a `WorkspaceHost` → edits go through `set_file_text` → re-extract just the affected crates → diff against existing content-addressed LMDB keys → patch deltas under one write txn → log inverses to an undo log". The current query layer (`OpenedSnapshot`, `query/*`) is unchanged. The **working snapshot** (D1) is an `mdb_copy` of the published base, opened with `WithoutTls + write_txn` enabled, and lives under `working/<session_id>/`.

P0.3 wraps this in a **Checkpoint contract** spanning source (jj op id), graph (undo-log marker), and RA host (edit-seq). `rollback()` runs `jj op restore <op_id>`, replays inverse `set_file_text` calls on the warm RA database to mark, then replays the LMDB undo log in reverse. The fallback for divergence: drop the working snapshot, copy the base again, re-open the warm host (slow path; tracked but never the hot path).

## New modules / files

- `crates/rmc-graph/src/host/mod.rs` — `WorkspaceHost` + lifecycle. Exports `WorkspaceHost`, `FileEdit`, `EditSeq`, `EditClass`, `Checkpoint`, `HostError`.
- `crates/rmc-graph/src/host/edits.rs` — `FileEdit`, `EditClass`, `EditSeq`, `apply_edits`.
- `crates/rmc-graph/src/host/file_to_crate.rs` — bidirectional cache `PathBuf → SmallVec<NodeId>`, built once at host open by walking `Node.kind == Item` grouped by `Node.file → Node.crate_id`.
- `crates/rmc-graph/src/host/affected.rs` — D2 algorithm using `crate_edges` reversed.
- `crates/rmc-graph/src/host/re_extract.rs` — per-crate emit. Refactors `extract::extract` so the same helpers can be driven on a subset of `local_crates`. Output: `PartialExtractionModel`.
- `crates/rmc-graph/src/host/diff_patch.rs` — the LMDB diff-patch. Owns per-sub-DB delete/insert logic, DUP_SORT key/value pair semantics, `meta_by_key` counter updates, manifest counter write-back.
- `crates/rmc-graph/src/host/undo_log.rs` — `UndoLog`, `UndoOp` (one variant per primary sub-DB + each DUP_SORT secondary), `UndoMarker(u64)`.
- `crates/rmc-graph/src/host/rollback.rs` — `Checkpoint::take`, `Checkpoint::restore`, jj wrapper via `tokio::process::Command`.
- `crates/rmc-graph/src/host/working_snapshot.rs` — D1 machinery: `WorkingSnapshot::init_from_base` using heed `env.copy_to_path`.
- `crates/rmc-graph/benches/incremental_extract.rs` — criterion bench for the 5 edit classes.
- `crates/rmc-graph/src/host/tests/` — differential tests.

Refactored existing files:
- `crates/rmc-graph/src/graph/extract.rs`: split `extract` into `extract_full(loaded)` (current shape) and `extract_partial(loaded, crates)`. The per-crate `emit_crate` stays; callers move into `extract_partial`. `extract_bindings`, `extract_impl_items`, `extract_attributes`, `extract_signatures`, `extract_statics`, `extract_usages` take a `local_crates: &[Crate]` arg.
- `crates/rmc-graph/src/graph/snapshot.rs`: `write_model` factors a helper `apply_full_model(env, dbs, model)`. `binding_id_for` / `usage_id_for` become `pub(in crate::graph)`.
- `crates/rmc-graph/src/graph/mod.rs`: `pub mod host;`.

## Type definitions

```rust
// crates/rmc-graph/src/host/mod.rs

pub struct WorkspaceHost {
    analysis: AnalysisHost,          // ra_ap_ide::AnalysisHost; warm RootDatabase
    vfs: Vfs,
    workspace_root: PathBuf,
    local_crates: Vec<Crate>,
    working: WorkingSnapshot,
    env: Arc<Env<WithoutTls>>,
    dbs: GraphDatabases,
    edit_seq: EditSeq,
    undo: UndoLog,
    file_to_crate: HashMap<PathBuf, SmallVec<[NodeId; 2]>>,
    crate_id_to_handle: HashMap<NodeId, Crate>,
    locks: crate::host::Locks,
}

#[derive(Clone)]
pub struct Locks {
    pub workspace_locks: Arc<dyn WorkspaceLockHandle>,
}
```

```rust
// crates/rmc-graph/src/host/edits.rs

#[derive(Debug, Clone)]
pub struct FileEdit {
    pub path: PathBuf,           // workspace-relative
    pub new_text: String,
    pub edit_class: EditClass,   // set by CRUD layer, NOT inferred
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditClass {
    Body, Signature, ItemAddRemove, ModuleTree, Macro, CargoManifest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct EditSeq(pub u64);

impl WorkspaceHost {
    pub fn apply_edits(&mut self, edits: &[FileEdit]) -> Result<EditSeq, HostError>;
}
```

```rust
// crates/rmc-graph/src/host/re_extract.rs

#[derive(Debug, Clone)]
pub struct PartialExtractionModel {
    pub dirty_crates: Vec<NodeId>,
    pub nodes: BTreeMap<NodeId, Node>,
    pub bindings: Vec<Binding>,
    pub usages: Vec<Usage>,
    pub contains: Vec<(NodeId, NodeId)>,
    pub signatures: Vec<(NodeId, FunctionSignature)>,
    pub statics: Vec<(NodeId, StaticMetadata)>,
}

pub(crate) fn extract_partial(loaded: &LoadedWorkspace, crates: &[Crate]) -> PartialExtractionModel;
```

```rust
// crates/rmc-graph/src/host/diff_patch.rs

#[derive(Debug, Default)]
pub struct DiffPatch {
    pub node_inserts: Vec<Node>,
    pub node_updates: Vec<Node>,          // same key, different bincode
    pub node_removes: Vec<NodeId>,
    pub binding_inserts: Vec<(BindingId, Binding)>,
    pub binding_removes: Vec<BindingId>,
    pub usage_inserts: Vec<(UsageId, Usage)>,
    pub usage_removes: Vec<UsageId>,
    pub contains_inserts: Vec<(NodeId, NodeId)>,
    pub contains_removes: Vec<(NodeId, NodeId)>,
    pub signature_inserts: Vec<(NodeId, FunctionSignature)>,
    pub signature_removes: Vec<NodeId>,
    pub static_inserts: Vec<(NodeId, StaticMetadata)>,
    pub static_removes: Vec<NodeId>,
}

impl WorkspaceHost {
    pub(crate) fn compute_patch(&self, partial: &PartialExtractionModel) -> Result<DiffPatch>;
    pub(crate) fn apply_patch(&mut self, patch: DiffPatch, next_seq: EditSeq) -> Result<()>;
}
```

```rust
// crates/rmc-graph/src/host/undo_log.rs

#[derive(Debug, Clone, Copy)] pub struct UndoMarker(pub EditSeq);

#[derive(Debug, Clone)]
pub enum UndoOp {
    NodeUpsert { key: [u8;32], prior: Option<Node> },
    NodeRemove { key: [u8;32], prior: Node },
    BindingUpsert { key: [u8;32], prior: Option<Binding> },
    BindingRemove { key: [u8;32], prior: Binding },
    UsageUpsert { key: [u8;32], prior: Option<Usage> },
    UsageRemove { key: [u8;32], prior: Usage },
    SignatureUpsert { key: [u8;32], prior: Option<FunctionSignature> },
    SignatureRemove { key: [u8;32], prior: FunctionSignature },
    StaticUpsert { key: [u8;32], prior: Option<StaticMetadata> },
    StaticRemove { key: [u8;32], prior: StaticMetadata },
    BindingByFromModuleInsert { key: [u8;32], value: [u8;32] },
    BindingByFromModuleDelete { key: [u8;32], value: [u8;32] },
    BindingByTargetInsert { key: [u8;32], value: [u8;32] },
    BindingByTargetDelete { key: [u8;32], value: [u8;32] },
    ChildrenByParentInsert { key: [u8;32], value: [u8;32] },
    ChildrenByParentDelete { key: [u8;32], value: [u8;32] },
    UsagesByTargetInsert { key: [u8;32], value: [u8;32] },
    UsagesByTargetDelete { key: [u8;32], value: [u8;32] },
    UsagesByConsumerInsert { key: [u8;32], value: [u8;32] },
    UsagesByConsumerDelete { key: [u8;32], value: [u8;32] },
    UsagesByConsumerFunctionInsert { key: [u8;32], value: [u8;32] },
    UsagesByConsumerFunctionDelete { key: [u8;32], value: [u8;32] },
    MetaCounter { name: &'static str, prior_le_bytes: [u8;8] },
}

#[derive(Debug, Default)] pub struct UndoBatch { pub seq: EditSeq, pub ops: Vec<UndoOp> }
#[derive(Debug, Default)] pub struct UndoLog { pub batches: Vec<UndoBatch> }
```

```rust
// crates/rmc-graph/src/host/rollback.rs

#[derive(Debug, Clone)]
pub struct Checkpoint {
    pub jj_op_id: String,
    pub file_prior_text: HashMap<PathBuf, String>,
    pub edit_seq_marker: EditSeq,
}

impl Checkpoint { pub fn take(host: &WorkspaceHost) -> Self; }

impl WorkspaceHost {
    pub async fn rollback(&mut self, ck: &Checkpoint) -> Result<(), HostError>;
}
```

## Step-by-step implementation

### Step 1 — Refactor `extract.rs` to expose per-crate emission

WHERE: `crates/rmc-graph/src/graph/extract.rs`. DEPENDS: nothing.

(a) Split into `extract_full` (calls `extract_partial(loaded, &loaded.local_crates)`) and `extract_partial(loaded, crates)`. (b) `extract_partial_inner` rebuilds the three local maps (`crate_node_for`, `crate_name_for`, `module_node_for`) only for `crates`; every for-loop iterating `loaded.local_crates` now takes `crates` and short-circuits when `crate_for_def_id` is not in the dirty set. (c) `extract_bindings`, `extract_impl_items`, `extract_attributes`, `extract_signatures`, `extract_statics`, `extract_usages` take a `local_crates: &[Crate]` parameter. (d) `extract_usages` only emits references whose originating module is in a dirty crate. (e) Re-export `pub use self::extract::{extract_full, extract_partial};`.

VERIFY: existing extract tests pass; new `extract_partial_matches_full_on_subset` test asserts partial nodes are a subset of full nodes with identical bincode bytes for shared keys.

### Step 2 — Working snapshot + `WorkspaceHost::open_from_published`

WHERE: `crates/rmc-graph/src/host/working_snapshot.rs`, `host/mod.rs`. DEPENDS: Step 1.

(a) `WorkingSnapshot::init_from_base(base: &OpenedSnapshot, session_id: &str)`:
```rust
let working_dir = working_root().join(session_id);
fs::create_dir_all(&working_dir)?;
base.env.copy_to_path(working_dir.join("data.mdb"), CompactionOption::Disabled)?;
```

(b) `WorkspaceHost::open_from_published(workspace, base_graph_id, session_id)`:
1. `let base = open_specific(&paths, base_graph_id, GraphEnvOptions::default())?.context(...)?;`
2. `let working = WorkingSnapshot::init_from_base(&base, session_id)?;`
3. `let env = Arc::new(unsafe { GraphEnvOptions::default().to_open_options().open(&working.dir)? });`
4. `let dbs = { let rtxn = env.read_txn()?; GraphDatabases::open(&env, &rtxn)?.context(...)? };`
5. `let loaded = loader::load(workspace)?;` (slow path, once per host open).
6. `let mut analysis = AnalysisHost::with_database(loaded.db);` Keep `loaded.vfs`, `loaded.local_crates`, `loaded.workspace_root`.
7. Build `file_to_crate` by scanning `dbs.nodes_by_id` for `kind == Item`, grouping `Node.file → Node.crate_id`.
8. Build `crate_id_to_handle`: walk `local_crates`; compute NodeId via `NodeId::from_components(&[workspace_hash, "crate", crate_display_name(db, krate)])`; cross-reference with `nodes_by_id`.

VERIFY: `open_from_published_round_trips_node_count`.

### Step 3 — RA edit ingestion in `apply_edits`

WHERE: `crates/rmc-graph/src/host/edits.rs`. DEPENDS: Step 2.

```rust
let _guard = self.locks.workspace_locks.lock_exclusive(&self.workspace_root).await;
let next_seq = EditSeq(self.edit_seq.0 + 1);

for edit in edits {
    let abs = self.workspace_root.join(&edit.path);
    let vfs_path = ra_ap_vfs::VfsPath::new_real_path(abs.to_string_lossy().into_owned());
    let Some(file_id) = self.vfs.file_id(&vfs_path) else {
        return Err(HostError::UnknownFile(edit.path.clone()));
    };
    let prior = self.analysis.raw_database().file_text(file_id).to_string();
    self.recent_file_prior_text.entry(edit.path.clone()).or_insert(prior);
}

let mut change = ra_ap_ide::Change::new();
for edit in edits {
    let abs = self.workspace_root.join(&edit.path);
    let vfs_path = ra_ap_vfs::VfsPath::new_real_path(abs.to_string_lossy().into_owned());
    let file_id = self.vfs.file_id(&vfs_path).expect("checked");
    change.change_file(file_id, Some(std::sync::Arc::from(edit.new_text.as_str())));
}
self.analysis.apply_change(change);
```

VERIFY: `apply_edits_invalidates_salsa`.

### Step 4 — Dirty-file → crate map + D2 affected-set

WHERE: `crates/rmc-graph/src/host/affected.rs`. DEPENDS: 1–3.

```rust
pub(crate) fn affected_crates(host: &WorkspaceHost, edits: &[FileEdit]) -> Vec<NodeId> {
    let mut dirty_directly: HashSet<NodeId> = HashSet::new();
    for edit in edits {
        let crates = host.file_to_crate.get(&edit.path).cloned().unwrap_or_default();
        if crates.is_empty() {
            dirty_directly.extend(host.fallback_crates_for_path(&edit.path));
        } else {
            dirty_directly.extend(crates);
        }
    }
    let class = edits.iter().map(|e| e.edit_class).max_by_key(class_severity).unwrap_or(EditClass::Body);
    match class {
        EditClass::Body => dirty_directly.into_iter().collect(),
        EditClass::Signature | EditClass::ItemAddRemove | EditClass::ModuleTree => {
            let reverse = build_reverse_dep_index(host);   // memoised on host
            let mut closure = dirty_directly.clone();
            let mut queue: Vec<_> = dirty_directly.into_iter().collect();
            while let Some(c) = queue.pop() {
                if let Some(rdeps) = reverse.get(&c) {
                    for &r in rdeps {
                        if closure.insert(r) { queue.push(r); }
                    }
                }
            }
            closure.into_iter().collect()
        }
        EditClass::Macro => full_workspace_crates(host),
        EditClass::CargoManifest => return Err(HostError::ColdRebuildRequired),
    }
}
```

`build_reverse_dep_index` reuses `OpenedSnapshot::crate_edges` once at host open; cached as `HashMap<NodeId, Vec<NodeId>>`. Module-tree edits invalidate the cache.

VERIFY: `body_edit_does_not_expand_reverse_deps`, `sig_edit_does_expand`.

### Step 5 — Scoped re-extract

WHERE: `crates/rmc-graph/src/host/re_extract.rs`. DEPENDS: 1, 4.

```rust
pub(crate) fn re_extract(host: &WorkspaceHost, dirty: &[NodeId]) -> Result<PartialExtractionModel> {
    let dirty_crates: Vec<Crate> = dirty.iter()
        .filter_map(|nid| host.crate_id_to_handle.get(nid).copied())
        .collect();
    let loaded_view = LoadedWorkspaceRef {
        workspace_root: &host.workspace_root,
        db: host.analysis.raw_database(),
        vfs: &host.vfs,
        local_crates: &dirty_crates,
        crate_target_kinds_by_name: &host.crate_target_kinds_by_name,
        crate_target_kinds_by_root_file: &host.crate_target_kinds_by_root_file,
    };
    Ok(extract::extract_partial(&loaded_view.to_loaded(), &dirty_crates))
}
```

`LoadedWorkspaceRef` is a borrowed mirror so we don't clone `RootDatabase`. If borrowck is painful, generalize `extract_partial` to a `LoadedAccess` trait.

VERIFY: `partial_extract_after_body_edit_matches_cold_subset`.

### Step 6 — Compute `DiffPatch`

WHERE: `crates/rmc-graph/src/host/diff_patch.rs`. DEPENDS: 5.

```rust
let rtxn = self.env.read_txn()?;
// 6a. Existing primary records for the dirty crates only.
let existing_nodes: HashMap<NodeId, Node> = self.dbs.nodes_by_id
    .iter(&rtxn)?
    .filter_map(|r| r.ok())
    .filter(|(_, n)| n.crate_id.map_or(false, |c| partial.dirty_crates.contains(&c)))
    .map(|(k, n)| (NodeId::from_bytes_arr(k.try_into().unwrap()), n))
    .collect();

// 6b. Set difference for nodes.
let new_node_ids: HashSet<NodeId> = partial.nodes.keys().copied().collect();
let old_node_ids: HashSet<NodeId> = existing_nodes.keys().copied().collect();
for &id in new_node_ids.difference(&old_node_ids) { patch.node_inserts.push(partial.nodes[&id].clone()); }
for &id in old_node_ids.difference(&new_node_ids) { patch.node_removes.push(id); }
for &id in new_node_ids.intersection(&old_node_ids) {
    if bincode::serialize(&partial.nodes[&id])? != bincode::serialize(&existing_nodes[&id])? {
        patch.node_updates.push(partial.nodes[&id].clone());
    }
}
```

Bindings + usages: same set-difference on `BindingId`/`UsageId`. Existing IDs read via the right secondary for dirty crates (`bindings_by_from_module.iter_dup_of(&rtxn, mod_id.as_bytes())`, `usages_by_consumer.prefix(parent_module)`). Cross-crate usages from a clean crate to a dirty crate remain valid in LMDB because content-addressed IDs don't change. `contains` / `signatures` / `statics`: per-dirty-NodeId diff.

VERIFY: `diff_patch_is_empty_on_no_change`.

### Step 7 — Apply patch under write txn, record undo

WHERE: `crates/rmc-graph/src/host/diff_patch.rs::apply_patch`. DEPENDS: 6.

Strict ordering: deletes (secondaries first, then primary) → updates → inserts (primary first, then secondaries).

```rust
let mut wtxn = self.env.write_txn()?;
let mut batch = UndoBatch { seq: next_seq, ops: Vec::with_capacity(patch.size_hint()) };

for bid in &patch.binding_removes {
    let prior = self.dbs.bindings_by_id.get(&wtxn, bid.as_bytes())?.expect("removing nonexistent");
    self.dbs.bindings_by_from_module.delete_one_duplicate(
        &mut wtxn, prior.from_module.as_bytes(), bid.as_bytes(),
    )?;
    batch.ops.push(UndoOp::BindingByFromModuleInsert {
        key: *prior.from_module.as_bytes(), value: *bid.as_bytes(),
    });
    self.dbs.bindings_by_target.delete_one_duplicate(
        &mut wtxn, prior.target.as_bytes(), bid.as_bytes(),
    )?;
    batch.ops.push(UndoOp::BindingByTargetInsert {
        key: *prior.target.as_bytes(), value: *bid.as_bytes(),
    });
    self.dbs.bindings_by_id.delete(&mut wtxn, bid.as_bytes())?;
    batch.ops.push(UndoOp::BindingRemove { key: *bid.as_bytes(), prior });
}
// ... mirror for usages (three secondaries), children_by_parent, node_removes.

for node in &patch.node_updates {
    let prior = self.dbs.nodes_by_id.get(&wtxn, node.id.as_bytes())?;
    self.dbs.nodes_by_id.put(&mut wtxn, node.id.as_bytes(), node)?;
    batch.ops.push(UndoOp::NodeUpsert { key: *node.id.as_bytes(), prior });
}

for node in &patch.node_inserts {
    self.dbs.nodes_by_id.put(&mut wtxn, node.id.as_bytes(), node)?;
    batch.ops.push(UndoOp::NodeUpsert { key: *node.id.as_bytes(), prior: None });
}
for (bid, binding) in &patch.binding_inserts {
    self.dbs.bindings_by_id.put(&mut wtxn, bid.as_bytes(), binding)?;
    batch.ops.push(UndoOp::BindingUpsert { key: *bid.as_bytes(), prior: None });
    self.dbs.bindings_by_from_module.put(&mut wtxn, binding.from_module.as_bytes(), bid.as_bytes())?;
    batch.ops.push(UndoOp::BindingByFromModuleDelete {
        key: *binding.from_module.as_bytes(), value: *bid.as_bytes(),
    });
    // ... bindings_by_target ...
}
```

**Critical:** `delete_one_duplicate` is the heed 0.22 helper that positions the cursor on the (key, value) pair. `Database::delete` on a DUP_SORT db removes *every* dup for that key — wrong here. Highest-risk correctness item; covered by `dup_sort_secondary_delete` test.

### Step 8 — Counter / manifest updates

```rust
let dn = patch.node_inserts.len() as i64 - patch.node_removes.len() as i64;
let db = patch.binding_inserts.len() as i64 - patch.binding_removes.len() as i64;
let du = patch.usage_inserts.len() as i64 - patch.usage_removes.len() as i64;

for (name, delta) in [("node_count", dn), ("binding_count", db), ("usage_count", du)] {
    let prior_bytes: [u8; 8] = self.dbs.meta_by_key.get(&wtxn, name)?
        .map(|b| b.try_into().unwrap()).unwrap_or([0; 8]);
    let prior = i64::from_le_bytes(prior_bytes);
    let new = (prior + delta).max(0) as u64;
    self.dbs.meta_by_key.put(&mut wtxn, name, &new.to_le_bytes())?;
    batch.ops.push(UndoOp::MetaCounter { name, prior_le_bytes: prior_bytes });
}

wtxn.commit()?;
self.undo.batches.push(batch);
self.edit_seq = next_seq;
```

On-disk `manifest.json` rewritten too; atomic via temp + `fs::rename`.

VERIFY: `meta_counters_match_inserts_minus_removes`.

### Step 9 — Host trusts caller's EditClass

WHERE: `crates/rmc-graph/src/host/edits.rs`. Host does NOT parse textual diff. Caller (P1.5) constructs `FileEdit { edit_class }` from its verb dispatch.

### Step 10 — P0.3 jj wrapper

WHERE: `crates/rmc-graph/src/host/rollback.rs`.

```rust
async fn jj_op_log_head(workspace_root: &Path) -> Result<String> {
    let out = Command::new("jj").current_dir(workspace_root)
        .args(["op", "log", "--no-graph", "-n", "1", "--template", r#"self.id().short() ++ "\n""#])
        .output().await?;
    if !out.status.success() { return Err(HostError::Jj(...)); }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn jj_op_restore(workspace_root: &Path, op_id: &str) -> Result<()> {
    let out = Command::new("jj").current_dir(workspace_root)
        .args(["op", "restore", op_id]).output().await?;
    if !out.status.success() { return Err(HostError::Jj(...)); }
    Ok(())
}
```

`Checkpoint::take(host)` captures `(jj_op_log_head(), edit_seq, drain recent_file_prior_text)`.

VERIFY: `jj_op_id_captured_on_take`.

### Step 11 — Host restore path

WHERE: `crates/rmc-graph/src/host/rollback.rs::WorkspaceHost::rollback`. DEPENDS: 7, 10.

```rust
pub async fn rollback(&mut self, ck: &Checkpoint) -> Result<()> {
    let _guard = self.locks.workspace_locks.lock_exclusive(&self.workspace_root).await;
    // 1. Source.
    jj_op_restore(&self.workspace_root, &ck.jj_op_id).await?;
    // 2. RA host: replay inverse set_file_text.
    let mut change = ra_ap_ide::Change::new();
    for (path, prior_text) in &ck.file_prior_text {
        let vfs_path = ra_ap_vfs::VfsPath::new_real_path(
            self.workspace_root.join(path).to_string_lossy().into_owned()
        );
        let Some(file_id) = self.vfs.file_id(&vfs_path) else { continue };
        change.change_file(file_id, Some(std::sync::Arc::from(prior_text.as_str())));
    }
    self.analysis.apply_change(change);
    // 3. LMDB: pop undo batches > marker, replay inverses.
    let mut wtxn = self.env.write_txn()?;
    while let Some(top) = self.undo.batches.last() {
        if top.seq <= ck.edit_seq_marker { break; }
        let batch = self.undo.batches.pop().unwrap();
        for op in batch.ops.iter().rev() {
            self.apply_undo_op(&mut wtxn, op)?;
        }
    }
    wtxn.commit()?;
    self.edit_seq = ck.edit_seq_marker;
    // 4. Divergence guard.
    if self.is_diverged_from_expected(ck)? {
        tracing::warn!("undo replay diverged; falling back to mdb_copy from base");
        self.reopen_from_base(ck)?;
    }
    Ok(())
}
```

`apply_undo_op` matches `UndoOp` (NodeUpsert {prior: None} → delete; {prior: Some(n)} → put; DUP_SORT inserts → put pair; etc.).

`reopen_from_base` is the slow bail-out.

### Step 12 — Bench harness

WHERE: `crates/rmc-graph/benches/incremental_extract.rs`.

```rust
fn bench_body_only(c: &mut Criterion) {
    let workspace = corpus::large_100k_loc();
    let base = build_and_persist(&workspace, BuildOptions::default()).unwrap();
    let mut host = WorkspaceHost::open_from_published(&workspace, &base.graph_id, "bench-session").unwrap();
    let target_file = corpus::pick_body_target(&workspace);
    let original = std::fs::read_to_string(&target_file).unwrap();
    c.bench_function("body_only_edit", |b| {
        let mut alt = 0;
        b.iter(|| {
            let text = if alt % 2 == 0 { mutate_body(&original, alt) } else { original.clone() };
            host.apply_edits(&[FileEdit { path: target_file.clone(), new_text: text, edit_class: EditClass::Body }]).unwrap();
            alt += 1;
        });
    });
}
```

Classes: `body_only_edit` (< 500ms p95), `sig_edit_reverse_deps_5` (< 2s p95), `item_add_remove` (< 1s p95), `module_tree` (< 2s p95). Output: JSON per bench `{name, p50_ms, p95_ms, p99_ms, max_ms, dirty_crate_count, patch_size}`.

## Tests

- **`roundtrip_body_only`** (`tests/host_body_roundtrip.rs`). 5-crate, ~3k LOC fixture. Cold-build → snapshot `cold_pre`. Apply body edit via host; cold-rebuild → `cold_post`. For dirty crate: working LMDB == `cold_post` on every persisted record. Non-dirty == `cold_pre`.
- **`roundtrip_sig_change`** (`tests/host_sig_roundtrip.rs`). Same shape, sig change in a leaf crate with 2 consumers; affected set = 3 crates; LMDB == cold for all three.
- **`undo_replay_equiv`** (`tests/host_undo.rs`). Apply 3 edits; `Env::copy_to_path` to side dir. `Checkpoint::take` before; `rollback(ck)`. Re-snapshot; walk every sub-DB pair and assert byte equality including DUP_SORT iteration order.
- **`concurrent_rollouts`** (`tests/host_concurrent.rs`). Two `WorkspaceHost`s over disjoint working snapshots, both initialised from same base. 10 edits each from two tokio tasks. Neither sees the other's mutations; published base manifest unchanged.
- **`dup_sort_secondary_delete`** (`tests/host_dup_sort.rs`). Two distinct bindings sharing `from_module` (DUP_SORT same key). Remove only one. `bindings_by_from_module.iter_dup_of(...)` returns exactly 1 entry after (not 0, not 2).
- **`affected_set_reverse_deps`** (`tests/host_affected.rs`). A depends on B. `EditClass::Body` in B → affected = {B}. `EditClass::Signature` → {A, B}.
- **`checkpoint_restore_source`** (`tests/host_jj.rs`). `jj init`; write file; describe; take checkpoint; edit + describe; rollback → file reverted, `jj log -r @` shows old description.
- **`bench_incremental_extract`** — Step 12.

## Open decisions / risks

- **RA salsa fan-out (#1 lethal).** Body edit in `core` may invalidate types in 100 reverse-deps. D2 says "Body class → editing crate only"; salsa recomputes lazily wherever a query touches stale memo. Mitigation: the differential test (`roundtrip_body_only`) — if cold-rebuild diverges from warm-host for a non-dirty crate, fan-out leaked. Deeper mitigation: don't query non-dirty crates during re-extract (partial extractor passes only dirty `Crate` handles).
- **Memory.** Warm `RootDatabase` + `Vfs` ≈ 500MB-1GB for 100k LOC. N concurrent rollouts × ~750MB. Mitigation: episode pool with bounded concurrency (start at 2), reuse hosts across episodes (rollback to base instead of dropping).
- **DUP_SORT delete fiddliness.** heed 0.22 exposes `Database::delete_one_duplicate(&mut wtxn, &key, &value)` — the only safe call. `Database::delete(&mut wtxn, &key)` removes *all* dups → corruption. The `dup_sort_secondary_delete` test is the sentinel.
- **proc-macro / build.rs edits.** Per D2 they escalate to Full re-extract of every reverse-dep. Route to cold rebuild like CargoManifest until measurements show partial is worth the complexity.
- **Restore divergence detection.** Counter-check now (Step 11.4). Stronger check: Merkle root over `nodes_by_id` post-rollback compared to checkpoint root; gated by `debug_assertions`.
- **`AnalysisHost::apply_change` vs raw `set_file_text`.** Use `ra_ap_ide::Change::change_file(file_id, Option<Arc<str>>)` via `AnalysisHost::apply_change`. Both invalidate the same salsa input.
- **`crate_target_kinds_by_root_file` invalidation.** Cached at host open. ModuleTree edits do NOT invalidate. CargoManifest re-runs `load_crate_target_kinds` on cold-rebuild path.
- **File path canonicalisation.** `FileEdit.path` workspace-relative; `file_to_crate` keys workspace-relative; VFS paths absolute. Convert at edge in `apply_edits`.
- **`Vfs.file_id` returns None for newly-created files.** ModuleTree edits adding new `.rs` files need `vfs.set_file_contents(..., Some(bytes))` first. P1.5e concern.
- **`recent_file_prior_text` size.** Bounded by `Σ file_size for files edited since last Checkpoint::take`. 50 × 5 × ~10KB = ~500KB live.


---

# Section D — P1.1 Read View / Navigate

## Overview

P1.1 is the **observation half** of the agent loop: the apparatus the agent uses to *see* the workspace before it asks for a write. It sits in M1 alongside P1.2 and P1.3; all three run on the **slow, cold-built** `OpenedSnapshot` and have no dependency on the warm-host writer (P0.2). The read-side is purely a thin composition layer on top of `rmc_graph::graph::query/*` — `lookup_by_qualified_name`, `module_tree`, `who_calls`, `call_graph`, `imports_of`, `exports_of`, `re_export_chain`, `enum_variants`, `crate_edges`, `find_root_module_of`, `node_by_id`, `callees_of`, `referrers_of`, plus the snapshot-internal `span_index()` / `line_to_byte()` — wrapped in a stateful `Navigator` that knows about *scale*, *focus*, and *cost*.

The five verbs (`goto`, `zoom`, `show_body`, `show_callers`, `follow_trail`) compose into one canonical observation type (`ContextView`) and one canonical addressing type (`Location`). The body operator is the **inverse of skeleton**: instead of stripping bodies for cheap surface dumps, it materialises a body span on demand and adds its byte/4 token cost to the view. Cluster scale (P1.3 territory) gets a stub so `Scale::Cluster` and `Location::Cluster(ClusterId)` are wired today and refilled later without re-shaping callers.

## New modules / files

- `crates/rmc-graph/src/graph/view/mod.rs` — public surface: `Location`, `Scale`, `ContextView`, `Navigator`, `NavStep`, `ZoomDir`, `NeighborSlot`, `NeighborKind`, `CallSlice`, `BodySlice`, `MapPane`, `CratePin`, `ModulePin`, `ClusterPin`, `ClusterId`, `ViewError`.
- `crates/rmc-graph/src/graph/view/location.rs` — `Location` enum, `Scale`, `Location::scale()`, `Location::from_qualified(snap, &str)`, `Location::node_id()`, `ClusterId` newtype stub.
- `crates/rmc-graph/src/graph/view/context.rs` — `ContextView`, `MapPane`, `NeighborSlot`, `CallSlice`, `BodySlice`, per-scale assemblers.
- `crates/rmc-graph/src/graph/view/navigate.rs` — `Navigator`, 5 verbs, `NavStep`, `follow_trail`, `ViewError`.
- `crates/rmc-graph/src/graph/view/body.rs` — skeleton-inverse: given `Node` with `(file, span)`, slice file bytes via `OpenedSnapshot::line_to_byte`.
- `crates/rmc-graph/src/graph/view/cost.rs` — `TokenCost`, `estimate_*` helpers, `BUDGET_DEFAULT`.
- `crates/rmc-graph/src/graph/view/cluster_stub.rs` — `ClusterId`, `ClusterPin`, `placeholder_cluster_neighbors()`; P1.3 replaces.
- Optional later: `crates/rmc-server/src/tools/graph/navigate.rs` — MCP handlers `navigate_goto`, `navigate_zoom`, `navigate_show_body`, `navigate_show_callers`, `navigate_follow_trail`.

`graph/mod.rs` gains `pub mod view;` and re-exports `pub use view::{Location, Scale, ContextView, Navigator, NavStep, ZoomDir};`. Placing `view` inside `crate::graph` keeps `span_index` / `line_to_byte` accessible at `pub(crate)` (matches the `codemap` precedent).

## Type definitions

```rust
// view/location.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClusterId(pub [u8; 16]);  // P1.3 replaces

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Scale { Crate, Module, Cluster, Item, Body }

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Location {
    Workspace,
    Crate(NodeId),
    Module(NodeId),
    Cluster(ClusterId),
    Item(NodeId),
    Body { item: NodeId, file: String, span: (u32, u32) },
}

impl Location {
    pub fn scale(&self) -> Scale {
        match self {
            Location::Workspace => Scale::Crate,
            Location::Crate(_) => Scale::Crate,
            Location::Module(_) => Scale::Module,
            Location::Cluster(_) => Scale::Cluster,
            Location::Item(_) => Scale::Item,
            Location::Body { .. } => Scale::Body,
        }
    }
    pub fn node_id(&self) -> Option<NodeId> {
        match self {
            Location::Crate(id) | Location::Module(id) | Location::Item(id) => Some(*id),
            Location::Body { item, .. } => Some(*item),
            _ => None,
        }
    }
    pub fn from_qualified(snap: &OpenedSnapshot, q: &str) -> Result<Self, ViewError> {
        let (id, node) = snap.lookup_by_qualified_name(q)?
            .ok_or_else(|| ViewError::Unresolved(q.to_string()))?;
        Ok(match node.kind {
            NodeKind::Workspace => Location::Workspace,
            NodeKind::Crate => Location::Crate(id),
            NodeKind::Module => Location::Module(id),
            NodeKind::Item => Location::Item(id),
            NodeKind::ExternalSymbol => return Err(ViewError::ExternalSymbol(q.to_string())),
        })
    }
}
```

```rust
// view/context.rs

#[derive(Debug, Clone, Serialize)]
pub struct ContextView {
    pub focus: Location,
    pub scale: Scale,
    pub map_pane: MapPane,
    pub focal_node: Option<NodePin>,
    pub neighbors: Vec<NeighborSlot>,
    pub callgraph: Option<CallSlice>,
    pub exports: Vec<EnrichedBinding>,
    pub body: Option<BodySlice>,
    pub token_cost: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct NodePin {
    pub id: NodeId,
    pub qualified_name: String,
    pub display_name: String,
    pub kind: &'static str,
    pub item_kind: Option<String>,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    pub visibility: Option<String>,
    pub signature: Option<String>,
    pub attributes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MapPane {
    pub crates: Vec<CratePin>,
    pub modules: Vec<ModulePin>,
    pub clusters: Vec<ClusterPin>,
    pub current_path: Vec<NodeId>,
}

#[derive(Debug, Clone, Serialize)] pub struct CratePin { pub id: NodeId, pub name: String, pub efferent: u32, pub afferent: u32 }
#[derive(Debug, Clone, Serialize)] pub struct ModulePin { pub id: NodeId, pub qualified_name: String, pub display_name: String, pub depth: u8, pub child_count: u32 }
#[derive(Debug, Clone, Serialize)] pub struct ClusterPin { pub id: ClusterId, pub label: String }

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum NeighborKind { Sibling, Parent, Child, Import, Reexport, EnumVariant, Cluster }

#[derive(Debug, Clone, Serialize)]
pub struct NeighborSlot { pub label: String, pub loc: Location, pub kind: NeighborKind, pub item_kind: Option<String> }

#[derive(Debug, Clone, Serialize)]
pub struct CallSlice {
    pub callers: Vec<EnrichedCallSite>,
    pub callees: Vec<EnrichedCallSite>,
    pub callers_tree: Option<CallGraphNode>,
    pub callees_tree: Option<CallGraphNode>,
    pub depth: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct BodySlice { pub file: String, pub start: u32, pub end: u32, pub line_start: u32, pub line_end: u32, pub text: String }
```

```rust
// view/navigate.rs

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)] pub enum ZoomDir { In, Out }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NavStep {
    Goto(Location), ZoomIn, ZoomOut, ShowBody, ShowCallers(u32),
}

#[derive(Debug, thiserror::Error)]
pub enum ViewError {
    #[error("qualified name did not resolve: {0}")] Unresolved(String),
    #[error("qualified name is an external symbol: {0}")] ExternalSymbol(String),
    #[error("cannot zoom {dir:?} from {from:?}")] NoZoom { from: Scale, dir: ZoomDir },
    #[error("show_body requires Item or Body scale (got {0:?})")] BodyAtWrongScale(Scale),
    #[error("focus item has no body (file/span missing)")] BodyMissing,
    #[error("cluster scale is a stub until P1.3")] ClusterStub,
    #[error("view too large: {tokens} tokens > budget {budget}")] ViewTooLarge { tokens: usize, budget: usize },
    #[error(transparent)] Anyhow(#[from] anyhow::Error),
    #[error(transparent)] Io(#[from] std::io::Error),
}

pub struct Navigator<'a> {
    pub snap: &'a OpenedSnapshot,
    pub host: Option<&'a crate::graph::host::WorkspaceHost>,  // P0.2 placeholder
    pub budget: usize,
}

impl<'a> Navigator<'a> {
    pub fn new(snap: &'a OpenedSnapshot) -> Self { ... }
    pub fn with_budget(self, b: usize) -> Self { ... }
    pub fn goto(&self, loc: Location) -> Result<ContextView, ViewError>;
    pub fn zoom(&self, view: &ContextView, dir: ZoomDir) -> Result<ContextView, ViewError>;
    pub fn show_body(&self, view: &ContextView) -> Result<ContextView, ViewError>;
    pub fn show_callers(&self, view: &ContextView, depth: u32) -> Result<ContextView, ViewError>;
    pub fn follow_trail(&self, start: Location, steps: &[NavStep]) -> Result<ContextView, ViewError>;
}
```

## Step-by-step implementation

1. **`Location` + qualified-name parser.** WHERE: `view/location.rs`. Implement `Location::from_qualified` as a wrapper over `OpenedSnapshot::lookup_by_qualified_name`; pattern-match `Node.kind`. Implement `Location::scale()`, `node_id()`, `Location::parent(&self, snap) -> Option<Location>` via `Node.parent_id`. DEPENDS: `OpenedSnapshot::lookup_by_qualified_name`, `node_by_id`. VERIFY: `goto_qualified_resolves`.

2. **Scale ladder + `zoom`.** WHERE: `view/navigate.rs::Navigator::zoom`. **ZoomIn:** `Workspace → Crate(first by sorted crate_edges); Crate → Module(find_root_module_of); Module → Item(first child via children_by_parent); Item → Body{item, file, span}`; `Body` → `NoZoom`. **ZoomOut:** `Body → Item; Item → Module(parent_id); Module → Crate(if parent is crate); Crate → Workspace; Workspace → NoZoom`. Cluster scale today errors `ClusterStub` unless P1.3 is wired. DEPENDS: `Node.parent_id`, `find_root_module_of`, `crate_edges`. VERIFY: `zoom_in_out_idempotent`.

3. **MapPane assembly.** WHERE: `view/context.rs::build_map_pane`. **Crates rim (always):** run `snap.crate_edges()` once + `snap.crate_dependency_metric()`; resolve crate NodeIds via `lookup_by_qualified_name`. **Module tree (at Module/Item/Body scale):** walk `parent_id` up to crate; call `snap.module_tree(&crate_qualified, Some(N))` (default N=2); flatten DFS into `ModulePin`s; re-resolve each via `lookup_by_qualified_name`. **Current path:** walk `parent_id` from focus up to workspace; `[crate_root_module, ..., focus]`. **Clusters:** empty stub. DEPENDS: `crate_edges`, `crate_dependency_metric`, `module_tree`, `find_root_module_of`, `node_by_id`. VERIFY: `mappane_includes_path`.

4. **Neighbor enumeration.** WHERE: `view/context.rs::collect_neighbors`. Per scale:
   - **Crate:** edges from `crate_edges()` filtered to `name`; add root module as `Child`.
   - **Module:** `children_by_parent` via `dbs.children_by_parent.get_duplicates(rtxn, mid.as_bytes())` (same pattern as `build_module_tree` at `query/modules.rs:134-145`); each child → `node_by_id` → `NeighborSlot { kind: Child }`. Parent: `Node.parent_id`. Imports: `snap.imports_of(mid)` enriched via `snap.enrich_bindings`.
   - **Item:** siblings via `parent_id` then `children_by_parent` excluding self. Reexports: `snap.re_export_chain(iid)`. Enum variants: if `item_kind == Some(Enum)`, `snap.enum_variants(iid)`.
   - **Body:** same as Item.
   - **Cluster:** stub returns `vec![]` and tracing warn.
   VERIFY: unit test on `imports_of` against a known module.

5. **`goto` assembling ContextView.** WHERE: `view/navigate.rs::Navigator::goto`. Sequence: scale → `build_map_pane` → `build_focal_node` (populating `signature` via `function_signature(iid)`, `attributes` via `item_attributes(iid)`) → `collect_neighbors` → `exports = if module { snap.exports_of(focus_mid, focus_mid).and_then(enrich_bindings).unwrap_or_default() } else { vec![] }` → `cost::estimate(...)` → if `> budget` → `ViewTooLarge`. VERIFY: `goto_qualified_resolves`.

6. **`show_body` (skeleton-inverse).** WHERE: `view/body.rs::materialise_body`. Pull `file = node.file.clone().ok_or(BodyMissing)?`, `(start, end) = node.span.ok_or(BodyMissing)?`. Get line-to-byte via `OpenedSnapshot::line_to_byte(file)`. Convert byte offsets to line via `partition_point(|&off| off <= start)`. `text = String::from_utf8_lossy(&bytes[start..end]).into_owned()`. In `Navigator::show_body`: focus must be `Item` or `Body`. Update `view` clone with `focus = Body { item, file, span }`, `scale = Body`, `body = Some(...)`, `token_cost += body_tokens`. If `> budget` → `ViewTooLarge`. VERIFY: `show_body_token_growth`.

7. **`show_callers`.** WHERE: `view/navigate.rs::Navigator::show_callers`. For `Item(iid)`: `callers = snap.who_calls(iid)?; callees = snap.calls_from(iid)?;`. If `depth > 1`: `callees_tree = Some(snap.call_graph(iid, depth)?)`; callers_tree via reverse BFS using `snap.referrers_of(target)` iteratively, synthesise into `CallGraphNode`-shaped tree. Update clone's `callgraph`. VERIFY: `show_callers_matches_who_calls`.

8. **`follow_trail`.** Pure interpreter:
   ```rust
   let mut view = self.goto(start)?;
   for step in steps {
       view = match step {
           NavStep::Goto(loc) => self.goto(loc.clone())?,
           NavStep::ZoomIn => self.zoom(&view, ZoomDir::In)?,
           NavStep::ZoomOut => self.zoom(&view, ZoomDir::Out)?,
           NavStep::ShowBody => self.show_body(&view)?,
           NavStep::ShowCallers(d) => self.show_callers(&view, *d)?,
       };
   }
   Ok(view)
   ```
   Every step re-checks budget; trail can fail mid-way with `ViewTooLarge`. VERIFY: `follow_trail_replays`.

9. **Token cost estimator.** WHERE: `view/cost.rs`. Coefficients (conservative `bytes/4` baseline for Claude tokenizers):
   - `FOCAL_NODE_BASE = 60`, `SIGNATURE_TOK = 40`, `ATTRIBUTE_TOK = 10` per attr.
   - `NEIGHBOR_SLOT_TOK = 12`, `MAP_CRATE_PIN_TOK = 8`, `MAP_MODULE_PIN_TOK = 14`.
   - `EXPORT_BINDING_TOK = 20`, `CALL_SITE_TOK = 25`, `BODY_TOK = body.text.len().div_ceil(4)`.
   - `CALLGRAPH_NODE_TOK = 18` per node recursively.
   ```rust
   pub fn estimate(focal: &Option<NodePin>, neighbors: &[NeighborSlot], map: &MapPane,
                   body: Option<&BodySlice>, calls: Option<&CallSlice>) -> usize { ... }
   pub fn body_tokens(body: &BodySlice) -> usize { body.text.len().div_ceil(4) }
   pub const BUDGET_DEFAULT: usize = 8_000;
   ```

10. **Optional MCP handlers.** WHERE: `crates/rmc-server/src/tools/graph/navigate.rs`. Five tools (`navigate_goto`, `_zoom`, `_show_body`, `_show_callers`, `_follow_trail`) mirroring the `who_calls` pattern at `tools/graph/core.rs`. Params files in `tools/params/`. `navigate_follow_trail` accepts `start: NavigateGotoParams` + `steps: Vec<NavStepJson>` (externally-tagged serde enum). Gated by `#[cfg(feature = "navigate")]`.

11. **Serde round-trip.** All view types derive `Serialize`; address types (`Location`, `Scale`, `NavStep`, `ZoomDir`, `ClusterId`) also `Deserialize`. `#[serde(rename_all = "snake_case")]` on enums. `Location` externally tagged. `NodeId` already serde. `#[serde(skip)]` `callgraph`/`body` when None. VERIFY: round-trip test.

12. **Body hide/show round-trip.** VERIFY: `body_round_trip`.

13. **Wire the module.** `graph/mod.rs`:
    ```rust
    pub mod view;
    pub use view::{Location, Scale, ContextView, Navigator, NavStep, ZoomDir,
                   NeighborSlot, NeighborKind, CallSlice, BodySlice, MapPane,
                   ClusterId, ViewError};
    ```

## Tests

(`crates/rmc-graph/src/graph/view/tests.rs`, reusing `test_support::shared_snapshot()`)

- **`goto_qualified_resolves`** — `Location::from_qualified(snap, "rmc_graph::graph::snapshot::open_current")`; assert `scale == Item`, `focal_node.signature.is_some()`.
- **`zoom_in_out_idempotent`**.
- **`show_body_token_growth`** — `cost2 - cost1 ≈ body.text.len() / 4 ± 16`.
- **`show_callers_matches_who_calls`** — pick `lookup_by_qualified_name_inner`; assert `slice.callers.len() == snap.who_calls(iid)?.len()`.
- **`follow_trail_replays`** — manual chain vs `follow_trail` produce same final view.
- **`mappane_includes_path`** — `current_path.first()` is crate root, `last() == iid`.
- **`view_too_large_refused`** — `with_budget(10)` on a large module → `ViewTooLarge`.
- **`external_symbol_rejected`** — `Location::from_qualified(snap, "std::sync::Arc")` → `ExternalSymbol`.
- **`body_round_trip`** — show_body then zoom_out returns Item with `body: None`.
- **`serde_json_round_trip`**.
- **`zoom_at_floor_errors`** — `Body{..}` + `ZoomIn` → `NoZoom`.

## Open decisions / risks

- **Cluster stub.** `Location::Cluster(ClusterId)` and `MapPane.clusters` wired today; `cluster_stub.rs` returns `ClusterStub` when explicitly asked. P1.3 swaps `cluster_stub.rs` for the real assembler without changing `ContextView` or callers.
- **Cost calibration.** `bytes/4` is conservative; log `(actual_tokenized, estimated)` pairs during M3 and recalibrate.
- **JSON vs compact text.** Ship JSON; P1.8 adds `render_textual(&ContextView) -> String` adapter.
- **File-text caching.** Re-read on each `show_body` (μs). With P0.2 host, `Navigator.host` field consults latest live text first.
- **MCP handler placement.** Thin layer — `Navigator` composes existing `OpenedSnapshot` queries; don't duplicate.
- **`exports_of` consumer.** Pass `consumer = focus_module` ("what this module exposes internally").
- **`show_callers` on Module.** Optional fanout cap 50; off by default.
- **`follow_trail` loops.** No detection; budget is the safety net.
- **`Location::Workspace` semantics.** `goto(Workspace)` → Crate-scale view, `focus = Workspace`, all crates in `MapPane.crates`, empty modules/neighbors, no focal_node.
- **`span_index` / `line_to_byte` visibility.** Both `pub(crate)` on `OpenedSnapshot`. Place `view` inside `crate::graph` (codemap precedent).
- **`ViewTooLarge` policy.** Fires after full assembly (so cost is accurate). Add cheap `estimate_lower_bound(loc, snap)` pre-flight later as perf TODO.


---

# Section E — P1.2 Description Index

## Overview

The description index is the fourth persisted index alongside `nodes_by_id` / `bindings_by_*` / `usages_by_*` / `embeddings_by_target`. Per-Item it stores a **one-sentence natural-language description** (≤15 words) generated by a small LLM from the item's signature, doc-comment, attributes, and 1-hop neighbor names; storage is a new LMDB sub-DB `descriptions_by_target` keyed by `NodeId`, payloaded as `DescriptionRecord { content_hash, description, model_version, generated_at_unix }` — mirroring `EmbeddingRecord` and following the same content-hash invalidation rule used by `crates/rmc-graph/src/graph/embedding_cache.rs::prepare_embeddings_for`. A separate LanceDB table `descriptions_vec` stores embeddings of those descriptions so `search_by_description(query) -> Vec<NodeId>` is a single vector query.

This slice belongs to **M1** (read-side, parallel, on slow build). It runs once per episode on a published snapshot, with regen riding the P0.2 affected set in M2a. Per D3, descriptions are class **C** (content-hash cache, self-invalidating): every read path verifies that the cached `content_hash` matches the item's current trimmed-source SHA-256; mismatch → re-generate. There is no explicit invalidation step for any edit class — the read-side hash check is authoritative. Cost is bounded by a token-bucket rate limiter (default 60 generations/min) and a back-pressure work queue.

**Default LLM choice (justification):** Anthropic Claude Haiku 4.5 (`claude-haiku-4-5-20251001`). At ~$1/$5 per MTok and ~50 output tokens per description, 5k items costs $1–3; vs. a local Qwen3 ~0.6B–1B instruct model that competes with the embedding generator's GPU residency. The `DescriptionModel` trait parameterizes the choice; `LocalQwen3Client` ships as a stub behind `descriptions-local` for offline runs.

## New modules / files

- `crates/rmc-graph/src/graph/descriptions/mod.rs` — public surface; gated behind `descriptions` cargo feature.
- `crates/rmc-graph/src/graph/descriptions/store.rs` — heed reader/writer over `descriptions_by_target` sub-DB.
- `crates/rmc-graph/src/graph/descriptions/prompt.rs` — `PromptCtx` and `PromptCtx::build_for(snap, target)`.
- `crates/rmc-graph/src/graph/descriptions/generator.rs` — `DescriptionGenerator<M>` orchestrator + token-bucket rate limiter.
- `crates/rmc-graph/src/graph/descriptions/queue.rs` — back-pressure work queue with rate limiter.
- `crates/rmc-graph/src/graph/descriptions/models/mod.rs` — `DescriptionModel` trait.
- `crates/rmc-graph/src/graph/descriptions/models/anthropic.rs` — `AnthropicHaikuClient` (default).
- `crates/rmc-graph/src/graph/descriptions/models/local.rs` — `LocalQwen3Client` (behind `descriptions-local`).
- `crates/rmc-graph/src/graph/descriptions/retrieval.rs` — `DescriptionRetrieval` wrapping a new LanceDB table `descriptions_vec`.
- New sub-DB in `crates/rmc-graph/src/graph/storage.rs`: `descriptions_by_target: Database<Bytes, SerdeBincode<DescriptionRecord>>`. NOT DUP_SORT. Created via `open_or_create_bytes_bincode`. `DEFAULT_MAX_DBS` bumped 16 → 20.
- Bump `SCHEMA_VERSION` 12 → 13 (consistent with v11/v12 pattern).
- Add re-exports to `crates/rmc-graph/src/graph/mod.rs` (gated by `cfg(feature = "descriptions")`).
- `crates/rmc-server/src/tools/graph/descriptions.rs` — server-side handler `handle_search_by_description`.
- `crates/rmc-server/src/tools/router.rs` — new `#[tool] async fn search_by_description(...)`.
- `crates/rmc-server/src/tools/params.rs` — `SearchByDescriptionParams`.
- `crates/rmc-graph/Cargo.toml` — new `descriptions` feature enabling `dep:reqwest`, `dep:tokio`, `dep:rmc-engine` with `rmc-engine/embeddings`.

## Type definitions

```rust
// crates/rmc-graph/src/graph/descriptions/mod.rs

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DescriptionRecord {
    pub content_hash: [u8; 16],   // SHA-256(trimmed_item_source)[..16]
    pub description: String,       // ≤15 words, no trailing newline
    pub model_version: String,
    pub generated_at_unix: u64,
}

pub struct DescriptionStore<'a> {
    pub(crate) snap: &'a OpenedSnapshot,
    pub(crate) db: heed::Database<heed::types::Bytes, heed::types::SerdeBincode<DescriptionRecord>>,
}

impl<'a> DescriptionStore<'a> {
    pub fn new(snap: &'a OpenedSnapshot) -> Self {
        Self { snap, db: snap.dbs.descriptions_by_target }
    }
    pub fn get(&self, target: NodeId) -> Result<Option<DescriptionRecord>>;
    pub fn put(&self, target: NodeId, rec: &DescriptionRecord) -> Result<()>;
    pub fn delete(&self, target: NodeId) -> Result<()>;
    pub fn iter_stale<'b>(&'b self, items: &'b [NodeId], model_version: &'b str)
        -> Result<impl Iterator<Item = (NodeId, StaleReason)> + 'b>;
}

#[derive(Debug, Clone, Copy)]
pub enum StaleReason { Missing, HashMismatch, ModelMismatch, Unresolvable }

pub struct PromptCtx {
    pub node: Node,
    pub signature: Option<FunctionSignature>,
    pub doc_comment: Option<String>,
    pub attributes: Vec<String>,
    pub neighbor_labels: Vec<String>,    // up to NEIGHBOR_FANOUT = 8
    pub item_source: String,
    pub content_hash: [u8; 16],
}

const NEIGHBOR_FANOUT: usize = 8;

impl PromptCtx {
    pub fn build_for(snap: &OpenedSnapshot, target: NodeId) -> Result<Option<PromptCtx>>;
    pub fn render_prompt(&self) -> String;
}

#[async_trait::async_trait]
pub trait DescriptionModel: Send + Sync {
    async fn batch(&self, prompts: Vec<String>) -> Result<Vec<String>>;
    fn version(&self) -> &str;
    fn recommended_batch_size(&self) -> usize { 8 }
}

pub struct AnthropicHaikuClient {
    http: reqwest::Client,
    api_key: String,
    model: String,            // "claude-haiku-4-5-20251001"
    version_id: String,
    base_url: String,         // "https://api.anthropic.com/v1/messages"
}

impl AnthropicHaikuClient {
    pub const DEFAULT_MODEL: &'static str = "claude-haiku-4-5-20251001";
    pub const API_KEY_ENV: &'static str = "ANTHROPIC_API_KEY";
    pub fn from_env() -> Result<Self>;
}

pub struct RateLimiter { inner: Arc<tokio::sync::Mutex<TokenBucket>>, rpm_cap: usize }
struct TokenBucket { capacity: f64, tokens: f64, refill_per_sec: f64, last_refill: Instant }

impl RateLimiter {
    pub fn new(rpm_cap: usize) -> Self;
    pub async fn acquire(&self, n: usize);
    pub fn rpm_cap(&self) -> usize { self.rpm_cap }
}

pub struct DescriptionGenerator<M: DescriptionModel> {
    pub model: M,
    pub batch_size: usize,
    pub limiter: RateLimiter,
    pub store: DescriptionStoreOwned,
    pub stats: Arc<tokio::sync::Mutex<DescriptionGenStats>>,
}

pub struct DescriptionStoreOwned {
    pub env: Arc<heed::Env<heed::WithoutTls>>,
    pub db: heed::Database<heed::types::Bytes, heed::types::SerdeBincode<DescriptionRecord>>,
    pub workspace_root: PathBuf,
}

impl<M: DescriptionModel> DescriptionGenerator<M> {
    pub async fn regenerate_targets(&mut self, snap: &OpenedSnapshot, targets: Vec<NodeId>,
                                     mut progress: impl FnMut(usize, usize)) -> Result<usize>;
}

#[derive(Debug, Clone, Default)]
pub struct DescriptionGenStats {
    pub generations_total: u64,
    pub generations_today: u64,
    pub failed_total: u64,
    pub last_batch_latency_ms: u64,
    pub rpm_cap: usize,
}

pub struct DescriptionRetrieval {
    table: lancedb::Table,
    embed: rmc_engine::embeddings::EmbeddingGenerator,
    table_name: &'static str,    // "descriptions_vec"
    vector_dim: usize,
}

impl DescriptionRetrieval {
    pub async fn open_or_create(path: &Path, embed: EmbeddingGenerator) -> Result<Self>;
    pub async fn reindex(&self, store: &DescriptionStoreOwned) -> Result<usize>;
    pub async fn upsert_one(&self, target: NodeId, description: &str) -> Result<()>;
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<(NodeId, f32)>>;
}
```

## Step-by-step implementation

1. **Declare new sub-DB + bump schema.** WHERE: `storage.rs`. Add `pub descriptions_by_target: Database<Bytes, SerdeBincode<DescriptionRecord>>` to `GraphDatabases`. Add creation line after `embeddings_by_target`. Mirror in `open` path. Bump `SCHEMA_VERSION 12 → 13`, `DEFAULT_MAX_DBS 16 → 20`. Gate behind `cfg(feature = "descriptions")`. VERIFY: `cargo check -p rmc-graph --features descriptions`.

2. **`DescriptionRecord` + serde + match policy.** Implement struct in `mod.rs`; derive `Serialize`/`Deserialize`. Match policy in `iter_stale` and generator read path: `stored.content_hash == current_hash && stored.model_version == active_model.version()`. VERIFY: `record_roundtrip`.

3. **`PromptCtx::build_for(snap, target)`.** Open `RoTxn`. Load `Node` via `snap.node`; bail with `None` if not found or non-Item. Load `FunctionSignature` via `snap.function_signature`. Load attributes via `snap.item_attributes`; partition into doc-comment lines (starts with `///` or `//!`) joined as `doc_comment` and other `attributes`. Read trimmed `item_source` (recipe from `embedding_cache.rs:120-138`): `let abs = workspace_root.join(file_rel); let content = fs::read_to_string(&abs)?; let slice = content.get(span.0..span.1)?; let trimmed = slice.trim();`. Compute `content_hash = Sha256::digest(trimmed)[..16]`. Neighbors: `snap.callees_of(target)?` + `snap.referrers_of(target)?`, concat, dedupe, take first 8. Format each as `{qualified_name}({item_kind_short_label})`. VERIFY: `prompt_includes_signature_and_neighbors`.

4. **Prompt template.** WHERE: `descriptions/prompt.rs::PromptCtx::render_prompt` + `models/anthropic.rs::SYSTEM_PROMPT`.

   System (`system` field):
   ```
   You summarize a single Rust item in one sentence (max 15 words). Output only the sentence, no quotes, no leading "This function". Use present tense, active voice.
   ```

   User body:
   ```
   Item: {qualified_name}
   Kind: {item_kind_short_label}
   {Signature: {signature_str}}             ← if present
   {Doc: {first 6 doc-comment lines}}       ← if present
   {Attributes: {attributes.join(", ")}}    ← if present
   Neighbors: {neighbor_labels.join(", ")}
   Source (head):
   {first 40 lines of item_source}
   ```

   `signature_str` rendered via private fn walking `sig.params`/`sig.return_type` and prepending `async fn` / `fn`. VERIFY: golden snapshot test.

5. **`DescriptionModel` trait + `AnthropicHaikuClient`.** `#[async_trait]` from existing workspace dep. HTTP via `reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?`. URL `https://api.anthropic.com/v1/messages`. Headers: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`. Per-prompt request body:
   ```json
   {
     "model": "claude-haiku-4-5-20251001",
     "max_tokens": 50,
     "system": "<SYSTEM_PROMPT>",
     "messages": [{"role": "user", "content": "<rendered prompt>"}]
   }
   ```
   Anthropic API has no batch — `batch(prompts)` issues `prompts.len()` parallel requests via `futures::future::join_all`, capped by per-client semaphore (default 8 concurrent). Extract `response.content[0].text`, trim, truncate at first newline. Retry policy: copy from `crates/rmc-engine/src/embeddings/openrouter/retry.rs::is_retryable_reqwest_error` — retry on 429/5xx/connect timeout with exponential backoff (250ms, 500ms, 1s, 2s, 4s; MAX_RETRIES=5); 429 honors `retry-after`. `version()` returns the model string. VERIFY: gated integration test with `ANTHROPIC_API_KEY`.

6. **`DescriptionGenerator::regenerate_targets`.**
   ```
   fn regenerate_targets(snap, targets):
       ctxs = targets.filter_map(|t| PromptCtx::build_for(snap, t).transpose())
       for batch in ctxs.chunks(self.batch_size):
           self.limiter.acquire(1).await
           prompts = batch.iter().map(|(_, c)| c.render_prompt()).collect()
           outputs = self.model.batch(prompts).await?
           assert outputs.len() == batch.len()
           for ((nid, ctx), desc) in batch.iter().zip(outputs):
               rec = DescriptionRecord {
                   content_hash: ctx.content_hash,
                   description: clean_description(&desc),
                   model_version: self.model.version().to_string(),
                   generated_at_unix: now_unix(),
               }
               self.store.put(*nid, &rec)?
               written += 1
           progress(written, total)
           stats.lock().generations_today += batch.len()
   ```
   `clean_description`: trim, strip surrounding quotes, truncate at first `\n`, hard 200-char cap. Rate limiter token-bucket: `capacity = rpm_cap as f64, refill_per_sec = rpm_cap as f64 / 60.0`. Default 60 rpm (batches/min). Cost ceiling: `generations_today_cap: Option<u64>` field; abort with `DescriptionError::DailyCapReached` if exceeded. VERIFY: `regen_only_dirty`, `rpm_cap_respected`.

7. **Workspace-wide initial population.** New free function `populate_workspace_descriptions(workspace_root)`. Open snapshot; walk `nodes_by_id` collecting Items sorted by `qualified_name`. Pass to `regenerate_targets`. Resumable: pre-filter via `iter_stale`. After all writes, `DescriptionRetrieval::reindex(&store_owned)`.

8. **Edit-time regen (P0.2 wiring).** Public `regen_for_affected_items(snap, store, generator, affected: Vec<NodeId>)`. Filter through `iter_stale`; call `regenerate_targets`. `Unresolvable` → `store.delete(target)`. Self-invalidation: read paths re-hash and compare, no explicit invalidation step (class C in D3). VERIFY: `regen_only_dirty`.

9. **Retrieval table.** Use `lancedb` (already in `rmc-engine`). Mirror `LanceDbBackend::new`:
   ```
   conn = lancedb::connect(path).execute().await?
   schema: { node_id_hex: Utf8 (primary), description: Utf8, vector: FixedSizeList<Float32, vector_dim> }
   table_name = "descriptions_vec"   // DELIBERATELY DIFFERENT from "vectors"
   ```
   `reindex(store)`: iterate `descriptions_by_target.iter(&rtxn)`; embed in chunks of 64 via `embed.embed_documents(docs_chunk)`; build Arrow `RecordBatch`; `table.add(...).await?`. On full reindex, first `table.delete("true").await?`. `upsert_one(target, description)`: delete then add. Path: `<data_dir>/graphs/<workspace_hash>/snapshots/<graph_id>/descriptions_vec/`. Embedder identity persisted in sibling `metadata.json` per `vector_store/lancedb.rs::write_metadata_if_missing`. VERIFY: `search_returns_expected`.

10. **`search_by_description(query, limit) -> Vec<(NodeId, f32)>`.**
    ```
    qvec = embed.embed_queries(vec![query]).await?.pop()?
    res = table.vector_search(qvec)?.limit(limit).distance_type(DistanceType::Cosine).execute().await?
    decode node_id_hex; return Vec<(NodeId, 1.0 - distance)> sorted desc
    ```
    Add `NodeId::from_hex(s: &str)` to `crates/rmc-graph/src/graph/ids.rs` paralleling `to_hex`. BM25 fallback behind `descriptions-bm25` feature deferred — skip in M1.

11. **MCP tool registration.** `SearchByDescriptionParams { directory, query, limit }`. Handler:
    ```rust
    pub(crate) async fn handle_search_by_description(params, workspace_locks, search_cache)
        -> Result<CallToolResult, McpError>
    {
        canonicalize directory; lock_shared
        open snapshot via rmc_graph::open_current_for_workspace
        construct EmbeddingGenerator via search_cache (mirror tools/graph/similarity.rs::semantic_overlaps)
        DescriptionRetrieval::open_or_create(&snapshot_path.join("descriptions_vec"), embed).await?
        hits = retrieval.search(&params.query, params.limit).await?
        for (nid, score) in hits: snap.node(&rtxn, id) to enrich
        return JSON CallToolResult
    }
    ```
    Wire into `router.rs` with `#[tool(description = "Search workspace items by natural-language description ...")] async fn search_by_description(...)`. VERIFY: smoke test mirroring `embedding_profile_smoke.rs`.

12. **Cost ceiling + stats endpoint.** Daily rollover: `day_start_unix: u64` on stats struct. `generations_today_cap = Some(5_000)` by default (~$2.50/day Haiku). Configurable via env `RMC_DESCRIPTIONS_DAILY_CAP`. Read-only MCP tool `description_gen_status` returning `DescriptionGenStats`.

## Tests

(`crates/rmc-graph/src/graph/descriptions/tests.rs`)

- **`record_roundtrip`** — put then get.
- **`content_hash_mismatch_is_stale`** — write hash A, source becomes B → `iter_stale` yields `HashMismatch`.
- **`model_version_mismatch_is_stale`** — write `v1`, call `iter_stale(_, "v2")` → `ModelMismatch`.
- **`prompt_includes_signature_and_neighbors`** — fixture `public_function`; assert prompt contains `Signature:`, param names, `Neighbors:` with at least one label.
- **`regen_only_dirty`** — 3 items, mutate source of 1; `MockDescriptionModel` records calls; assert `mock.calls.lock().len() == 1`.
- **`search_returns_expected`** — pre-populate `[("fn parse_cli_args", "this function parses CLI arguments"), ("fn open_file", "opens a file by path"), ("fn add", "adds two integers")]`. `reindex`. `search("parse CLI", 3)` → top result is `parse_cli_args`.
- **`rpm_cap_respected`** — `RateLimiter::new(60)`; submit 4 targets; wall-clock ≥ ~3s; use `tokio::time::pause()` + `advance` for determinism.
- **`daily_cap_blocks`** — `generations_today_cap = Some(2)`; submit 5 → 2 written + `DailyCapReached`.
- **`unresolvable_node_is_deleted`** — synthetic NodeId; `iter_stale` yields `Unresolvable`; wrapper calls `delete`.
- **`apply_to_cold_rebuild_diff`** — M2a `#[ignore]` stub.

## Open decisions / risks

- **Model choice.** DEFAULT = Anthropic Haiku 4.5. Justification: cheapest path-to-quality for one-line descriptions; LocalQwen3 alternative behind `descriptions-local`.
- **LanceDB reuse vs new table.** DECISION = new table `descriptions_vec`. Different ID space (NodeId vs ChunkId); different semantic purpose (NL vs code-chunk-vs-code-chunk); mixing would corrupt RRF.
- **Description staleness within an episode.** Accepted per plan's M1 contract; rides P0.2 affected set in M2a. Surface `stale: bool` in `search_by_description` response.
- **First-population cost.** 5–50k items × ~50 tok × ~1s/req @ 8 concurrent ≈ 10 min–1 hr cold-start per workspace. Background work during M1; resumable via skip-if-fresh.
- **Bincode forward-compat.** Fresh sub-DB; no migration. Future fields use `#[serde(default)]`. Schema bump to 13 invalidates unrelated stale snapshots.
- **Concurrency vs heed write txns.** heed serializes writers. Step 6 batches one write txn per LLM batch (8 writes default). Readers (`search_by_description`) use `RoTxn`, never blocked.
- **Anthropic API key in tests.** Step 5 integration test `#[ignore]` and gated on `ANTHROPIC_API_KEY`.
- **NodeId encoding in LanceDB.** `node_id_hex: Utf8` over `Binary(32)` for portability/debuggability. 64-char overhead negligible. Add `NodeId::from_hex` to `ids.rs`.
- **Goodhart-on-descriptions.** LLM that writes descriptions has incentive to misrepresent in adversarial setups. Out of scope for M1; swap to local model in Phase 2/3 to remove gameable channel.


---

# Section F — P1.3 Analyze / Vision Layer

## Overview

This slice introduces an `analyze` subtree inside `rmc-graph` that turns the per-Item embedding cache (`embeddings_by_target`) and the call/usage substrates into the mesoscale "city map" the navigator (P1.1) renders at the cluster zoom level. It computes per-Item joined feature vectors (Qwen3 embedding ⊕ structural features), clusters them with GMM over the joined substrate (with a spectral fallback over the Laplacian of the call graph), labels each cluster with a 2–5 word concept name produced by the same small LLM that P1.2 uses, scores per-Item outliers (LOF + per-cluster Mahalanobis), and emits two queryable affinity scores (`affinity`: random-walk PMI on the call/usage graph; `co_change`: file-co-occurrence lift from `jj log` / `git log --name-only`). All outputs land in a single `VisionIndex` value cached per `(graph_id, head_commit_hash)`.

M1 work, runs against the slow published snapshot, no P0.2 dependency. Load-bearing for perception (Issue #9): (a) soft membership, not hard assignments; (b) zoom-through to raw nodes via `assignment: HashMap<NodeId, Vec<(ClusterId, f32)>>`; (c) silhouette + Davies–Bouldin per build for quality monitoring; (d) deterministic `seed` from `BuildOptions` threaded through every random source. Fills P1.1's "cluster scale stub" via `clusters_at_zoom(scale)` that the navigator's `MapPane::Cluster` layer reads.

## New modules / files

- `crates/rmc-graph/src/analyze/mod.rs` — public surface + `build_vision()` entry: `ClusterId`, `Cluster`, `OutlierFinding`, `OutlierKind`, `AffinityIndex`, `CoChangeIndex`, `VisionIndex`, `BuildVisionOptions`.
- `crates/rmc-graph/src/analyze/features.rs` — `FeatureVector`, `StructuralFeatures`, `build_features()`.
- `crates/rmc-graph/src/analyze/cluster.rs` — GMM via `linfa-clustering::GaussianMixtureModel` + spectral fallback (`petgraph` Laplacian + `nalgebra` symmetric eigen + k-means).
- `crates/rmc-graph/src/analyze/outliers.rs` — LOF via `linfa-anomaly::LocalOutlierFactor` + per-cluster Mahalanobis using `nalgebra`.
- `crates/rmc-graph/src/analyze/affinity.rs` — biased random walks on merged `petgraph` graph; pair-count → PMI.
- `crates/rmc-graph/src/analyze/cochange.rs` — async wrapper shelling out to `jj log` (preferred) or `git log --name-only` (fallback).
- `crates/rmc-graph/src/analyze/labels.rs` — LLM-based cluster labeling via P1.2's model handle.
- `crates/rmc-graph/src/analyze/cache.rs` — per-episode cache keyed `(graph_id, head_commit_hash)`. JSON files under `working/<session_id>/vision/<key>.json`.
- `crates/rmc-graph/src/analyze/zoom.rs` — `clusters_at_zoom(scale)` for `MapPane::Cluster`.
- `crates/rmc-graph/src/lib.rs` — `pub mod analyze;`.
- `crates/rmc-graph/Cargo.toml` — new `analyze` feature pulling `petgraph = "0.6"`, `linfa = "0.7"`, `linfa-clustering = "0.7"`, `linfa-anomaly = "0.7"`, `nalgebra = "0.33"`, `ndarray = "0.16"`, `rand = "0.8"`, `rand_chacha = "0.8"`.

## Type definitions

```rust
// crates/rmc-graph/src/analyze/mod.rs

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClusterId(pub u32);
// Derived by hashing (graph_id, head_commit_hash, "cluster", local_idx)

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cluster {
    pub id: ClusterId,
    pub members: Vec<(NodeId, f32)>,    // (id, soft_membership), sorted desc
    pub centroid: Vec<f32>,
    pub silhouette: f32,
    pub davies_bouldin_contrib: f32,
    pub label: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OutlierKind { LocalLOF, Mahalanobis, UnclusteredLone }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutlierFinding { pub item: NodeId, pub cluster: ClusterId, pub score: f32, pub kind: OutlierKind }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffinityIndex { pairs: HashMap<(NodeId, NodeId), f32> }
impl AffinityIndex { pub fn score(&self, a: NodeId, b: NodeId) -> f32; }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoChangeIndex { pairs: HashMap<(NodeId, NodeId), f32>, pub window: Duration }
impl CoChangeIndex {
    pub fn score(&self, a: NodeId, b: NodeId) -> f32;
    pub async fn build_from_vcs(snap: &OpenedSnapshot, workspace_root: &Path, window: Duration) -> Result<Self>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionIndex {
    pub graph_id: String,
    pub head_commit_hash: String,
    pub clusters: Vec<Cluster>,
    pub assignment: HashMap<NodeId, Vec<(ClusterId, f32)>>,
    pub outliers: Vec<OutlierFinding>,
    pub affinity: AffinityIndex,
    pub co_change: CoChangeIndex,
    pub quality: VisionQuality,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionQuality {
    pub silhouette_mean: f32,
    pub davies_bouldin: f32,
    pub picked_k: usize,
    pub bic_curve: Vec<(usize, f64)>,
}

#[derive(Debug, Clone)]
pub struct BuildVisionOptions {
    pub seed: u64,
    pub k_candidates: Vec<usize>,          // default vec![8, 16, 32, 64]
    pub min_cluster_weight: f32,           // default 0.05
    pub lof_k: usize,                       // default 20
    pub walks_per_node: usize,              // default 10
    pub walk_length: usize,                 // default 40
    pub node2vec_p: f32,                    // default 1.0
    pub node2vec_q: f32,                    // default 1.0
    pub cochange_window: Duration,          // default 90 days
    pub labeler: Option<Arc<dyn LabelGenerator + Send + Sync>>,
    pub cache_dir: Option<PathBuf>,
}

pub trait LabelGenerator {
    fn label_cluster(&self, member_snippets: &[String]) -> Result<String>;
}
```

```rust
// crates/rmc-graph/src/analyze/features.rs

// d_struct = 1 (in_deg log) + 1 (out_deg log) + 1 (module_depth) + 11 (item_kind one-hot) + 1 (attr_bits) = 15
// d_embed = backend dim (1024 for Qwen3-0.6B)

#[derive(Debug, Clone)]
pub struct FeatureVector {
    pub embedding: Vec<f32>,
    pub structural: StructuralFeatures,
}
impl FeatureVector {
    pub fn dim(&self) -> usize { self.embedding.len() + StructuralFeatures::DIM }
    pub fn flatten(&self, w_embed: f32, w_struct: f32) -> Vec<f32>;
}

#[derive(Debug, Clone, Copy)]
pub struct StructuralFeatures {
    pub in_deg: u32,
    pub out_deg: u32,
    pub module_depth: u32,
    pub kind_onehot: [f32; 11],   // Function, Struct, Enum, Union, Trait, TypeAlias, Const, Static, AssocFunction, AssocType, EnumVariant
                                  // (Method collapsed → AssocFunction, AssocConst → Const)
    pub attr_bits: u32,            // bitset: must_use, non_exhaustive, deprecated, derive_present, repr_present, doc_present, async_present, unsafe_present, inline_present, allow_present, target_feature_present, ...
}
impl StructuralFeatures { pub const DIM: usize = 1 + 1 + 1 + 11 + 1; }

pub fn build_vision(snap: &OpenedSnapshot, embeds: &EmbeddingsLookup, opts: &BuildVisionOptions) -> Result<VisionIndex>;

pub struct EmbeddingsLookup<'a> {
    pub by_target: &'a HashMap<NodeId, Vec<f32>>,
    pub embedder_dim: usize,
}
```

## Step-by-step implementation

1. **Item-feature builder.** WHERE: `analyze/features.rs`. Single `RoTxn`. Iterate `nodes_by_id` keeping `NodeKind::Item` with `file.is_some() && span.is_some()` (reuse `enumerate_similarity_seeds` from `query/similarity.rs`). For each, fetch vector from `EmbeddingsLookup.by_target` (skip if absent — callers pre-populate via `embedding_cache::ensure_embeddings_for`). Structural: `in_deg = usages_by_target.prefix(nid).count()`, `out_deg = usages_by_consumer_function.prefix(nid).count()` (fall back to `usages_by_consumer.prefix(parent_module).filter(consumer_function)` for non-fn items); `module_depth` walks `parent_id` chain; `kind_onehot` from `Node.item_kind` (collapse Method→AssocFunction, AssocConst→Const); `attr_bits` from `snap.item_attributes(nid)` matching known markers. `log1p` in/out degrees to compress tail. VERIFY: `feature_vector_shape`.

2. **Joined-substrate matrix.** WHERE: `cluster.rs::assemble_matrix`. `ndarray::Array2<f32>` shape `n × (d_embed + d_struct)`. ℓ2-normalize each block; multiply by per-block weights (1.0 embedding, 0.5 structural). Keep `row_to_nid: Vec<NodeId>`. VERIFY: `assemble_matrix_row_count`.

3. **GMM clustering with BIC sweep.** Use `linfa_clustering::GaussianMixtureModel::params(k).with_rng(rng).max_n_iterations(200).tolerance(1e-3).init_method(GmmInitMethod::KMeansPlusPlus)`. RNG = `rand_chacha::ChaCha8Rng::seed_from_u64(opts.seed)`. For each `k ∈ opts.k_candidates`: fit; compute log-likelihood + BIC = `−2·ll + k·log(n)·d_params` (diagonal covariance variant if d > 256). `picked_k = argmin BIC`. Re-fit; soft membership from `predict_proba`; apply `min_cluster_weight` floor. VERIFY: `cluster_count_under_bic`.

4. **Spectral fallback.** WHEN GMM rejected (picked_k at max-k with monotonic BIC, suggests under-fitting) OR rank-deficient. Build undirected `petgraph::Graph<NodeId, f32>` from call/usage adjacency. Symmetric normalized Laplacian as dense `nalgebra::DMatrix<f64>` (workspaces top at ~5k items; dense fine). `L.symmetric_eigen()` → bottom-k eigenvectors → `Y ∈ R^{n×k}` → seeded `linfa_clustering::KMeans` → map back via `softmax(−|y − μ|² / τ)`.

5. **Default = GMM** (direct soft membership for Issue #9 zoom-through). Spectral wired as `BuildVisionOptions::clustering = ClusteringKind::Spectral`.

6. **Outliers.** LOF via `linfa_anomaly::LocalOutlierFactor::params(opts.lof_k).fit(&matrix)?.predict_score(&matrix)`; above 95th percentile → `LocalLOF`. Per-cluster Mahalanobis: clusters with ≥ `2·d` members; compute sample mean + covariance via `nalgebra`; for points with `H(p) > log(picked_k)·0.8` (high entropy), Mahalanobis to dominant centroid; above 97.5th percentile → `Mahalanobis`. Items absent from `assignment` after floor → `UnclusteredLone`. VERIFY: `outlier_finds_planted`.

7. **AffinityIndex.** node2vec-ish PMI: build `petgraph::DiGraph<NodeId, f32>` from `usages_by_consumer_function` + `bindings_by_target`. For each Item, `walks_per_node` walks of length `walk_length` with biased step (prob ∝ `1/p` revisit, `1/q` farther). RNG = `ChaCha8Rng` seeded with `(opts.seed, nid)` for per-node determinism. Accumulate co-occurrence in window 5 along each walk. After all walks: `PMI(a,b) = log(p(a,b) / (p(a) · p(b)))` with add-one smoothing. Keep positive only; canonicalize keys. VERIFY: `affinity_directional_invariant`.

8. **CoChangeIndex.** Detect VCS: `.jj` present → use jj; else `.git` → git; else empty.
   - jj: `Command::new("jj").args(["log", "-r", &format!("ancestors(@) & description(glob:'*') & after({})", since_iso8601), "-T", r#"separate(" ",commit_id,"\n",files.map(|f| f.path()))"#, "--no-graph"]).output()`.
   - git fallback: `git log --since="<duration>" --name-only --pretty=format:"COMMIT %H"`.
   Parse into `Vec<HashSet<String>>`. File → NodeId map from `nodes_by_id` where `Node.file == path`. Co-change between files becomes cross-product of items in each commit weighted by `1/(|set_a|·|set_b|)`. `log(p(a∧b) / (p(a)·p(b)))` with add-one smoothing. Sparse canonical-key. WHERE: `tokio::task::spawn_blocking`. VERIFY: `cochange_from_synthetic_history`.

9. **LLM cluster labels.** Top-K (default 5) members per cluster by membership. Look up source slice via P1.2's `embedding_cache::prepare_embeddings_for` recipe (file, span, trim, ~400 chars cap). If `opts.labeler.is_some()`: prompt `"Name the concept these {N} Rust items share in 2 to 5 words. Output only the name.\n\n{snippets}"`. Sanitize: trim, collapse whitespace, cap to 5 words. Else fallback: longest common qualified-name prefix beyond crate root → modal item kind label. VERIFY: `labels_are_short`.

10. **`VisionIndex` assembly + cache.** Resolve `head_commit_hash` via `jj log -r @ -T 'commit_id'` or `git rev-parse HEAD`; empty disables cache. Key = `format!("{}_{}", graph_id, head_commit_hash)`. Read: `cache_dir.join(format!("{key}.json"))` — early return if exists. Write: `serde_json::to_writer_pretty` to `tempfile::NamedTempFile::persist`. **Flat JSON over LMDB** because cache rows are large, per-episode, and ride D1's working-dir convention.

11. **`MapPane::Cluster` integration.**
    ```rust
    impl VisionIndex {
        pub fn clusters_at_zoom(&self, scale: f32) -> Vec<ClusterId>;  // scale ∈ [0,1]
        pub fn clusters_for_node(&self, node: NodeId) -> Vec<(ClusterId, f32)>;
    }
    ```

12. **Determinism plumbing.** Every RNG: `ChaCha8Rng::seed_from_u64(opts.seed)` for top-level; `seed.wrapping_add(k)` per-k; `hash64((seed, nid))` per-node walks. Linfa: `.with_rng(rng)`. `opts.seed` from `BuildOptions.seed` (P0.1). VERIFY: `seeded_clustering_stable`.

13. **Incremental update on P0.2 affected set.** Do NOT recluster. Recompute features for affected items + new items. Assign to nearest existing cluster by evaluating each cluster's stored Gaussian (centroid persisted; cov recomputed from members on demand). Apply `min_cluster_weight` floor. Update `assignment` in place; write new cache file. Track drift: recompute `silhouette_mean` from affected rows; if drops > 0.1, `tracing::warn!` "cluster quality drift". Full recluster at episode end (build_vision entry), not incremental. VERIFY: `incremental_update_stable_for_unaffected`.

## Tests

(`crates/rmc-graph/src/analyze/tests.rs`, gated on `analyze` feature)

- **`feature_vector_shape`** — `dim() == 1024 + 15`; `flatten(1.0, 0.5).len() == 1039`; zero-norm guarded.
- **`structural_features_pulls_correct_degrees`** — 3-node synthetic graph; in_deg.log1p() ≈ ln(3).
- **`cluster_count_under_bic`** — plant 3 Gaussians 5σ apart at d=8; assert `picked_k == 3`.
- **`seeded_clustering_stable`** — two builds same seed → identical assignment + outliers.
- **`outlier_finds_planted`** — plant `embedding = vec![20.0; d]`; flagged with `LocalLOF` and max score.
- **`affinity_directional_invariant`** — 4-cycle; `score(a,b) == score(b,a)` for all pairs.
- **`cochange_from_synthetic_history`** — synthetic git repo: c1={A}, c2={A,B}, c3={B,C}; `score(A,B) > 0`, `score(A,C) ≈ 0`.
- **`cochange_handles_mega_commit`** — 200-file commit yields smaller per-pair than 2-file commit.
- **`labels_are_short`** — fallback + stub LabelGenerator both produce ≤ 5 words.
- **`incremental_update_stable_for_unaffected`** — original NodeIds' assignment byte-identical after adding 1 new item.
- **`vision_cache_round_trip`** — write, drop, re-read; serde_json round-trip equality.
- **`vision_cache_key_changes_with_commit_hash`**.
- **`zoom_through_returns_raw_nodes`** — 3 clusters of 10; `clusters_at_zoom(0.0).len() <= 3`; `clusters_for_node(nid)` returns top cluster with weight > 0.5.
- **`end_to_end_on_shared_snapshot`** — `test_support::shared_snapshot()`, `k_candidates: vec![4]`, `walks_per_node: 2`; build < 30s, clusters non-empty, silhouette > 0.

## Open decisions / risks

- **Feature engineering** — commits to: ℓ2-normalize embedding+structural separately; concat with 1.0/0.5; d_struct = 15; collapse Method→AssocFunction + AssocConst→Const. Open: add `signature.params.len()` + `signature.generics.len()` after BIC curves stabilize.
- **Clustering library** — `linfa-clustering` + `linfa-anomaly` (pure Rust, no Python). HDBSCAN drops soft membership → kills Issue #9 mitigation. `nalgebra` dense for spectral fallback (5k Items → 200MB Laplacian fits).
- **Co-change window** — 90 days default; `BuildVisionOptions::cochange_window`. < 30 days history → `tracing::warn!`; treat scores as low-confidence.
- **VCS detection precedence** — jj first (rmc uses jj per AGENTS.md), git second. CI must exercise jj branch.
- **Cluster quality monitoring** — silhouette + Davies-Bouldin per build in `VisionQuality`. Warn on > 0.1 drop between episodes. Future P1.3.x feeds metric stream into P1.7 reward.
- **Reclustering policy** — incremental assign mid-episode; full refit at episode end. Bounds per-step cost at O(k·d·|affected|). Risk: long episodes drift; mitigation via silhouette drift warning + forced full refit.
- **Where labels live** — `cluster_labels` JSON sidecar in cache file. NOT mixed with P1.2 descriptions (different ID space, ephemeral per-episode). Promote to LMDB sub-DB if survive episodes is needed.
- **LLM label cost** — 32 clusters × ~500ms ≈ 16s added per build. Mitigation: `futures::future::join_all` inside `LabelGenerator` adapter.
- **Determinism of jj/git output** — sort parsed commits by `(commit_id, files)` before consuming.
- **`min_cluster_weight`** — 0.05 default; truncates 99% of trailing memberships. Calibrate after M1 dogfooding.
- **Cache invalidation on embedder change** — add `backend_identity: Option<String>` to `BuildVisionOptions`; include in cache filename. Avoids coupling `analyze` to `rmc-engine`.


---

# Section G — P1.5a modify_body + P1.5b move / delete

## Overview

This slice delivers **`modify_body` (P1.5a)** and **`move` + `delete` (P1.5b)** — three verbs that close enough of the loop to power M3's first end-to-end episode. `modify_body` is the verb on the critical path — per the milestone order, M2a finishes P0.2 together with `modify_body`, and M3 immediately opens an end-to-end loop on `modify_body` alone (the entire point of the P1.5 split: prove the apply→gate→reward loop with the cheapest possible edit class — D2 `BodyOnly` — before adding propagation). `move` and `delete` arrive in M2b as the first two verbs that *do* propagate.

All three verbs are thin wrappers around four substrates: (a) the persisted `OpenedSnapshot` read layer for resolving `NodeId → Node`, spans, and ref-checks via `who_imports` + `usages_of`; (b) RA's `ast::Fn::body().syntax().text_range()` pattern from `crates/rmc-graph/src/graph/skeleton/source.rs` for brace-to-brace body sub-spans; (c) the **`WorkspaceHost::apply_edits`** entry point from P0.2 — does the `set_file_text` → re-extract → LMDB diff-patch pipeline, classified by D2 `EditClass`; and (d) the **`Checkpoint::take`/`Checkpoint::restore`** contract from D4. Each verb: classify the edit, compute byte-level source edits, hand a `Vec<FileEdit>` + `EditClass` to the host inside a checkpoint, translate the result into `EditOutcome` (or roll back).

## New modules / files

A new workspace crate `rmc-crud` is the cleanest home. It cannot live inside `rmc-graph` (would force `rmc-graph` to depend on `ra_ap_ide`'s rename machinery via `rmc-server::semantic`); cannot live inside `rmc-server` (would drag MCP binary surface into every consumer).

- `crates/rmc-crud/Cargo.toml` — new crate. Deps: `rmc-graph` (path), `rmc-host` (path — or the host module re-exported from rmc-graph), `rmc-semantic` (NEW crate, see below) for `SemanticService`/`RenamePreview`/`RenameEdit`/`RenameFileMove`; `ra_ap_syntax`; `anyhow`/`thiserror`; `tracing`; `tempfile` in dev-deps.
- `crates/rmc-crud/src/lib.rs` — facade re-exporting `Crud`, `EditOutcome`, `EditError`, `CascadePolicy`, `BodyEdit`, `MoveOp`, `DeleteOp`, `GraphDiffSummary`.
- `crates/rmc-crud/src/edit.rs` — pure data types.
- `crates/rmc-crud/src/source_edit.rs` — byte-level splicing helpers.
- `crates/rmc-crud/src/body_span.rs` — given `Node` + file text, returns brace-to-brace `(body_start, body_end)`. Uses `ra_ap_syntax::SourceFile::parse(..., Edition::Edition2024)` then `ast::Fn::cast(...).body().syntax().text_range()`.
- `crates/rmc-crud/src/modify_body.rs` — P1.5a.
- `crates/rmc-crud/src/move_item.rs` — P1.5b move.
- `crates/rmc-crud/src/delete.rs` — P1.5b delete.
- `crates/rmc-crud/src/preview_apply.rs` — translates `RenamePreview { edits, file_moves }` into `Vec<FileEdit>` (NEW APPLY logic — RA's preview is unapplied today; converts `(line, col)` → byte offsets via `OpenedSnapshot::line_to_byte` and sorts edits descending by `(file, byte_start)`).
- `crates/rmc-crud/src/cycle_check.rs` — pure-graph helper: walk `Node.parent_id` from `dest_parent` upward; refuse if `target.id` appears.

**Required upstream changes (cross-slice):**
1. **NEW crate `crates/rmc-semantic/`** (recommended). Promote `crates/rmc-server/src/semantic/` to its own crate. Types `SemanticService`, `RenamePreview`, `RenameEdit`, `RenameFileMove` become `pub`. `rmc-server` then depends on `rmc-semantic`.
2. `crates/rmc-server/src/semantic/mod.rs:53` — `SemanticService` → `pub`.
3. `crates/rmc-server/src/semantic/rename.rs:15,41,61` — `RenameEdit`, `RenameFileMove`, `RenamePreview` → `pub` (fields too).
4. `crates/rmc-server/src/semantic/rename.rs:70,168` — `rename_by_name`, `rename_by_position` → `pub`.
5. `crates/rmc-graph/src/graph/snapshot.rs:629` — `OpenedSnapshot::line_to_byte` → `pub`.

## Type definitions

```rust
// crates/rmc-crud/src/edit.rs

pub use rmc_host::FileEdit;   // re-export from P0.2

#[derive(Debug, Clone)]
pub struct BodyEdit {
    pub target: NodeId,
    /// MUST include outer braces. Convention: agent supplies complete block,
    /// e.g. `"{ self.x + 1 }"`. Bodies not starting with `{` and ending with `}` rejected.
    pub new_body_block: String,
}

#[derive(Debug, Clone)]
pub struct MoveOp {
    pub target: NodeId,
    pub dest_parent: NodeId,           // MUST be a Module
    pub new_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DeleteOp {
    pub target: NodeId,
    pub cascade: CascadePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CascadePolicy {
    #[default] Refuse,
    DeleteCallers,           // bounded-depth (cap 5); recursive delete of caller fns
    DeleteUnused,            // not implemented in P1.5b; reserved
}

#[derive(Debug, Clone, Serialize)]
pub struct GraphDiffSummary {
    pub nodes_added: usize, pub nodes_removed: usize,
    pub bindings_added: usize, pub bindings_removed: usize,
    pub usages_delta: i64,
}

#[derive(Debug)]
pub struct EditOutcome {
    pub checkpoint: Checkpoint,
    pub affected_items: Vec<NodeId>,
    pub affected_files: Vec<PathBuf>,
    pub edit_class: EditClass,
    pub graph_diff_summary: GraphDiffSummary,
}

#[derive(Debug, thiserror::Error)]
pub enum EditError {
    #[error("target node {0:?} not found in snapshot")] TargetNotFound(NodeId),
    #[error("target node {target:?} has wrong kind: expected {expected}, got {actual:?}")]
    WrongKind { target: NodeId, expected: &'static str, actual: Option<ItemKind> },
    #[error("target has no span/file recorded (likely macro-generated)")] TargetHasNoSource(NodeId),
    #[error("body splice failed: {reason}")] BodySpliceFailed { reason: String },
    #[error("new body must start with '{{' and end with '}}': got {first_bytes:?}…{last_bytes:?}")]
    BodyConvention { first_bytes: String, last_bytes: String },
    #[error("rust-analyzer refused the rewrite: {reason}")] RaRefused { reason: String },
    #[error("item has live references; cascade=Refuse")] RefsExist { refs: Vec<Binding>, usages: Vec<Usage> },
    #[error("move would introduce a module cycle")] ModuleCycle,
    #[error("destination already contains '{path}'")] PathConflict { path: String },
    #[error("warm-host apply rejected the edit: {0}")] HostRejected(String),
    #[error("io error: {0}")] IoError(#[from] io::Error),
    #[error("cascade depth limit (5) exceeded")] CascadeDepthExceeded,
}
```

```rust
// crates/rmc-crud/src/lib.rs

pub mod edit;
mod body_span; mod source_edit; mod modify_body; mod move_item;
mod delete; mod preview_apply; mod cycle_check;
pub use edit::*;

pub struct Crud<'a> {
    pub host: &'a mut WorkspaceHost,
    pub snap: &'a OpenedSnapshot,
    pub semantic: &'a mut SemanticService,
    pub workspace_root: PathBuf,
}

impl<'a> Crud<'a> {
    pub fn new(host: &'a mut WorkspaceHost, snap: &'a OpenedSnapshot,
               semantic: &'a mut SemanticService, workspace_root: impl Into<PathBuf>) -> Self {
        Self { host, snap, semantic, workspace_root: workspace_root.into() }
    }
    pub fn modify_body(&mut self, op: BodyEdit) -> Result<EditOutcome, EditError>;
    pub fn move_item(&mut self, op: MoveOp) -> Result<EditOutcome, EditError>;
    pub fn delete(&mut self, op: DeleteOp) -> Result<EditOutcome, EditError>;
}
```

## Step-by-step implementation

### P1.5a — `modify_body`

**Step 1 — Resolve target + validate kind.** WHERE: `modify_body.rs::run`.
```rust
let rtxn = crud.snap.read_txn()?;
let node = crud.snap.node(&rtxn, op.target)?.ok_or(EditError::TargetNotFound(op.target))?;
let kind = node.item_kind;
if !kind.map(|k| k.is_callable()).unwrap_or(false) {
    return Err(EditError::WrongKind { target: op.target, expected: "callable", actual: kind });
}
let (item_start, item_end) = node.span.ok_or(EditError::TargetHasNoSource(op.target))?;
let rel_file = node.file.clone().ok_or(EditError::TargetHasNoSource(op.target))?;
drop(rtxn);
```
DEPENDS: `ItemKind::is_callable` (`model.rs:50`). VERIFY: `modify_body_rejects_non_fn`.

**Step 2 — Convention check.**
```rust
let body = op.new_body_block.trim_start();
let trailing = op.new_body_block.trim_end();
if !body.starts_with('{') || !trailing.ends_with('}') {
    return Err(EditError::BodyConvention {
        first_bytes: body.chars().take(4).collect(),
        last_bytes:  trailing.chars().rev().take(4).collect::<String>().chars().rev().collect(),
    });
}
```
VERIFY: unit test on `"self.x + 1"` returns `BodyConvention`.

**Step 3 — Find body sub-span.** WHERE: `body_span.rs`.
```rust
pub(crate) fn body_byte_range(file_text: &str, node: &Node) -> Result<(u32, u32), EditError> {
    use ra_ap_syntax::{SourceFile, Edition, TextRange, TextSize, ast, AstNode};
    let parse = SourceFile::parse(file_text, Edition::Edition2024);
    if !parse.errors().is_empty() {
        return Err(EditError::BodySpliceFailed {
            reason: format!("source already has {} parse errors before edit", parse.errors().len()),
        });
    }
    let parsed = parse.tree();
    let (s, e) = node.span.unwrap();
    let wanted = TextRange::new(TextSize::from(s), TextSize::from(e));
    let fn_syntax = parsed.syntax().descendants()
        .filter_map(ast::Fn::cast)
        .find(|f| {
            let r = f.syntax().text_range();
            r == wanted || r.contains_range(wanted) || wanted.contains_range(r)
        })
        .ok_or_else(|| EditError::BodySpliceFailed {
            reason: "could not locate ast::Fn matching the node's span".into(),
        })?;
    let body = fn_syntax.body().ok_or_else(|| EditError::BodySpliceFailed {
        reason: "fn is a trait-declaration only (no body)".into(),
    })?;
    let r = body.syntax().text_range();
    Ok((u32::from(r.start()), u32::from(r.end())))
}
```
DEPENDS: `ra_ap_syntax` (already in `rmc-graph` deps; mirror version in `rmc-crud`). VERIFY: `body_byte_offsets_correct` on `pub fn foo() -> u32 { 1 + 2 }`.

**Step 4 — Take checkpoint before writing.**
```rust
let checkpoint = Checkpoint::take(crud.host)
    .map_err(|e| EditError::HostRejected(format!("checkpoint failed: {e}")))?;
```
DEPENDS: D4 contract. VERIFY: contract test `Checkpoint::take` + immediate `restore` is no-op.

**Step 5 — Compute new file text in memory.**
```rust
let abs_path = crud.workspace_root.join(&rel_file);
let original = std::fs::read_to_string(&abs_path)?;
let (body_start, body_end) = body_span::body_byte_range(&original, &node)?;
let new_text = source_edit::splice_bytes(&original, body_start as usize, body_end as usize, &op.new_body_block);
let byte_delta: i64 = (op.new_body_block.len() as i64) - ((body_end - body_start) as i64);
```
Helper:
```rust
pub(crate) fn splice_bytes(src: &str, start: usize, end: usize, replacement: &str) -> String {
    let mut out = String::with_capacity(src.len() - (end - start) + replacement.len());
    out.push_str(&src[..start]);
    out.push_str(replacement);
    out.push_str(&src[end..]);
    out
}
```
Host re-extracts the file; agent does not patch downstream spans.

**Step 6 — Hand edit to host with BodyOnly class.**
```rust
let edit = FileEdit {
    path: abs_path.clone(),
    new_text,
    edit_class: EditClass::BodyOnly,
};
let apply = crud.host.apply_edits(&[edit]).map_err(|e| {
    if let Err(restore_err) = Checkpoint::restore(crud.host, &checkpoint) {
        tracing::error!(?e, ?restore_err, "modify_body: both apply AND restore failed");
    }
    EditError::HostRejected(format!("apply rejected: {e}"))
})?;
```
**Load-bearing decision:** host owns the `fs::write`, not the CRUD layer. Keeps atomicity in one place (RA `set_file_text` + LMDB write txn + disk write under same lock).

**Step 7 — Translate `ApplyOutcome` → `EditOutcome`.**
```rust
Ok(EditOutcome {
    checkpoint,
    affected_items: apply.affected_node_ids,
    affected_files: apply.affected_files,
    edit_class: EditClass::BodyOnly,
    graph_diff_summary: GraphDiffSummary {
        nodes_added: apply.nodes_added,
        nodes_removed: apply.nodes_removed,
        bindings_added: apply.bindings_added,
        bindings_removed: apply.bindings_removed,
        usages_delta: apply.usages_delta,
    },
})
```
DEPENDS: `ApplyOutcome` shape from P0.2 (returns counts + `affected_node_ids` so P1.6 doesn't re-scan).

### P1.5b — `move`

**Step 8 — Resolve + validate.** WHERE: `move_item.rs::run`. Resolve `target_node` + `dest_node`; require `dest_node.kind == Module`; extract `target_kind`, `span`, `rel_src_file`; compute `new_name = op.new_name.unwrap_or_else(|| target_node.display_name.clone())`; `new_qualified = format!("{}::{}", dest_node.qualified_name, new_name)`. VERIFY: `move_item_rejects_non_module_dest`.

**Step 9 — Compute dest file + cycle check.** WHERE: `cycle_check.rs`.
```rust
pub(crate) fn would_introduce_cycle(snap: &OpenedSnapshot, rtxn: &GraphRoTxn<'_>,
                                     target: NodeId, dest_parent: NodeId) -> Result<bool> {
    let mut cursor = Some(dest_parent);
    while let Some(id) = cursor {
        if id == target { return Ok(true); }
        let n = snap.node(rtxn, id)?;
        cursor = n.and_then(|n| n.parent_id);
    }
    Ok(false)
}
```
Then in `run`: `if cycle_check::would_introduce_cycle(...)? { return Err(EditError::ModuleCycle); }`. VERIFY: `move_cycle_refused`.

**Step 10 — Compute identifier (line, col) for RA.** RA's `rename_by_position` needs `(file, line, column)`, not bytes. Re-parse file with `ra_ap_syntax`; find `ast::Fn::name().syntax().text_range().start()` (mirrors `declaration_name` in `skeleton/source.rs:228`); convert byte → (line, col) via `OpenedSnapshot::line_to_byte` binary search.
```rust
let abs_src_path = crud.workspace_root.join(&rel_src_file);
let file_text = std::fs::read_to_string(&abs_src_path)?;
let ident_byte_offset = body_span::identifier_byte_offset(&file_text, &target_node)?;
let (line, col) = byte_offset_to_line_col(&file_text, ident_byte_offset);
let preview = crud.semantic.rename_by_position(
    &crud.workspace_root, &abs_src_path, line, col,
    &target_node.display_name, &new_name,
).map_err(|e| EditError::RaRefused { reason: e.to_string() })?;
```
**UPSTREAM CHANGES REQUIRED** — see top-level visibility list above.

**Step 11 — Translate RenamePreview → FileEdits.** WHERE: `preview_apply.rs`.
```rust
pub(crate) fn preview_to_file_edits(snap: &OpenedSnapshot, workspace_root: &Path,
                                    preview: &RenamePreview) -> Result<Vec<FileEdit>, EditError> {
    let mut by_file: BTreeMap<PathBuf, Vec<(u32, u32, String)>> = BTreeMap::new();
    for e in &preview.edits {
        let rel = e.file_path.strip_prefix(workspace_root).unwrap_or(&e.file_path);
        let line_to_byte = snap.line_to_byte(rel.to_string_lossy().as_ref())?;
        let start_byte = line_to_byte[(e.start_line - 1) as usize] + (e.start_column - 1);
        let end_byte   = line_to_byte[(e.end_line - 1) as usize]   + (e.end_column - 1);
        by_file.entry(e.file_path.clone()).or_default()
            .push((start_byte, end_byte, e.new_text.clone()));
    }
    let mut out = Vec::new();
    for (path, mut edits) in by_file {
        edits.sort_by(|a, b| b.0.cmp(&a.0));        // descending so earlier splices keep offsets
        let mut text = std::fs::read_to_string(&path)?;
        for (s, e, repl) in &edits {
            text = source_edit::splice_bytes(&text, *s as usize, *e as usize, repl);
        }
        out.push(FileEdit { path, new_text: text, edit_class: EditClass::ModuleTree });
    }
    Ok(out)
}
```
DEPENDS: `OpenedSnapshot::line_to_byte` must be `pub`.

**Step 12 — Source-file move (delete old, insert new).** Two cases:
(a) **Same-file move:** RA's `rename` doesn't handle item-level moves. Manually cut bytes `[item_start..item_end]`, insert at end of dest module's range (`dest_end - 1` before closing `}` or end of file for file-modules).
(b) **Cross-file move:** delete from src, append to dest with newline+indent.

```rust
let dest_rel_file = dest_node.file.clone().ok_or(EditError::TargetHasNoSource(op.dest_parent))?;
let same_file = dest_rel_file == rel_src_file;
let item_text = file_text[item_start as usize .. item_end as usize].to_string();
let mut src_new_text = source_edit::delete_byte_range(&file_text, item_start as usize, item_end as usize);
src_new_text = source_edit::collapse_blank_lines(&src_new_text, item_start as usize);
let dest_file_text = if same_file { src_new_text.clone() }
                    else { std::fs::read_to_string(crud.workspace_root.join(&dest_rel_file))? };
let insertion_point = compute_dest_insertion_byte(&dest_file_text, &dest_node);
let dest_new_text = source_edit::insert_at_byte_offset(&dest_file_text, insertion_point, &format!("\n\n{}\n", item_text));

let mut file_edits = preview_to_file_edits(crud.snap, &crud.workspace_root, &preview)?;
upsert_file_edit(&mut file_edits, FileEdit { path: abs_src, new_text: src_new_text, edit_class: EditClass::ModuleTree });
if !same_file {
    upsert_file_edit(&mut file_edits, FileEdit { path: abs_dst, new_text: dest_new_text, edit_class: EditClass::ModuleTree });
}
```

**Step 13 — EditClass selection.** Cross-file or rename → `ModuleTree`. Pure no-op (same file + no rename) → `SigOrVis` (shouldn't happen — early-out).

**Step 14 — Checkpoint + apply + finalize.** Same pattern as Step 6/7.

### P1.5b — `delete`

**Step 15 — Resolve target.** Same shape as Step 1/8.

**Step 16 — Ref-check.**
```rust
let refs   = crud.snap.who_imports(op.target)?;
let usages = crud.snap.usages_of(op.target)?;
if (!refs.is_empty() || !usages.is_empty()) && matches!(op.cascade, CascadePolicy::Refuse) {
    return Err(EditError::RefsExist { refs, usages });
}
```
DEPENDS: `who_imports` (`query/usage.rs:798`), `usages_of` (line 802) — both already `pub`. VERIFY: `delete_refuses_with_refs`.

**Step 17 — Cascade plan (DeleteCallers).**
```rust
let mut deletions: Vec<NodeId> = vec![op.target];
if matches!(op.cascade, CascadePolicy::DeleteCallers) {
    cascade_collect(&mut deletions, crud.snap, op.target, 0)?;
}
fn cascade_collect(out: &mut Vec<NodeId>, snap: &OpenedSnapshot, target: NodeId, depth: u8) -> Result<()> {
    const MAX_DEPTH: u8 = 5;
    if depth >= MAX_DEPTH { return Err(EditError::CascadeDepthExceeded); }
    let usages = snap.usages_of(target)?;
    let caller_fns: HashSet<NodeId> = usages.iter().filter_map(|u| u.consumer_function).collect();
    for f in caller_fns {
        if !out.contains(&f) {
            out.push(f);
            cascade_collect(out, snap, f, depth + 1)?;
        }
    }
    Ok(())
}
```
DEPENDS: `Usage.consumer_function` (`model.rs:193`). VERIFY: cascade test.

**Step 18 — Per-file deletion edits.** Group by `Node.file`, sort ranges descending within each file:
```rust
let mut by_file: BTreeMap<String, Vec<(u32, u32)>> = BTreeMap::new();
for id in &deletions {
    if let Some(n) = crud.snap.node(&rtxn, *id)? {
        if let (Some(file), Some(span)) = (n.file.clone(), n.span) {
            by_file.entry(file).or_default().push(span);
        }
    }
}
let mut file_edits = Vec::new();
for (rel_file, mut ranges) in by_file {
    ranges.sort_by(|a, b| b.0.cmp(&a.0));
    let abs = crud.workspace_root.join(&rel_file);
    let mut text = std::fs::read_to_string(&abs)?;
    for (s, e) in &ranges {
        text = source_edit::delete_byte_range(&text, *s as usize, *e as usize);
    }
    file_edits.push(FileEdit { path: abs, new_text: text, edit_class: EditClass::ItemAddRemove });
}
```
*Optional:* drop `mod foo;` if delete removed last item from a child file (out of MVP scope).

**Step 19 — Checkpoint + apply + return.** Identical pattern. `EditClass::ItemAddRemove` (or `ModuleTree` if removing a module file).

## Tests

(`crates/rmc-crud/tests/`)

1. **`modify_body_roundtrip`** — 2-crate fixture (`producer` exporting `pub fn add(a, b)`, `consumer` calling it). Cold-build; resolve `add` via `lookup_by_qualified_name("producer::add")`; call `Crud::modify_body(BodyEdit { target, new_body_block: "{ a.wrapping_add(b) }".into() })`. Then: (a) file body replaced; (b) `usages_of(add)` count unchanged; (c) cold-rebuild against post-edit source matches incremental state on affected crate (differential test mandated by Issue #3).

2. **`modify_body_rollback_on_compile_break`** — body that breaks the parse: `"{ a + }"`. `apply_edits` rejects → `Checkpoint::restore` → file bytes match pre-edit + `Node` span identical.

3. **`move_updates_imports`** — `core_crate::utils::foo` moved to `core_crate::helpers::foo`; consumer crate had `use core_crate::utils::foo;`. After: (a) `lookup_by_qualified_name("core_crate::helpers::foo")` resolves; (b) `imports_of(consumer_module)` returns binding with `target = foo_id`, `visible_name = "foo"`; (c) consumer file contains `use core_crate::helpers::foo;`.

4. **`delete_refuses_with_refs`** — same `producer`/`consumer`. `Crud::delete(DeleteOp { target: add_id, cascade: Refuse })` → `Err(EditError::RefsExist { refs, usages })`. Then `cascade: DeleteCallers` succeeds; consumer's caller fn deleted; `who_imports(add_id)` empty.

5. **`move_cycle_refused`** — `core_crate::a::b::c`; `MoveOp { target: a_id, dest_parent: c_id }` → `Err(EditError::ModuleCycle)` without file mutation.

6. **`body_byte_offsets_correct`** — pure unit test: (a) `pub fn foo() { return 1; }` → body range includes braces and `return 1;`; (b) two fns in same file `pub fn foo(){}\npub fn bar(){ panic!() }` — `bar`'s byte range does not shift after splicing longer body into `foo`.

7. **`preview_apply_byte_offsets_match_line_col`** — synthetic `RenamePreview { edits: vec![RenameEdit { start_line: 3, start_column: 5, end_line: 3, end_column: 8, new_text: "BAR".into() }] }`; assert resulting `FileEdit.new_text` has `"BAR"` at the byte offset `(line=3, col=5)` resolves to via `line_to_byte`.

8. **`cross_file_apply_ordering`** — 3-file fixture with multiple non-overlapping edit positions per file; verify three `FileEdit`s with all positions correctly spliced (descending-sort trick).

## Open decisions / risks

- **Body-span source-of-truth.** `Node.span` covers the whole item; we re-parse on every `modify_body`. 100k-LOC files parse in ~50ms — acceptable for ~500ms P0.2 target. If hot: cache parses by `(file_path, file_mtime)`. Alternative (rejected): store `body_span: Option<(u32, u32)>` on `Node` (LMDB bloat + schema bump).
- **`syn` vs `ra_ap_syntax`.** Use `ra_ap_syntax` — codebase already uses it; same edition handling; RA's error recovery on partially-broken files.
- **Applying RA's RenamePreview is net-new.** `SemanticService::rename_by_position` is preview-only today. P1.5b adds APPLY logic; complexity is the `(line, col) → byte` conversion. RA's `LineCol` is 0-indexed then `+1`'d in `source_change_to_preview` (`rename.rs:296-301`); we `-1` on the way back. Test 7 pins this down.
- **RA's `FileSystemEdit::CreateFile` / `MoveFile`.** Appear in `RenamePreview.file_moves` for module renames. For P1.5b move we're moving items, not modules — should be empty. Defensive: if `preview.file_moves` non-empty → `EditError::RaRefused { reason: "RA proposed file move; not supported in P1.5b" }`. Lift in P1.5e.
- **DeleteCallers cascade depth.** Hard cap 5. Above → `CascadeDepthExceeded`. Not configurable in MVP — predictable behavior for reward signal.
- **`new_body_block` convention.** Braces required. Zero ambiguity; byte-range we splice IS the braced range; easy to validate.
- **Source-write ownership.** Host owns `fs::write` (recommended) — atomicity in one place. Fallback: CRUD does `fs::write` then `host.notify_files_changed(...)`. Lock this in D4 contract before P0.2 ships.
- **Upstream visibility changes.** Cleanest: extract `rmc-server::semantic` to new `rmc-semantic` crate (one PR; `rmc-server` imports from `rmc_semantic::`). Smaller intervention: add `pub mod semantic_api` re-export with the `pub use` items — but `rmc-crud` then depends on `rmc-server` (a binary host crate). **Recommend crate-extraction.**
- **Multi-file transactionality.** Each verb takes one `Checkpoint::take`, submits one `host.apply_edits(&[...])` with full edit set. Host implements all-or-nothing per D4. CRUD calls `Checkpoint::restore` on `Err` arm.
- **D2 BodyOnly assumption.** Body-only = editing fn's outgoing usages only, no reverse-dep walk. If M0 spike #1 shows body edits still invalidate cross-crate inference, `modify_body` latency tracks P0.2's actual incremental performance; CRUD code unchanged.


---

# Section H — P1.5c modify_signature + P1.5d extract/inline + P1.5e module ops

## Overview

This slice completes the structural CRUD surface that P1.5a/b opened, by adding the nine remaining verbs: `modify_signature` (P1.5c), `extract_function` / `extract_trait` / `inline` (P1.5d), and `split_module` / `merge_modules` / `create_module` / `move_module` / `lift_to_crate` / `lower_to_module` (P1.5e). All ride the apply==rebuild engine and Checkpoint envelope built in Section G. P1.4 simulate is layered on top by reusing `Crud::compute_effects()` and skipping `persist()`.

**M2b work** (CRUD expansion after M3 proves the loop on `modify_body` alone). **P1.5c carries the highest correctness risk in the entire CRUD surface**: when a parameter is added to a fn signature, every callsite suddenly under-supplies arguments. The question of what to put in their place — refuse, silently fill with `Default::default()`, or insert `todo!()` — is a real semantic call. We pick **`todo!()` as the default policy** (encoded in `CallsiteFill::Todo`): keeps the workspace compiling against the type system, makes intent visible to downstream grep / cargo-check, avoids the silent-change failure mode of `Default`. The other tricky item is **`lift_to_crate`** which mutates workspace `Cargo.toml` + a member `Cargo.toml` — D2 classifies Cargo edits as **full-rebuild** class, so `lift_to_crate` and `lower_to_module` are flagged as **high-cost** verbs.

## New modules / files

All in `crates/rmc-crud/src/`:

- `crates/rmc-crud/src/modify_signature.rs` — P1.5c (sig rewrite + callsite synthesis).
- `crates/rmc-crud/src/extract_function.rs` — P1.5d extract code range → new fn + call.
- `crates/rmc-crud/src/extract_trait.rs` — P1.5d hoist inherent-impl methods into new trait + `impl Trait for T`.
- `crates/rmc-crud/src/inline.rs` — P1.5d inverse of extract_function.
- `crates/rmc-crud/src/split_module.rs` — P1.5e partition module items into N sibling modules.
- `crates/rmc-crud/src/merge_modules.rs` — P1.5e fold N modules into one.
- `crates/rmc-crud/src/create_module.rs` — P1.5e add empty/item-seeded child module.
- `crates/rmc-crud/src/move_module.rs` — P1.5e relocate/rename whole module file + update `use` paths.
- `crates/rmc-crud/src/lift_to_crate.rs` — P1.5e promote module to new workspace crate.
- `crates/rmc-crud/src/lower_to_module.rs` — P1.5e inverse: fold small workspace crate back.
- `crates/rmc-crud/src/callsite_fill.rs` — `CallsiteFill` enum + `CallsiteCtx`.
- `crates/rmc-crud/src/cargo_surgery.rs` — `toml_edit`-based reads/writes of `Cargo.toml`; format-preserving.
- `crates/rmc-crud/src/syn_ast.rs` — common helpers for `syn`/`ra_ap_syntax` parse + **byte-range location only** (signature span, arg-list span, impl-block item ranges). **No `prettyplease::unparse`** — replacement text is string-built and spliced via `source_edit::splice_bytes` per E5 (see Canonical Reconciliation §R4).
- `crates/rmc-crud/src/name_resolution.rs` — thin wrapper around RA's `Semantics` for capture analysis in `extract_function`.

`crates/rmc-crud/src/lib.rs` re-exports new verbs; `facade.rs` gains nine methods; `edit.rs` gains the `Cargo` variant + `is_full_rebuild()`; `error.rs` gains new variants.

New deps in `crates/rmc-crud/Cargo.toml` (E5: analysis-only — no `printing`,
no `prettyplease`, no `quote`/`proc-macro2` codegen; build replacement
strings by hand and splice):
```
syn = { version = "2", features = ["full", "parsing", "extra-traits", "visit", "visit-mut"] }
toml_edit = "0.22"
cargo_metadata = { workspace = true }
ra_ap_hir = "0.0.330"
ra_ap_ide_db = { workspace = true }
ra_ap_syntax = { workspace = true }
ra_ap_vfs = "0.0.330"
rmc-graph = { path = "../rmc-graph" }
rmc-server = { path = "../rmc-server" }
```

## Type definitions

### callsite_fill.rs

```rust
pub enum CallsiteFill {
    /// DEFAULT: `todo!("filled in by modify_signature: <param>")`. Compiles,
    /// runtime-panics if reached, easy to grep.
    Todo,
    /// `Default::default()` if param type implements Default (best-effort
    /// scan; falls back to Todo if unknown).
    Default,
    /// Refuse the op entirely → `EditError::SignatureSynthesisRefused`.
    Refuse,
    /// Caller-supplied builder; receives (callsite, new_param), returns string spliced verbatim.
    ClosureBuilder(Box<dyn Fn(&CallsiteCtx) -> String + Send + Sync>),
}

impl Default for CallsiteFill { fn default() -> Self { CallsiteFill::Todo } }

pub struct CallsiteCtx<'a> {
    pub fn_id: NodeId,
    pub added_param: &'a Param,
    pub call_site_file: &'a str,
    pub call_site_byte: u32,
    pub caller_fn: Option<NodeId>,
}
```

### modify_signature.rs

```rust
pub struct SignatureChange {
    pub target: NodeId,
    pub new_sig: FunctionSignature,     // ENTIRE new sig, not a delta
    pub callsite_fill: CallsiteFill,    // default Todo
}

pub(crate) struct SignatureDelta {
    pub added:    Vec<(usize, Param)>,
    pub removed:  Vec<usize>,
    pub renamed:  Vec<(usize, String)>,
    pub retyped:  Vec<(usize, String)>,
    pub reordered: Option<Vec<usize>>,
    pub self_changed: bool,
    pub return_changed: bool,
    pub generics_changed: bool,
    pub async_changed: bool,
}
```

### extract_function.rs / extract_trait.rs / inline.rs

```rust
pub struct ExtractFunctionOp {
    pub source_fn: NodeId,
    pub byte_range: (u32, u32),               // inside source_fn's file
    pub new_fn_name: String,
    pub captured_locals: Vec<String>,         // hint; empty = auto-detect
    pub new_fn_visibility: BindingVisibility,
}

pub struct ExtractTraitOp {
    pub source_struct: NodeId,
    pub method_subset: Vec<NodeId>,
    pub trait_name: String,
    pub trait_visibility: BindingVisibility,
    pub place_trait_inline: bool,
}

pub struct InlineOp { pub target_fn: NodeId, pub policy: InlinePolicy }
pub enum InlinePolicy { InlineAll, InlineSites(Vec<UsageId>) }
```

### split_module / merge_modules / create_module / move_module

```rust
pub struct SplitModuleOp { pub source_module: NodeId, pub splits: Vec<ModuleSplit> }
pub struct ModuleSplit {
    pub new_name: String,
    pub items: Vec<NodeId>,
    pub keep_reexport: bool,
}
pub struct MergeModulesOp { pub sources: Vec<NodeId>, pub dest: NodeId }
pub struct CreateModuleOp {
    pub parent: NodeId,
    pub name: String,
    pub initial_items: Vec<NodeId>,
    pub use_mod_rs: bool,
}
pub struct MoveModuleOp {
    pub source_module: NodeId,
    pub new_parent: NodeId,
    pub new_name: Option<String>,
}
```

### lift_to_crate / lower_to_module

```rust
pub struct LiftToCrateOp {
    pub source_module: NodeId,
    pub new_crate_name: String,    // kebab-case
    pub edition: String,            // "2021" / "2024"
    pub keep_facade: bool,
}

pub struct LowerToModuleOp {
    pub source_crate: NodeId,
    pub dest_parent_module: NodeId,
    pub new_module_name: Option<String>,
}
```

### Crud methods

```rust
impl Crud {
    pub fn modify_signature(&mut self, op: SignatureChange) -> Result<EditOutcome, EditError>;
    pub fn extract_function(&mut self, op: ExtractFunctionOp) -> Result<EditOutcome, EditError>;
    pub fn extract_trait(&mut self, op: ExtractTraitOp) -> Result<EditOutcome, EditError>;
    pub fn inline(&mut self, op: InlineOp) -> Result<EditOutcome, EditError>;
    pub fn split_module(&mut self, op: SplitModuleOp) -> Result<EditOutcome, EditError>;
    pub fn merge_modules(&mut self, op: MergeModulesOp) -> Result<EditOutcome, EditError>;
    pub fn create_module(&mut self, op: CreateModuleOp) -> Result<EditOutcome, EditError>;
    pub fn move_module(&mut self, op: MoveModuleOp) -> Result<EditOutcome, EditError>;
    pub fn lift_to_crate(&mut self, op: LiftToCrateOp) -> Result<EditOutcome, EditError>;
    pub fn lower_to_module(&mut self, op: LowerToModuleOp) -> Result<EditOutcome, EditError>;
}
```

### New `EditError` variants

```rust
SignatureSynthesisRefused { fn_id: NodeId, callsite_count: usize },
CargoTomlConflict { crate_name: String, reason: String },
ExtractFunctionScopeCapture { unresolved: Vec<String> },
ExtractTraitMethodsNotInherent { stray: Vec<NodeId> },
ItemsNotInModule { module: NodeId, stray: Vec<NodeId> },
ModuleTreeConflict { parent: NodeId, name: String, reason: String },
InlineRecursiveFn { fn_id: NodeId },
```

### EditOutcome extension

```rust
pub struct EditOutcome {
    pub file_edits: Vec<FileEdit>,
    pub file_moves: Vec<FileMove>,
    pub cargo_edits: Vec<CargoEdit>,    // NEW; usually empty
    pub class: EditClass,
    pub affected_items: Vec<NodeId>,
    pub checkpoint: Checkpoint,
}

pub struct CargoEdit {
    pub manifest_path: PathBuf,
    pub new_contents: String,            // toml_edit-rendered, format-preserved
}

pub enum EditClass {
    Body, SigOrVis, ItemAddRemove, ModuleTree, Macro,
    Cargo,                               // → COLD REBUILD
}
impl EditClass {
    pub fn is_full_rebuild(self) -> bool { matches!(self, Self::Cargo | Self::Macro) }
}
```

## Step-by-step implementation

Each verb's skeleton:
```
1. take Checkpoint
2. resolve/validate → EditError on bad input
3. compute_effects()  → FileEdits + FileMoves + CargoEdits + EditClass
4. apply_to_disk()
5. host.apply_edits()
6. on error → Checkpoint::restore() → bubble EditError
7. else → return EditOutcome
```

### P1.5c — `modify_signature`

**Step 1 — Resolve + validate.** Require `item_kind ∈ {Function, Method, AssocFunction}`. Fetch current sig via `snap.function_signature(op.target)?`. VERIFY: `modify_sig_rejects_non_fn`.

**Step 2 — Diff old vs new sig.** `SignatureDelta`: pair by position, refine by name where positions don't match. Same-name+different-ty → `retyped`. New name not in old → `added`. Old name not in new → `removed`. Same set different order → `reordered = Some(permutation)`. VERIFY: `diff_detects_add_remove_rename_reorder`.

**Step 3 — Rewrite the function declaration.** Read file, `syn::parse_file(&src)?`. Locate `ItemFn`/`ImplItemFn`/`TraitItemFn` by span (via `visit_mut::VisitMut`). Replace its `syn::Signature` with translated `FunctionSignature`:
- `is_async` → `sig.asyncness`.
- `self_param` → `sig.inputs.first_mut()` set to `FnArg::Receiver`.
- `params` → `FnArg::Typed(PatType { pat: Ident, ty: syn::parse_str(&p.ty)?, .. })`.
- `return_type` → `syn::parse_str(&format!("-> {}", new_sig.return_type))?`.
- `generics` → `syn::parse_str(&render_generics(&new_sig.generics))?`.

Re-render: `let new_src = prettyplease::unparse(&file);`. One `FileEdit { path, new_contents: new_src }`. VERIFY: `modify_sig_rewrites_decl_only`.

**Step 4 — Find all callsites of OLD sig.** `let sites = snap.who_calls(op.target)?;` + `snap.usages_of(op.target)?` for non-fn-body refs. Union is the rewrite set. Tag with body-call vs const-ref (Default-substitution only valid in body context). VERIFY: `modify_sig_collects_all_callsites`.

**Step 5 — Rewrite each callsite.** Group by file; descending byte-offset order. For each file: parse `syn::File`; for each site (visit_mut to find topmost `ExprCall`/`ExprMethodCall` containing the offset); manipulate `call.args`:
- **Reorder:** permute `args` per `perm`.
- **Remove:** `args.remove(i)` for each `removed` (descending).
- **Add:** for each `(j, new_param)`, build `syn::Expr` per `callsite_fill`:
  - `Todo` → `syn::parse_str::<syn::Expr>(&format!(r#"todo!("filled in by modify_signature: {}")"#, new_param.name))?`.
  - `Default` → small allowlist `{i*, u*, f*, bool, String, Vec<_>, Option<_>, HashMap<_,_>}` or `: Default` bound in `new_sig.generics`; else fall back to `Todo`.
  - `Refuse` → `EditError::SignatureSynthesisRefused { fn_id, callsite_count }`.
  - `ClosureBuilder(f)` → `syn::parse_str(&f(&ctx))?`.

Insertion order: removals/reorder first, then insertions in increasing index. Re-render per touched file; emit `FileEdit`. VERIFY: `modify_sig_add_param_inserts_todo`, `_remove_param_drops_arg`, `_reorder_perm_correct`.

**Step 6 — Classify + apply.** `EditClass::SigOrVis` (D2 expands to editing crate + reverse-deps). `Crud::take_checkpoint()` → `Crud::apply_file_edits(edits, class)` → host writes + LMDB patch + return `EditOutcome`. On error → `Checkpoint::restore()`.

### P1.5d — `extract_function`

**Step 7 — Parse + locate range.** Resolve `source_fn` (is_callable). Open file, `syn::parse_file(&src)?`, walk to `ItemFn` matching span; find contiguous statement sub-slice covering `op.byte_range`. Fail if crosses statement boundary → `EditError::InvalidByteRange`. VERIFY: `extract_fn_rejects_mid_statement`.

**Step 8 — Capture analysis via RA.** Need `Semantics`; warm host lives behind `WorkspaceHost::semantics()`. Compute `TextRange` from `op.byte_range` via line index; `let scope = sema.scope_at_offset(file_id, range.start())?;`. Collect every `syn::Ident` inside slice (via `syn::visit::Visit`); filter to those resolving (`scope.process_all_names`) to `ScopeDef::Local(Local)`. For each captured local: `Local::ty(db)` → `Type::display(db).to_string()` → param type. Decide `&T` / `&mut T` / `T` from `Local::is_mut(db)` + whether lifted code mutates (re-walk: `=` LHS, `&mut`, method call on `&mut self`). Non-local free idents (paths, use-imports, macro names) left alone. Sanity-check against `op.captured_locals` if non-empty; mismatch → `EditError::ExtractFunctionScopeCapture { unresolved }`. VERIFY: `extract_fn_captures_locals_with_correct_mut`.

**Step 9 — Synthesize + splice new fn.** Build `syn::ItemFn`: visibility, ident, inputs from captured locals as `&[mut] <ty>`, output from tail expression type if any. Insert `file.items.insert(idx + 1, ItemFn(...))`. Replace `byte_range` with `let _ = new_fn_name(&mut captured_a, captured_b, ...);` (or just call if `()` return, or `let r = ...` if tail-expr). Re-render. VERIFY: `extract_fn_emits_callable_new_fn`.

**Step 10 — Classify + apply.** `EditClass::ItemAddRemove`. New fn private by default → no reverse-dep impact. VERIFY: `extract_fn_full_round_trip`.

### P1.5d — `extract_trait`

**Step 11 — Validate method subset.** Require `parent.item_kind ∈ {Struct, Enum, Union}`. For each method: `parent_id == op.source_struct` and `item_kind == Some(Method)`. Stray → `ExtractTraitMethodsNotInherent`. Locate inherent `ItemImpl` via `syn` (trait_ is None, self_ty resolves to struct).

**Step 12 — Emit trait + impl.** Build `syn::ItemTrait { vis, ident, items: method_subset.map(|m| TraitItem::Fn(TraitItemFn { sig, default: None })) }` (signature only, no body). Build `syn::ItemImpl { trait_: Some(TypePath(trait_name)), self_ty: struct_path, items: ImplItem::Fn(ImplItemFn { sig, block: lifted body }) }`. Remove method nodes from inherent impl; prepend new trait + impl in file (or new `<mod>/<trait_snake>.rs` if `place_trait_inline == false`). VERIFY: `extract_trait_moves_methods_preserving_bodies`.

**Step 13 — Classify + apply.** `EditClass::ItemAddRemove` if private; `SigOrVis` if pub (changes reverse-dep import resolution).

### P1.5d — `inline`

**Step 14 — Fetch body + callsites.** Resolve target_fn (callable). Locate `ItemFn`/`ImplItemFn`; capture `block: syn::Block` + `sig.inputs` (param names). Reject if any `&mut` ref with conditionally-evaluated param read; reject recursive: `snap.recursive_callers_count(target_fn, 1)?.callers > 0 && body_calls_itself` → `InlineRecursiveFn`. VERIFY: `inline_rejects_recursive`.

**Step 15 — Determine callsite set.** `InlineAll` → `snap.who_calls + usages_of(call-shaped)`. `InlineSites(usage_ids)` → load each via `usages_by_id`.

**Step 16 — Per-callsite substitution with arg-lifting (no double-eval).** Descending byte order per file. For each callsite, build:
```
{
    let __arg_0 = <expr_0>;
    let __arg_1 = <expr_1>;
    ...
    <body_with_param_names_replaced_by___arg_n>
}
```
Param substitution via `visit_mut`: any `syn::Path` with single segment `param_n` → `Ident::new("__arg_n", ...)`. Self handling: method calls get `__arg_self = <receiver>`; receiver was `&self`/`&mut self` → prepend `&` or `&mut`. Replace callsite expression with this block. VERIFY: `inline_substitutes_args_no_double_eval`, `_method_call_self_handling`.

**Step 17 — Delete fn if InlineAll.** After splice, count remaining usages; for safety `delete = (policy == InlineAll)`. Remove `ItemFn` from file. EditClass: `ItemAddRemove` if deleting, else `Body`. VERIFY: `inline_all_deletes_fn_when_no_remaining_callers`.

### P1.5e — `create_module`

**Step 18 — Validate.** Parent must be Module or Crate. Name regex `^[a-z_][a-z0-9_]*$`. Parent file: for Module = `parent.file`; for Crate = root module's file. Decide new path: `<dir>/<name>.rs` or `<dir>/<name>/mod.rs` per `use_mod_rs`. Conflict → `ModuleTreeConflict`.

**Step 19 — Emit files.** `FileMove { from: None, to: new_path, contents: "// new module\n" }`. Append `pub mod <name>;` (or `mod <name>;`) to parent file via `syn::parse_file` + `file.items.push(ItemMod { ... })` + `prettyplease`.

**Step 20 — Move initial items.** Cut from current file, paste into new module file as part of same edit batch (NodeIds for new module don't exist until re-extract). `EditClass::ModuleTree`. Apply. VERIFY: `create_module_with_initial_items_round_trip`.

### P1.5e — `split_module`

**Step 21 — Validate.** Union of `splits[*].items` is subset of source_module's current items (via `children_by_parent`). Stray → `ItemsNotInModule`. Names unique, not colliding with existing children of `source_module.parent_id`.

**Step 22 — Per-split create + move.** For each `ModuleSplit`: in-process `create_module` logic with `parent = source_module.parent_id`, `name`, `items`, `use_mod_rs = false`. If `keep_reexport`: append `pub use <new_name>::*;` to source file.

**Step 23 — Cleanup re-exports.** Walk source_module file; prune `pub use <child>::X` lines pointing to moved items (unless `keep_reexport`). `EditClass::ModuleTree`. Apply. VERIFY: `split_module_three_ways_items_partitioned`.

### P1.5e — `merge_modules`

**Step 24 — Validate.** All `sources` + `dest` share `parent_id`. Item-name collisions → `ModuleTreeConflict`.

**Step 25 — Move items into dest.** For each source: parse `<source>.rs`, take `file.items`, paste into dest file. Rewrite import-cycles inside merged module.

**Step 26 — Delete source files + `mod` decls.** `FileMove { from: source_file, to: None }`; remove `mod <source>;` from parent.

**Step 27 — Workspace-wide `use` rewrite.** For every workspace file: `use <parent>::<source_name>::X` → `use <parent>::<dest_name>::X`. Via `SemanticService`-style mechanism + `syn`-based prefix substitution. `EditClass::ModuleTree`. Apply. VERIFY: `merge_modules_collapses_two_into_one`.

### P1.5e — `move_module`

**Step 28 — Validate.** source_module is Module (not Crate, not root module). new_parent is Module or Crate. If same parent + no rename → noop. Cycle check via `module_tree` descendant walk. Name collision in new_parent's children → `ModuleTreeConflict`.

**Step 29 — Compute file move.** Old path: `source_module.file`. New path: under `<dir of new_parent's file>/<new_name>.rs` (preserve mod.rs style). `FileMove { from: old, to: new }` plus directory moves if children exist.

**Step 30 — Update `mod` declarations.** Remove `mod <old>;` from old parent file; add `mod <new>;` to new parent file (via `syn` walk).

**Step 31 — Rewrite all `use` paths workspace-wide.** For each `.rs` (Merkle-filtered to those importing the moved module): parse, walk `UseTree`, prefix-replace `<old_qualified>` → `<new_qualified>`. `EditClass::ModuleTree`. Apply. VERIFY: `move_module_updates_all_uses`.

### P1.5e — `lift_to_crate` (Cargo surgery — FULL REBUILD)

**Step 32 — Validate.** source_module.crate_id exists; not lifting root module. new_crate_name kebab-case + not in workspace members (read root `Cargo.toml` via `toml_edit`). edition ∈ {"2021", "2024"}.

**Step 33 — Generate new crate skeleton.** New dir `crates/<new_crate_name>/`. `Cargo.toml`:
```toml
[package]
name = "<new_crate_name>"
version = "0.1.0"
edition = "<op.edition>"

[dependencies]
```
`src/lib.rs` = source_module's current file contents (verbatim). If source_module is dir-style (`mod.rs`), recursively copy subtree. Emit `CargoEdit` + `FileMove` per copied file.

**Step 34 — Compute + inject deps.** `cargo metadata --format-version 1 --no-deps` via `Command`. Walk lifted files; collect `use <name>::...` for each `<name>` that's a workspace or registry crate. Look up version in source crate's `Cargo.toml`; path-dep → `<name> = { path = "../<name>" }`; registry → copy version as-is. Apply via `toml_edit::DocumentMut::insert("dependencies", ...)`.

**Step 35 — Update workspace `Cargo.toml`.** `members.push(format!("crates/{}", new_crate_name))`. If broadly usable, add to `[workspace.dependencies]`. Emit `CargoEdit`.

**Step 36 — Update source crate's `Cargo.toml`.** Add `<new_crate_name> = { workspace = true }` (or path-form) to `[dependencies]`. Emit `CargoEdit`.

**Step 37 — Rewrite import paths workspace-wide.** Replace `<src_crate>::<source_module_path>::X` → `<new_crate_name>::X`. If `keep_facade`: replace source module file contents with `pub use <new_crate_name>::*;` so existing internal callers continue to work.

**Step 38 — Classify + apply (slow path).** `EditClass::Cargo` → `is_full_rebuild() == true`. `Crud::apply_file_edits` routes to cold-rebuild path: write all files, close warm host, delete working LMDB, re-run `build_and_persist`. Checkpoint records jj op id + copy of pre-edit Cargo manifests (LMDB undo log doesn't cover them). VERIFY: `lift_to_crate_full_rebuild_succeeds`, `_workspace_compiles_after`.

### P1.5e — `lower_to_module` (inverse — also FULL REBUILD)

**Step 39 — Validate.** source_crate is workspace lib. dest_parent_module in different crate. If `!keep_facade`, at most one path-dep consumer.

**Step 40 — Copy code in.** Read `crates/<src>/src/lib.rs` → new module body. Recursively walk subtree → reproduce under `<dest_parent>/<new_module_name>/`.

**Step 41 — Update consumer manifests.** For every crate depending on `<src>` (per cargo metadata): remove dep; replace `<src>::X` → `<dest_crate>::<dest_path>::<new_module_name>::X`.

**Step 42 — Remove from workspace.** Root `Cargo.toml`: remove `crates/<src>` from members; remove from `[workspace.dependencies]`. `FileMove` deleting `crates/<src>/` directory.

**Step 43 — Apply (slow).** Same cold rebuild as Step 38. VERIFY: `lower_to_module_round_trip`.

## Tests

(`crates/rmc-crud/tests/`)

**Per-verb behavioral:**
- `modify_sig_add_param_inserts_todo` — add `x: u32` as 1st param to fn with 3 callers; every callsite has `todo!("filled in by modify_signature: x")` at position 0.
- `modify_sig_remove_param_drops_arg` — remove `y` from `fn f(x, y, z)`; `f(1, 2, 3)` becomes `f(1, 3)`.
- `modify_sig_rename_param_no_callsite_change`.
- `modify_sig_reorder_perm_correct`.
- `modify_sig_retype_param_no_callsite_change` (cargo check may fail; that's the agent's problem).
- `modify_sig_refuse_policy_errors` — `Refuse` with add-param → `SignatureSynthesisRefused`; no files modified, checkpoint reverted.
- `modify_sig_default_fallback_to_todo_for_unknown_type`.
- `modify_sig_closure_builder_called_per_site`.
- `modify_sig_method_call_handling`.
- `modify_sig_const_initializer_ref`.
- `extract_fn_captures_locals` — block referencing `i: i32` and `s: &str` → new fn `fn new_fn(i: i32, s: &str)`.
- `extract_fn_captures_mut_borrow` — `*counter += 1` → `counter: &mut i32`.
- `extract_fn_handles_tail_expression`.
- `extract_fn_rejects_mid_statement`.
- `extract_fn_uncaptured_outer_ident_ignored`.
- `extract_fn_round_trips_compile`.
- `extract_trait_moves_methods` — `impl Foo { fn a, fn b, fn c }`; extract `[a, c]` into `trait Bar`; inherent has only `b`, new `impl Bar for Foo { fn a, fn c }`.
- `extract_trait_emits_trait_with_correct_visibility`.
- `extract_trait_separate_file`.
- `extract_trait_rejects_methods_from_other_impls`.
- `inline_substitutes_args` — `f(a + b)` for `fn f(x) { dbg!(x); dbg!(x); }` → `{ let __arg_0 = a + b; dbg!(__arg_0); dbg!(__arg_0); }`.
- `inline_method_call_self_handling`.
- `inline_all_deletes_fn_when_no_remaining_callers`.
- `inline_sites_subset_preserves_fn`.
- `inline_rejects_recursive`.
- `create_module_empty`.
- `create_module_with_initial_items_round_trip`.
- `create_module_mod_rs_style`.
- `create_module_name_collision_errors`.
- `split_module_three_ways`.
- `split_module_keep_reexport_preserves_external_use`.
- `merge_modules_collapses_two_into_one`.
- `merge_modules_name_collision_errors`.
- `move_module_updates_all_uses`.
- `move_module_rejects_cycle`.
- `lift_to_crate_full_rebuild`.
- `lift_to_crate_dep_inference` — module uses `serde::Serialize`; new crate's `Cargo.toml` has `serde = "..."`.
- `lift_to_crate_keep_facade_preserves_external_callers`.
- `lift_to_crate_rejects_duplicate_name`.
- `lower_to_module_inverse_of_lift`.

**Differential / property tests:**
- `differential_apply_vs_cold<Verb>` — for every verb + fixture: apply via warm host; rebuild cold from post-apply source; LMDB byte-equal on reward-bearing fields.
- `checkpoint_restore_roundtrip<Verb>` — take, apply, restore; source byte-equal, edit_seq reverted, queries equal.

**Failure-mode tests:**
- `modify_sig_partial_failure_reverts_atomically` — inject write error mid-apply; restore leaves no half-written files.
- `lift_to_crate_cargo_failure_reverts` — corrupt workspace `Cargo.toml` post-lift; restore reinstates prior manifest.

## Open decisions / risks

- **`CallsiteFill` default = `Todo`.** Picked over `Default` (silent semantic change) and `Refuse` (too aggressive). `Todo` keeps workspace type-checking, panics at runtime if reached, greppable.
- **`syn` 2 + `prettyplease`, not RA's `TextEdit`.** RA's TextEdit perfect for single-symbol edits; verbs here do structural mutation (whole sigs, blocks, ItemFns). Downside: prettyplease re-formats whole files — accepted (rustfmt normalizes anyway).
- **Cargo.toml lib: `toml_edit`.** `DocumentMut` preserves comments + ordering. `cargo_toml` round-trips lossily.
- **Capture analysis depends on warm RA host.** If unavailable → `EditError::HostUnavailable`. P0.2 is critical path anyway.
- **`inline` always lifts each arg.** Even if param used once — small cost, large correctness win (no double-eval, no precedence surprises).
- **Multi-file atomicity = Checkpoint's job.** Every verb calls `take_checkpoint()` BEFORE first write; on ANY failure → `restore()`. D4 covers source + graph + RA host; `EditClass::Cargo` extends to capture pre-edit Cargo manifest contents.
- **`lift_to_crate` / `lower_to_module` are HIGH-COST.** Unambiguous `EditClass::Cargo` → full cold rebuild → tens of seconds. Episode runner exposes cost in action-space metadata; agent's RL signal accounts for it. Alternative: gate behind `declare_done`. Current: emit via regular Crud API, mark `EditClass::Cargo`, let runner decide.
- **`modify_signature` cannot eliminate every false positive in callsite detection.** RA-unresolvable calls (`dyn Trait` over external trait, generic `F: Fn(..)`) won't appear in `who_calls`. Silently break post-edit. **Mitigation:** pair every `modify_signature` with cargo gate (P1.7) — fail → checkpoint reverts.
- **`extract_trait` self-call subtleties.** `impl Trait for Foo { fn a(...) { self.b(...) } }` works if `b` stays on Foo. Trait default method shadowing may compile-but-mean-something-subtly-different. Cargo gate catches compile-break subset.
- **`split_module` re-exports.** Default `keep_reexport = false` requires rewriting external users. Partial `splits` (not covering every item) leaves unsplit items in source_module (no error). Underspecified by design.
- **Determinism.** Every verb's source rewriting is deterministic: `syn::parse_file` deterministic; `prettyplease::unparse` deterministic; `toml_edit` round-trips deterministically; `who_calls` results iterated in LMDB-sorted order (P0.1 guarantee).


---

# Section I — P1.4 Counterfactual Simulator + P1.6 Write-Time Gates

## Overview

P1.4 and P1.6 are co-designed because both consume the D2 dirty-set classification produced by the apply engine and both run *before* the heavy cargo gate (P1.7). The simulator (P1.4) is a dry-run mode of the CRUD verbs from P1.5 — it must share apply's *exact* effect-computation logic, or it lies and the agent reasons against fiction. The gates (P1.6) are the pre-commit hard/soft filter that runs existing audits (`fn_body_audit`, `unsafe_audit`, `recursion_check`, `mut_static_audit`, `derive_audit`, `missing_docs_audit`, `channel_capacity_audit`, `forbidden_dependency_check`) scoped to the D2 dirty set.

The two slices share infrastructure (a `Baseline` snapshot of per-item metrics captured once per episode, dirty-set audit wrappers, threshold/allowlist config) and sit structurally between `rmc-crud` (P1.5) and `rmc-reward` (P1.7). Gates live in their own crate `rmc-gates` so they're independently testable, callable from `Crud::simulate`, `Crud::apply`, and any future REPL/query tool.

**Source-plan key insight (P1.4):** *"simulate must share apply's exact logic or it lies — hence built after, as a mode of, P1.5"*. Plan: factor `Crud::apply` into `compute_effects() + persist()`; `simulate()` is the `compute_effects()`-only path.

## New modules / files

- `crates/rmc-crud/src/simulate.rs` — `Crud::simulate(op) -> SimulateOutcome`; assembles cascade preview + token-cost estimate; delegates to `effects::compute_effects` + `GateRunner::evaluate`.
- `crates/rmc-crud/src/effects.rs` — `Effects` struct + free `compute_effects(host, op)` dispatching per verb.
- `crates/rmc-crud/src/apply.rs` (refactor of P1.5 verbs) — each verb becomes `pub(crate) fn persist(host, effects, checkpoint) -> Result<EditOutcome>`. `Crud::apply` = `compute_effects → GateRunner::evaluate → if hard refusal → bail; else persist → commit`.
- `crates/rmc-gates/Cargo.toml` — new crate. Deps: `rmc-graph`, `rmc-config`, `petgraph`, `toml`, `serde`, `anyhow`, `thiserror`.
- `crates/rmc-gates/src/lib.rs` — `GateRunner`, `GateOutcome`, `RefusalReason`, `RefusalCode`, `Severity`.
- `crates/rmc-gates/src/thresholds.rs` — `GateThresholds` + TOML loader.
- `crates/rmc-gates/src/allowlist.rs` — `ForbiddenDepAllowlist { rules: Vec<ForbiddenDependencyRule> }`, reusing `rmc_graph::query::model::ForbiddenDependencyRule`.
- `crates/rmc-gates/src/baseline.rs` — `Baseline::capture(snap, loaded)` — episode-start per-item complexity, LOC, had_unsafe, crate_dep_set.
- `crates/rmc-gates/src/audits.rs` — dirty-set audit wrappers.
- `crates/rmc-gates/src/complexity.rs` — `compute_complexity(body_text)` via `ra_ap_syntax` walking.
- `crates/rmc-gates/src/cycle.rs` — `detect_new_module_cycles`, `detect_new_crate_cycles` using `petgraph::algo::tarjan_scc`.
- `crates/rmc-gates/src/dirty_dep_filter.rs` — filter `crate_edges` by dirty set; feed `forbidden_dependency_check`.
- `gates.toml` at workspace root — sample config with all thresholds at defaults + starter `[forbidden_dependencies]` table. Read-only to agent.

## Type definitions

```rust
// crates/rmc-crud/src/effects.rs

pub struct Effects {
    pub source_edits: Vec<FileEdit>,
    pub edit_class: EditClass,
    pub estimated_affected_items: Vec<NodeId>,
    pub estimated_graph_diff: GraphDiffSummary,
    pub would_refuse: Vec<RefusalReason>,
}

pub struct FileEdit { pub file: String, pub range: (u32, u32), pub replacement: Vec<u8> }

pub struct GraphDiffSummary {
    pub nodes_added: usize, pub nodes_removed: usize,
    pub bindings_added: usize, pub bindings_removed: usize,
    pub usages_added: usize, pub usages_removed: usize,
}
```

```rust
// crates/rmc-crud/src/simulate.rs

pub struct SimulateOutcome {
    pub effects: Effects,
    pub cascade_preview: Vec<CascadeStep>,
    pub estimated_token_cost: usize,
}

pub struct CascadeStep { pub op_kind: String, pub node: NodeId, pub reason: String }
```

```rust
// crates/rmc-gates/src/thresholds.rs

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GateThresholds {
    pub max_complexity: u32,          // default 20 (cyclomatic, McCabe)
    pub max_params: u32,              // default 7
    pub max_fn_loc: u32,              // default 60
    pub max_nesting: u32,             // default 4
    pub max_unwrap_per_fn: u32,       // default 0 in production crates
    pub forbid_unsafe_introduce: bool,// default true
    pub forbid_new_cycles: bool,      // default true
    pub soft_complexity: u32,         // default 12
    pub soft_fn_loc: u32,             // default 40
    pub soft_unwrap: u32,             // default 0
}

impl Default for GateThresholds { fn default() -> Self { /* above */ } }
```

```rust
// crates/rmc-gates/src/lib.rs

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RefusalCode {
    ComplexityTooHigh, FnTooLong, TooManyParams, NestingTooDeep,
    TooManyUnwraps, UnsafeIntroduced,
    ModuleCycleIntroduced, CrateCycleIntroduced,
    ForbiddenDependency, MissingDocsOnPub,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Severity { Hard, Soft }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RefusalReason {
    pub code: RefusalCode,
    pub severity: Severity,
    pub item: NodeId,
    pub threshold: Option<u32>,
    pub actual: Option<u32>,
    pub message: String,         // includes file:span + "what to do" hint
}

pub struct GateRunner<'a> {
    pub snap: &'a OpenedSnapshot,
    pub loaded: &'a LoadedWorkspace,
    pub thresholds: GateThresholds,
    pub allowlist: ForbiddenDepAllowlist,
    pub baseline: &'a Baseline,
}

impl<'a> GateRunner<'a> {
    pub fn evaluate(&self, dirty: &[NodeId], effects: &Effects) -> Result<GateOutcome>;
}

pub struct GateOutcome {
    pub hard: Vec<RefusalReason>,
    pub soft: Vec<RefusalReason>,
    pub passed: bool,           // == hard.is_empty()
}
```

```rust
// crates/rmc-gates/src/baseline.rs

pub struct Baseline {
    pub per_item_complexity: HashMap<NodeId, u32>,
    pub per_item_loc: HashMap<NodeId, u32>,
    pub per_item_unwraps: HashMap<NodeId, u32>,
    pub had_unsafe: HashMap<NodeId, bool>,
    pub crate_dep_set: HashSet<(NodeId, NodeId)>,
    pub module_parent_edges: HashSet<(NodeId, NodeId)>,
}
```

## Step-by-step implementation

### P1.4 — Counterfactual Simulator

1. **Refactor every P1.5 verb into `compute_effects` + `persist`.** WHERE: `crates/rmc-crud/src/{body,move_item,delete,modify_signature,...}.rs`. Template: `modify_body`. Split into:
   ```rust
   pub(crate) fn compute_effects(host: &WorkspaceHost, op: &ModifyBodyOp) -> Result<Effects>;
   pub(crate) fn persist(host: &mut WorkspaceHost, effects: Effects, checkpoint: &mut CheckpointBuilder) -> Result<EditOutcome>;
   ```
   `compute_effects` takes `&WorkspaceHost` (immutable — no `set_file_text`, no LMDB writes); computes byte range via `OpenedSnapshot::span_index()` + `Node.span`; builds `Effects { source_edits, edit_class, estimated_affected_items: vec![op.target], estimated_graph_diff: GraphDiffSummary::default(), would_refuse: vec![] }`. `persist` does RA `set_file_text` + scoped re-extract + LMDB diff-patch. `Crud::apply` becomes:
   ```rust
   pub fn apply<O: CrudOp>(&mut self, op: O) -> Result<EditOutcome> {
       let effects = O::compute_effects(&self.host, &op)?;
       let dirty = effects.estimated_affected_items.clone();
       let gate_outcome = self.gates.evaluate(&dirty, &effects)?;
       if !gate_outcome.passed { return Err(CrudError::Refused(gate_outcome.hard)); }
       let mut checkpoint = self.host.begin_checkpoint()?;
       let edit_outcome = O::persist(&mut self.host, effects, &mut checkpoint)?;
       checkpoint.commit()?;
       Ok(edit_outcome)
   }
   ```
   DEPENDS: P0.2 `apply_edits`, D4 `Checkpoint`, D2 `EditClass`, P1.5 verb impls. VERIFY: every verb's existing apply test passes after split.

2. **`Crud::simulate(op) -> Result<SimulateOutcome>`.** WHERE: `simulate.rs`.
   ```rust
   pub fn simulate<O: CrudOp>(&self, op: O) -> Result<SimulateOutcome> {
       let mut effects = O::compute_effects(&self.host, &op)?;
       let dirty = effects.estimated_affected_items.clone();
       let gate_outcome = self.gates.evaluate(&dirty, &effects)?;
       effects.would_refuse = gate_outcome.hard.iter().chain(gate_outcome.soft.iter()).cloned().collect();
       let cascade_preview = predict_cascade(&self.host, &effects)?;
       let estimated_token_cost = estimate_context_view_cost(&self.host, &effects)?;
       Ok(SimulateOutcome { effects, cascade_preview, estimated_token_cost })
   }
   ```
   Same `compute_effects` apply uses — by construction simulate cannot diverge.

3. **Cascade preview.** WHERE: `simulate.rs::predict_cascade`. Walks D2's affected-set graph. Per `effects.edit_class`:
   - `BodyOnly` → no cascade.
   - `SignatureOrVis` → for each NodeId in reverse-deps (from `who_uses` + `who_imports`), emit `CascadeStep { op_kind: "re_resolve_usage", node, reason: "signature changed" }`.
   - `ItemAddRemove` → `fix_use_path` per consumer module.
   - `ModuleTree` → `fix_use_path` for every `Binding` whose `target` lies under moved subtree.
   - `Macro` / `Cargo` → single coarse `op_kind: "full_reextract"`.
   DEPENDS: D2 classification; `who_uses`, `who_imports`, `usages_of`, `imports_of`, `re_export_chain`. VERIFY: `modify_body` cascade empty; `move` of `pub fn` with N consumers → `cascade_preview.len() >= N`.

4. **Fold GateRunner output into `would_refuse`.** Wired in Step 2.

5. **Token-cost estimate.** WHERE: `simulate.rs::estimate_context_view_cost`. For each NodeId in `estimated_affected_items`, ask P1.1's `Navigator::cost_of_slot(node)` and sum. Items being created (don't exist yet) approximate by `source_edits` byte length / 4.

6. **Differential test.** WHERE: `crates/rmc-crud/tests/simulate_eq_apply.rs`. For every verb: `simulate(op).effects == apply(op).recorded_effects` (apply re-runs `compute_effects` internally; instrument via `tracing::trace` or `EffectsRecorder`). VERIFY: `cargo test -p rmc-crud --test simulate_eq_apply` green for every verb.

### P1.6 — Write-Time Gates

7. **Define `GateThresholds` defaults.** WHERE: `thresholds.rs`. Defaults per table at top.
   - `max_complexity` 20: matches rust-guidelines §9; `analyze_complexity` reports cyclomatic.
   - `max_params` 7: matches `function_signature.params.len()` audit.
   - `max_fn_loc` 60: matches `fn_body_audit` informal ceiling.
   - `max_nesting` 4: matches rust-guidelines §3.
   - `max_unwrap_per_fn` 0: `fn_body_audit` flags any unwrap.
   - `forbid_unsafe_introduce` true: `unsafe_audit` baseline.
   - `forbid_new_cycles` true: hard always.
   - Softs at ~half/2/3 of hard ceilings.
   VERIFY: `gate_thresholds_defaults`.

8. **TOML loader.** WHERE: `thresholds.rs::load_from_workspace_root`. `<workspace>/gates.toml`; `toml::from_str::<GateThresholds>` over `#[serde(default)]`. Missing file → `GateThresholds::default()`. Sample:
   ```toml
   max_complexity = 20
   max_params = 7
   max_fn_loc = 60
   max_nesting = 4
   max_unwrap_per_fn = 0
   forbid_unsafe_introduce = true
   forbid_new_cycles = true
   soft_complexity = 12
   soft_fn_loc = 40
   soft_unwrap = 0

   [forbidden_dependencies.config_to_engine]
   consumer = "rmc-config"
   producer = "rmc-engine*"
   severity = "error"
   message = "Config must not import engine — layering violation"

   [forbidden_dependencies.graph_to_server]
   consumer = "rmc-graph"
   producer = "rmc-server*"
   severity = "error"
   ```
   VERIFY: `loader_returns_defaults_on_missing_file`, `loader_parses_sample_gates_toml`.

9. **Allowlist loader.** WHERE: `allowlist.rs::load_from_workspace_root`. Parse `[forbidden_dependencies]` table; merge with hard-coded baseline:
   ```rust
   const BASELINE_RULES: &[(&str, &str, &str)] = &[
       ("rmc-config", "rmc-engine*", "config must not import engine"),
       ("rmc-config", "rmc-graph*",  "config must not import graph"),
       ("rmc-graph",  "rmc-server*", "graph must not import server"),
       ("rmc-graph",  "rmc-engine*", "graph must not import engine"),
       ("rmc-engine", "rmc-server*", "engine must not import server"),
   ];
   ```
   Agent has NO verb mutating `gates.toml`. VERIFY: `allowlist_includes_baseline_rules`, `_merges_toml_rules`.

10. **Baseline capture.** WHERE: `baseline.rs::Baseline::capture`. Called once at episode start (P1.8 `EpisodeRunner::begin`).
    1. **`per_item_complexity`**: walk every `Function | Method | AssocFunction` via `workspace_stats` + `crate_types(crate, kinds, ...)`; for each fn read body source bytes via `Node.file + Node.span`; call `compute_complexity(body_text).cyclomatic`.
    2. **`per_item_loc`**: count `\n` in body.
    3. **`per_item_unwraps`**: run `fn_body_audit(loaded, snap, FnBodyAuditOpts { patterns: HashSet::from(["unwrap", "expect"]), skip_test_fns: true, .. })`; aggregate per `FnBodyFinding.target`.
    4. **`had_unsafe`**: run `unsafe_audit(loaded)`; per `UnsafeFinding.enclosing_function`, set `had_unsafe[node] = true`.
    5. **`crate_dep_set`**: `crate_edges()` → `(consumer_id, producer_id)` via `lookup_by_qualified_name`.
    6. **`module_parent_edges`**: walk `module_tree("<crate>", None)` → `(parent, child)` Module pairs.
    Cached in `EpisodeState`. DEPENDS: `workspace_stats`, `crate_types`, `crate_edges`, `unsafe_audit`, `module_tree`, `fn_body_audit`, `compute_complexity`. VERIFY: `baseline_captures_all_local_functions`, `_per_item_unwraps_matches_fn_body_audit`.

10b. **`compute_complexity` helper.** WHERE: `complexity.rs`. Parse fn body via `ra_ap_syntax::SourceFile::parse` (body-only, cheap); walk syntax tree counting:
    - **cyclomatic**: +1 per `if`, `else if`, `match arm`, `while`, `for`, `loop` with `break`, `&&`, `||`, `?`, `return` in conditional branch.
    - **nesting**: max stack depth across `BlockExpr` descendants.
    - **fn_loc**: count `\n` in body.
    - **unwrap_count**: count `MethodCallExpr` whose `name_ref == "unwrap" | "expect"`.
    - **param_count**: from `function_signature(node).params.len() + self_param.is_some()`.
    Only new metric implementation; everything else delegates to existing audits. VERIFY: unit tests on hand-written fixtures.

11. **`GateRunner::evaluate(dirty, effects)`.** WHERE: `lib.rs`.
    ```rust
    pub fn evaluate(&self, dirty: &[NodeId], effects: &Effects) -> Result<GateOutcome> {
        let mut hard = Vec::new();
        let mut soft = Vec::new();
        // 1. Per-fn metrics on each dirty function.
        for &node in dirty {
            let item = self.snap.node(&self.snap.read_txn()?, node)?;
            let Some(item) = item else { continue };
            let Some(ItemKind::Function | ItemKind::Method | ItemKind::AssocFunction) = item.item_kind else { continue };
            let body = read_body_post_edit(&item, effects)?;
            let m = compute_complexity(&body);
            // Complexity hard/soft.
            if m.cyclomatic > self.thresholds.max_complexity {
                hard.push(refusal(RefusalCode::ComplexityTooHigh, Severity::Hard, node,
                                  self.thresholds.max_complexity, m.cyclomatic, &item));
            } else if m.cyclomatic > self.thresholds.soft_complexity {
                soft.push(refusal(RefusalCode::ComplexityTooHigh, Severity::Soft, node,
                                  self.thresholds.soft_complexity, m.cyclomatic, &item));
            }
            // Fn LOC + Params + Nesting (similar pattern).
            // Unwraps: NEW count vs baseline only.
            let baseline_unwraps = self.baseline.per_item_unwraps.get(&node).copied().unwrap_or(0);
            let new_unwraps = m.unwrap_count.saturating_sub(baseline_unwraps);
            if new_unwraps > self.thresholds.max_unwrap_per_fn {
                hard.push(refusal(RefusalCode::TooManyUnwraps, Severity::Hard, node, ...));
            }
        }
        // 2. Unsafe-introduction (post-edit snapshot vs baseline.had_unsafe).
        let unsafe_findings = audits::unsafe_findings_for(dirty, self.snap, self.loaded, effects)?;
        for f in unsafe_findings {
            if let Some(node) = f.enclosing_function {
                let was = *self.baseline.had_unsafe.get(&node).unwrap_or(&false);
                if !was && self.thresholds.forbid_unsafe_introduce {
                    hard.push(refusal_unsafe(node, &f));
                }
            }
        }
        // 3. Cycle checks (Step 12).
        if self.thresholds.forbid_new_cycles {
            for c in cycle::detect_new_module_cycles(&self.baseline.module_parent_edges, effects)? {
                hard.push(refusal_mod_cycle(c));
            }
            for c in cycle::detect_new_crate_cycles(&self.baseline.crate_dep_set, effects)? {
                hard.push(refusal_crate_cycle(c));
            }
        }
        // 4. Forbidden-dep (Step 13).
        for v in dirty_dep_filter::check_forbidden(self.snap, &self.allowlist, dirty, effects)? {
            hard.push(refusal_forbidden_dep(v));
        }
        // 5. Missing-docs (soft).
        for f in audits::missing_docs_for(dirty, self.snap)? {
            soft.push(refusal_missing_docs(f));
        }
        Ok(GateOutcome { passed: hard.is_empty(), hard, soft })
    }
    ```
    `read_body_post_edit` overlays `effects.source_edits` onto current source before measuring — gates evaluate would-be state without writing.

11b. **Dirty-set audit wrappers (Phase 1 fallback).** WHERE: `audits.rs`. Run workspace-wide audit, post-filter:
    ```rust
    pub fn unsafe_findings_for(dirty: &[NodeId], snap: &OpenedSnapshot, loaded: &LoadedWorkspace,
                                _effects: &Effects) -> Result<Vec<UnsafeFinding>> {
        let dirty_set: HashSet<NodeId> = dirty.iter().copied().collect();
        let all = snap.unsafe_audit(loaded)?;
        Ok(all.into_iter()
            .filter(|f| f.enclosing_function.map_or(false, |n| dirty_set.contains(&n)))
            .collect())
    }
    ```
    Same shape for `fn_body_findings_for`, `recursion_cycles_for`, `mut_static_findings_for`, `derive_findings_for`, `missing_docs_for`, `channel_capacity_findings_for`. Phase 2 grows each audit a `fn audit_only(crate_id: NodeId, items: &HashSet<NodeId>) -> Vec<Finding>` overload.

12. **Cycle detection.** WHERE: `cycle.rs`. `petgraph::algo::tarjan_scc`. **Module-cycle:**
    1. Project post-edit module-parent graph: start from `baseline.module_parent_edges`; apply `effects.source_edits` (only when `effects.edit_class == ModuleTree`).
    2. Build `petgraph::DiGraph<NodeId, ()>` over projected edges.
    3. `tarjan_scc` → SCC > 1 = cycle. Subtract baseline cycles → return new only.
    **Crate-cycle:**
    1. Start from `baseline.crate_dep_set`.
    2. For each `FileEdit` in `Cargo.toml`, parse manifest before/after, compute dep-set delta. (Cargo-toml verbs deferred to P1.5e; branch returns empty until then.)
    3. For body/sig edits introducing `use other_crate::...`: regex-match `use\s+([a-zA-Z_][a-zA-Z0-9_]*)` over `source_edits` (cheap approximation; correct for body edits).
    4. `tarjan_scc` over projected `crate_dep_set` → return new cycles.
    VERIFY: `cycle_introduced_refused`.

13. **Forbidden-dep dirty filter.** WHERE: `dirty_dep_filter.rs`.
    ```rust
    pub fn check_forbidden(snap: &OpenedSnapshot, allowlist: &ForbiddenDepAllowlist,
                            dirty: &[NodeId], effects: &Effects) -> Result<Vec<ForbiddenDependencyViolation>> {
        let touched_crates: HashSet<NodeId> = dirty.iter()
            .filter_map(|n| snap.node(&snap.read_txn()?, *n).ok().flatten()?.crate_id).collect();
        let all_violations = snap.forbidden_dependency_check(&allowlist.rules)?;
        Ok(all_violations.into_iter()
            .filter(|v| /* consumer/producer maps back to touched_crate */ true)
            .filter(|v| !baseline_had_violation(v))
            .collect())
    }
    ```
    Existing `forbidden_dependency_check` in `query/crates.rs` called as-is; wrapper narrows. VERIFY: `forbidden_dep_refused`.

14. **Aggregate.** Step 11's pseudocode. `passed = hard.is_empty()`. `soft` flows into P1.7 scalarizer.

15. **Wire `Crud::apply` to gate.** Step 1's `apply` body. Critical ordering:
    1. `compute_effects` (no I/O).
    2. `gates.evaluate(dirty, &effects)`.
    3. `!passed` → `Err(CrudError::Refused(hard))`. NO checkpoint, NO source, NO LMDB.
    4. Else → `begin_checkpoint` → `persist` → `commit`.
    Guarantees refused op is byte-identical to never having been called. VERIFY: `simulate_predicts_refusal`.

16. **Wire `Crud::simulate` to gate.** Step 2.

## Tests

(`crates/rmc-crud/tests/`, `crates/rmc-gates/tests/`)

- **`simulate_equals_apply_for_modify_body`** (`crates/rmc-crud/tests/simulate_eq_apply.rs`) — `simulate(op).effects == apply(op).recorded_effects`. Repeat for every verb.
- **`simulate_predicts_refusal`** — `ModifyBodyOp` whose body is `fn foo() { unsafe { std::ptr::null::<u8>().read() } }` on fn with `baseline.had_unsafe == false`. Assert `simulate.effects.would_refuse` contains `RefusalReason { code: UnsafeIntroduced, severity: Hard }`. Then `apply(op)` → `Err(CrudError::Refused(..))`.
- **`hard_complexity_refuses`** — body with 25 `if` branches (cyclomatic = 26); `max_complexity = 20`; `evaluate` returns `Hard ComplexityTooHigh { threshold: 20, actual: 26 }`; `passed == false`.
- **`soft_unwrap_warns`** — body with 1 `.unwrap()`; baseline 0. `max_unwrap_per_fn = 0, soft_unwrap = 0`: `hard` contains `Hard TooManyUnwraps`. Then `max_unwrap_per_fn = 1, soft_unwrap = 0`: `hard` empty, `soft` contains `Soft TooManyUnwraps`.
- **`cycle_introduced_refused`** — fixture with modules `a`, `b`; `MoveModuleOp` re-parenting `a` under `b` + `ModifyBodyOp` inserting `use crate::a::Foo` into `b`. Assert `ModuleCycleIntroduced` in `hard`.
- **`forbidden_dep_refused`** — allowlist `rmc-config !-> rmc-engine`; `ModifyBodyOp` inserting `use rmc_engine::EmbeddingGenerator;` into fn in `rmc-config`. Assert `ForbiddenDependency` in `hard`.
- **`dirty_set_audit_speed`** (`crates/rmc-gates/benches/dirty_audit.rs`) — 100k-LOC fixture; baseline captured once; `evaluate(&[one_node], ...)` × 100 < 50ms each. 10 dirty nodes < 200ms each.
- **`baseline_does_not_count_pre_existing`** — fixture fn with 3 existing `.unwrap()`s. Capture baseline. `ModifyBodyOp` that doesn't touch any unwrap → 0 `TooManyUnwraps`. Then add 4th unwrap → 1 refusal with `actual: 1` (NEW count, not total 4).
- **`evaluate_uses_post_edit_body`** — existing body 0 unwraps; `effects.source_edits` adds one. Gate runs against overlay (sees 1), not on-disk (still 0).
- **`gates_toml_round_trip`** — non-default values; load; equals expected; missing fields default.

## Open decisions / risks

- **Audit latency at write time.** Phase 1 "filter workspace-wide audit" is a stopgap — existing audits walk whole workspace (~100s of ms on 100k LOC). 50/200ms targets in `dirty_set_audit_speed` are aspirational and likely fail in Phase 1; Phase 2 needs each audit to grow `audit_only(crate_id, items)`. Documented as known limit.
- **`compute_complexity` duplicative.** Regex-counting `analyze_complexity` exists in `rmc-server/src/tools/endpoints/analysis.rs`. New syntax-tree version is more accurate but lives in `rmc-gates` to avoid pulling `rmc-server` into gate path. Server's tool should adopt new impl in follow-up.
- **Threshold calibration.** Defaults from rust-guidelines. After P1.7 first runs, ratchet down based on false-refusal rate observed in trajectories. Track in `EpisodeRunner::stats.refusals_by_code`.
- **Allowlist format ownership.** `gates.toml` at workspace root; agent has NO verb to touch it. If future Phase needs agent-editable thresholds, route through `agent_overrides.toml` with constraint that overrides can only *tighten*.
- **Soft penalty accumulator.** `GateOutcome.soft` is `Vec<RefusalReason>`; P1.7 reward scalarizer consumes it. Per-code weights recommended: `missing_docs = 0.1`, `soft_complexity = 0.5`, `soft_unwrap = 1.0`.
- **Simulator cost for `extract_function`.** Requires RA type-check for captures (expensive full RA query). Open: acceptable inside simulate's "no-apply" contract? **Yes** — simulator value is honesty; cheaper approximation that diverges violates P1.4 invariant. Charge the cost; cache by `(host_edit_seq, op_hash)`.
- **Cycle detection on Cargo.toml edits.** Phase 1 defers (no Cargo verbs until P1.5e); branch returns empty. Use-import-based crate-cycle approximation (regex over `source_edits`) correct for body edits, not manifest dep changes; flag in code comments.
- **Baseline staleness.** `Baseline` captured at episode start; mid-episode `apply` ops accumulate. Intentional — baseline represents "what agent inherited". Open: incrementally update on every successful `persist`? Recommend Phase 2.
- **`forbid_unsafe_introduce` per-crate granularity.** Blanket "no new unsafe" too strict for `rmc-engine` (Candle bindings). Open: per-crate override via `[unsafe.allowed_in]` in `gates.toml`. Recommend yes; until then, test workspace's `rmc-engine` must already-have-unsafe to baseline.
- **What counts as "introduced unsafe"?** Currently: `baseline.had_unsafe[node] == false && post_edit.has_unsafe_in(node)`. Open: `unsafe fn` declared (not just `unsafe { }` block). `unsafe_audit` only catches blocks; need separate check on `function_signature` for newly-declared `unsafe fn`. Add to Phase 1 follow-up.


---

# Section J — P1.7 Commit/Reward + P1.8 Episode Runner

## Overview

This slice is the **integration milestone** of Phase 1. Every preceding piece — `WorkspaceHost` (P0.2), `Crud::apply` returning `EditOutcome` (P1.5a), `Checkpoint::take/restore` (D4), `GateRunner::evaluate` (P1.6), `Crud::simulate` (P1.4), `Navigator` (P1.1), `VisionIndex` (P1.3) — was scaffolding for the moment an actual model picks an action, the system applies it, scores it, and writes a step into a trajectory. M3 = "first end-to-end loop with `modify_body` only" — exactly what this delivers. Two new crates (`rmc-reward`, `rmc-episode`) plus the `rmc-rl` binary turn the static apply==rebuild engine into a closed-loop RL environment.

The dominant unsolved problem is **cargo gate latency** (Issues #2). The phase-1 plan lists four options without committing. This plan commits: **default = `CargoGateMode::RaPlusCheckEveryK { k: 5 }`.** RA-based type check on every step (~50ms), full `cargo check` every 5 steps (~1–2s warm), full `cargo test --workspace --no-fail-fast` only at `declare_done`. Justification + fallbacks in Open decisions.

## New modules / files

- `crates/rmc-reward/Cargo.toml` — new crate. Deps: `rmc-graph`, `rmc-engine`, `tokio`, `serde`, `serde_json`, `petgraph`, `anyhow`, `thiserror`, `tracing`.
- `crates/rmc-reward/src/lib.rs` — `Commit`, `CommitResult`, `RewardVector`, `AuditDelta`, `MetricDelta`, `RewardWeights`, `Scalarizer`, `CargoGateMode`, `CargoGateRunner`, `CargoGateOutcome`.
- `crates/rmc-reward/src/cargo_gate.rs` — warm cargo invocation; `tokio::process::Command`; persistent `CARGO_TARGET_DIR`; JSON message parsing.
- `crates/rmc-reward/src/ra_gate.rs` — RA fast type check via `WorkspaceHost::ra_type_check_dirty(&[CrateId])` (P0.2 adds method).
- `crates/rmc-reward/src/audit_delta.rs` — before/after audit diff over **dirty NodeId set only** (O(|dirty|) not O(workspace)).
- `crates/rmc-reward/src/graph_metrics.rs` — modularity (Louvain local-move), conductance (per-cluster boundary), clustering coefficient, betweenness centrality top-p95 (Brandes restricted to k-hop neighborhood of dirty nodes). `petgraph` + custom community-detection.
- `crates/rmc-reward/src/scalarize.rs` — `Scalarizer::scalarize`. Hard floor: compile fail → -1.0.
- `crates/rmc-episode/Cargo.toml` — new crate. Deps: `rmc-reward`, `rmc-graph`, `rmc-engine`, `tokio`, `serde`, `serde_json`, `reqwest`, `clap`, `async-trait`, `anyhow`, `thiserror`, `tracing`.
- `crates/rmc-episode/src/lib.rs` — `Episode`, `Trajectory`, `Action`, `ActionRouter`, `StepRecord`, `EpisodeOutcome`, `TaskSpec`, `SuccessCriteria`, `StepBudget`.
- `crates/rmc-episode/src/router.rs` — 5-verb dispatch + per-step budget check.
- `crates/rmc-episode/src/trajectory.rs` — JSONL writer to `working/<session_id>/trajectory.jsonl`. PII scrubber masks `/^sk-[A-Za-z0-9_-]{20,}$/`.
- `crates/rmc-episode/src/model_client.rs` — `ModelClient` trait + `AnthropicClient`. POSTs to `https://api.anthropic.com/v1/messages` with `anthropic-version: 2023-06-01`. Anthropic prompt caching (`cache_control: { "type": "ephemeral" }`).
- `crates/rmc-episode/src/prompt.rs` — system prompt template, ContextView serialization, tool-use schema for 5 verbs.
- `crates/rmc-rl/Cargo.toml` — new bin-only crate. Deps: `rmc-episode`, `clap`, `tokio` (`flavor = "multi_thread"`), `anyhow`.
- `crates/rmc-rl/src/main.rs` — CLI; subcommand `run` with `--task`, `--model`, `--budget`, `--workspace`.
- `tasks/dedupe_project_paths.toml` — seed task (rmc duplication of `project_paths`).

## Type definitions

```rust
// crates/rmc-reward/src/lib.rs

pub struct Commit<'a> {
    pub host:       &'a mut WorkspaceHost,
    pub snap:       &'a OpenedWorkingSnapshot,
    pub thresholds: GateThresholds,
    pub gate:       CargoGateRunner,
    pub weights:    RewardWeights,
}

#[derive(Debug, Clone)]
pub struct CommitResult {
    pub passed:            bool,
    pub reward:            RewardVector,
    pub scalar:            f32,
    pub rollback_executed: bool,
    pub elapsed_ms:        u64,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct RewardVector {
    pub compile_ok:          f32,        // 1.0 if gate passed, else 0.0
    pub test_pass_rate:      f32,        // 0..1; 1.0 if no tests ran
    pub audit_delta:         AuditDelta,
    pub graph_metric_delta:  MetricDelta,
    pub gates_soft_penalty:  f32,        // sum of GateOutcome.soft_penalties
    pub token_cost:          u32,        // input + output since prior commit
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct AuditDelta {
    pub unsafe_added:         i32,
    pub unwrap_added:         i32,
    pub missing_docs_added:   i32,
    pub mut_static_added:     i32,
    pub complexity_max_delta: i32,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct MetricDelta {
    pub modularity:          f32,
    pub conductance:         f32,
    pub clustering_coef:     f32,
    pub betweenness_top_p95: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RewardWeights {
    pub compile:      f32,   // default 1.0 — hard floor
    pub test:         f32,   // default 0.5
    pub audit_unsafe: f32,   // default 0.1
    pub audit_unwrap: f32,   // default 0.1
    pub audit_docs:   f32,   // default 0.1
    pub metrics:      f32,   // default 0.2
    pub soft_penalty: f32,   // default -0.05 per soft violation
    pub token:        f32,   // default -1e-5 per token
}

impl Default for RewardWeights {
    fn default() -> Self {
        Self { compile: 1.0, test: 0.5,
               audit_unsafe: 0.1, audit_unwrap: 0.1, audit_docs: 0.1,
               metrics: 0.2, soft_penalty: -0.05, token: -1e-5 }
    }
}

pub struct Scalarizer { pub weights: RewardWeights }
impl Scalarizer {
    pub fn scalarize(&self, rv: &RewardVector) -> f32;
    pub fn from_toml(path: &Path) -> Result<Self>;
}

#[derive(Debug, Clone)]
pub enum CargoGateMode {
    CheckOnly,
    CheckAndScopedTest { test_pattern: String },
    RaOnly,
    /// DEFAULT: RA every step; full cargo check every K steps; full test only on declare_done.
    RaPlusCheckEveryK { k: u32 },
    FullAtDoneOnly,
}

pub struct CargoGateRunner {
    pub mode:               CargoGateMode,
    pub workspace:          PathBuf,
    pub session_target_dir: PathBuf,    // CARGO_TARGET_DIR pinned per session
    pub step_counter:       AtomicU32,
    pub devshell_prefix:    Option<Vec<String>>,    // ["nix","develop","../nix-devshells#cuda-code","--command"]
}

impl CargoGateRunner {
    pub async fn run(&self, host: &mut WorkspaceHost, dirty_crates: &[String], force_full: bool)
        -> Result<CargoGateOutcome>;
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CargoGateOutcome {
    pub check_passed: bool,
    pub test_results: Option<TestResults>,
    pub elapsed_ms:   u64,
    pub diagnostics:  Vec<CargoDiagnostic>,
    pub mode_used:    String,           // "ra" | "cargo-check" | "cargo-test"
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestResults {
    pub total: u32, pub passed: u32, pub failed: u32, pub ignored: u32,
    pub per_test: Vec<TestRecord>,      // truncated to first 500
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TestRecord {
    pub name: String, pub crate_: String, pub passed: bool,
    pub stdout: Option<String>,         // last 4 KB if failed
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CargoDiagnostic {
    pub level: String,                   // "error" | "warning"
    pub message: String,
    pub file: Option<String>, pub line: Option<u32>,
}
```

```rust
// crates/rmc-episode/src/lib.rs

// Per E2: store ONLY owned / config / Arc-backed fields. `Crud<'a>`,
// `Navigator`, and `Commit<'a>` each borrow `host` (and `snap`), so storing
// any of them inside `Episode` is self-referential and will NOT compile in
// safe Rust. They are built per-step inside `step()` and dropped at end of
// step — their lifetime is local to the call.
pub struct Episode<M: ModelClient> {
    pub host:          WorkspaceHost,
    pub snap:          OpenedWorkingSnapshot,
    pub semantic:      SemanticService,   // &mut-borrowed by the per-step Crud
    pub crud_cfg:      CrudConfig,        // stateless config (callsite_fill, cascade default)
    pub navigator_cfg: NavigatorConfig,   // stateless config
    pub gate_runner:   CargoGateRunner,   // owns session_target_dir
    pub thresholds:    GateThresholds,    // borrowed by the per-step Commit
    pub weights:       RewardWeights,
    pub metric_cache:  MetricCache,       // &mut-borrowed by the per-step Commit
    pub before_audits: AuditCounts,       // &mut-borrowed by the per-step Commit
    pub model:         M,
    pub budget:        StepBudget,
    pub trajectory:    TrajectoryRecorder,
    pub task:          TaskSpec,
}

impl<M: ModelClient> Episode<M> {
    async fn step(&mut self, action: &Action, tokens: u32) -> Result<StepRecord> {
        // Build the borrowing structs per-action; their `'_` lifetime is local.
        let mut crud = Crud::new(&mut self.host, &self.snap, &mut self.semantic,
                                 self.task.workspace_root());
        let nav = Navigator::new(&self.snap, &self.navigator_cfg);
        // ... dispatch on `action`; for a committing verb build Commit per-op:
        let mut commit = Commit {
            host:       &mut self.host,   // NB: not simultaneously with `crud`'s &mut host —
            snap:       &self.snap,       // dispatch finishes the Crud borrow before Commit borrows.
            thresholds: self.thresholds.clone(),
            gate:       self.gate_runner.clone(),
            weights:    self.weights.clone(),
        };
        // dispatch on `action`; for a committing verb: commit.run(...).await?
        // then assemble and return the StepRecord.
        todo!()
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StepBudget {
    pub max_steps:     u32,            // hard cap; default 50
    pub max_tokens:    u64,
    pub max_wall_secs: u64,            // default 600
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "verb", rename_all = "snake_case")]
pub enum Action {
    Navigate(NavStep),
    Crud(CrudOp),
    Commit,                              // Crud::apply commits per-op; reserved for batched future
    Simulate(CrudOp),
    DeclareDone { summary: String },
    AskNoOp,                             // small penalty to avoid stall loops
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StepRecord {
    pub step:              u32,
    pub view:              ContextView,
    pub action:            Action,
    pub action_result:     ActionResult,
    pub reward:            f32,
    pub reward_components: RewardVector,
    pub elapsed_ms:        u64,
    pub model_tokens_in:   u32,
    pub model_tokens_out:  u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Trajectory {
    pub task:    TaskSpec,
    pub steps:   Vec<StepRecord>,
    pub outcome: EpisodeOutcome,
    pub started_at_unix: u64,
    pub ended_at_unix:   u64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EpisodeOutcome {
    CompletedDeclareDone { final_reward: f32 },
    BudgetExhausted     { reason: String, partial_reward: f32 },
    HardFailure         { reason: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskSpec {
    pub id:               String,
    pub initial_loc:      Location,
    pub goal_prompt:      String,
    pub success_criteria: SuccessCriteria,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SuccessCriteria {
    pub all_tests_pass:           bool,
    pub max_audit_regressions:    AuditDeltaThresholds,
    pub disallowed_patterns:      Vec<String>,
}

#[async_trait::async_trait]
pub trait ModelClient: Send + Sync {
    async fn next_action(&self, view: &ContextView, history: &[StepRecord], task: &TaskSpec)
        -> Result<NextActionResponse>;
}

pub struct NextActionResponse { pub action: Action, pub tokens_in: u32, pub tokens_out: u32 }

pub struct AnthropicClient {
    pub model:     String,               // "claude-opus-4-7" or "claude-sonnet-4-6"
    pub api_key:   String,
    pub endpoint:  String,
    pub http:      reqwest::Client,
    pub max_retries: u32,                // default 3
}
```

## Step-by-step implementation

### P1.7 — commit + reward

**Step 1 — Pick the cargo gate mode (DECISION COMMITTED).**
- **Default = `CargoGateMode::RaPlusCheckEveryK { k: 5 }`.**
- **Justification:**
  - 50-step full `cargo check` per step ≈ 50–100s; dominates episode wall-clock.
  - RA catches majority of compile errors (unresolved names, type mismatches, missing trait impls) using the warm `RootDatabase` from P0.2.
  - RA misses: (a) proc-macro side effects (no build.rs rerun), (b) cfg flag changes (fixed cfg set), (c) Cargo feature unification (static view). Every 5th step's full `cargo check` catches drift.
  - `k = 5` is the smallest k whose amortized wall-clock fits a 50-step episode under 60s cargo work (50/5 × ~2s warm = 20s).
- **Fallback escalation:** `CargoGateRunner::run` MUST escalate `RaOnly` → `cargo check` immediately, regardless of step counter, if:
  1. `host.ra_type_check_dirty(...)` returns Err (host corruption), OR
  2. edit touched `build.rs`, `Cargo.toml`, or any file containing `#[proc_macro]` / `#[proc_macro_derive]` / `#[proc_macro_attribute]` (classifiable from D2's edit-class).
- DEPENDS: P0.2 adds `WorkspaceHost::ra_type_check_dirty(&[CrateId]) -> Result<Vec<CargoDiagnostic>>`. RA's `Analysis::diagnostics` already returns per-file diagnostics; collect across dirty set.
- VERIFY: `RaOnly` ≤ 100ms p95 on 100k-LOC workspace.

**Step 2 — `CargoGateRunner::run`.** WHERE: `cargo_gate.rs`.
```rust
pub async fn run(&self, host: &mut WorkspaceHost, dirty_crates: &[String], force_full: bool)
    -> Result<CargoGateOutcome>
{
    let started = Instant::now();
    let step = self.step_counter.fetch_add(1, SeqCst) + 1;
    let need_full = force_full
        || matches!(self.mode, CheckOnly | CheckAndScopedTest{..} | FullAtDoneOnly)
        || matches!(self.mode, RaPlusCheckEveryK { k } if step % k == 0);

    // 1) Always run RA first (~50ms confirms type sanity).
    let ra_diags = ra_gate::run(host, dirty_crates).await?;
    let ra_errors: Vec<_> = ra_diags.iter().filter(|d| d.level == "error").cloned().collect();
    if !ra_errors.is_empty() {
        return Ok(CargoGateOutcome {
            check_passed: false, test_results: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
            diagnostics: ra_diags, mode_used: "ra".into(),
        });
    }
    if !need_full && matches!(self.mode, RaOnly | RaPlusCheckEveryK{..}) {
        return Ok(CargoGateOutcome {
            check_passed: true, test_results: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
            diagnostics: ra_diags, mode_used: "ra".into(),
        });
    }
    // 2) Full cargo check + optionally tests.
    let mut diags = ra_diags;
    let check_ok = self.cargo_check(dirty_crates, &mut diags).await?;
    if !check_ok {
        return Ok(CargoGateOutcome {
            check_passed: false, test_results: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
            diagnostics: diags, mode_used: "cargo-check".into(),
        });
    }
    let tests = if force_full {
        Some(self.cargo_test_full().await?)
    } else if let CheckAndScopedTest { test_pattern } = &self.mode {
        Some(self.cargo_test_scoped(test_pattern, dirty_crates).await?)
    } else { None };
    Ok(CargoGateOutcome {
        check_passed: true, test_results: tests,
        elapsed_ms: started.elapsed().as_millis() as u64,
        diagnostics: diags,
        mode_used: if force_full { "cargo-test" } else { "cargo-check" }.into(),
    })
}
```

Cargo invocation:
```rust
let mut cmd = tokio::process::Command::new("cargo");
if let Some(prefix) = &self.devshell_prefix {
    cmd = tokio::process::Command::new(&prefix[0]);
    cmd.args(&prefix[1..]).arg("cargo");
}
cmd.arg("check").arg("--message-format=json").arg("--locked").arg("--offline").arg("--workspace");
for c in dirty_crates { cmd.args(&["-p", c]); }
cmd.env("CARGO_TARGET_DIR", &self.session_target_dir)
   .env("RUSTC_WRAPPER", "")
   .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
```

Parse JSON output line-by-line: `reason == "compiler-message"` carries `message.spans[]` + `message.level`; aggregate into `CargoDiagnostic`. `reason == "build-finished"` carries `success: bool`.

Test invocation (force_full only):
```
cargo test --workspace --no-fail-fast --message-format=json -- -Z unstable-options --format json --report-time
```
If not on nightly, fall back to parsing plain text "test foo::bar ... ok"/"FAILED" lines.

VERIFY: `cargo_check_parses_compile_error`.

**Step 3 — RA fast gate.** WHERE: `ra_gate.rs`.
```rust
pub async fn run(host: &mut WorkspaceHost, dirty_crates: &[String]) -> Result<Vec<CargoDiagnostic>> {
    let raw = host.ra_type_check_dirty(dirty_crates).await?;
    Ok(raw.into_iter().map(|d| CargoDiagnostic {
        level: if d.is_error { "error" } else { "warning" }.into(),
        message: d.message, file: d.file, line: d.line,
    }).collect())
}
```
VERIFY: `ra_fast_gate_detects_type_error` — body edit using undefined name returns error within 100ms.

**Step 4 — Audit delta.** WHERE: `audit_delta.rs`. Restrict every audit to dirty NodeId set from `EditOutcome.affected_items`.
```rust
pub struct AuditDeltaComputer<'a> {
    pub snap: &'a OpenedSnapshot,
    pub before: AuditCounts,
}

#[derive(Debug, Clone, Default)]
pub struct AuditCounts {
    pub unsafe_findings: u32, pub unwrap_findings: u32,
    pub missing_docs_findings: u32, pub mut_static_findings: u32,
    pub complexity_max: u32,
}

impl<'a> AuditDeltaComputer<'a> {
    pub fn capture(snap: &'a OpenedSnapshot, dirty: &[NodeId]) -> Result<AuditCounts> {
        let unsafe_n = snap.unsafe_audit(loaded)?.iter()
                           .filter(|f| dirty.contains(&f.target)).count() as u32;
        let unwrap_n = snap.fn_body_audit_filtered(/* unwrap pattern */)?.iter()
                           .filter(|f| dirty.contains(&f.target)).count() as u32;
        let docs_n = snap.missing_docs_audit()?.iter()
                          .filter(|f| dirty.contains(&f.target)).count() as u32;
        let mut_static_n = snap.mut_static_audit()?.iter()
                                .filter(|f| dirty.contains(&f.target)).count() as u32;
        let complexity_max = snap.analyze_complexity_for(dirty)?.max_score;
        Ok(AuditCounts { unsafe_findings: unsafe_n, unwrap_findings: unwrap_n,
                          missing_docs_findings: docs_n, mut_static_findings: mut_static_n,
                          complexity_max })
    }
    pub fn delta(before: &AuditCounts, after: &AuditCounts) -> AuditDelta {
        AuditDelta {
            unsafe_added: after.unsafe_findings as i32 - before.unsafe_findings as i32,
            unwrap_added: after.unwrap_findings as i32 - before.unwrap_findings as i32,
            missing_docs_added: after.missing_docs_findings as i32 - before.missing_docs_findings as i32,
            mut_static_added: after.mut_static_findings as i32 - before.mut_static_findings as i32,
            complexity_max_delta: after.complexity_max as i32 - before.complexity_max as i32,
        }
    }
}
```
VERIFY: `audit_delta_unsafe_increment`.

**Step 5 — Graph metric delta (incremental).** WHERE: `graph_metrics.rs`. Full betweenness on 5k-50k nodes = seconds; compute **delta on affected sub-graph only**.

1. **Modularity** (Louvain local-move): from prior partition, move only nodes in dirty set to best neighbor community, recompute Q. O(|dirty| · avg_degree).
2. **Conductance** per community: only recompute for communities containing dirty nodes.
3. **Clustering coefficient** per node: recompute only for dirty nodes + 1-hop neighbors.
4. **Betweenness centrality top-p95**: Brandes restricted to 2-hop neighborhood of dirty nodes. Test asserts `visit_count < 0.1 * total_nodes`.

```rust
pub struct MetricCache {
    pub partition:        Vec<u32>,
    pub modularity:       f32, pub conductance: f32,
    pub clustering_coef:  f32, pub betweenness_p95: f32,
    pub graph:            petgraph::graph::UnGraph<NodeId, ()>,
    pub node_index:       HashMap<NodeId, petgraph::graph::NodeIndex>,
}

impl MetricCache {
    pub fn rebuild_full(snap: &OpenedSnapshot) -> Result<Self>;
    pub fn apply_delta(&mut self, edit_outcome: &EditOutcome, snap: &OpenedSnapshot)
        -> Result<MetricDelta>
    {
        let (before_mod, before_con, before_cc, before_bw) =
            (self.modularity, self.conductance, self.clustering_coef, self.betweenness_p95);
        for e in &edit_outcome.removed_edges { self.remove_edge(e); }
        for e in &edit_outcome.added_edges   { self.insert_edge(e); }
        self.modularity = self.louvain_local_move(&edit_outcome.affected_items);
        let touched = self.communities_containing(&edit_outcome.affected_items);
        self.conductance = self.recompute_conductance(touched);
        self.clustering_coef = self.recompute_clustering(self.k_hop(&edit_outcome.affected_items, 1));
        self.betweenness_p95 = self.recompute_betweenness_local(self.k_hop(&edit_outcome.affected_items, 2));
        Ok(MetricDelta {
            modularity: self.modularity - before_mod,
            conductance: self.conductance - before_con,
            clustering_coef: self.clustering_coef - before_cc,
            betweenness_top_p95: self.betweenness_p95 - before_bw,
        })
    }
}
```
DEPENDS: `EditOutcome.added_edges` / `removed_edges` (P1.5a records for LMDB diff-patch); `petgraph` from P1.3. VERIFY: `metric_delta_local_only`.

**Step 6 — Scalarizer.** WHERE: `scalarize.rs`.
```rust
impl Scalarizer {
    pub fn scalarize(&self, rv: &RewardVector) -> f32 {
        let w = &self.weights;
        if rv.compile_ok < 0.5 { return -1.0; }   // Hard floor.
        let audit_term =
              w.audit_unsafe * (-rv.audit_delta.unsafe_added as f32)
            + w.audit_unwrap * (-rv.audit_delta.unwrap_added as f32)
            + w.audit_docs   * (-rv.audit_delta.missing_docs_added as f32);
        let metric_term = w.metrics * (
              rv.graph_metric_delta.modularity
            - rv.graph_metric_delta.conductance
            + 0.5 * rv.graph_metric_delta.clustering_coef
        );
        w.compile * rv.compile_ok
            + w.test * rv.test_pass_rate
            + audit_term
            + metric_term
            + w.soft_penalty * rv.gates_soft_penalty
            + w.token * rv.token_cost as f32
    }
}
```
Tunables via `RewardWeights::from_toml(path)`. VERIFY: `scalarizer_hard_floor`.

**Step 7 — `Commit::run` orchestration.** WHERE: `lib.rs`.
```rust
impl<'a> Commit<'a> {
    pub async fn run(&mut self, edit_outcome: &EditOutcome, gate_outcome: &GateOutcome,
                      checkpoint: &Checkpoint, dirty_crates: &[String], token_cost: u32,
                      force_full: bool) -> Result<CommitResult>
    {
        let t0 = Instant::now();
        let cargo_outcome = self.gate.run(self.host, dirty_crates, force_full).await?;
        if !cargo_outcome.check_passed {
            self.host.restore(checkpoint).map_err(|e| anyhow!("rollback failed: {e}"))?;
            let rv = RewardVector {
                compile_ok: 0.0, test_pass_rate: 0.0,
                audit_delta: AuditDelta::default(),
                graph_metric_delta: MetricDelta::default(),
                gates_soft_penalty: gate_outcome.soft_penalties as f32,
                token_cost,
            };
            return Ok(CommitResult {
                passed: false, reward: rv.clone(),
                scalar: Scalarizer { weights: self.weights.clone() }.scalarize(&rv),
                rollback_executed: true,
                elapsed_ms: t0.elapsed().as_millis() as u64,
            });
        }
        let test_pass_rate = match &cargo_outcome.test_results {
            None => 1.0,
            Some(tr) if tr.total == 0 => 1.0,
            Some(tr) => tr.passed as f32 / tr.total as f32,
        };
        let audits_after  = AuditDeltaComputer::capture(self.snap, &edit_outcome.affected_items)?;
        let audit_delta   = AuditDeltaComputer::delta(&self.before_audits, &audits_after);
        let metric_delta  = self.metric_cache.apply_delta(edit_outcome, self.snap)?;
        let rv = RewardVector {
            compile_ok: 1.0, test_pass_rate, audit_delta, graph_metric_delta: metric_delta,
            gates_soft_penalty: gate_outcome.soft_penalties as f32, token_cost,
        };
        Ok(CommitResult {
            passed: true, reward: rv.clone(),
            scalar: Scalarizer { weights: self.weights.clone() }.scalarize(&rv),
            rollback_executed: false,
            elapsed_ms: t0.elapsed().as_millis() as u64,
        })
    }
}
```
VERIFY: `commit_rollback_on_compile_break`.

**Step 8 — Tests on `declare_done`.** `Commit::run(force_full = true)` runs `cargo test --workspace --no-fail-fast --message-format=json`. Parse JSON for `reason == "test"` events. Truncate per-test stdout to last 4 KB; cap `per_test` at 500. Record `total/passed/failed/ignored` exactly. VERIFY: `declare_done_runs_full_tests`.

### P1.8 — episode runner

**Step 9 — `Episode::new`.** WHERE: `lib.rs`.
```rust
impl<M: ModelClient> Episode<M> {
    pub async fn new(task: TaskSpec, model: M, budget: StepBudget,
                      base_workspace: &Path, session_id: &str, weights: RewardWeights)
        -> Result<Self>
    {
        // 1. Working snapshot per D1: copy published LMDB to working/<session_id>/.
        let host = WorkspaceHost::open_for_session(base_workspace, session_id).await?;
        let snap = host.open_working_snapshot()?;
        // 2. Initial ContextView.
        let navigator = Navigator::new(&snap);
        let view0 = navigator.view_at(&task.initial_loc)?;
        // 3. Reward + commit.
        let gate = CargoGateRunner {
            mode: CargoGateMode::RaPlusCheckEveryK { k: 5 },
            workspace: base_workspace.into(),
            session_target_dir: working_dir(session_id).join("cargo-target"),
            step_counter: 0.into(),
            devshell_prefix: Some(vec![
                "nix".into(), "develop".into(),
                "../nix-devshells#cuda-code".into(), "--command".into()
            ]),
        };
        let crud = Crud::new(&snap);
        let commit = Commit { /* host, snap, thresholds, gate, weights ... */ };
        let traj = TrajectoryRecorder::open(&working_dir(session_id).join("trajectory.jsonl"))?;
        traj.write_header(&task)?;
        Ok(Self { host, snap, crud, navigator, commit, model, budget, trajectory: traj, task })
    }
}
```
VERIFY: `episode_new_creates_working_snapshot`.

**Step 10 — Loop body.**
```rust
pub async fn run(mut self) -> Result<Trajectory> {
    let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let mut step_num = 0u32;
    let mut total_tokens = 0u64;
    let wall_deadline = Instant::now() + Duration::from_secs(self.budget.max_wall_secs);
    let mut prior_audits = AuditDeltaComputer::capture(&self.snap, &[])?;
    let mut prior_view   = self.navigator.view_at(&self.task.initial_loc)?;
    loop {
        if step_num >= self.budget.max_steps {
            return Ok(self.finalize(EpisodeOutcome::BudgetExhausted { reason: "max_steps".into(), partial_reward: 0.0 }, started).await?);
        }
        if total_tokens >= self.budget.max_tokens {
            return Ok(self.finalize(EpisodeOutcome::BudgetExhausted { reason: "max_tokens".into(), partial_reward: 0.0 }, started).await?);
        }
        if Instant::now() > wall_deadline {
            return Ok(self.finalize(EpisodeOutcome::BudgetExhausted { reason: "wall_clock".into(), partial_reward: 0.0 }, started).await?);
        }
        step_num += 1;
        let step_t0 = Instant::now();
        let history_view = self.trajectory.history_view();
        let resp = self.model.next_action(&prior_view, &history_view, &self.task).await?;
        total_tokens += (resp.tokens_in + resp.tokens_out) as u64;
        let checkpoint = self.host.checkpoint().await?;
        let dispatch = ActionRouter::dispatch(
            &resp.action, &mut self.host, &self.snap,
            &mut self.crud, &self.navigator, &mut self.commit,
            &mut prior_audits,
            (resp.tokens_in + resp.tokens_out),
            &checkpoint,
        ).await?;
        let rec = StepRecord {
            step: step_num, view: prior_view.clone(),
            action: resp.action.clone(), action_result: dispatch.result,
            reward: dispatch.scalar, reward_components: dispatch.reward_vec,
            elapsed_ms: step_t0.elapsed().as_millis() as u64,
            model_tokens_in: resp.tokens_in, model_tokens_out: resp.tokens_out,
        };
        self.trajectory.append(&rec)?;
        prior_view   = self.navigator.refresh(&prior_view, &dispatch.affected)?;
        prior_audits = dispatch.new_audit_baseline.unwrap_or(prior_audits);
        if let Action::DeclareDone { .. } = resp.action {
            return Ok(self.finalize(
                EpisodeOutcome::CompletedDeclareDone { final_reward: dispatch.scalar },
                started,
            ).await?);
        }
    }
}
```
VERIFY: `episode_loop_terminates_on_declare_done`.

**Step 11 — Action router.** WHERE: `router.rs`.
```rust
pub struct DispatchOutcome {
    pub result: ActionResult, pub reward_vec: RewardVector, pub scalar: f32,
    pub affected: Vec<NodeId>, pub new_audit_baseline: Option<AuditCounts>,
}

impl ActionRouter {
    pub async fn dispatch(action: &Action, host: &mut WorkspaceHost, snap: &OpenedSnapshot,
                           crud: &mut Crud, navigator: &Navigator, commit: &mut Commit<'_>,
                           prior_audits: &mut AuditCounts, tokens: u32, checkpoint: &Checkpoint)
        -> Result<DispatchOutcome>
    {
        match action {
            Action::Navigate(step) => {
                let res = navigator.step(step)?;
                Ok(DispatchOutcome {
                    result: ActionResult::Navigated(res),
                    reward_vec: RewardVector { compile_ok: 1.0, ..Default::default() },
                    scalar: 0.0, affected: vec![], new_audit_baseline: None,
                })
            }
            Action::Simulate(op) => {
                let sim = crud.simulate(op)?;
                Ok(DispatchOutcome {
                    result: ActionResult::Simulated(sim),
                    reward_vec: RewardVector { compile_ok: 1.0, ..Default::default() },
                    scalar: -0.001, affected: vec![], new_audit_baseline: None,
                })
            }
            Action::Crud(op) => {
                let edit = crud.apply(op).await?;
                let gate_outcome = host.evaluate_gates(&edit)?;
                if gate_outcome.refused {
                    host.restore(checkpoint)?;
                    return Ok(DispatchOutcome {
                        result: ActionResult::Refused(gate_outcome.reason),
                        reward_vec: RewardVector { compile_ok: 0.0, ..Default::default() },
                        scalar: -1.0, affected: vec![], new_audit_baseline: None,
                    });
                }
                let dirty_crates = host.crates_of(&edit.affected_items);
                let cr = commit.run(&edit, &gate_outcome, checkpoint,
                                     &dirty_crates, tokens, false).await?;
                let new_baseline = AuditDeltaComputer::capture(snap, &edit.affected_items).ok();
                Ok(DispatchOutcome {
                    result: ActionResult::Committed { passed: cr.passed },
                    reward_vec: cr.reward, scalar: cr.scalar,
                    affected: edit.affected_items, new_audit_baseline: new_baseline,
                })
            }
            Action::Commit => {
                Ok(DispatchOutcome {
                    result: ActionResult::NoOp,
                    reward_vec: RewardVector { compile_ok: 1.0, ..Default::default() },
                    scalar: 0.0, affected: vec![], new_audit_baseline: None,
                })
            }
            Action::DeclareDone { summary: _ } => {
                let edit = EditOutcome::empty();
                let gate_outcome = GateOutcome::neutral();
                let dirty_crates = host.all_workspace_crate_names();
                let cr = commit.run(&edit, &gate_outcome, checkpoint,
                                     &dirty_crates, tokens, /*force_full*/ true).await?;
                Ok(DispatchOutcome {
                    result: ActionResult::DeclaredDone,
                    reward_vec: cr.reward, scalar: cr.scalar,
                    affected: vec![], new_audit_baseline: None,
                })
            }
            Action::AskNoOp => {
                Ok(DispatchOutcome {
                    result: ActionResult::NoOp,
                    reward_vec: RewardVector { compile_ok: 1.0, ..Default::default() },
                    scalar: -0.01, affected: vec![], new_audit_baseline: None,
                })
            }
        }
    }
}
```
VERIFY: `router_crud_refused_rolls_back`.

**Step 12 — Trajectory writer.** WHERE: `trajectory.rs`. JSONL one record per line; first line `TrajectoryHeader { task, started_at_unix }`; intermediate `StepRecord`; final `TrajectoryFooter { outcome, ended_at_unix }`. Mirror Phase 2 SFT format.
```rust
pub struct TrajectoryRecorder {
    file: Mutex<File>,
    path: PathBuf,
    steps_buffered: Mutex<Vec<StepRecord>>,
    secret_mask: regex::Regex,    // /sk-[A-Za-z0-9_-]{20,}/
}

impl TrajectoryRecorder {
    pub fn append(&self, rec: &StepRecord) -> Result<()> {
        let sanitized = rec.clone();
        let line = serde_json::to_string(&sanitized)?;
        let masked = self.secret_mask.replace_all(&line, "<REDACTED-KEY>");
        let mut f = self.file.lock().unwrap();
        f.write_all(masked.as_bytes())?;
        f.write_all(b"\n")?;
        f.flush()?;
        drop(f);
        self.steps_buffered.lock().unwrap().push(rec.clone());
        Ok(())
    }
    pub fn history_view(&self) -> Vec<StepRecord> {
        self.steps_buffered.lock().unwrap().clone()
    }
}
```
VERIFY: `trajectory_writes_one_line_per_step`.

**Step 13 — Model client + Anthropic prompt caching.** WHERE: `model_client.rs`.
```rust
#[async_trait::async_trait]
impl ModelClient for AnthropicClient {
    async fn next_action(&self, view: &ContextView, history: &[StepRecord], task: &TaskSpec)
        -> Result<NextActionResponse>
    {
        let system_blocks = vec![
            json!({
                "type": "text", "text": SYSTEM_PROMPT,
                "cache_control": { "type": "ephemeral" }       // breakpoint 1
            }),
            json!({
                "type": "text", "text": serde_json::to_string(&task)?,
                "cache_control": { "type": "ephemeral" }       // breakpoint 2
            }),
        ];
        let tools = json!([
            { "name": "navigate",     "description": "Move the navigation cursor.",
              "input_schema": NAV_SCHEMA, "cache_control": { "type": "ephemeral" } },
            { "name": "crud",         "description": "Apply a structural CRUD op.",
              "input_schema": CRUD_SCHEMA },
            { "name": "simulate",     "description": "Dry-run a CRUD op; no state mutation.",
              "input_schema": CRUD_SCHEMA },
            { "name": "commit",       "description": "Force gate+commit evaluation.",
              "input_schema": {} },
            { "name": "declare_done", "description": "Finalize episode with full test pass.",
              "input_schema": DONE_SCHEMA },
        ]);
        let messages = build_messages(view, history);
        let body = json!({
            "model": self.model, "max_tokens": 4096,
            "system": system_blocks, "tools": tools,
            "tool_choice": { "type": "any" },
            "messages": messages,
        });
        let resp = self.http.post(&self.endpoint)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&body).send().await?.error_for_status()?
            .json::<serde_json::Value>().await?;
        let tokens_in  = resp["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
        let tokens_out = resp["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
        let action = parse_action_from_tool_use(&resp)?;
        Ok(NextActionResponse { action, tokens_in, tokens_out })
    }
}
```

**Model selection:**
- `claude-opus-4-7` — default; best multi-step reasoning for pilot.
- `claude-sonnet-4-6` — cost-optimized; via `--model claude-sonnet-4-6`.

**Prompt caching:** mark `SYSTEM_PROMPT` + task block + tools schema (and initial ContextView when fits) as ephemeral breakpoints. First turn pays full; subsequent turns within 5-min TTL replay cached prefix at ~10% input-token cost. 50-step episodes → 5–10× cost reduction.

**Retries:** network/5xx → exponential backoff `max_retries = 3`. 4xx (401, 429) → fail-fast → `HardFailure`.

VERIFY: `model_client_parses_tool_use`, `anthropic_client_uses_prompt_cache` (mock HTTP server captures outgoing body; assert `cache_control: ephemeral` on system + tools).

**Step 14 — CLI binary `rmc-rl run`.** WHERE: `crates/rmc-rl/src/main.rs`.
```rust
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rmc-rl")]
struct Cli { #[command(subcommand)] cmd: Cmd }

#[derive(Subcommand)]
enum Cmd {
    Run {
        #[arg(long)] task: PathBuf,
        #[arg(long, default_value = "claude-opus-4-7")] model: String,
        #[arg(long, default_value_t = 50)] budget: u32,
        #[arg(long)] workspace: PathBuf,
        #[arg(long, default_value = "600")] max_wall_secs: u64,
        #[arg(long, default_value_t = 200_000)] max_tokens: u64,
        #[arg(long)] weights: Option<PathBuf>,
        #[arg(long)] session_id: Option<String>,
    },
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { task, model, budget, workspace, max_wall_secs, max_tokens, weights, session_id } => {
            let task_spec: TaskSpec = toml::from_str(&std::fs::read_to_string(&task)?)?;
            let api_key = std::env::var("ANTHROPIC_API_KEY")?;
            let client = AnthropicClient {
                model, api_key,
                endpoint: "https://api.anthropic.com/v1/messages".into(),
                http: reqwest::Client::new(), max_retries: 3,
            };
            let session = session_id.unwrap_or_else(|| format!("rl-{}", uuid::Uuid::new_v4()));
            let budget = StepBudget { max_steps: budget, max_tokens, max_wall_secs };
            let weights = match weights {
                Some(p) => Scalarizer::from_toml(&p)?.weights,
                None    => RewardWeights::default(),
            };
            let episode = Episode::new(task_spec, client, budget, &workspace, &session, weights).await?;
            let traj = episode.run().await?;
            let mut total = 0.0;
            for s in &traj.steps { total += s.reward; }
            println!("session:           {}", session);
            println!("outcome:           {:?}", traj.outcome);
            println!("steps:             {}", traj.steps.len());
            println!("cumulative_reward: {:.4}", total);
            println!("trajectory_path:   working/{}/trajectory.jsonl", session);
        }
    }
    Ok(())
}
```

Invocation:
```
nix develop ../nix-devshells#cuda-code --command cargo run -p rmc-rl --release -- \
    run --task tasks/dedupe_project_paths.toml \
        --model claude-opus-4-7 --budget 50 \
        --workspace /home/molaco/Documents/rust-code-mcp-refactor
```

VERIFY: `cli_runs_dry_pilot` with FakeModelClient feature-flag.

**Step 15 — Seed task TOML.** WHERE: `tasks/dedupe_project_paths.toml`.
```toml
id = "dedupe-project-paths"
goal_prompt = """
The crates rmc-server and rmc-indexing each define a `project_paths` struct
that holds the same information. Collapse the two definitions into one,
re-export it from both crates, and update all usages so the workspace still
compiles and all tests pass. Do not introduce new unsafe, unwrap, or todo.
"""

[initial_loc]
kind = "item"
qualified_name = "rmc_server::project_paths::ProjectPaths"

[success_criteria]
all_tests_pass = true
disallowed_patterns = ["todo!()", "panic!"]

[success_criteria.max_audit_regressions]
unsafe_max = 0
unwrap_max = 0
missing_docs_max = 5
```

**Step 16 — End-to-end fake-model test.** WHERE: `crates/rmc-episode/tests/e2e_fake_model.rs`.
```rust
struct FakeModel { script: Mutex<Vec<Action>> }
#[async_trait::async_trait]
impl ModelClient for FakeModel {
    async fn next_action(&self, _v: &ContextView, _h: &[StepRecord], _t: &TaskSpec)
        -> Result<NextActionResponse>
    {
        let action = self.script.lock().unwrap().remove(0);
        Ok(NextActionResponse { action, tokens_in: 100, tokens_out: 50 })
    }
}

#[tokio::test]
async fn dedupe_pilot_with_fake_model() {
    let script = vec![
        Action::Navigate(NavStep::Goto("rmc_server::project_paths::ProjectPaths".into())),
        Action::Simulate(CrudOp::ModifyBody(BodyEdit { /* ... */ })),
        Action::Crud(CrudOp::ModifyBody(BodyEdit { /* ... */ })),
        Action::DeclareDone { summary: "merged".into() },
    ];
    let traj = ep.run().await.unwrap();
    assert!(matches!(traj.outcome, EpisodeOutcome::CompletedDeclareDone { final_reward } if final_reward > 0.0));
    assert_eq!(traj.steps.len(), 4);
}
```

## Tests

- **`commit_rollback_on_compile_break`** (`crates/rmc-reward/tests/commit_rollback.rs`) — body with `let _: u32 = "string";`. `Commit::run` → `passed = false`, `rollback_executed = true`, scalar = -1.0, files byte-identical to pre-edit.
- **`cargo_gate_warm_under_2s`** (criterion bench) — 10 sequential small body edits with persistent `CARGO_TARGET_DIR`; `cargo check` between each; p95 < 2s after warmup.
- **`ra_fast_gate_under_100ms`** (criterion bench) — same 10 edits in `RaOnly`; p95 < 100ms.
- **`metric_delta_local_only`** — 1000-node synthetic crate graph; edit one node; assert internal `visit_count < 100` (10%); resulting betweenness matches full recompute within 1e-3.
- **`scalarizer_hard_floor`** — `compile_ok = 0` returns exactly -1.0.
- **`episode_records_full_trajectory`** — fake model emits 5 actions ending `DeclareDone`; JSONL has header + 5 step records + footer (7 lines).
- **`episode_respects_budget`** — fake model never done; budget = 10 → `BudgetExhausted { reason: "max_steps" }`.
- **`episode_respects_wall_clock`** — fake model sleeps 100ms/action; `max_wall_secs = 1` → `BudgetExhausted { reason: "wall_clock" }`.
- **`router_crud_refused_rolls_back`** — gate threshold violation → `ActionResult::Refused`, scalar -1.0, snapshot reverted.
- **`trajectory_pii_masking`** — inject `sk-ABC...` into stub `CargoDiagnostic.message`; assert `sk-` does not appear in JSONL.
- **`anthropic_client_uses_prompt_cache`** — mock HTTP captures outgoing body; assert `cache_control: ephemeral` on system + tools.
- **`anthropic_client_retries_5xx`** — mock returns 503 twice then 200; client retries with backoff.
- **`dedupe_project_paths_pilot`** (`#[ignore]` requires `ANTHROPIC_API_KEY`) — runs seed task with `claude-opus-4-7`; trajectory exists, outcome `CompletedDeclareDone` or `BudgetExhausted` with non-trivial partial; manual review for grading.
- **`declare_done_runs_full_tests`** — fixture with 3 passing + 1 failing test; `CargoGateOutcome.test_results.passed == 3, .failed == 1`; `test_pass_rate == 0.75`.
- **`commit_audit_delta_unsafe_increment`** — body edit adds `unsafe { *ptr }`; `audit_delta.unsafe_added == 1`.
- **`ra_misses_proc_macro_force_fallback`** — edit `build.rs`; next gate upgrades RA → full `cargo check` regardless of step counter.

## Open decisions / risks

- **Cargo gate mode (RESOLVED):** Default `RaPlusCheckEveryK { k: 5 }`. Calibrate `k` upward to 10 if telemetry shows RA false-negatives rare; down to 3 if frequent. Operator override per-task TOML or per-session `--gate-mode` (deferred).
- **Credit assignment.** Compile breaks reported at commit time may be consequence of moves 5 steps earlier. Per-step + final-episode reward both recorded; Phase 2 RL trainer handles credit assignment (TD(λ), GAE).
- **Reward weights.** Defaults conservative (compile dominates, audits 0.1 each, soft -0.05). `RewardWeights::from_toml` lets operators ablate without recompile.
- **Goodhart on metrics.** Modularity/conductance gaming (trivial extractions inflating community count). Deferred per source plan. Mitigation when observed: cap `metric_term` in `Scalarizer` or require sustained-multi-step improvement.
- **Model choice.** `claude-opus-4-7` for pilot; `claude-sonnet-4-6` for cheap batched runs in Phase 2. Selector lives in CLI flag; protocol identical.
- **Prompt-cache breakpoints.** Two ephemeral (system + task). When initial ContextView fits 1024-token cacheable block, add third. 5-min TTL fine for typical episodes (10–60s).
- **PII / source content.** Secrets masked via regex before writing. Source-code excerpts in `ContextView` NOT masked (they're the dataset). For third-party customer code, add per-snippet redaction policy.
- **External tasks via TOML.** Single supported input format. `TaskSpec` doubles as Phase 2 dataset key; structured task sources (DB, API) feed through `TaskSpec` directly.
- **Cargo concurrency.** Multiple concurrent episodes against same workspace → corrupt shared `CARGO_TARGET_DIR`. Each session gets `working/<session_id>/cargo-target`. Disk cost: 1–3 GB per concurrent episode; cap concurrency in orchestrator.
- **Test discovery on non-nightly.** `--format json --report-time` requires `-Z unstable-options`. If stable channel, parse plain "test foo::bar ... ok" lines (`parse_libtest_plain`).
- **`Commit::run` blocks the loop.** Cargo gates async (`tokio::process`); RA queries CPU-bound on `tokio::task::spawn_blocking`. Per-episode wall-clock bounded by `StepBudget`. Multi-episode parallelism is orchestrator concern.
- **Determinism of Trajectory.** RA query ordering may differ run-to-run; per P0.1 invariant, reward-bearing fields must be stable. Cross-check: re-running same FakeModel script produces identical reward components within 1e-6.


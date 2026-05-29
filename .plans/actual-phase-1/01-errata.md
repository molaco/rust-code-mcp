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

**`EditError` is a `thiserror`-derived enum** (library crate — no `anyhow`;
`anyhow` is confined to the `rmc-spikes`/`rmc-rl` binaries). It pins the
existing workspace `thiserror = "1"`. The variants distinguish a *transient
I/O / host fault* (retryable, source preserved) from a *domain refusal*
(the gate intentionally said no — not an error to retry):

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EditError {
    /// Hard gate refused the edit before anything was written. Not retryable.
    #[error("edit refused by hard gates")]
    Refused(Vec<RefusalReason>),
    /// Host rejected the write (e.g. cargo/test gate failed after persist);
    /// checkpoint already rolled back.
    #[error("host rejected the edit")]
    HostRejected(#[source] HostError),
    /// Transient filesystem/IO fault while writing source. Retryable.
    #[error("io error editing {path}")]
    Io { path: PathBuf, #[source] source: std::io::Error },
}
```

`#[from]` is used for the unambiguous transient cases (e.g. wrapping a
`HostError` where there is a single host-error arm); the path-carrying `Io`
variant uses an explicit `#[source]` so the failing file is preserved
alongside the chained `std::io::Error`. Callers branch on the variant:
`Refused` is terminal, `HostRejected`/`Io` may be retried.

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
stored. **The primary resolution is scoped per-action construction** —
build `Commit`/`Crud`/`Navigator` from `Episode`'s owned fields inside
`step()`, so each `&mut self.host` borrow is short-lived and the borrow
checker sequences them naturally. Because `step()` dispatches one verb at a
time, the constructs do not actually need *simultaneous* `&mut self.host`;
sequence the borrows (build `Crud`, run it, drop it, then build `Commit`)
rather than holding both at once.

`Arc<Mutex<WorkspaceHost>>` is a **last-resort fallback only**, to be used
only if a future verb genuinely requires two live `&mut host` handles in the
same scope. It contradicts the guideline preference for scoped ownership
over shared mutable state (§6/§12): it adds a lock whose poisoning must be
handled and reintroduces interior-mutability hazards for no benefit in a
single-threaded loop. Prefer `RefCell<WorkspaceHost>` (single-threaded
interior mutability, no lock) over `Arc<Mutex<…>>` if interior mutability is
unavoidable; reach for the `Arc<Mutex<…>>` only when the host must also cross
threads. Section J §9 (`Episode::new`) is revised: store only owned fields;
never store a borrowing struct, and do not preemptively wrap the host in a
lock.

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
`crud.workspace_root.join(...)` — pass the relative path instead.

**Preferred enforcement: a `RelativePath` newtype validated on
construction** so the invariant holds in *release* as well as debug builds
(a `debug_assert!` is stripped in release and would let an absolute path
slip through a production episode). Make the field's type carry the
guarantee rather than re-checking strings:

```rust
/// A workspace-relative path. The inner `PathBuf` is private; the only way
/// to obtain one is via `TryFrom`, which rejects absolute paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativePath(PathBuf);

impl TryFrom<PathBuf> for RelativePath {
    type Error = NotRelative;
    fn try_from(path: PathBuf) -> Result<Self, Self::Error> {
        if path.is_relative() { Ok(Self(path)) } else { Err(NotRelative(path)) }
    }
}

impl RelativePath {
    pub fn as_path(&self) -> &Path { &self.0 }
}

impl FileEdit {
    /// `path` is guaranteed workspace-relative by the `RelativePath` type.
    pub fn new(path: RelativePath, new_text: String, edit_class: EditClass) -> Self {
        Self { path, new_text, edit_class }
    }
}
```

This makes the absolute-path state unrepresentable (§7) — callers cannot
construct a `FileEdit` from an absolute path at all. If introducing the
newtype is deferred, the interim guard must be a real runtime check that
returns an error (not a release-stripped `debug_assert!`). Test
`file_edit_relative_path_invariant` still asserts every emitted edit's path
is relative.

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
    .filter(|(_, n)| n.crate_id.is_some_and(|c| scope_crates.contains(&c)))
    .map(|(k, n)| {
        // Local invariant: every key in `nodes_by_id` is a 16-byte NodeId,
        // written only by our own emitter. A wrong length means LMDB
        // corruption, not user input — surface it loudly rather than
        // silently truncating.
        let bytes: [u8; 16] = k.try_into()
            .expect("nodes_by_id key is always a 16-byte NodeId");
        (NodeId::from_bytes_arr(bytes), n)
    })
    .collect();
```

(`.is_some_and(…)` replaces the older `.map_or(false, …)`; the `expect`
documents the 16-byte key invariant. If LMDB-corruption is to be treated as
recoverable rather than a panic, return `HostError::CorruptKey` via `?`
instead — see Section C's `HostError`.)

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


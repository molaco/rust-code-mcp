# Section I — P1.4 Counterfactual Simulator + P1.6 Write-Time Gates

## Overview

P1.4 and P1.6 are co-designed because both consume the D2 dirty-set classification produced by the apply engine and both run *before* the heavy cargo gate (P1.7). The simulator (P1.4) is a dry-run mode of the CRUD verbs from P1.5 — it must share apply's *exact* effect-computation logic, or it lies and the agent reasons against fiction. The gates (P1.6) are the pre-commit hard/soft filter that runs existing audits (`fn_body_audit`, `unsafe_audit`, `recursion_check`, `mut_static_audit`, `derive_audit`, `missing_docs_audit`, `channel_capacity_audit`, `forbidden_dependency_check`) scoped to the D2 dirty set.

The two slices share infrastructure (a `Baseline` snapshot of per-item metrics captured once per episode, dirty-set audit wrappers, threshold/allowlist config) and sit structurally between `rmc-crud` (P1.5) and `rmc-reward` (P1.7). Gates live in their own crate `rmc-gates` so they're independently testable, callable from `Crud::simulate`, `Crud::apply`, and any future REPL/query tool.

**Source-plan key insight (P1.4):** *"simulate must share apply's exact logic or it lies — hence built after, as a mode of, P1.5"*. Plan: factor `Crud::apply` into `compute_effects() + persist()`; `simulate()` is the `compute_effects()`-only path.

## New modules / files

- `crates/rmc-crud/src/simulate.rs` — `Crud::simulate(op) -> SimulateOutcome`; assembles cascade preview + token-cost estimate; delegates to `effects::compute_effects` + `GateRunner::evaluate`.
- `crates/rmc-crud/src/effects.rs` — `Effects` struct + free `compute_effects(host, op)` dispatching per verb.
- `crates/rmc-crud/src/apply.rs` (refactor of P1.5 verbs) — each verb becomes `pub(crate) fn persist(host, effects, checkpoint) -> Result<EditOutcome, CrudError>`. `Crud::apply` = `compute_effects → GateRunner::evaluate → if hard refusal → bail; else persist → commit`.
- `crates/rmc-gates/Cargo.toml` — new crate. Deps: `rmc-graph`, `rmc-config`, `petgraph`, `toml`, `serde`, `thiserror` (workspace `"1"`). No `anyhow` — `rmc-gates` is a library; it returns the typed `GateError` (below). `anyhow` stays confined to the binaries (`rmc-spikes`, `rmc-rl`).
- `crates/rmc-gates/src/lib.rs` — `GateRunner`, `GateOutcome`, `RefusalReason`, `RefusalCode`, `Severity`.
- `crates/rmc-gates/src/config.rs` — `GatesConfig { thresholds: GateThresholds, allowlist: ForbiddenDepAllowlist }` + the *single* validated `gates.toml` loader `load_from_workspace_root`. One file read, deserialized once, then validated (e.g. `soft_* <= max_*`). Returns `Result<GatesConfig, GateConfigError>` (thiserror). `thresholds.rs`/`allowlist.rs` no longer read the file themselves.
- `crates/rmc-gates/src/thresholds.rs` — `GateThresholds` type + defaults only (no I/O).
- `crates/rmc-gates/src/allowlist.rs` — `ForbiddenDepAllowlist { rules: Vec<ForbiddenDependencyRule> }` type + baseline-merge logic only (no I/O), reusing `rmc_graph::query::model::ForbiddenDependencyRule`.
- `crates/rmc-gates/src/baseline.rs` — `Baseline::capture(snap, loaded)` — episode-start per-item complexity, LOC, had_unsafe, crate_dep_set.
- `crates/rmc-gates/src/audits.rs` — dirty-set audit wrappers.
- `crates/rmc-gates/src/complexity.rs` — `compute_complexity(body_text)` via `ra_ap_syntax` walking.
- `crates/rmc-gates/src/cycle.rs` — `detect_new_module_cycles`, `detect_new_crate_cycles` using `petgraph::algo::tarjan_scc`; both return `Result<_, GateError>` so `evaluate` can `?` them.
- `crates/rmc-gates/src/dirty_dep_filter.rs` — filter `crate_edges` by dirty set; feed `forbidden_dependency_check`.
- `gates.toml` at workspace root — sample config with all thresholds at defaults + starter `[forbidden_dependencies]` table. Read-only to agent.

## Type definitions

```rust
// crates/rmc-crud/src/effects.rs

#[non_exhaustive]
pub struct Effects {
    source_edits: Vec<FileEdit>,
    edit_class: EditClass,
    estimated_affected_items: Vec<NodeId>,
    estimated_graph_diff: GraphDiffSummary,
    would_refuse: Vec<RefusalReason>,
}
// Constructed inside `compute_effects`; read via accessors
// (`source_edits()`, `edit_class()`, `estimated_affected_items()`,
// `estimated_graph_diff()`, `would_refuse()`). `would_refuse` is set by
// the gate fold in `simulate` (Step 2) via a `set_would_refuse(..)` setter.

// File path of a read-only edit. The read view never mutates it, so a
// workspace-relative path newtype (validated relative at construction)
// is enough — no raw `String`.
pub struct FileEdit { file: RelativePath, range: (u32, u32), replacement: Vec<u8> }
// `RelativePath` is the workspace newtype from errata E3; accessors
// `file()`, `range()`, `replacement()`.

#[non_exhaustive]
pub struct GraphDiffSummary {
    pub nodes_added: usize, pub nodes_removed: usize,
    pub bindings_added: usize, pub bindings_removed: usize,
    pub usages_added: usize, pub usages_removed: usize,
}
```

```rust
// crates/rmc-crud/src/simulate.rs

#[non_exhaustive]
pub struct SimulateOutcome {
    pub effects: Effects,
    pub cascade_preview: Vec<CascadeStep>,
    pub estimated_token_cost: usize,
}

pub struct CascadeStep { pub kind: CascadeKind, pub node: NodeId, pub reason: String }

/// Closed set of follow-up operations a cascade can predict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum CascadeKind {
    ReResolveUsage,  // signature/visibility changed → re-resolve a usage
    FixUsePath,      // item added/removed/moved → fix a `use` path
    FullReExtract,   // macro / cargo edit → coarse full re-extract
}
```

```rust
// crates/rmc-gates/src/thresholds.rs

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
#[non_exhaustive]
pub struct GateThresholds {
    max_complexity: u32,          // default 20 (cyclomatic, McCabe)
    max_params: u32,              // default 7
    max_fn_loc: u32,              // default 60
    max_nesting: u32,             // default 4
    max_unwrap_per_fn: u32,       // default 0 in production crates
    forbid_unsafe_introduce: bool,// default true
    forbid_new_cycles: bool,      // default true
    soft_complexity: u32,         // default 12
    soft_fn_loc: u32,             // default 40
    soft_unwrap: u32,             // default 0
}
// Fields are private; serde populates them at deserialize time and the
// validated loader (Step 8) is the only other constructor. Read via
// accessors (`max_complexity()`, `soft_complexity()`, …).

impl Default for GateThresholds { fn default() -> Self { /* above */ } }
```

```rust
// crates/rmc-gates/src/lib.rs

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum RefusalCode {
    ComplexityTooHigh, FnTooLong, TooManyParams, NestingTooDeep,
    TooManyUnwraps, UnsafeIntroduced,
    ModuleCycleIntroduced, CrateCycleIntroduced,
    ForbiddenDependency, MissingDocsOnPub,
    MutableStaticIntroduced,   // mut_static_audit (Overview audit list)
    UnboundedChannel,          // channel_capacity_audit (Overview audit list)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum Severity { Hard, Soft }

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct RefusalReason {
    code: RefusalCode,
    severity: Severity,
    item: NodeId,
    threshold: Option<u32>,
    actual: Option<u32>,
    message: String,         // includes file:span + "what to do" hint
}
// Built by the `refusal(..)` constructors below; read via accessors
// (`code()`, `severity()`, `item()`, `threshold()`, `actual()`, `message()`).

/// Typed gate error (`rmc-gates` is a library — no `anyhow`).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GateError {
    /// A graph/snapshot query failed while reading dirty-set state.
    #[error("snapshot query failed")]
    Query(#[source] rmc_graph::Error),
    /// An underlying audit failed.
    #[error("audit failed")]
    Audit(#[source] rmc_graph::Error),
    /// Reading the would-be post-edit body failed.
    #[error("could not read post-edit body for {item:?}")]
    PostEditBody { item: NodeId },
    /// Cycle projection / petgraph SCC stage failed.
    #[error("cycle detection failed")]
    Cycle(#[source] rmc_graph::Error),
}

pub struct GateRunner<'a> {
    snap: &'a OpenedSnapshot,
    loaded: &'a LoadedWorkspace,
    thresholds: GateThresholds,
    allowlist: ForbiddenDepAllowlist,
    baseline: &'a Baseline,
}
// Constructed via `GateRunner::new(snap, loaded, thresholds, allowlist, baseline)`;
// fields private so callers cannot swap a stale baseline/allowlist mid-evaluate.

impl<'a> GateRunner<'a> {
    /// # Errors
    /// Returns [`GateError`] if a snapshot/audit/cycle query fails. A refused
    /// edit is *not* an error — it is reported in [`GateOutcome::hard`].
    pub fn evaluate(&self, dirty: &[NodeId], effects: &Effects)
        -> Result<GateOutcome, GateError>;
}

#[non_exhaustive]
pub struct GateOutcome {
    pub hard: Vec<RefusalReason>,
    pub soft: Vec<RefusalReason>,
}

impl GateOutcome {
    /// `true` when there are no hard refusals. Derived from `hard` so it can
    /// never desync.
    #[must_use]
    pub fn passed(&self) -> bool { self.hard.is_empty() }
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
   pub(crate) fn compute_effects(host: &WorkspaceHost, op: &ModifyBodyOp) -> Result<Effects, CrudError>;
   pub(crate) fn persist(host: &mut WorkspaceHost, effects: Effects, checkpoint: &mut CheckpointBuilder) -> Result<EditOutcome, CrudError>;
   ```
   `compute_effects` takes `&WorkspaceHost` (immutable — no `set_file_text`, no LMDB writes); computes byte range via `OpenedSnapshot::span_index()` + `Node.span`; builds the value through the constructor `Effects::new(source_edits, edit_class, /* estimated_affected_items */ vec![op.target], GraphDiffSummary::default())` (private fields; `would_refuse` starts empty and is filled later via `set_would_refuse`). `persist` does RA `set_file_text` + scoped re-extract + LMDB diff-patch. `Crud::apply` becomes:
   ```rust
   pub fn apply<O: CrudOp>(&mut self, op: O) -> Result<EditOutcome, CrudError> {
       let effects = O::compute_effects(&self.host, &op)?;
       let dirty = effects.estimated_affected_items().to_vec();
       // `GateError` → `CrudError::Gate(#[from] GateError)`.
       let gate_outcome = self.gates.evaluate(&dirty, &effects)?;
       if !gate_outcome.passed() { return Err(CrudError::Refused(gate_outcome.hard)); }
       let mut checkpoint = self.host.begin_checkpoint()?;
       let edit_outcome = O::persist(&mut self.host, effects, &mut checkpoint)?;
       checkpoint.commit()?;
       Ok(edit_outcome)
   }
   ```
   DEPENDS: P0.2 `apply_edits`, D4 `Checkpoint`, D2 `EditClass`, P1.5 verb impls. VERIFY: every verb's existing apply test passes after split.

2. **`Crud::simulate(op) -> Result<SimulateOutcome, CrudError>`.** WHERE: `simulate.rs`.
   ```rust
   pub fn simulate<O: CrudOp>(&self, op: O) -> Result<SimulateOutcome, CrudError> {
       let mut effects = O::compute_effects(&self.host, &op)?;
       let dirty = effects.estimated_affected_items().to_vec();
       let gate_outcome = self.gates.evaluate(&dirty, &effects)?;
       effects.set_would_refuse(
           gate_outcome.hard.iter().chain(gate_outcome.soft.iter()).cloned().collect());
       let cascade_preview = predict_cascade(&self.host, &effects)?;
       let estimated_token_cost = estimate_context_view_cost(&self.host, &effects)?;
       Ok(SimulateOutcome { effects, cascade_preview, estimated_token_cost })
   }
   ```
   Same `compute_effects` apply uses — by construction simulate cannot diverge.

3. **Cascade preview.** WHERE: `simulate.rs::predict_cascade`. Walks D2's affected-set graph. Per `effects.edit_class()`:
   - `BodyOnly` → no cascade.
   - `SignatureOrVis` → for each NodeId in reverse-deps (from `who_uses` + `who_imports`), emit `CascadeStep { kind: CascadeKind::ReResolveUsage, node, reason: "signature changed" }`.
   - `ItemAddRemove` → `CascadeKind::FixUsePath` per consumer module.
   - `ModuleTree` → `CascadeKind::FixUsePath` for every `Binding` whose `target` lies under moved subtree.
   - `Macro` / `CargoManifest` → single coarse `CascadeKind::FullReExtract`.
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

8. **Single validated TOML loader.** WHERE: `config.rs::load_from_workspace_root`. Reads `<workspace>/gates.toml` **once** and deserializes the whole file into one `GatesConfig { thresholds: GateThresholds, allowlist: ForbiddenDepAllowlist }` (thresholds from the top-level keys via `#[serde(default)]`, allowlist from the `[forbidden_dependencies]` table). Missing file → `GatesConfig::default()` (defaults + baseline rules). After deserializing, **validate**: every `soft_*` must be `<= ` its `max_*` counterpart (`soft_complexity <= max_complexity`, `soft_fn_loc <= max_fn_loc`, `soft_unwrap <= max_unwrap_per_fn`); on violation return `GateConfigError::SoftExceedsHard { field, soft, hard }`. This replaces the two former independent reads in `thresholds.rs` + `allowlist.rs` — there is now a single read with cross-field validation, so soft/hard can never be configured inconsistently.
   ```rust
   #[derive(Debug, thiserror::Error)]
   #[non_exhaustive]
   pub enum GateConfigError {
       #[error("could not read gates.toml")]
       Read(#[source] std::io::Error),
       #[error("could not parse gates.toml")]
       Parse(#[from] toml::de::Error),
       #[error("soft threshold for `{field}` ({soft}) exceeds hard limit ({hard})")]
       SoftExceedsHard { field: &'static str, soft: u32, hard: u32 },
   }
   ```
   Sample `gates.toml`:
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
   VERIFY: `loader_returns_defaults_on_missing_file`, `loader_parses_sample_gates_toml`, `loader_rejects_soft_above_hard`.

9. **Allowlist baseline merge.** WHERE: `allowlist.rs::ForbiddenDepAllowlist::with_baseline` (pure — no file read; the `[forbidden_dependencies]` table is already deserialized by the single loader in Step 8). Merge the deserialized rules with the hard-coded baseline:
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
    pub fn evaluate(&self, dirty: &[NodeId], effects: &Effects)
        -> Result<GateOutcome, GateError> {
        let mut hard = Vec::new();
        let mut soft = Vec::new();
        // 1. Per-fn metrics on each dirty function.
        for &node in dirty {
            let item = self.snap.node(&self.snap.read_txn()?, node)
                .map_err(GateError::Query)?;
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
        Ok(GateOutcome { hard, soft })   // `passed()` is derived from `hard`
    }
    ```
    `read_body_post_edit` overlays `effects.source_edits()` onto current source before measuring — gates evaluate would-be state without writing.

11b. **Dirty-set audit wrappers (Phase 1 fallback).** WHERE: `audits.rs`. Run workspace-wide audit, post-filter:
    ```rust
    pub fn unsafe_findings_for(dirty: &[NodeId], snap: &OpenedSnapshot, loaded: &LoadedWorkspace,
                                _effects: &Effects) -> Result<Vec<UnsafeFinding>, GateError> {
        let dirty_set: HashSet<NodeId> = dirty.iter().copied().collect();
        let all = snap.unsafe_audit(loaded).map_err(GateError::Audit)?;
        Ok(all.into_iter()
            .filter(|f| f.enclosing_function.is_some_and(|n| dirty_set.contains(&n)))
            .collect())
    }
    ```
    Same shape (all `-> Result<_, GateError>`) for `fn_body_findings_for`, `recursion_cycles_for`, `mut_static_findings_for` (→ `RefusalCode::MutableStaticIntroduced`), `derive_findings_for`, `missing_docs_for`, `channel_capacity_findings_for` (→ `RefusalCode::UnboundedChannel`). Phase 2 grows each audit a `fn audit_only(crate_id: NodeId, items: &HashSet<NodeId>) -> Vec<Finding>` overload.

12. **Cycle detection.** WHERE: `cycle.rs`. `petgraph::algo::tarjan_scc`. **Module-cycle:**
    1. Project post-edit module-parent graph: start from `baseline.module_parent_edges`; apply `effects.source_edits()` (only when `effects.edit_class() == EditClass::ModuleTree`).
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
                            dirty: &[NodeId], effects: &Effects)
        -> Result<Vec<ForbiddenDependencyViolation>, GateError> {
        let touched_crates: HashSet<NodeId> = dirty.iter()
            .filter_map(|n| snap.node(&snap.read_txn()?, *n).ok().flatten()?.crate_id).collect();
        let all_violations = snap.forbidden_dependency_check(&allowlist.rules)
            .map_err(GateError::Audit)?;
        Ok(all_violations.into_iter()
            .filter(|v| /* consumer/producer maps back to touched_crate */ true)
            .filter(|v| !baseline_had_violation(v))
            .collect())
    }
    ```
    Existing `forbidden_dependency_check` in `query/crates.rs` called as-is; wrapper narrows. VERIFY: `forbidden_dep_refused`.

14. **Aggregate.** Step 11's pseudocode. `GateOutcome::passed()` is derived from `hard` (no stored field). `soft` flows into P1.7 scalarizer.

15. **Wire `Crud::apply` to gate.** Step 1's `apply` body. Critical ordering:
    1. `compute_effects` (no I/O).
    2. `gates.evaluate(dirty, &effects)`.
    3. `!outcome.passed()` → `Err(CrudError::Refused(hard))`. NO checkpoint, NO source, NO LMDB.
    4. Else → `begin_checkpoint` → `persist` → `commit`.
    Guarantees refused op is byte-identical to never having been called. VERIFY: `simulate_predicts_refusal`.

16. **Wire `Crud::simulate` to gate.** Step 2.

## Tests

(`crates/rmc-crud/tests/`, `crates/rmc-gates/tests/`)

- **`simulate_equals_apply_for_modify_body`** (`crates/rmc-crud/tests/simulate_eq_apply.rs`) — `simulate(op).effects == apply(op).recorded_effects`. Repeat for every verb.
- **`simulate_predicts_refusal`** — `ModifyBodyOp` whose body is `fn foo() { unsafe { std::ptr::null::<u8>().read() } }` on fn with `baseline.had_unsafe == false`. Assert `simulate.effects.would_refuse()` contains a `RefusalReason` with `code() == RefusalCode::UnsafeIntroduced` and `severity() == Severity::Hard`. Then `apply(op)` → `Err(CrudError::Refused(..))`.
- **`hard_complexity_refuses`** — body with 25 `if` branches (cyclomatic = 26); `max_complexity = 20`; `evaluate` returns `Hard ComplexityTooHigh { threshold: 20, actual: 26 }`; `outcome.passed() == false`.
- **`soft_unwrap_warns`** — body with 1 `.unwrap()`; baseline 0. `max_unwrap_per_fn = 0, soft_unwrap = 0`: `hard` contains `Hard TooManyUnwraps`. Then `max_unwrap_per_fn = 1, soft_unwrap = 0`: `hard` empty, `soft` contains `Soft TooManyUnwraps`.
- **`cycle_introduced_refused`** — fixture with modules `a`, `b`; `MoveModuleOp` re-parenting `a` under `b` + `ModifyBodyOp` inserting `use crate::a::Foo` into `b`. Assert `ModuleCycleIntroduced` in `hard`.
- **`forbidden_dep_refused`** — allowlist `rmc-config !-> rmc-engine`; `ModifyBodyOp` inserting `use rmc_engine::EmbeddingGenerator;` into fn in `rmc-config`. Assert `ForbiddenDependency` in `hard`.
- **`dirty_set_audit_speed`** (`crates/rmc-gates/benches/dirty_audit.rs`) — **aspirational / `#[ignore]`** (see Open decisions: Phase 1 filters workspace-wide audits, so these latency targets are non-deterministic and likely fail until each audit grows an `audit_only` overload). 100k-LOC fixture; baseline captured once; *target* `evaluate(&[one_node], ...)` × 100 < 50ms each, 10 dirty nodes < 200ms each. Marked `#[ignore]` so CI does not gate on it; run explicitly with `cargo bench`/`-- --ignored`.
- **`baseline_does_not_count_pre_existing`** — fixture fn with 3 existing `.unwrap()`s. Capture baseline. `ModifyBodyOp` that doesn't touch any unwrap → 0 `TooManyUnwraps`. Then add 4th unwrap → 1 refusal with `actual: 1` (NEW count, not total 4).
- **`evaluate_uses_post_edit_body`** — existing body 0 unwraps; `effects.source_edits()` adds one. Gate runs against overlay (sees 1), not on-disk (still 0).
- **`gates_toml_round_trip`** — non-default values; `config.rs::load_from_workspace_root` parses the whole file into one `GatesConfig`; thresholds equal expected; missing fields default; allowlist includes baseline + TOML rules. (Soft-above-hard rejection covered by `loader_rejects_soft_above_hard`.)

## Open decisions / risks

- **Audit latency at write time.** Phase 1 "filter workspace-wide audit" is a stopgap — existing audits walk whole workspace (~100s of ms on 100k LOC). 50/200ms targets in `dirty_set_audit_speed` are aspirational and likely fail in Phase 1, so that bench is marked `#[ignore]` (run on demand, never CI-gating); Phase 2 needs each audit to grow `audit_only(crate_id, items)` before the targets become enforceable. Documented as known limit.
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


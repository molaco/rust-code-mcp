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


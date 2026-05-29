# Section J — P1.7 Commit/Reward + P1.8 Episode Runner

## Overview

This slice is the **integration milestone** of Phase 1. Every preceding piece — `WorkspaceHost` (P0.2), `Crud::apply` returning `EditOutcome` (P1.5a), `Checkpoint::take/restore` (D4), `GateRunner::evaluate` (P1.6), `Crud::simulate` (P1.4), `Navigator` (P1.1), `VisionIndex` (P1.3) — was scaffolding for the moment an actual model picks an action, the system applies it, scores it, and writes a step into a trajectory. M3 = "first end-to-end loop with `modify_body` only" — exactly what this delivers. Two new crates (`rmc-reward`, `rmc-episode`) plus the `rmc-rl` binary turn the static apply==rebuild engine into a closed-loop RL environment.

The dominant unsolved problem is **cargo gate latency** (Issues #2). The phase-1 plan lists four options without committing. This plan commits: **default = `CargoGateMode::RaPlusCheckEveryK { k: 5 }`.** RA-based type check on every step (~50ms), full `cargo check` every 5 steps (~1–2s warm), full `cargo test --workspace --no-fail-fast` only at `declare_done`. Justification + fallbacks in Open decisions.

## New modules / files

- `crates/rmc-reward/Cargo.toml` — new **library** crate (no `anyhow`). Deps: `rmc-graph`, `rmc-engine`, `tokio`, `serde`, `serde_json`, `petgraph`, `thiserror` (workspace `"1"`), `tracing`. `parking_lot` if the trajectory recorder lands here (it lives in `rmc-episode`, so not needed).
- `crates/rmc-reward/src/lib.rs` — crate root: re-exports + `RewardError` (the crate's typed `thiserror` error). Large public types are split into file-based modules (§4/§10): `reward_vector.rs` (`RewardVector`, `AuditDelta`, `MetricDelta`, `RewardWeights`) and `commit.rs` (`Commit`, `CommitResult`, `CargoGateMode`, `GateScope`, `CargoGateRunner`, `CargoGateOutcome`). `lib.rs` holds only the error enum + module wiring; no `mod.rs` files.
- `crates/rmc-reward/src/error.rs` — `RewardError` (typed `thiserror`): variants for cargo-subprocess IO/timeout, RA gate failure, gate-output JSON parse, rollback failure, weights-TOML load. Wrap sources with `#[source]`/`#[from]`.
- `crates/rmc-reward/src/cargo_gate.rs` — warm cargo invocation; `tokio::process::Command`; persistent `CARGO_TARGET_DIR`; JSON message parsing.
- `crates/rmc-reward/src/ra_gate.rs` — RA fast type check via `WorkspaceHost::ra_type_check_dirty(&[CrateId])` (P0.2 adds method).
- `crates/rmc-reward/src/audit_delta.rs` — before/after audit diff over **dirty NodeId set only** (O(|dirty|) not O(workspace)).
- `crates/rmc-reward/src/graph_metrics.rs` — modularity (Louvain local-move), conductance (per-cluster boundary), clustering coefficient, betweenness centrality top-p95 (Brandes restricted to k-hop neighborhood of dirty nodes). `petgraph` + custom community-detection.
- `crates/rmc-reward/src/scalarize.rs` — `Scalarizer::scalarize`. Hard floor: compile fail → -1.0.
- `crates/rmc-episode/Cargo.toml` — new **library** crate (no `anyhow`). Deps: `rmc-reward`, `rmc-graph`, `rmc-engine`, `tokio`, `serde`, `serde_json`, `reqwest` (with the `json` feature; per-request timeouts via the builder), `clap`, `async-trait`, `thiserror` (workspace `"1"`), `parking_lot` (poison-free mutexes for the trajectory recorder), `fastrand` (backoff jitter), `toml` (TaskSpec parse), `tracing`.
- `crates/rmc-episode/src/lib.rs` — crate root: module wiring + `EpisodeError` (the crate's typed `thiserror` error) + the `Secret<String>` newtype. `Episode`, `Trajectory`, `Action`, `ActionRouter`, `StepRecord`, `EpisodeOutcome`, `TaskSpec`, `SuccessCriteria`, `StepBudget` live in file-based submodules (no `mod.rs`).
- `crates/rmc-episode/src/router.rs` — 5-verb dispatch + per-step budget check.
- `crates/rmc-episode/src/trajectory.rs` — JSONL writer to `working/<session_id>/trajectory.jsonl`. PII scrubber masks `/^sk-[A-Za-z0-9_-]{20,}$/`.
- `crates/rmc-episode/src/model_client.rs` — `ModelClient` trait + `AnthropicClient`. POSTs to `https://api.anthropic.com/v1/messages` with `anthropic-version: 2023-06-01`. Anthropic prompt caching (`cache_control: { "type": "ephemeral" }`).
- `crates/rmc-episode/src/prompt.rs` — system prompt template, ContextView serialization, tool-use schema for 5 verbs.
- `crates/rmc-rl/Cargo.toml` — new bin-only crate. Deps: `rmc-episode`, `clap`, `tokio` (`flavor = "multi_thread"`), `anyhow` (the **only** crate in this section allowed to use `anyhow` — it is the binary boundary).
- `crates/rmc-rl/src/main.rs` — CLI; subcommand `run` with `--task`, `--model`, `--max-steps`, `--workspace`. `anyhow` is confined here; library errors (`RewardError`, `EpisodeError`) bubble up via `?` and are wrapped with context at the binary boundary.
- `tasks/dedupe_project_paths.toml` — seed task (rmc duplication of `project_paths`).

## Type definitions

```rust
// crates/rmc-reward/src/error.rs

/// Typed error for the reward/commit crate. No `anyhow` in this library.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RewardError {
    #[error("cargo subprocess failed to spawn or run")]
    CargoSpawn(#[source] std::io::Error),
    #[error("cargo gate timed out after {0:?}")]
    CargoTimeout(std::time::Duration),
    #[error("RA fast gate failed")]
    RaGate(#[source] HostError),
    #[error("failed to parse cargo JSON message output")]
    GateParse(#[source] serde_json::Error),
    #[error("rollback failed after a rejected edit")]
    Rollback(#[source] HostError),
    #[error("failed to load reward weights from {path}")]
    WeightsLoad { path: PathBuf, #[source] source: std::io::Error },
}

// crates/rmc-reward/src/commit.rs

// Borrowing struct: fields are crate-internal (built per-op by the episode
// runner). `&mut WorkspaceHost` here is exclusive — never alive at the same
// time as `Crud`'s `&mut host` (the dispatch finishes the Crud borrow first).
pub struct Commit<'a> {
    host:       &'a mut WorkspaceHost,
    snap:       &'a OpenedWorkingSnapshot,
    thresholds: GateThresholds,
    gate:       CargoGateRunner,
    weights:    RewardWeights,
}

impl<'a> Commit<'a> {
    pub fn new(host: &'a mut WorkspaceHost, snap: &'a OpenedWorkingSnapshot,
               thresholds: GateThresholds, gate: CargoGateRunner, weights: RewardWeights) -> Self {
        Self { host, snap, thresholds, gate, weights }
    }
}

#[derive(Debug, Clone)]
pub struct CommitResult {
    pub passed:            bool,
    pub reward:            RewardVector,
    pub scalar:            f32,
    pub rollback_executed: bool,
    pub elapsed_ms:        u64,
}

// crates/rmc-reward/src/reward_vector.rs
//
// NOTE: every reward struct carries `f32` fields, so it derives `PartialEq`
// (needed for serialization round-trip test assertions) but NOT `Eq`/`Hash`.
// None of these may ever be used as a map/set key.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RewardVector {
    pub compile_ok:          f32,        // 1.0 if gate passed, else 0.0
    pub test_pass_rate:      f32,        // 0..1; 1.0 if no tests ran
    pub audit_delta:         AuditDelta,
    pub graph_metric_delta:  MetricDelta,
    pub gates_soft_penalty:  f32,        // sum of GateOutcome.soft_penalties
    pub token_cost:          u32,        // input + output since prior commit
}

// All-integer; safe to derive `Eq`/`Hash` here, but kept `PartialEq`-only for
// consistency with the enclosing `RewardVector`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AuditDelta {
    pub unsafe_added:         i32,
    pub unwrap_added:         i32,
    pub missing_docs_added:   i32,
    pub mut_static_added:     i32,
    pub complexity_max_delta: i32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
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

pub struct Scalarizer { weights: RewardWeights }
impl Scalarizer {
    #[must_use]
    pub fn new(weights: RewardWeights) -> Self { Self { weights } }
    #[must_use]
    pub fn scalarize(&self, rv: &RewardVector) -> f32;
    /// # Errors
    /// Returns [`RewardError::WeightsLoad`] if the TOML cannot be read/parsed.
    pub fn from_toml(path: &Path) -> Result<Self, RewardError>;
    #[must_use]
    pub fn weights(&self) -> &RewardWeights { &self.weights }
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum CargoGateMode {
    CheckOnly,
    CheckAndScopedTest { test_pattern: String },
    RaOnly,
    /// DEFAULT: RA every step; full cargo check every K steps; full test only on declare_done.
    RaPlusCheckEveryK { k: u32 },
    FullAtDoneOnly,
}

/// Replaces the prior `force_full: bool` flag (§7: prefer enums over booleans).
/// `Incremental` = honor the per-step mode; `FullAtDone` = force a full
/// `cargo check` + `cargo test` pass (used by `declare_done`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GateScope {
    Incremental,
    FullAtDone,
}

pub struct CargoGateRunner {
    mode:               CargoGateMode,
    workspace:          PathBuf,
    session_target_dir: PathBuf,        // CARGO_TARGET_DIR pinned per session
    step_counter:       AtomicU32,
    devshell_prefix:    Option<Vec<String>>,    // ["nix","develop","../nix-devshells#cuda-code","--command"]
    per_run_timeout:    Duration,       // per cargo subprocess; default 300s
}

impl CargoGateRunner {
    /// # Errors
    /// [`RewardError::CargoSpawn`] / [`RewardError::CargoTimeout`] /
    /// [`RewardError::GateParse`] / [`RewardError::RaGate`].
    pub async fn run(&self, host: &mut WorkspaceHost, dirty_crates: &[String], scope: GateScope)
        -> Result<CargoGateOutcome, RewardError>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CargoGateOutcome {
    pub check_passed: bool,
    pub test_results: Option<TestResults>,
    pub elapsed_ms:   u64,
    pub diagnostics:  Vec<CargoDiagnostic>,
    pub mode_used:    GateModeUsed,     // closed set, serialized snake_case
}

/// Closed set (was a `String` "ra"|"cargo-check"|"cargo-test").
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
#[non_exhaustive]
pub enum GateModeUsed { Ra, CargoCheck, CargoTest }

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TestResults {
    pub total: u32, pub passed: u32, pub failed: u32, pub ignored: u32,
    pub per_test: Vec<TestRecord>,      // truncated to first 500
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TestRecord {
    pub name: String,
    #[serde(rename = "crate")]
    pub r#crate: String,                // serialized as `"crate"`, not `crate_`
    pub passed: bool,
    pub stdout: Option<String>,         // last 4 KB if failed
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CargoDiagnostic {
    pub level: String,                   // "error" | "warning"
    pub message: String,
    pub file: Option<String>, pub line: Option<u32>,
}
```

```rust
// crates/rmc-episode/src/lib.rs

/// Typed error for the episode crate. No `anyhow` in this library.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EpisodeError {
    #[error("model client request failed")]
    Model(#[source] ModelError),
    #[error("reward/commit pipeline failed")]
    Reward(#[from] rmc_reward::RewardError),
    #[error("workspace host operation failed")]
    Host(#[source] HostError),
    #[error("trajectory write failed at {path}")]
    Trajectory { path: PathBuf, #[source] source: std::io::Error },
    #[error("trajectory record serialization failed")]
    Serialize(#[source] serde_json::Error),
    #[error("task TOML at {path} is invalid")]
    TaskSpec { path: PathBuf, #[source] source: toml::de::Error },
}

/// Errors raised by a `ModelClient` implementation.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ModelError {
    #[error("HTTP transport error")]
    Http(#[source] reqwest::Error),
    #[error("request timed out")]
    Timeout,
    /// Retryable: HTTP 429 / 5xx after exhausting `max_retries`.
    #[error("server overloaded (status {status}) after {attempts} attempts")]
    Overloaded { status: u16, attempts: u32 },
    /// Fail-fast: 4xx other than 429 (e.g. 401 auth).
    #[error("non-retryable HTTP status {status}")]
    Status { status: u16 },
    #[error("response did not contain a valid tool_use action")]
    NoAction,
    #[error("script exhausted (no more queued actions)")]   // FakeModel
    ScriptExhausted,
}

/// Redacting newtype for the API key (§5/§13). Private inner; never logged.
#[derive(Clone)]
pub struct Secret<T>(T);

impl<T> Secret<T> {
    pub fn new(inner: T) -> Self { Self(inner) }
    /// Borrow the secret for use at the trust boundary (HTTP header).
    pub fn expose(&self) -> &T { &self.0 }
}

impl<T> std::fmt::Debug for Secret<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

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
    async fn step(&mut self, action: &Action, tokens: u32) -> Result<StepRecord, EpisodeError> {
        // Build the borrowing structs per-action; their `'_` lifetime is local.
        let mut crud = Crud::new(&mut self.host, &self.snap, &mut self.semantic,
                                 self.task.workspace_root());
        let nav = Navigator::new(&self.snap, &self.navigator_cfg);
        // ... dispatch on `action`; for a committing verb build Commit per-op.
        // NB: the `&mut self.host` borrow below is NOT live at the same time as
        // `crud`'s — dispatch finishes the Crud borrow before Commit borrows.
        let mut commit = Commit::new(
            &mut self.host,
            &self.snap,
            self.thresholds.clone(),
            self.gate_runner.clone(),
            self.weights.clone(),
        );
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
    /// # Errors
    /// Returns [`ModelError`] on transport failure, timeout, retry-exhaustion,
    /// non-retryable status, or an unparseable response.
    ///
    /// `history` is the windowed/projected recent steps (see `StepSummary`),
    /// NOT the full trajectory.
    async fn next_action(&self, view: &ContextView, history: &[StepSummary], task: &TaskSpec)
        -> Result<NextActionResponse, ModelError>;
}

#[derive(Debug, Clone, PartialEq)]
pub struct NextActionResponse { pub action: Action, pub tokens_in: u32, pub tokens_out: u32 }

// `api_key` is a `Secret<String>` with a redacting `Debug` (§5/§13); all fields
// private so a derived `Debug` on an enclosing type cannot leak the key.
pub struct AnthropicClient {
    model:        String,                // "claude-opus-4-7" or "claude-sonnet-4-6"
    api_key:      Secret<String>,
    endpoint:     String,
    http:         reqwest::Client,
    max_retries:  u32,                   // default 3
    request_timeout: Duration,           // per-request; default 60s
}

impl std::fmt::Debug for AnthropicClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicClient")
            .field("model", &self.model)
            .field("api_key", &self.api_key)   // -> Secret(<redacted>)
            .field("endpoint", &self.endpoint)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl AnthropicClient {
    /// `api_key` is taken by value and wrapped in [`Secret`]; callers pass the
    /// raw `String` from the environment.
    pub fn new(model: String, api_key: String, endpoint: String,
               http: reqwest::Client, max_retries: u32, request_timeout: Duration) -> Self {
        Self { model, api_key: Secret::new(api_key), endpoint, http, max_retries, request_timeout }
    }
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

**Step 2 — `CargoGateRunner::run`.** WHERE: `cargo_gate.rs`. Note `scope: GateScope` replaces the old `force_full: bool` flag.
```rust
pub async fn run(&self, host: &mut WorkspaceHost, dirty_crates: &[String], scope: GateScope)
    -> Result<CargoGateOutcome, RewardError>
{
    let started = Instant::now();
    let step = self.step_counter.fetch_add(1, SeqCst) + 1;
    let full = matches!(scope, GateScope::FullAtDone);
    let need_full = full
        || matches!(self.mode, CheckOnly | CheckAndScopedTest{..} | FullAtDoneOnly)
        || matches!(self.mode, RaPlusCheckEveryK { k } if step % k == 0);

    // 1) Always run RA first (~50ms confirms type sanity).
    let ra_diags = ra_gate::run(host, dirty_crates).await.map_err(RewardError::RaGate)?;
    let ra_errors: Vec<_> = ra_diags.iter().filter(|d| d.level == "error").cloned().collect();
    if !ra_errors.is_empty() {
        return Ok(CargoGateOutcome {
            check_passed: false, test_results: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
            diagnostics: ra_diags, mode_used: GateModeUsed::Ra,
        });
    }
    if !need_full && matches!(self.mode, RaOnly | RaPlusCheckEveryK{..}) {
        return Ok(CargoGateOutcome {
            check_passed: true, test_results: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
            diagnostics: ra_diags, mode_used: GateModeUsed::Ra,
        });
    }
    // 2) Full cargo check + optionally tests.
    let mut diags = ra_diags;
    let check_ok = self.cargo_check(dirty_crates, &mut diags).await?;
    if !check_ok {
        return Ok(CargoGateOutcome {
            check_passed: false, test_results: None,
            elapsed_ms: started.elapsed().as_millis() as u64,
            diagnostics: diags, mode_used: GateModeUsed::CargoCheck,
        });
    }
    let tests = if full {
        Some(self.cargo_test_full().await?)
    } else if let CheckAndScopedTest { test_pattern } = &self.mode {
        Some(self.cargo_test_scoped(test_pattern, dirty_crates).await?)
    } else { None };
    Ok(CargoGateOutcome {
        check_passed: true, test_results: tests,
        elapsed_ms: started.elapsed().as_millis() as u64,
        diagnostics: diags,
        mode_used: if full { GateModeUsed::CargoTest } else { GateModeUsed::CargoCheck },
    })
}
```

Cargo invocation — wrap every subprocess in `tokio::time::timeout` and **kill on
timeout** so a hung `cargo`/devshell cannot stall the episode (§12). `cargo_check`,
`cargo_test_full`, `cargo_test_scoped` all funnel through this helper:
```rust
async fn run_cargo(&self, mut cmd: tokio::process::Command) -> Result<std::process::Output, RewardError> {
    cmd.kill_on_drop(true)              // belt-and-braces: kill if the future is dropped
       .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(RewardError::CargoSpawn)?;
    match tokio::time::timeout(self.per_run_timeout, child.wait_with_output()).await {
        Ok(res) => res.map_err(RewardError::CargoSpawn),
        Err(_elapsed) => {
            // Timed out: kill the process group and surface the timeout.
            let _ = child.start_kill();   // best-effort; `kill_on_drop` covers the rest
            Err(RewardError::CargoTimeout(self.per_run_timeout))
        }
    }
}

// Command construction (unchanged shape):
let mut cmd = tokio::process::Command::new("cargo");
if let Some(prefix) = &self.devshell_prefix {
    cmd = tokio::process::Command::new(&prefix[0]);
    cmd.args(&prefix[1..]).arg("cargo");
}
cmd.arg("check").arg("--message-format=json").arg("--locked").arg("--offline").arg("--workspace");
for c in dirty_crates { cmd.args(&["-p", c]); }
cmd.env("CARGO_TARGET_DIR", &self.session_target_dir)
   .env("RUSTC_WRAPPER", "");
let output = self.run_cargo(cmd).await?;   // timeout + kill-on-timeout applied here
```
> NOTE on `kill_on_drop`: when `tokio::process::Child` is spawned with
> `kill_on_drop(true)`, dropping the child (or the timed-out future) sends
> `SIGKILL`. The explicit `start_kill()` makes the intent greppable and
> immediate; relying solely on drop would defer the kill to scope exit.

Parse JSON output line-by-line: `reason == "compiler-message"` carries `message.spans[]` + `message.level`; aggregate into `CargoDiagnostic`. `reason == "build-finished"` carries `success: bool`.

Test invocation (`GateScope::FullAtDone` only):
```
cargo test --workspace --no-fail-fast --message-format=json -- -Z unstable-options --format json --report-time
```
If not on nightly, fall back to parsing plain text "test foo::bar ... ok"/"FAILED" lines.

VERIFY: `cargo_check_parses_compile_error`.

**Step 3 — RA fast gate.** WHERE: `ra_gate.rs`. Returns the host's typed error (mapped to `RewardError::RaGate` by the caller).
```rust
pub async fn run(host: &mut WorkspaceHost, dirty_crates: &[String])
    -> Result<Vec<CargoDiagnostic>, HostError>
{
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
    pub fn capture(snap: &'a OpenedSnapshot, dirty: &[NodeId]) -> Result<AuditCounts, RewardError> {
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
    pub fn rebuild_full(snap: &OpenedSnapshot) -> Result<Self, RewardError>;
    pub fn apply_delta(&mut self, edit_outcome: &EditOutcome, snap: &OpenedSnapshot)
        -> Result<MetricDelta, RewardError>
    {
        let (before_mod, before_con, before_cc, before_bw) =
            (self.modularity, self.conductance, self.clustering_coef, self.betweenness_p95);
        for e in &edit_outcome.removed_edges { self.remove_edge(e); }
        for e in &edit_outcome.added_edges   { self.insert_edge(e); }
        // Each recompute can emit NaN/inf on a degenerate sub-graph (empty
        // community, zero-degree node, single-node component). Clamp the cached
        // values BEFORE differencing so a NaN can never propagate into the
        // reward vector (§9/determinism). `finite_or` maps non-finite -> prior.
        self.modularity      = finite_or(self.louvain_local_move(&edit_outcome.affected_items), before_mod);
        let touched          = self.communities_containing(&edit_outcome.affected_items);
        self.conductance     = finite_or(self.recompute_conductance(touched), before_con);
        self.clustering_coef = finite_or(self.recompute_clustering(self.k_hop(&edit_outcome.affected_items, 1)), before_cc);
        self.betweenness_p95 = finite_or(self.recompute_betweenness_local(self.k_hop(&edit_outcome.affected_items, 2)), before_bw);
        // Differences of two finite values are finite, but guard once more so
        // the serialized `MetricDelta` is provably NaN-free.
        Ok(MetricDelta {
            modularity:          finite_or(self.modularity - before_mod, 0.0),
            conductance:         finite_or(self.conductance - before_con, 0.0),
            clustering_coef:     finite_or(self.clustering_coef - before_cc, 0.0),
            betweenness_top_p95: finite_or(self.betweenness_p95 - before_bw, 0.0),
        })
    }
}

/// Returns `v` if finite, else `fallback`. Centralizes the NaN/inf policy so
/// every metric delta is guaranteed finite before scalarization.
#[inline]
fn finite_or(v: f32, fallback: f32) -> f32 { if v.is_finite() { v } else { fallback } }
```
DEPENDS: `EditOutcome.added_edges` / `removed_edges` (P1.5a records for LMDB diff-patch); `petgraph` from P1.3. VERIFY: `metric_delta_local_only`, `metric_delta_nan_clamped` (degenerate single-node sub-graph; assert all four deltas are finite).

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
        let scalar = w.compile * rv.compile_ok
            + w.test * rv.test_pass_rate
            + audit_term
            + metric_term
            + w.soft_penalty * rv.gates_soft_penalty
            + w.token * rv.token_cost as f32;
        // Final safety net: `MetricDelta` is already clamped finite (Step 5),
        // but guard the scalar too so the reward signal is never NaN/inf.
        if scalar.is_finite() { scalar } else { -1.0 }
    }
}
```
Tunables via `RewardWeights::from_toml(path) -> Result<_, RewardError>`. VERIFY: `scalarizer_hard_floor`, `scalarizer_nan_safe` (inject a non-finite delta — defends even if Step 5's clamp were bypassed).

**Step 7 — `Commit::run` orchestration.** WHERE: `lib.rs`.
```rust
impl<'a> Commit<'a> {
    /// # Errors
    /// Propagates [`RewardError`] from the cargo/RA gate, audit capture, or
    /// metric recompute; [`RewardError::Rollback`] if restore-on-failure fails.
    ///
    /// NOTE: `self.before_audits` / `self.metric_cache` are accessed through the
    /// `Commit` owner; the episode threads them as `&mut` per-step (see E2).
    pub async fn run(&mut self, edit_outcome: &EditOutcome, gate_outcome: &GateOutcome,
                      checkpoint: &Checkpoint, dirty_crates: &[String], token_cost: u32,
                      scope: GateScope) -> Result<CommitResult, RewardError>
    {
        let t0 = Instant::now();
        let cargo_outcome = self.gate.run(self.host, dirty_crates, scope).await?;
        if !cargo_outcome.check_passed {
            self.host.restore(checkpoint).map_err(RewardError::Rollback)?;
            let rv = RewardVector {
                compile_ok: 0.0, test_pass_rate: 0.0,
                audit_delta: AuditDelta::default(),
                graph_metric_delta: MetricDelta::default(),
                gates_soft_penalty: gate_outcome.soft_penalties as f32,
                token_cost,
            };
            return Ok(CommitResult {
                passed: false,
                scalar: Scalarizer::new(self.weights.clone()).scalarize(&rv),
                reward: rv,
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
            passed: true,
            scalar: Scalarizer::new(self.weights.clone()).scalarize(&rv),
            reward: rv,
            rollback_executed: false,
            elapsed_ms: t0.elapsed().as_millis() as u64,
        })
    }
}
```
VERIFY: `commit_rollback_on_compile_break`.

**Step 8 — Tests on `declare_done`.** `Commit::run(.., GateScope::FullAtDone)` runs `cargo test --workspace --no-fail-fast --message-format=json`. Parse JSON for `reason == "test"` events. Truncate per-test stdout to last 4 KB; cap `per_test` at 500. Record `total/passed/failed/ignored` exactly. VERIFY: `declare_done_runs_full_tests`.

### P1.8 — episode runner

**Step 9 — `Episode::new`.** WHERE: `lib.rs`.
```rust
impl<M: ModelClient> Episode<M> {
    /// # Errors
    /// [`EpisodeError::Host`] (snapshot open) or [`EpisodeError::Trajectory`]
    /// (recorder open / header write).
    pub async fn new(task: TaskSpec, model: M, budget: StepBudget,
                      base_workspace: &Path, session_id: &str, weights: RewardWeights)
        -> Result<Self, EpisodeError>
    {
        // 1. Working snapshot per D1: copy published LMDB to working/<session_id>/.
        let host = WorkspaceHost::open_for_session(base_workspace, session_id).await
            .map_err(EpisodeError::Host)?;
        let snap = host.open_working_snapshot().map_err(EpisodeError::Host)?;
        // 2. Initial ContextView (built per-step via NavigatorConfig; see E2).
        // 3. Reward + commit runner (built per-step inside `step()`; only the
        //    owned `CargoGateRunner` lives in `Episode`).
        let gate_runner = CargoGateRunner::new(
            CargoGateMode::RaPlusCheckEveryK { k: 5 },
            base_workspace.into(),
            working_dir(session_id).join("cargo-target"),   // session_target_dir
            Some(vec![
                "nix".into(), "develop".into(),
                "../nix-devshells#cuda-code".into(), "--command".into(),
            ]),
            Duration::from_secs(300),                        // per_run_timeout
        );
        let traj = TrajectoryRecorder::open(&working_dir(session_id).join("trajectory.jsonl"))
            .map_err(|source| EpisodeError::Trajectory {
                path: working_dir(session_id).join("trajectory.jsonl"), source,
            })?;
        traj.write_header(&task).map_err(EpisodeError::Serialize)?;
        Ok(Self {
            host, snap, gate_runner, weights, model, budget,
            trajectory: traj, task,
            // semantic / crud_cfg / navigator_cfg / thresholds / metric_cache /
            // before_audits initialized from defaults + the opened snapshot.
            ..Episode::scaffold(/* config */)
        })
    }
}
```
> `CargoGateRunner::new(mode, workspace, session_target_dir, devshell_prefix,
> per_run_timeout)` constructs the runner with `step_counter: AtomicU32::new(0)`
> internally (fields are private). The `..Episode::scaffold(..)` shorthand stands
> in for the remaining owned-config fields per E2 — none of them borrow `host`.
VERIFY: `episode_new_creates_working_snapshot`.

**Step 10 — Loop body.**
```rust
pub async fn run(mut self) -> Result<Trajectory, EpisodeError> {
    let started = SystemTime::now().duration_since(UNIX_EPOCH)
        .expect("system clock is after the UNIX epoch").as_secs();
    let mut step_num = 0u32;
    let mut total_tokens = 0u64;
    let wall_deadline = Instant::now() + Duration::from_secs(self.budget.max_wall_secs);
    let mut prior_audits = AuditDeltaComputer::capture(&self.snap, &[])?;   // RewardError -> EpisodeError via #[from]
    let nav = Navigator::new(&self.snap, &self.navigator_cfg);
    let mut prior_view   = nav.view_at(&self.task.initial_loc).map_err(EpisodeError::Host)?;
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
        // Window the history sent to the model: only the last `HISTORY_WINDOW`
        // (e.g. 8) StepRecords, and project each to a compact `StepSummary`
        // (drops the heavy `ContextView`) — see Step 12. Avoids the O(n) clone
        // of every full StepRecord each turn (§12/§18).
        let history_view = self.trajectory.history_view(HISTORY_WINDOW);
        let resp = self.model.next_action(&prior_view, &history_view, &self.task)
            .await.map_err(EpisodeError::Model)?;
        total_tokens += (resp.tokens_in + resp.tokens_out) as u64;
        let checkpoint = self.host.checkpoint().await.map_err(EpisodeError::Host)?;
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
        self.trajectory.append(&rec)?;   // TrajectoryError -> EpisodeError
        prior_view   = nav.refresh(&prior_view, &dispatch.affected).map_err(EpisodeError::Host)?;
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
    /// # Errors
    /// [`EpisodeError`] from navigation, CRUD apply, gate evaluation, host
    /// restore, or the commit pipeline.
    pub async fn dispatch(action: &Action, host: &mut WorkspaceHost, snap: &OpenedSnapshot,
                           crud: &mut Crud, navigator: &Navigator, commit: &mut Commit<'_>,
                           prior_audits: &mut AuditCounts, tokens: u32, checkpoint: &Checkpoint)
        -> Result<DispatchOutcome, EpisodeError>
    {
        match action {
            Action::Navigate(step) => {
                let res = navigator.step(step).map_err(EpisodeError::Host)?;
                Ok(DispatchOutcome {
                    result: ActionResult::Navigated(res),
                    reward_vec: RewardVector { compile_ok: 1.0, ..Default::default() },
                    scalar: 0.0, affected: vec![], new_audit_baseline: None,
                })
            }
            Action::Simulate(op) => {
                let sim = crud.simulate(op).map_err(EpisodeError::Host)?;
                Ok(DispatchOutcome {
                    result: ActionResult::Simulated(sim),
                    reward_vec: RewardVector { compile_ok: 1.0, ..Default::default() },
                    scalar: -0.001, affected: vec![], new_audit_baseline: None,
                })
            }
            Action::Crud(op) => {
                let edit = crud.apply(op).await.map_err(EpisodeError::Host)?;
                let gate_outcome = host.evaluate_gates(&edit).map_err(EpisodeError::Host)?;
                if gate_outcome.refused {
                    host.restore(checkpoint).map_err(EpisodeError::Host)?;
                    return Ok(DispatchOutcome {
                        result: ActionResult::Refused(gate_outcome.reason),
                        reward_vec: RewardVector { compile_ok: 0.0, ..Default::default() },
                        scalar: -1.0, affected: vec![], new_audit_baseline: None,
                    });
                }
                let dirty_crates = host.crates_of(&edit.affected_items);
                let cr = commit.run(&edit, &gate_outcome, checkpoint,
                                     &dirty_crates, tokens, GateScope::Incremental).await?;
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
                                     &dirty_crates, tokens, GateScope::FullAtDone).await?;
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

Uses `parking_lot::Mutex` so a panic while a lock is held cannot poison the mutex
(no `.lock().unwrap()` / `PoisonError` to handle — `parking_lot` locks return the
guard directly). The error type is the crate's `EpisodeError` (its `Trajectory` /
`Serialize` variants). `history_view` is **windowed and projected** so the model
prompt is not re-sent every full `StepRecord` (incl. `ContextView`) each turn.
```rust
use parking_lot::Mutex;

/// Compact per-step view sent to the model in `history` (no heavy `ContextView`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StepSummary {
    pub step:    u32,
    pub action:  Action,
    pub result_kind: String,     // discriminant of ActionResult, not the payload
    pub reward:  f32,
}

pub struct TrajectoryRecorder {
    file: Mutex<File>,           // parking_lot: poison-free
    path: PathBuf,
    /// Ring of recent summaries (NOT full StepRecords). Bounded length, so
    /// `history_view` is O(window), not O(total steps) (§12/§18).
    recent: Mutex<std::collections::VecDeque<StepSummary>>,
    secret_mask: regex::Regex,   // /sk-[A-Za-z0-9_-]{20,}/
}

impl TrajectoryRecorder {
    /// # Errors
    /// [`EpisodeError::Serialize`] (JSON) or [`EpisodeError::Trajectory`] (IO).
    pub fn append(&self, rec: &StepRecord) -> Result<(), EpisodeError> {
        let line = serde_json::to_string(rec).map_err(EpisodeError::Serialize)?;
        let masked = self.secret_mask.replace_all(&line, "<REDACTED-KEY>");
        {
            let mut f = self.file.lock();                 // no .unwrap()
            let write = (|| {
                f.write_all(masked.as_bytes())?;
                f.write_all(b"\n")?;
                f.flush()
            })();
            write.map_err(|source| EpisodeError::Trajectory { path: self.path.clone(), source })?;
        }
        let mut recent = self.recent.lock();              // no .unwrap()
        recent.push_back(StepSummary {
            step: rec.step, action: rec.action.clone(),
            result_kind: rec.action_result.kind().to_string(), reward: rec.reward,
        });
        // Caller bounds via the `window` arg of `history_view`; keep a hard cap
        // so the deque can't grow unbounded across a long episode.
        while recent.len() > HISTORY_CAP { recent.pop_front(); }
        Ok(())
    }

    /// Last `window` step summaries (most recent last). O(window), bounded clone.
    #[must_use]
    pub fn history_view(&self, window: usize) -> Vec<StepSummary> {
        let recent = self.recent.lock();                  // no .unwrap()
        recent.iter().rev().take(window).rev().cloned().collect()
    }
}
```
`ModelClient::next_action`'s `history` parameter changes from `&[StepRecord]` to
`&[StepSummary]` accordingly. The full `StepRecord` stream still lands in the
JSONL file verbatim — only the *in-prompt* history is windowed/projected.
VERIFY: `trajectory_writes_one_line_per_step`, `history_view_is_windowed`
(append > window steps; assert `history_view(8).len() == 8` and contains no
`ContextView`).

**Step 13 — Model client + Anthropic prompt caching.** WHERE: `model_client.rs`.
The send is wrapped in an explicit **retry/backoff loop** (no longer a single
`.send().await?`), each attempt carries a per-request **timeout**, and status
codes are **classified** instead of collapsed by `error_for_status()`:
429 / 5xx → retry with exponential backoff; other non-2xx → fail-fast.
```rust
#[async_trait::async_trait]
impl ModelClient for AnthropicClient {
    async fn next_action(&self, view: &ContextView, history: &[StepSummary], task: &TaskSpec)
        -> Result<NextActionResponse, ModelError>
    {
        let system_blocks = vec![
            json!({
                "type": "text", "text": SYSTEM_PROMPT,
                "cache_control": { "type": "ephemeral" }       // breakpoint 1
            }),
            json!({
                "type": "text",
                "text": serde_json::to_string(task).map_err(|_| ModelError::NoAction)?,
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

        // Retry/backoff loop. `attempt` is 1-based; `max_retries` extra tries.
        let mut last_status = 0u16;
        for attempt in 1..=(self.max_retries + 1) {
            let req = self.http.post(&self.endpoint)
                .header("x-api-key", self.api_key.expose())   // Secret -> &str at the boundary
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .timeout(self.request_timeout)                // per-request timeout (§12)
                .json(&body);

            let sent = req.send().await;
            let resp = match sent {
                Ok(r) => r,
                Err(e) if e.is_timeout() => {
                    // Timeouts are transient: back off and retry.
                    last_status = 0;
                    backoff(attempt).await;
                    continue;
                }
                Err(e) => return Err(ModelError::Http(e)),
            };

            let status = resp.status();
            if status.is_success() {
                let v: serde_json::Value = resp.json().await.map_err(ModelError::Http)?;
                let tokens_in  = v["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
                let tokens_out = v["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
                let action = parse_action_from_tool_use(&v).ok_or(ModelError::NoAction)?;
                return Ok(NextActionResponse { action, tokens_in, tokens_out });
            }

            last_status = status.as_u16();
            // Classify: 429 + 5xx are retryable; everything else fails fast.
            if status.as_u16() == 429 || status.is_server_error() {
                backoff(attempt).await;
                continue;
            }
            return Err(ModelError::Status { status: last_status });
        }
        Err(ModelError::Overloaded { status: last_status, attempts: self.max_retries + 1 })
    }
}

/// Exponential backoff with full jitter: base 250ms, doubling, capped at 8s.
async fn backoff(attempt: u32) {
    let base_ms = 250u64.saturating_mul(1u64 << attempt.min(5));
    let capped = base_ms.min(8_000);
    let jittered = fastrand::u64(0..=capped);   // full jitter
    tokio::time::sleep(Duration::from_millis(jittered)).await;
}
```

**Model selection:**
- `claude-opus-4-7` — default; best multi-step reasoning for pilot.
- `claude-sonnet-4-6` — cost-optimized; via `--model claude-sonnet-4-6`.

**Prompt caching:** mark `SYSTEM_PROMPT` + task block + tools schema (and initial ContextView when fits) as ephemeral breakpoints. First turn pays full; subsequent turns within 5-min TTL replay cached prefix at ~10% input-token cost. 50-step episodes → 5–10× cost reduction.

**Retries (implemented above, not prose):** the loop runs `max_retries + 1`
attempts. Retryable: connect/timeout errors, HTTP 429, and 5xx — each followed by
`backoff(attempt)` (exponential, full jitter, capped 8s). Fail-fast: any other
non-2xx (e.g. 401 auth) → `ModelError::Status`. Exhausting retries →
`ModelError::Overloaded`. The episode loop maps every `ModelError` to
`EpisodeError::Model`; `Episode::run`'s caller may finalize as `HardFailure`.

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
        /// Hard cap on episode steps (was `--budget`; renamed for clarity vs
        /// `--max-tokens` / `--max-wall-secs`, which are separate budgets).
        #[arg(long, default_value_t = 50)] max_steps: u32,
        #[arg(long)] workspace: PathBuf,
        #[arg(long, default_value = "600")] max_wall_secs: u64,
        #[arg(long, default_value_t = 200_000)] max_tokens: u64,
        #[arg(long)] weights: Option<PathBuf>,
        #[arg(long)] session_id: Option<String>,
    },
}

// `anyhow` is allowed ONLY here (the binary). Library errors (`RewardError`,
// `EpisodeError`) bubble up via `?` and gain context at this boundary.
#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Run { task, model, max_steps, workspace, max_wall_secs, max_tokens, weights, session_id } => {
            let task_spec: TaskSpec = toml::from_str(&std::fs::read_to_string(&task)?)?;
            let api_key = std::env::var("ANTHROPIC_API_KEY")
                .context("ANTHROPIC_API_KEY must be set")?;
            // Build the HTTP client; per-request timeout is applied per-call in
            // `next_action`, but set a connect timeout here too.
            let http = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .build()?;
            let client = AnthropicClient::new(
                model, api_key,
                "https://api.anthropic.com/v1/messages".into(),
                http, /*max_retries*/ 3, /*request_timeout*/ Duration::from_secs(60),
            );
            let session = session_id.unwrap_or_else(|| format!("rl-{}", uuid::Uuid::new_v4()));
            let budget = StepBudget { max_steps, max_tokens, max_wall_secs };
            let weights = match weights {
                Some(p) => Scalarizer::from_toml(&p)?.weights().clone(),
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
        --model claude-opus-4-7 --max-steps 50 \
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
Uses `parking_lot::Mutex` (poison-free) and bounds-checks the script pop with
`VecDeque::pop_front` so an exhausted script yields `ModelError::ScriptExhausted`
instead of panicking on `Vec::remove(0)`.
```rust
use parking_lot::Mutex;
use std::collections::VecDeque;

struct FakeModel { script: Mutex<VecDeque<Action>> }
#[async_trait::async_trait]
impl ModelClient for FakeModel {
    async fn next_action(&self, _v: &ContextView, _h: &[StepSummary], _t: &TaskSpec)
        -> Result<NextActionResponse, ModelError>
    {
        let action = self.script.lock().pop_front().ok_or(ModelError::ScriptExhausted)?;
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
- **`anthropic_client_retries_5xx`** — mock returns 503 twice then 200; client retries with backoff; assert exactly 3 requests reach the mock.
- **`anthropic_client_429_retries_then_overloaded`** — mock always returns 429; assert `ModelError::Overloaded` after `max_retries + 1` attempts.
- **`anthropic_client_4xx_fails_fast`** — mock returns 401; assert `ModelError::Status { status: 401 }` after exactly one request (no retry).
- **`anthropic_client_request_timeout`** — mock delays past `request_timeout`; assert the call times out and (eventually) errors rather than hanging.
- **`secret_debug_redacts`** — `format!("{:?}", AnthropicClient::new(.., "sk-realkey".into(), ..))` and `format!("{:?}", Secret::new("sk-x"))` contain neither the key nor `sk-`; only `<redacted>`.
- **`cargo_gate_kills_on_timeout`** — stub a cargo command that sleeps past `per_run_timeout`; assert `RewardError::CargoTimeout` and that the child process is no longer running.
- **`reward_round_trip_eq`** — serialize a `RewardVector`/`MetricDelta`/`CargoGateOutcome`, deserialize, assert `PartialEq`.
- **`test_record_serializes_crate_key`** — serialize a `TestRecord`; assert the JSON key is `"crate"`, not `"crate_"`.
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
- **Determinism of Trajectory.** RA query ordering may differ run-to-run; per P0.1 invariant, reward-bearing fields must be stable. Cross-check: re-running same FakeModel script produces identical reward components within 1e-6. NaN/inf is now structurally excluded: every `MetricDelta` field passes through `finite_or` (Step 5) and `Scalarizer::scalarize` returns `-1.0` for any non-finite scalar (Step 6), so a degenerate sub-graph cannot poison the reward vector or break the 1e-6 reproducibility check.
- **Secret handling.** The API key lives in `Secret<String>` with a redacting `Debug` and a private field; `AnthropicClient` has a hand-written `Debug` that prints `Secret(<redacted>)`. The trajectory writer additionally regex-masks `sk-…` from serialized output as defence-in-depth.


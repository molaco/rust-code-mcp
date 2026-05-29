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
  - **Public surface:** two `[[bin]]` targets — `ra_fanout` and `cargo_gate`; one `lib.rs` exposing `bench_harness::{Workload, run, Report}` (`Report` is `#[non_exhaustive]` with private fields + accessors, since the JSON-report schema will grow) so both spikes share the same JSON-report format.
  - **Cargo deps:** `rmc-graph` (path), `rmc-config` (path, for `seed`), `anyhow` (allowed: `rmc-spikes` is a binary crate), `serde`, `serde_json`, `tracing`, `tracing-subscriber`, `clap = { workspace = true }` (the shared `clap = "4"` workspace dep), `tokio` (workspace), and `criterion = "0.5"` as a dev-dependency only.
  - **Built in:** Section B. Removed from `default-members` so a `cargo build` from the root does not compile spikes.

- **`crates/rmc-host` — *optional* extracted warm host (Section C)**
  - **Purpose:** if M2a measurement reveals a circular dep between rmc-graph (which holds extract / storage) and the new working-snapshot + RA-host machinery, lift `WorkspaceHost`, `EditSeq`, and the apply==rebuild engine into their own crate. **Default recommendation: keep inside `rmc-graph::host` and skip this crate.** Listed here so the integration plan does not need a re-shuffle if the extraction does become necessary.
  - **Public surface:** `WorkspaceHost` (private fields + accessors/constructor), `WorkspaceHost::apply_edits`, `WorkspaceHost::checkpoint`, `WorkspaceHost::restore`, `#[non_exhaustive] EditClass` (variants `BodyOnly, SignatureOrVis, ItemAddRemove, ModuleTree, Macro, CargoManifest`), `#[non_exhaustive] AffectedSet`. Fallible methods return the typed `HostError` (Section C), not bare `Result`.
  - **Cargo deps if extracted:** `rmc-graph` (path, for `storage`, `extract::per_crate`, `ids`, `model`), `ra_ap_*` (workspace), `heed` (workspace), `thiserror = { workspace = true }` (workspace pins `"1"`; typed `HostError`), `tracing`, `serde`, `bincode`, `sha2`. **No `anyhow`** (library crate).
  - **Built in:** Section C, conditionally. The plan tracks both forks; the file-tree diff below shows the default (in-graph) layout.

- **`crates/rmc-semantic` — rename/refactor mechanics (extracted from `rmc-server`) — M2a prerequisite**
  - **Purpose:** the symbol-rename engine (`SemanticService` + RA `rename` preview) lifted out of `rmc-server::semantic` into its own crate so that **both** `rmc-server` (MCP handlers) **and** `rmc-crud` (CRUD verbs) can depend on it without a cycle. `rmc-server` gains a dep on `rmc-crud` (via the `rl` feature, line below); `rmc-crud` needs the rename engine — if the engine stayed in `rmc-server`, the two crates would depend on each other and **fail to compile**. Extracting `rmc-semantic` is therefore mandatory, not optional. This rejects the earlier "promote in place + depend on `rmc-server`" approach.
  - **Public surface:** `pub struct SemanticService` (private fields + constructor), `#[non_exhaustive] pub struct RenamePreview` (private `edits`, `file_moves` + accessors), `#[non_exhaustive] pub struct RenameEdit`, `#[non_exhaustive] pub struct RenameFileMove`, `pub fn rename_by_name(..)`, `pub fn rename_by_position(..)`.
  - **Cargo deps:** `rmc-graph` (path), `ra_ap_ide` (workspace), `ra_ap_ide_db` (workspace), `ra_ap_syntax` (workspace), `thiserror = { workspace = true }` (workspace pins `"1"`; typed `SemanticError`), `tracing`, `serde`. **No `anyhow`** (library crate).
  - **Built in:** prerequisite for Section G (M2a). Mechanical move: `crates/rmc-server/src/semantic/` → `crates/rmc-semantic/src/`; flip `pub(crate)` → `pub` on the four types + two fns (real locs `semantic/mod.rs:53`, `rename.rs:15/41/61/70/168`); `rmc-server` re-points its handlers at the new crate.

- **`crates/rmc-crud` — CRUD verbs (Sections G + H)**
  - **Purpose:** the five Phase-1 verbs (`modify_body`, `move`, `delete`, `modify_signature`, `extract_*`/`inline`, `*_module`/`lift_to_crate`/`lower_to_module`) as pure operations over the working snapshot, expressed as `Crud::compute_effects(&self, host) -> Result<Effects, CrudError>` and `Crud::apply_effects(host, &effects) -> Result<Outcome, CrudError>` (inherent methods on the `Crud` enum, dispatched by `match` over the closed verb set). The split satisfies P1.4 (simulator) — simulate is `compute_effects` only.
  - **Public surface:** `#[non_exhaustive] pub enum Crud { ModifyBody{..}, Move{..}, Delete{..}, ModifySignature{..}, ExtractFunction{..}, ExtractTrait{..}, Inline{..}, SplitModule{..}, MergeModules{..}, CreateModule{..}, MoveModule{..}, LiftToCrate{..}, LowerToModule{..} }`, `#[non_exhaustive] pub struct Effects` with private fields + accessors (`source_patches()`, `graph_patches()`, `manifest_patches()`, `would_refuse()`), `#[non_exhaustive] pub enum CallsiteFill { Todo, RefuseIfMissing, Explicit(String) }`.
    - **DD-2 consolidation (§8 "skip a trait when there is one implementation").** The earlier draft listed *both* the 13-variant `Crud` enum *and* a `CrudVerb` trait with an associated `Op`. That is redundant dispatch machinery for one closed, in-crate verb set with no substitution pressure: the trait is **removed** from the public surface. Verbs are dispatched by `match` inside `compute_effects` / `apply_effects` (inherent methods on `Crud`; equivalently free fns over `&Crud`). `#[non_exhaustive]` keeps the enum growable without breaking downstream `match` arms.
  - **Cargo deps:** `rmc-graph` (path, for `WorkspaceHost`, `ids`, `model`, `extract`, `storage`), `rmc-config` (path), `rmc-semantic` (path, for `SemanticService`/`RenamePreview`/`RenameEdit`/`RenameFileMove` rename mechanics — **extracted from `rmc-server` to break the `rmc-server` ⇄ `rmc-crud` dependency cycle**; see Canonical Reconciliation §R4), `ra_ap_syntax` (workspace), `ra_ap_ide` (workspace, for `rename` preview), `ra_ap_ide_db` (workspace), `syn = "2"` (**new shared dep**, for AST *analysis only* — locate byte ranges; replacement text is string-built and spliced, never AST-unparsed, per E5), `toml_edit = "0.22"` (**new shared dep**, format-preserving `Cargo.toml` surgery in P1.5e), `thiserror = { workspace = true }` (the workspace pins `"1"`; this crate's errors are the typed `CrudError`), `tracing`, `serde`, `serde_json`. **No `anyhow`** — `rmc-crud` is a library, so it uses the typed `CrudError` (`thiserror`); `anyhow` is reserved for the `rmc-spikes` / `rmc-rl` binaries. **No `prettyplease`** (banned by E5).
  - **Built in:** Sections G (modify_body, move, delete) and H (signature, extract/inline, module-tree).

- **`crates/rmc-gates` — write-time guideline gates (Section I)**
  - **Purpose:** wrap the existing audits (`fn_body_audit`, `unsafe_audit`, `recursion_check`, `derive_audit`, `channel_audit`, `docs_audit`, `analyze_complexity`) and the SCC cycle check from `petgraph` into a `GateHarness` that runs over the dirty set produced by D2, returns hard refusals (with `RefusalReason`) and soft penalties.
  - **Public surface:** `pub struct GateHarness` (private fields + constructor), `#[non_exhaustive] pub struct GateReport` (private `hard_refusals: Vec<RefusalReason>`, `soft_penalties: Vec<Penalty>` + accessors), `pub fn run_gates(host: &WorkspaceHost, dirty: &AffectedSet, allowlist: &BoundaryAllowlist) -> Result<GateReport, GateError>`, `pub struct BoundaryAllowlist` (private-fielded read-only loader for `rmc.gates.toml`).
  - **Cargo deps:** `rmc-graph` (path, exposes `query/audits::*`, `fn_body_audit`, `unsafe_audit`, `recursion_check`, `derive_audit`, `channel_audit`, `docs_audit`), `rmc-config` (path), `petgraph = "0.6"` (**new shared dep**, for SCC), `thiserror = { workspace = true }` (workspace pins `"1"`; typed `GateError`), `tracing`, `serde`, `toml = "0.9"` (workspace already present). **No `anyhow`** (library crate).
  - **Built in:** Section I.

- **`crates/rmc-reward` — commit + reward vector (Section J first half, P1.7)**
  - **Purpose:** cargo gate runner (`CargoGateMode::{Off, RaOnly, CheckOnly, CheckAndTest, RaPlusCheckEveryK{k:5}}`), audit diff, graph-metric delta (modularity / conductance / clustering coefficient via `petgraph`), reward scalarizer.
  - **Public surface:** `#[non_exhaustive] pub struct RewardVector` (private `compile_ok`, `test_pass_delta`, `audit_deltas`, `graph_metric_deltas`, `gate_penalty`, `refusal_count` + accessors), `pub fn commit(host: &mut WorkspaceHost, gate_report: &GateReport, mode: CargoGateMode) -> Result<RewardVector, RewardError>`, `#[non_exhaustive] pub enum CargoGateMode`, `pub fn rollback(host: &mut WorkspaceHost, checkpoint: &Checkpoint) -> Result<(), RewardError>` (thin wrapper over `WorkspaceHost::restore` + `jj op restore`).
  - **Cargo deps:** `rmc-graph` (path), `rmc-gates` (path), `rmc-config` (path), `petgraph = "0.6"`, `linfa = "0.7"`, `linfa-clustering = "0.7"`, `thiserror = { workspace = true }` (workspace pins `"1"`; typed `RewardError`), `tracing`, `serde`, `serde_json`, `tokio` (workspace), `which = "6"`. **No `anyhow`** (library crate).
  - **Built in:** Section J first half (P1.7).

- **`crates/rmc-episode` — episode runner + trajectory (Section J second half, P1.8)**
  - **Purpose:** the loop: `observe -> act -> reward`, action dispatch over the 5-verb API, step budget, `declare_done`, trajectory log (the future SFT dataset format), per-episode jj checkpoint.
  - **Public surface:** `pub struct EpisodeRunner` (private fields + constructor), `#[non_exhaustive] pub struct Trajectory` (private `steps: Vec<Step>` + accessor), `#[non_exhaustive] pub struct Step` (private `observation: ContextView`, `action: Action`, `reward: RewardVector`, `refusal: Option<RefusalReason>` + accessors), `#[non_exhaustive] pub enum Action { Crud(Crud), Navigate(NavAction), Simulate(Crud), DeclareDone }`, `pub trait Policy { async fn act(&mut self, obs: &ContextView) -> Result<Action, EpisodeError>; }` (a real substitution port — `AnthropicPolicy` + test fakes), `pub struct AnthropicPolicy` (default impl wrapping the Anthropic Messages API; private fields incl. the redacted API key).
  - **Cargo deps:** `rmc-graph` (path), `rmc-crud` (path), `rmc-gates` (path), `rmc-reward` (path), `rmc-config` (path), `tokio` (workspace), `serde`, `serde_json`, `thiserror = { workspace = true }` (workspace pins `"1"`; typed `EpisodeError`), `tracing`, `reqwest = { workspace = true }` for the Anthropic API client, `chrono = { workspace = true }` (UTC `started_at`/`finished_at` timestamps stamped onto each `Step`/`Trajectory` for the JSONL trajectory log — this is the sole consumer of the workspace `chrono` dep). **No `anyhow`** (library crate).
  - **Built in:** Section J second half (P1.8).

- **`crates/rmc-rl` (bin) — CLI driver (Section J close)**
  - **Purpose:** thin `clap`-driven CLI: `rmc-rl episode --task <name> --model <id> --budget <n>` and `rmc-rl bench-spike` (forwards to `rmc-spikes`). Single `[[bin]]` target, no library surface.
  - **Cargo deps:** `rmc-episode` (path), `rmc-config` (path), `clap = { workspace = true }`, `anyhow` (allowed: `rmc-rl` is a binary crate), `tokio`, `tracing`, `tracing-subscriber`.
  - **Built in:** Section J close.

## Modified existing crate inventory

For each existing crate, the changes are anchored to the section that introduces them.

- **`crates/rmc-config` (Section A → updated in B, E, I, J)**
  - **New pub APIs:** `#[non_exhaustive] pub struct RuntimeConfig` with private fields (`seed: Seed`, `cargo_gate_mode: CargoGateMode`, `callsite_fill: CallsiteFill`, `working_snapshot_root: PathBuf`, `anthropic_model: String`, `description_model: String`) + accessors and `from_env_with_seed` constructor. The Anthropic/description API keys are *not* stored here in plaintext — `src/anthropic.rs` exposes them via a redacted `Secret<String>` newtype (custom `Debug`, private field) per Section J.
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
  - **Workspace dep additions:** `rmc-semantic` (path, replaces the in-tree `semantic/` module — non-feature-gated); `rmc-crud`, `rmc-gates`, `rmc-reward`, `rmc-episode` (all gated by a new `rl` feature, using `dep:`-prefixed optional deps so the feature is purely additive). The `rl`-feature dep on `rmc-crud` is exactly why `semantic/` had to leave this crate.
  - **§14 — `rl` is NOT a default feature.** `[features] default = []` (or the pre-existing default set with `rl` *excluded*). `rl` is enabled only by the `rmc-rl` bin and off in the published `rust-code-mcp` binary (Open decisions register). This is what keeps the cycle-break argument honest: `rmc-server` only reaches `rmc-crud`/`rmc-reward`/`rmc-episode` when `rl` is on, and `rmc-crud` reaches the rename engine through `rmc-semantic` (extracted), never back through `rmc-server` — so the dependency graph stays a DAG.
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
+# Phase 1: trajectory timestamps (consumed by rmc-episode for Step/Trajectory
+# started_at/finished_at in the JSONL log — NOT orphaned).
+chrono        = { version = "0.4", default-features = false, features = ["std", "serde"] }
+
+# thiserror is ALREADY a workspace dependency pinned to "1" (in the elided
+# entries above). It is NOT bumped: every new library crate
+# (rmc-semantic/-crud/-gates/-reward/-episode, optional rmc-host) declares
+# `thiserror = { workspace = true }` and inherits the "1" pin (DD-5). No
+# thiserror v2 is introduced. `anyhow` stays confined to the bins rmc-spikes
+# and rmc-rl.

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
+     # §5 keyword-collision convention: the `Crud::Move` verb's module is
+     # `move_.rs` (trailing underscore) because `move` is a Rust keyword and
+     # cannot name a module/identifier. The enum *variant* stays `Move`
+     # (variants are not keyword-restricted); only the file/`mod` ident takes
+     # the trailing `_`. (`move_module.rs` is a distinct verb, no collision.)
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


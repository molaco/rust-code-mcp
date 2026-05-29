# Phase 0 + Phase 1 Plan — Fix Checklist

Actionable, prioritized worklist derived from `REVIEW.md`. Work top-down:
**P0 blockers → P1 systemic → P2 per-section → design decisions.**

Most per-section nits are already covered by the P1 systemic passes — a finding
listed under P1 is *not* repeated under P2. P2 holds only what's left after the
systemic passes land.

Legend: `§N` = guidelines heading. Section files are `.plans/actual-phase-1/NN-*.md`.

---

## P0 — Blockers (resolve before implementing the affected section)

- [ ] **§H — remove `prettyplease::unparse` (E5 violation).** Rewrite Steps 3,
      5, 9, 12, 19, 31 from "mutate `syn::File` → unparse" to "locate byte range
      → splice hand-built string" (`source_edit::splice_bytes`). Delete every
      `prettyplease` reference and the "prettyplease re-formats whole files —
      accepted" Open-decision. (file 11)
- [ ] **§C — type the host pipeline.** Make `compute_patch`, `apply_patch`,
      `re_extract`, `Checkpoint::take` return `Result<_, HostError>`;
      `Checkpoint::take` becomes fallible (not `-> Self`). (file 06)
- [ ] **§C — kill panics in the host pipeline.** Replace `k.try_into().unwrap()`,
      `b.try_into().unwrap()`, `undo.batches.pop().unwrap()`, `.expect("checked")`
      with `HostError` variants (LMDB corruption is not a local invariant). Add a
      `// SAFETY:` comment to `unsafe { …open() }` (§19). (file 06)
- [ ] **§G — no `unwrap()` on spans.** `body_byte_range`: return
      `EditError::TargetHasNoSource` (or take a pre-validated span) instead of
      `node.span.unwrap()`. (file 10)

---

## P1 — Cross-cutting systemic passes (one fix pattern, many files)

### P1.1 — Replace `anyhow::Result` with typed `thiserror` errors in library crates (§9)
Confine `anyhow` to the binaries (`rmc-spikes`, `rmc-rl`). Use `#[source]`/`#[from]`
to preserve chains. Distinguish invalid-input / domain-rejection / transient-IO /
network / cancellation / internal-bug.
- [ ] file 05 (B): `WorkingSnapshotError`, `InvalidationError`, `UndoError`, `RestoreError`
- [ ] file 06 (C): one `HostError` across all fallible host fns (overlaps P0)
- [ ] file 07 (D): drop `ViewError::Anyhow`; add `Query(#[from] …)` typed variant
- [ ] file 08 (E): `DescriptionError` (network/429/cap/unresolvable/storage)
- [ ] file 09 (F): `AnalyzeError` (invalid-input/numeric/VCS-IO/labeler)
- [ ] file 10 (G): confirm `EditError` covers IO/parse with path context (no blanket `#[from]`)
- [ ] file 11 (H): add `SynParse`, `Io`, `CargoMetadata`, `LineIndex`, `HostUnavailable`
- [ ] file 12 (I): `GateError`; drop `anyhow` dep from `rmc-gates`
- [ ] file 13 (J): `RewardError`, `EpisodeError`
- [ ] Add `# Errors` / `# Panics` / `# Safety` doc sections to public fallible fns (§16)
- [ ] Add `# Errors` doc to `dump_snapshot` + define `SnapshotDumpError` (file 04)

### P1.2 — Privatize public struct fields; expose accessors/constructors (§5/§10)
- [ ] file 05 (B): `WorkingSnapshot` (esp. `env`, `dbs`), `UndoLog.path`, `SessionId`, `JjOpId`
- [ ] file 06 (C): `EditSeq`, `UndoMarker`, `UndoOp`, `DiffPatch`, `UndoBatch` fields
- [ ] file 07 (D): `Navigator` (`snap`/`host`/`budget` → private; budget via `with_budget`), `ClusterId`
- [ ] file 08 (E): `DescriptionGenerator` (`model`/`limiter`/`stats`/`store`), `DescriptionStoreOwned`, `PromptCtx`
- [ ] file 09 (F): `ClusterId(pub u32)`
- [ ] file 10 (G): `Crud` fields, `EditOutcome`
- [ ] file 12 (I): `Effects`, `RefusalReason`, `RefusalCode`, `GateThresholds`
- [ ] file 13 (J): all `pub`-field structs (see also P1.5 for `api_key`)
- [ ] file 02/04: `PartialExtractionModel`, `Checkpoint`, `SnapshotDump`/`SnapshotDiff`

### P1.3 — Add `#[non_exhaustive]` to growable public enums/structs (§7)
- [ ] `EditClass`, `SubDb`, `InvalidationAction` (files 02/05/06)
- [ ] `EditError` (files 10/11), `RefusalCode`, `Severity` (file 12)
- [ ] `Scale`, `NeighborKind`, `NavStep`, `ViewError` (file 07)
- [ ] `AffectedSet`, `Checkpoint`, `DiffPatch`, `CascadePolicy`, `GraphDiffSummary` (files 02/06/10)
- [ ] `SnapshotDump`/`SnapshotDiff` (file 04)

### P1.4 — Replace stringly-typed IDs with newtypes (§7/§11)
- [ ] `GraphId` for `base_graph_id`/`graph_id` (files 02/05/09)
- [ ] `JjOpId` — keep the newtype; revert §R2's downgrade to `String` (files 02/06) — *see design decision DD-4*
- [ ] `CommitHash` for `head_commit_hash` (file 09)
- [ ] `Edit::ItemAddRemove.target_qualified` → typed (file 05)
- [ ] `op_kind: String` → `enum CascadeKind`; typed `file` field (file 12)
- [ ] `NodePin.kind`/`item_kind`/`visibility` → `NodeKind` enum; `Span` newtype (file 07)

### P1.5 — Async hygiene (§12) + secret handling (§13)
- [ ] file 06 (C): scope the workspace lock so it is not held across `.await` (jj restore)
- [ ] file 08 (E): `RateLimiter::acquire` timeout + cancellation contract; bound `join_all` concurrency / `JoinSet`
- [ ] file 13 (J): per-request `reqwest` timeout + `tokio::time::timeout` (kill-on-timeout) on cargo subprocess
- [ ] file 13 (J): implement the retry/backoff loop (currently prose-only); classify 429/5xx vs fail-fast
- [ ] file 13 (J): `api_key` → `Secret<String>` newtype, private field, custom `Debug`, constructor

### P1.6 — Numeric / NaN guards + determinism (§9/§17)
- [ ] file 09 (F): pin a stable hash for `hash64`/seed derivation (not `DefaultHasher`/`ahash`); guard silhouette/BIC-argmin/softmax/Mahalanobis/PMI for NaN + zero-variance; use `f64::total_cmp`
- [ ] file 13 (J): `is_finite()` clamps on Louvain/conductance/betweenness f32 deltas before scalarizing
- [ ] file 04 (A): prefer `BTreeMap` over `FxHashMap` for emission order; surface malformed `RMC_SEED` instead of `unwrap_or(0)`

### P1.7 — Replace derived/desyncable fields with methods (§5/§7)
- [ ] file 12 (I): `GateOutcome.passed` → `fn passed(&self) -> bool` + `#[must_use]`
- [ ] file 07 (D): drop `ContextView.scale`; expose `view.scale()` (derive from `focus`)

### P1.8 — Module layout: file-based, no `mod.rs` (§10)
- [ ] file 02 (§R1): rename canonical homes `checkpoint/mod.rs` → `checkpoint.rs` + `checkpoint/`, and `working/patch/mod.rs` → `patch.rs` + `patch/`; update §R6 mapping
- [ ] file 09 (F): move `build_vision` entry + `EmbeddingsLookup` out of `features.rs` into `analyze.rs`

---

## P2 — Per-section remaining (after P1 passes land)

### file 01 — Errata
- [ ] Mark E2's `Arc<Mutex<WorkspaceHost>>` as last-resort; prefer scoped borrows / `RefCell` (§6/§12)
- [ ] E3: consider `RelativePath` newtype / `TryFrom` over release-stripped `debug_assert!` (§7)

### file 03 — Section Z
- [ ] Resolve `thiserror` version skew: bump workspace to `2` or pin new crates to `1` (§15) — *see DD-5*
- [ ] Add `#[must_use]` policy note for pure returners (cross-cuts B/D/G/I)
- [ ] Confirm `rl` feature is not in `default` features (cycle-break argument) (§14)
- [ ] Flag `Crud::Move` / `move_.rs` keyword-collision convention (§5)
- [ ] Wire `chrono` into a crate or drop it from the workspace diff (§15)

### file 04 — Section A
- [ ] Strengthen tests: assert the seed is actually threaded (not "no panic"); add a `dump_snapshot` failure-path test (§17)
- [ ] `Seed(u64)` newtype across Config + BuildOptions (§7)

### file 05 — Section B
- [ ] Fix the false "compile-time exhaustiveness" claim for the D3 matrix — *see DD-1*
- [ ] Reconcile `SubDb` variant count (15) vs the destructured `GraphDatabases` fields (13: add `manifest`, `descriptions_by_target`)
- [ ] `#[must_use]` on `classify`/`invalidations_for`/`consumers_of`; give `SessionId` a `Default` or rename `new()` (§5)

### file 06 — Section C
- [ ] Declare the missing `WorkspaceHost` fields referenced in Steps 3/5/11 (`recent_file_prior_text`, `crate_target_kinds_by_*`, `fallback_crates_for_path`, `is_diverged_from_expected`, `reopen_from_base`)
- [ ] `affected_crates` → `Result<Vec<NodeId>, HostError>` (it `return Err(...)` on CargoManifest)
- [ ] Surface a `HostError::RollbackDiverged` instead of silent `tracing::warn!` success

### file 07 — Section D
- [ ] Remove redundant `ZoomDir` (covered by `NavStep::{ZoomIn,ZoomOut}`)
- [ ] `#[must_use]` on verbs returning `ContextView`

### file 08 — Section E
- [ ] Drop `#[async_trait]` for native async-fn-in-trait, or justify `dyn` need vs `DescriptionGenerator<M>` static dispatch (§8/§20)
- [ ] Map the `content.get(span)?` Option into an explicit error in the `Result` fn (§9)
- [ ] Confirm `descriptions` feature gates whole items, not fields/methods (§14)

### file 09 — Section F
- [ ] `Default` for `BuildVisionOptions`; add `Debug`/`Send`/`Sync` supertrait bounds to `LabelGenerator` (§8)
- [ ] Use `dep:` syntax for the `analyze` feature deps (§14)

### file 10 — Section G
- [ ] Separate lifetimes on `Crud<'a>` so `&mut host`/`&mut semantic` aren't pinned to `&snap` (§6)
- [ ] `checked_sub` + bounds-check on RA 1-based `start_line`/`start_column`; document byte-vs-char column assumption (§9)
- [ ] Specify `EditOutcome` checkpoint RAII/commit semantics on the success path; `#[must_use]` (§9)
- [ ] Replace abbreviations `snap`/`rtxn`/`e`/`s`/`repl` with full names (§5)

### file 11 — Section H
- [ ] Reconsider `SignatureChange.new_sig` (whole-sig) — lossy delta inference — *see DD-3*
- [ ] Add `#[derive(Debug)]` etc.; flag that `CallsiteFill::ClosureBuilder(Box<dyn Fn>)` blocks `Debug`/`Clone`/`PartialEq` on the enum (§8)
- [ ] Confirm `EditError` isn't an unbounded crate-level god enum; scope per-operation if it grows (§9)
- [ ] Specify a hygiene/uniqueness scheme for synthesized `__arg_0`/`__arg_self` idents (§5)

### file 12 — Section I
- [ ] Add `RefusalCode` variants for `mut_static_audit`/`channel_capacity_audit` (or drop them from scope)
- [ ] Single validated `gates.toml` loader (validate soft ≤ hard) instead of two independent reads (§14)
- [ ] Mark latency benches whose targets "likely fail" as aspirational/`#[ignore]` (§17)

### file 13 — Section J
- [ ] Handle `PoisonError` on mutex locks (or `parking_lot`); bounds-check `FakeModel` `script.remove(0)`
- [ ] `run(..., force_full: bool)` → `enum GateScope::{Incremental, FullAtDone}` (§7)
- [ ] Add `PartialEq` for serialization round-trip tests; note f32 fields preclude `Eq`/`Hash` map keys (§8/§17)
- [ ] `TestRecord.crate_` → `#[serde(rename = "crate")] r#crate` or `crate_name` (§5)
- [ ] Window/bound `history_view()` (currently O(n) clone of full `StepRecord`s incl. `ContextView` per step) (§12/§18)
- [ ] Rename CLI `--budget` → `--max-steps` (§14)
- [ ] Consider splitting `lib.rs` into `reward_vector.rs`/`commit.rs` (§4/§10)
- [ ] Ensure `anthropic_client_*` mock-server tests can't hit the real endpoint (§17)

---

## Design decisions (need a human call — not mechanical)

- [ ] **DD-1 (file 05):** D3 invalidation matrix — keep per-`EditClass` matching
      with a runtime exhaustiveness test (and drop the "compile-time" claim), or
      restructure to key on `(EditClass, SubDb)` pairs for real compile-time
      coverage?
- [ ] **DD-2 (file 03):** `Crud` 13-variant enum **and** `CrudVerb` trait with
      associated `Op` is redundant dispatch for one closed verb set — pick a
      `match` over `Crud` *or* per-verb types behind the trait, not both.
- [ ] **DD-3 (file 11):** `modify_signature` input — whole new
      `FunctionSignature` (lossy rename-vs-add inference) vs an explicit ordered
      edit-list (unambiguous, more verbose to construct)?
- [ ] **DD-4 (files 02/06):** revisit §R2's downgrade of `JjOpId` newtype →
      `String` — restore the newtype, or document why `String` is acceptable here?
- [ ] **DD-5 (file 03):** `thiserror` version — bump the workspace pin from `1`
      to `2`, or pin all new crates to the existing `1`?

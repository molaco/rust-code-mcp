# Phase 0 + Phase 1 Plan — Guidelines Review

Per-section review of `.plans/actual-phase-1/*.md` against
`/home/molaco/Documents/chart-refactor/.docs/rust-guidelines-final.md`.

Each section was reviewed by an independent subagent that read the full
guidelines doc and the full section. Findings judge the **design the plan
prescribes** (types, APIs, error handling, module layout) — not prose style.
Section 00 (title + TOC) was not reviewed.

Guideline references (`§N`) point at headings in the guidelines doc.

---

## Verdicts at a glance

| Section | Verdict | Blockers |
|---|---|---|
| 01 Errata | minor issues | — |
| 02 Canonical Reconciliation | minor issues | — |
| 03 Section Z (Integration) | minor issues | — |
| 04 Section A (Determinism + Bench) | minor issues | — |
| 05 Section B (Contracts D1–D4 + Spikes) | **major issues** | — |
| 06 Section C (Warm Host + Rollback) | **major issues** | 2 |
| 07 Section D (Read View / Navigator) | **major issues** | — |
| 08 Section E (Description Index) | minor issues | — |
| 09 Section F (Analyze / Vision) | **major issues** | — |
| 10 Section G (modify_body / move / delete) | **major issues** | 1 |
| 11 Section H (signature / extract / module) | **major issues** | 1 |
| 12 Section I (Simulator + Gates) | minor issues | — |
| 13 Section J (Reward + Episode Runner) | **major issues** | — |

---

## Blockers — resolve before implementing the affected section

1. **§H still prescribes `prettyplease::unparse`** (Steps 3, 5, 9, 12, 19, 31
   + Open-decisions) — directly contradicts errata **E5**, which the section's
   own header says voids those calls. The "mutate `syn::File` → unparse" rebuild
   approach must be rewritten to locate byte ranges and splice hand-built
   strings (`source_edit::splice_bytes`). Delete every `prettyplease` reference
   and the "prettyplease re-formats whole files — accepted" rationale.
2. **§C untyped + panic-prone host pipeline** — `compute_patch` / `apply_patch`
   / `re_extract` / `Checkpoint::take` return bare `Result` with no error type;
   `unwrap()`/`expect()` on LMDB key bytes (`k.try_into().unwrap()`,
   `b.try_into().unwrap()`), `undo.batches.pop().unwrap()`, `.expect("checked")`;
   `unsafe { …open() }` with no `// SAFETY:` comment. LMDB corruption is not a
   local invariant — these must become `HostError` variants (§9), and the unsafe
   block needs a SAFETY justification (§19). `Checkpoint::take` is fallible (jj
   op log) so it must return `Result`, not `Self`.
3. **§G `node.span.unwrap()`** in `body_byte_range` (Step 3) panics on a `None`
   span that is already modeled as `EditError::TargetHasNoSource` — pass the
   validated span in or return that error.

---

## Cross-cutting themes (systemic — fix once, benefits many sections)

Ranked by leverage. These recur across most sections; addressing them at the
pattern level clears the bulk of the findings.

1. **Untyped `anyhow::Result` in library crates** (§9) — B, C, D, E, F, G, I, J.
   The pervasive issue. Domain crates whose callers branch on failure mode
   (restore-on-error, retry-on-429, refuse-vs-host-reject) need `thiserror`
   enums with `#[source]`/`#[from]` to preserve error chains. Confine `anyhow`
   to the binaries (`rmc-spikes`, `rmc-rl`). Note: workspace pins
   `thiserror = "1"` — decide whether to bump to `2` or pin new crates to `1`.
2. **Public fields on public structs** (§5/§10 "keep public fields private") —
   02, 04, 05, 06, 07, 08, 09, 10, 11, 12, 13 (nearly universal). Worst cases:
   `WorkingSnapshot { pub env, pub dbs }` (lets callers corrupt edit-seq/undo
   invariants behind the snapshot's back) and `AnthropicClient { pub api_key }`.
   Privatize fields; expose accessors/constructors.
3. **Missing `#[non_exhaustive]`** on growable public enums/structs (§7) — 02,
   04, 06, 07, 10, 11, 12. `EditClass`, `EditError`, `RefusalCode`, `Severity`,
   `Scale`, `SubDb`, `NeighborKind`, etc.
4. **Stringly-typed IDs** instead of newtypes (§7/§11 anti-pattern) — 02, 05,
   06, 07, 09, 12, 13. `jj_op_id: String`, `graph_id: String`,
   `head_commit_hash: String`, `op_kind: String`, `base_graph_id: String`.
   Note §R2 *consciously* downgraded B's `JjOpId` newtype to `String` — a
   deliberate regression worth revisiting.
5. **`unwrap()`/`expect()` in production paths** (§9) — 01, 04, 05, 06, 10, 13.
   LMDB bytes, mutex locks (`file.lock().unwrap()`), `Vec::remove(0)`, span
   options. Replace with typed errors or documented `expect` on real local
   invariants.
6. **Async hygiene** (§12) — §C holds a workspace lock across `.await` (jj
   restore); §E rate-limiter `acquire` and §J HTTP/cargo calls lack timeouts;
   §J describes retries/backoff in prose but the code does a single
   `.send().await?`. Add per-request timeouts, `tokio::time::timeout` +
   kill-on-timeout around subprocesses, and an actual backoff loop.
7. **Secret handling** (§13) — §J's `api_key: String` has no redaction wrapper
   and risks leaking via derived `Debug` on enclosing types. Use a
   `Secret<String>` newtype with custom `Debug` and a private field.
8. **NaN/numeric guards** (§9/§17) — §F (silhouette / BIC argmin / softmax;
   unspecified hash function `hash64` breaks the determinism the section
   promises) and §J (Louvain / conductance / betweenness f32 deltas can
   silently poison the reward vector). State NaN policy, use `f64::total_cmp`,
   guard zero variances/empty clusters, clamp with `is_finite()`.
9. **Derived, desyncable fields** (§5/§7 "make invalid states unrepresentable")
   — `GateOutcome.passed` (== `hard.is_empty()`) and `ContextView.scale`
   (derivable from `focus`) should be methods, not stored fields.
10. **`mod.rs` vs file-based modules** (§10) — the reconciliation §R1 canonizes
    `checkpoint/mod.rs` and `working/patch/mod.rs`, which the guideline bans in
    favor of `checkpoint.rs` + `checkpoint/` and `patch.rs` + `patch/`.

**Strengths echoed across sections:** the pure `compute_effects` → gate →
`persist` split (Sans-I/O posture, §3/§11); closed-world enums (`EditClass`,
`Action`, `CargoGateMode`, `EpisodeOutcome`); the `ModelClient` /
`DescriptionModel` substitution traits; content-addressed determinism with
`sort_unstable_by`; and extracting `rmc-semantic` to keep the dependency graph
a DAG.

---

## Notable design issues (beyond style)

- **§B's "compile-time exhaustiveness" claim is false.** The D3 invalidation
  matrix `invalidations_for` matches on `EditClass`, not on `(class, SubDb)`
  pairs, so a missing rule is silently absent (caught only by a runtime test,
  not the compiler). The `_matches_storage_layout` destructure lists 13
  `GraphDatabases` fields while `SubDb` has 15 variants (no `manifest`, no
  `descriptions_by_target`). Reconcile the counts and downgrade the claim to a
  documented runtime test.
- **§Z's `Crud` enum + `CrudVerb` trait are redundant dispatch machinery** for
  one closed verb set (§8 "skip a trait when there is one implementation").
  Pick one: a `match` over `Crud`, or per-verb types behind the trait — not
  both.
- **§H's `SignatureChange.new_sig` is lossy** — taking a whole new signature
  then re-deriving a delta makes rename-vs-(remove+add) ambiguous (the "refine
  by name" guesswork in Step 2). Prefer an explicit ordered edit-list input.
- **§J `--budget` flag is misleading** — it maps to `max_steps` while
  `--max-tokens` / `--max-wall-secs` are separate; rename to `--max-steps`.
- **§Z orphan workspace dep** — `chrono` is added to the workspace diff but no
  crate lists it; wire it in or drop it.

---

## Per-section findings

### Section 01 — Errata — *minor issues*

- **[MINOR] §9 / §16** — `EditError::Refused` / `HostRejected` are good structured
  variants, but the error type's `thiserror` derivation and source-preservation
  for wrapped host/IO errors are unspecified. State it's `thiserror`-based with
  `#[source]`/`#[from]`, distinguishing transient IO from domain refusal.
- **[MINOR] §6/§12** — E2's `Arc<Mutex<WorkspaceHost>>` fallback for a
  single-threaded episode loop contradicts "prefer scoped ownership over shared
  mutable state." The primary per-action-borrow resolution is correct; mark the
  `Mutex` as last-resort or use `RefCell`/sequencing.
- **[MINOR] §9** — E4 snippet `NodeId::from_bytes_arr(k.try_into().unwrap())` and
  `.map_or(false, …)`: `unwrap()` in the diff-patch path; use `expect("…16 bytes")`
  with justification or propagate.
- **[NIT] §7** — E3's `FileEdit::new` uses `debug_assert!(path.is_relative())`,
  which won't fire in release; a `RelativePath` newtype / `TryFrom` would enforce
  it always.
- *Strengths:* E1's pure-compute → gate → checkpoint→persist ordering cleanly
  separates pure logic from effects; E5's analysis-only AST + keep `toml_edit`
  avoids lossy round-trips.

### Section 02 — Canonical Reconciliation — *minor issues*

- **[MAJOR] §10 (mod.rs ban)** — §R1/§R2/§R6 canonize `checkpoint/mod.rs` and
  `working/patch/mod.rs`; the guideline prefers `checkpoint.rs` + `checkpoint/`
  and `patch.rs` + `patch/`. The §R6 "Read as" mapping inverts the preferred form.
- **[MINOR] §7 `#[non_exhaustive]`** — public `EditClass`, `SubDb`,
  `InvalidationAction`, `AffectedSet`, `Checkpoint`, `DiffPatch`,
  `PartialExtractionModel` are likely to grow; state the policy.
- **[MINOR] §5/§10** — `PartialExtractionModel` and `Checkpoint` have all-public
  fields with no stated constructor/invariant; `Checkpoint.jj_op_id: String`
  discards B's `JjOpId` newtype (stringly-typed ID).
- **[NIT] §7** — `jj_op_id: String` chosen over `JjOpId`: minor regression, note
  it consciously.
- *Strengths:* `EditClass` as a closed world, `FileEdit` setting class by
  construction, `UndoMarker(EditSeq)` newtype; §R4 keeps the dep graph a DAG by
  extracting `rmc-semantic` and skipping `rmc-host`.

### Section 03 — Section Z (Integration) — *minor issues*

- **[MAJOR] §9/§15** — new crates list `thiserror "(workspace)"` but the
  workspace pins **v1**; declare the exact version policy (bump to 2 or pin to 1).
- **[MAJOR] §8/§5** — `Crud` 13-variant enum + per-impl `CrudVerb` trait with
  associated `Op` is redundant substitution machinery for one closed verb set;
  pick one dispatch mechanism.
- **[MINOR] §10/§14** — confirm the `rl` feature (rmc-server → rmc-crud) is not in
  `default` features so the cycle-break argument holds.
- **[MINOR] §5** — `Crud::Move` / file `move_.rs` collide with the keyword; flag
  the trailing-underscore convention.
- **[NIT] §14/§15** — `chrono` added to the workspace diff but unused by any crate.
- *Strengths:* clean DAG with effects pushed upward; correct additive `rl`
  feature, `default-members` exclusion of spikes/CLI, `resolver = "3"`.

### Section 04 — Section A (Determinism + Bench) — *minor issues*

- **[MAJOR] §9/§16** — `dump_snapshot(...) -> Result<SnapshotDump>` has no named
  error type and no `# Errors` doc. Define `SnapshotDumpError` (thiserror)
  preserving the heed error.
- **[MINOR] §5/§7** — `SnapshotDump`/`SnapshotDiff` all-public fields (12+ and
  growing); add `#[non_exhaustive]` / accessors.
- **[MINOR] §7** — `BTreeMap<Vec<u8>, BTreeSet<Vec<u8>>>` and `pub seed: u64`:
  prefer a `Seed(u64)` newtype (it crosses Config + BuildOptions public API).
- **[MINOR] §17** — `seed_field_propagates…` ("assert no panic") and
  `dump_round_trip` ("assert non-empty") assert almost nothing; assert the seed
  is actually threaded and add a failure-path test.
- **[MINOR] §14** — `from_env` parses `RMC_SEED` with `.ok()…unwrap_or(0)`,
  silently swallowing malformed input; surface it.
- **[NIT] §17/§22** — relying on `FxHashMap` (deterministic only with fixed
  seed/build) for emission order is fragile; prefer `BTreeMap`.
- *Strengths:* sort-then-dedup on content-addressed IDs is the right pure
  approach; `bench/` excluded from the main workspace preserves the DAG.

### Section 05 — Section B (Contracts D1–D4 + Spikes) — *major issues*

- **[MAJOR] §9** — nearly every fallible `rmc-graph` signature returns
  `anyhow::Result` (`init_from_published`, `expand`, `UndoLog::*`,
  `take_checkpoint`, `restore`). Define per-operation `thiserror` enums; reserve
  `anyhow` for the `rmc-spikes` binaries.
- **[MAJOR] §5/§7** — `WorkingSnapshot { pub env, pub dbs, … }`, `UndoLog { pub
  path }`, `SessionId(pub Uuid)`, `JjOpId(pub String)` expose internals that let
  callers break edit-seq/undo invariants. Privatize with accessors.
- **[MAJOR] §17/§3** — the "compile-time exhaustiveness" claim for the D3 matrix
  is false (matches on `EditClass`, not `(class, SubDb)`); field/variant counts
  mismatch (13 vs 15). Downgrade to a documented runtime test and reconcile.
- **[MINOR] §7** — `base_graph_id: String`, `Edit::ItemAddRemove.target_qualified:
  String`: use a `GraphId` newtype.
- **[MINOR] §5** — add `#[must_use]` to pure returners (`classify`,
  `invalidations_for`, `consumers_of`); `SessionId::new()` with no `Default` trips
  `new_without_default`.
- **[NIT] §16** — no `# Errors`/`# Panics` on `Result`-returning fns; Step 18
  truncate/seek and Step 20 `Command::status()` can fail silently.
- *Strengths:* D2/D3 modeled as pure deterministic functions over
  `BTreeMap`/`BTreeSet`; file-based module layout.

### Section 06 — Section C (Warm Host + Rollback) — *major issues* (2 blockers)

- **[BLOCKER] §9** — `compute_patch`/`apply_patch`/`re_extract`/`Checkpoint::take`
  return untyped `Result`, inconsistent with `apply_edits`/`rollback`'s
  `HostError`. Make every fallible host fn `Result<_, HostError>`;
  `Checkpoint::take` must be fallible.
- **[BLOCKER] §9/§19** — `unwrap()`/`expect()` on LMDB bytes and `undo.pop()`,
  plus `unsafe { …open() }` with no `// SAFETY:`. Replace with `HostError`;
  justify the unsafe.
- **[MAJOR] §12** — workspace lock held across `.await` (jj restore + other
  awaits); scope the guard or use a sync lock around the non-await critical
  section; document cancellation safety.
- **[MAJOR] §6** — `WorkspaceHost` references undeclared fields
  (`recent_file_prior_text`, `crate_target_kinds_by_*`, `fallback_crates_for_path`,
  `is_diverged_from_expected`, `reopen_from_base`); `affected_crates -> Vec<NodeId>`
  but `return Err(...)` on CargoManifest (type mismatch → make it `Result`).
- **[MAJOR] §5/§7** — `EditSeq(pub u64)`, `UndoMarker(pub EditSeq)`, all
  `UndoOp`/`DiffPatch`/`UndoBatch` fields public; add `#[non_exhaustive]` to
  growable public enums.
- **[MINOR] §9** — divergence fallback logged via `tracing::warn!` but succeeds
  silently; return `HostError::RollbackDiverged` or document why swallowing is OK.
- **[NIT] §16** — no `# Errors`/`# Panics`/`# Safety` sections.
- *Strengths:* explicit `EditClass`, content-addressed-ID reasoning, DUP_SORT
  `delete_one_duplicate` hazard handling, undo-log inverse design.

### Section 07 — Section D (Read View / Navigator) — *major issues*

- **[MAJOR] §9** — `ViewError` embeds `#[error(transparent)] Anyhow(#[from]
  anyhow::Error)` in a domain crate; wrap underlying graph-query errors in a
  concrete typed variant and drop the `anyhow` arm.
- **[MAJOR] §5/§10** — `Navigator<'a>` has `pub snap`, `pub host`, `pub budget`;
  `ClusterId(pub [u8;16])`; most view structs all-public. Privatize; budget via
  `with_budget`.
- **[MAJOR] §7** — `ContextView` stores both `focus` and `scale`, but `scale` is
  derivable from `focus` (contradictory states possible). Drop the field; expose
  `view.scale()`.
- **[MINOR] §5/§11** — `NodePin.kind: &'static str`, `item_kind/visibility:
  Option<String>`: stringly-typed; use the existing `NodeKind` enum; consider a
  `Span` newtype.
- **[MINOR] §7** — add `#[non_exhaustive]` to `Scale`, `NeighborKind`, `NavStep`,
  `ViewError`.
- **[NIT] §5** — `#[must_use]` on verbs returning `ContextView`; `ZoomDir` is
  redundant with `NavStep::{ZoomIn,ZoomOut}`.
- *Strengths:* Sans-I/O stateful core over the snapshot; exhaustive scale-ladder
  matches; otherwise-specific error variants.

### Section 08 — Section E (Description Index) — *minor issues*

- **[MAJOR] §9** — error type never named (bare `Result<T>`) yet
  `DescriptionError::DailyCapReached` is referenced. Define `DescriptionError`
  (thiserror) for network/429/cap/unresolvable/storage; preserve sources.
- **[MAJOR] §12** — `RateLimiter::acquire` has no timeout/cancellation contract;
  `join_all` gives no partial-completion semantics. Add timeouts, bound
  concurrency with a documented semaphore, consider `JoinSet`.
- **[MINOR] §5** — `DescriptionGenerator` exposes `pub model/limiter/stats/store`;
  privatize + builder.
- **[MINOR] §8/§20** — `#[async_trait]` boxes futures; Rust 2024 supports native
  async-fn-in-trait, and `DescriptionGenerator<M>` uses static dispatch — clarify
  whether `dyn` is needed.
- **[MINOR] §14** — confirm `descriptions` feature gating wraps whole items, not
  fields/methods; keep additive.
- **[NIT] §9** — `content.get(span.0..span.1)?` mixes Option-`?` into a `Result`
  fn; map explicitly.
- *Strengths:* content-hash self-invalidation (class C); trait port over LLM
  backends; separate LanceDB table justified.

### Section 09 — Section F (Analyze / Vision) — *major issues*

- **[MAJOR] §9** — `build_from_vcs`/`build_vision`/`label_cluster` all return
  bare `Result`; no error type. Define `AnalyzeError` distinguishing invalid
  input / numeric / VCS-IO / labeler failures.
- **[MAJOR] §17** — `hash64((seed, nid))` / `seed.wrapping_add(k)` use an
  unspecified hash; `DefaultHasher`/`ahash` aren't stable across runs/versions,
  breaking `seeded_clustering_stable`. Mandate a fixed hash / seed-derivation.
- **[MAJOR] §7/§11** — `graph_id`/`head_commit_hash`/`label` stringly-typed; use
  `CommitHash`/`GraphId` newtypes; `ClusterId(pub u32)` field is public.
- **[MAJOR] §7/§9** — no NaN/Inf policy on `silhouette: f32`, BIC argmin, softmax,
  Mahalanobis, PMI/co-change normalization. f32 isn't `Ord`; use `total_cmp`,
  guard zero variances/empty clusters.
- **[MINOR] §10** — `build_vision` (the entry point) and `EmbeddingsLookup` are
  misplaced in `features.rs`; the entry belongs in `mod.rs`.
- **[MINOR] §8** — no `Default` for `BuildVisionOptions` despite documented
  defaults; `LabelGenerator` trait lacks `Debug`/`Send+Sync` supertrait bounds.
- **[NIT] §14** — `analyze` feature pulls deps without `dep:` syntax.
- *Strengths:* deterministic seed threaded through every RNG (`ChaCha8Rng`) with
  an explicit test; soft-membership type design; quality metrics well modeled.

### Section 10 — Section G (modify_body / move / delete) — *major issues* (1 blocker)

- **[BLOCKER] §9/§7** — `body_byte_range` does `node.span.unwrap()`; return
  `EditError::TargetHasNoSource` or pass the validated span in.
- **[MAJOR] §6/§5** — `Crud<'a> { pub host: &'a mut …, pub snap: &'a …, pub
  semantic: &'a mut … }` ties `&mut` borrows to the same lifetime as `&snap` and
  exposes fields. Separate lifetimes; privatize with `new`.
- **[MAJOR] §5/§9** — `e.start_column - 1`, `line_to_byte[(e.start_line - 1)]`:
  unchecked subtraction/indexing on 1-based RA coords panics on `0`/out-of-range;
  char-vs-byte column ambiguity. Use `checked_sub` → `EditError`, validate, doc
  the byte-column assumption.
- **[MAJOR] §9** — `EditOutcome { pub checkpoint }` returned on success; cleanup
  ownership on the success path is unspecified. State RAII/commit semantics;
  `#[must_use]` on `EditOutcome`.
- **[MINOR] §5/§7** — add `#[must_use]` / `#[non_exhaustive]` to `EditOutcome`,
  `EditError`, `CascadePolicy`, `GraphDiffSummary`.
- **[MINOR] §9** — `EditError::IoError(#[from] io::Error)` loses which file/op
  failed; wrap with path context.
- **[NIT] §5** — abbreviations `snap`/`rtxn`/`e`/`s`/`repl`; prefer full names.
- *Strengths:* clean compute/apply split with no formatters (E5 honored);
  descending-sort splice preserves offsets; structured `thiserror` enum
  separating refusals from host rejection.

### Section 11 — Section H (signature / extract / module ops) — *major issues* (1 blocker)

- **[BLOCKER] E5 / code-gen** — Steps 3, 19 (and the whole mutate-syn-then-unparse
  rebuild) still call `prettyplease::unparse`, contradicting the section's own
  header and E5. Rewrite Steps 3, 5, 9, 12, 19, 31 to locate spans + splice
  hand-built strings; delete all `prettyplease` references and the supporting
  Open-decision.
- **[MAJOR] §9/§5** — many `syn::parse_file(&src)?` / `parse_str` / `Command` calls
  use bare `?` with no matching `EditError` variant (`SynParse`, `Io`,
  `CargoMetadata`, `LineIndex`, `HostUnavailable`). Add variants with
  `#[from]`/`#[source]`.
- **[MAJOR] §7/§5** — `SignatureChange.new_sig: FunctionSignature` (whole new sig)
  then re-derive a delta is lossy (rename vs remove+add ambiguous). Prefer an
  explicit ordered edit-list; at minimum document the ambiguity.
- **[MAJOR] §7/§8** — no `#[derive(Debug)]` etc. shown; public `EditError` should
  be `#[non_exhaustive]`; `CallsiteFill::ClosureBuilder(Box<dyn Fn>)` blocks
  `Debug`/`Clone`/`PartialEq` on the whole enum — flag the derive impact.
- **[MINOR] §9** — all variants on one shared `EditError`; confirm it isn't an
  unbounded crate-level god enum.
- **[MINOR] §5/§16** — `is_full_rebuild(self)` fine on a `Copy` enum, but `#
  Errors`/intent docs missing on public verbs.
- **[NIT] §5** — synthesized `__arg_0`/`__arg_self` idents risk hygiene
  collisions; specify a uniqueness scheme.
- *Strengths:* `CallsiteFill::Todo` default (compiles, greppable, non-silent);
  module-per-verb layout with no `mod.rs`.

### Section 12 — Section I (Simulator + Gates) — *minor issues*

- **[MAJOR] §9** — `GateRunner::evaluate(...) -> Result<GateOutcome>` bare; crate
  deps `anyhow`. Define `GateError` (thiserror); drop `anyhow` from `rmc-gates`.
- **[MAJOR] §5/§7** — `GateOutcome { hard, soft, passed }` where `passed ==
  hard.is_empty()` is a derived, desyncable field. Make it a method +
  `#[must_use]`.
- **[MINOR] §7/§11** — `CascadeStep { op_kind: String, reason: String }`,
  `FileEdit { file: String }`: `op_kind` is a closed set → `enum CascadeKind`;
  typed file field.
- **[MINOR] §5/§7** — all-public fields + no `#[non_exhaustive]` on `Effects`,
  `RefusalReason`, `RefusalCode`, `GateThresholds`.
- **[MINOR] consistency** — Overview lists `mut_static_audit`/
  `channel_capacity_audit` but `RefusalCode` has no variants for them.
- **[MINOR] §14** — `thresholds.rs` and `allowlist.rs` each read `gates.toml`
  separately with no validation (e.g. soft ≤ hard); single validated loader.
- **[NIT] §17** — latency benches whose targets "likely fail" are
  non-deterministic; mark aspirational/ignored.
- *Strengths:* pure `compute_effects(&Host)` vs `persist(&mut Host)` split;
  `tarjan_scc` with baseline subtraction for new-cycle detection.

### Section 13 — Section J (Reward + Episode Runner) — *major issues*

- **[MAJOR] §9** — `file.lock().unwrap()` / `steps_buffered.lock().unwrap()` and
  FakeModel `script.lock().unwrap().remove(0)` panic in production paths. Handle
  `PoisonError` (or `parking_lot`); bounds-check the script pop.
- **[MAJOR] §9** — library crates (`rmc-reward`/`rmc-episode`) return bare
  `anyhow::Result`; define `RewardError`/`EpisodeError`; reserve anyhow for
  `rmc-rl`.
- **[MAJOR] §12** — no timeouts on `AnthropicClient::next_action` HTTP or
  `tokio::process::Command` cargo runs; add `reqwest` per-request timeout +
  `tokio::time::timeout` with kill-on-timeout.
- **[MAJOR] §12/§9** — retries described in prose (max_retries=3) but code does a
  single `.send().await?`; `error_for_status()?` collapses 429/5xx. Implement
  backoff and classify status codes.
- **[MAJOR] §5/§13** — `AnthropicClient { pub api_key: String }`: plain public
  secret, `Debug`-leak risk, no redaction. Use `Secret<String>` newtype, private
  field, constructor.
- **[MAJOR] §9/determinism** — no NaN/inf guard; Louvain/conductance/betweenness
  f32 deltas can produce NaN that silently poisons reward. Add `is_finite()`
  clamps; the plan's own 1e-6 reproducibility risk is unenforced by types.
- **[MINOR] §7** — `run(..., force_full: bool)` boolean flag → enum
  `GateScope::{Incremental, FullAtDone}`.
- **[MINOR] §8/§17** — add `PartialEq` for round-trip test assertions; note f32
  fields preclude `Eq`/`Hash` for map keys.
- **[MINOR] §5** — `TestRecord.crate_: String` leaks into serde JSON; use
  `#[serde(rename = "crate")] r#crate` or `crate_name`.
- **[MINOR] §12/§18** — `steps_buffered: Mutex<Vec<StepRecord>>` cloned wholesale
  in `history_view()` each step; full `StepRecord` (incl. `ContextView`) re-sent
  to the model each turn — O(n) per step, unbounded. Window the history.
- **[MINOR] §14/§7** — `--budget` maps to `max_steps` while `--max-tokens`/
  `--max-wall-secs` are separate; rename `--max-steps`.
- **[NIT] §10** — `lib.rs` holds many large public types; consider splitting
  `reward_vector.rs`/`commit.rs`.
- **[NIT] §17** — pilot is correctly `#[ignore]`; ensure mock-server tests don't
  hit the real endpoint; good that `FakeModelClient` exists.
- *Strengths:* closed-world enums with `#[serde(tag=...)]`; `ModelClient` as a
  genuine substitution port; per-session `CARGO_TARGET_DIR` isolation; effects
  pushed to boundaries with a mostly pure `Scalarizer` core.

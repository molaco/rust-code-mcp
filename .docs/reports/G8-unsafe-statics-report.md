# G8 review — unsafe_audit, mut_static_audit, spawn_blocking offload

Reviewer: code review pass over commits `397efd61`, `0e30fa9a`, `22627641`.

## 1. Group summary

This group adds two query-time auditors over the persisted hypergraph
(`unsafe_audit` and `mut_static_audit`), a new persisted-extraction pass
(`extract_statics` → `static_metadata_by_target`), and then walks the most
expensive two MCP tools into `tokio::task::spawn_blocking`.

The three commits are coherent: 397efd61 builds the unsafe-block walker as
live-computation only (no caching), 0e30fa9a adds the persisted
counterpart for statics (write-once-at-build, classify-at-query), and
22627641 cleans up the async story for the two tools that do real
~2-18s synchronous work inside the request handler. The schema/storage
changes are guarded by `SCHEMA_VERSION = 11` (note: prior commit text
shows v10/v11 jump — see §4) and `graph_id_for` hashing, so old snapshots
auto-rebuild.

Overall the code is high-quality, well-documented, tested. The largest
correctness gaps are scope (`unsafe fn`/`impl`/`trait` are NOT detected
by the unsafe auditor, and `Mutex`/`RwLock`/atomics are NOT counted as
"mutable static" patterns) — these are documentation/expectation issues,
not bugs. The spawn_blocking commit is well-targeted but inconsistent
with ~30 other LMDB-touching async handlers in the same file.

## 2. Per-commit review

### 397efd61 — `[review] add unsafe_audit tool for unsafe block detection with SAFETY comment check`

LOC: 394. New module `src/graph/unsafe_audit.rs` (340 lines), tool
wiring in `graph_tools.rs`/`search_tool.rs`/`search_tool_router.rs`, one
method on `OpenedSnapshot` (`queries.rs`).

**What it does.** For every workspace `.rs` file, walks each `ast::BlockExpr`
in the syntax tree and emits a finding when `block.unsafe_token().is_some()`.
Each finding carries: relative file path, byte span (curlies included),
line count, enclosing fn (NodeId + qualified name via
`Semantics::scope_at_offset` → `containing_function`), and a heuristic
`has_safety_comment` flag (substring `SAFETY` anywhere in the up-to-5
preceding source lines). Live computation only — no LMDB write.

**Issues.**

- **[MAJOR — scope]** The auditor only detects `unsafe { ... }` *block expressions*.
  It does NOT detect `unsafe fn`, `unsafe impl`, or `unsafe trait`. The
  tool description and module docstring should make this explicit — and
  arguably the tool name is misleading because the four forms are all
  what Rust calls "unsafe code". `unsafe fn` and `unsafe impl` in
  particular are common (FFI bindings, `Send`/`Sync` impls). At minimum
  document the scope; preferably emit findings for `ast::Fn` with
  `fn.unsafe_token().is_some()`, `ast::Impl` with `impl_.unsafe_token()`,
  and `ast::Trait` with `trait_.unsafe_token()`.
  - Code reference: `src/graph/unsafe_audit.rs:100-110` — only
    `BlockExpr::can_cast(node.kind())` is gated.

- **[MINOR — SAFETY heuristic false positives]** The matcher is a literal
  substring scan for `SAFETY` (case-sensitive) over up to 5 preceding
  *raw* lines — it does NOT require the line to be a comment. Tests at
  `src/graph/unsafe_audit.rs:316-339` confirm the intended behaviour and
  acknowledge the 5-line window. False positives: any preceding code
  containing the bareword `SAFETY` (e.g. a string literal `"SAFETY"`, a
  constant name `pub const SAFETY_LEVEL`, a Rust doc paragraph mentioning
  "thread SAFETY" within a non-comment context — admittedly rare). The
  docstring on `UnsafeFinding.has_safety_comment` (lines 44-46) is honest
  about it being a heuristic. Suggestion: trim the line to the
  comment-marker region (`//` or `/*`) before scanning, or require the
  line to actually start with `//` / `*` after trimming whitespace.

- **[MINOR — SAFETY heuristic false negatives]** The window is exactly
  5 preceding source lines. A multi-line `// SAFETY: ...` rationale that
  exceeds 5 lines will be missed if the marker line is too far back.
  Documented; acceptable.

- **[MINOR — encoded line count not source-line count]** `line_count`
  counts `'\n'` characters in the byte range `[start..end]` of the block
  expr, +1. For DOS-line-ending files (`\r\n`) this still works; for
  files with `\r` only it would undercount. Likely a non-issue in this
  codebase.

- **[MINOR — defensive `min(file_text.len())`]** Line 118 clamps the slice
  with `end_us.min(file_text.len())`. The TextRange should always be in
  bounds because it came from RA's parse of the same file content RA
  loaded into VFS — but reading via `std::fs::read_to_string(&abs_path)`
  (line 89) is NOT guaranteed to be byte-identical to what RA parsed if
  the file was edited between RA load and the `fs::read`. This is fine
  as a guard; worth a code comment explaining the source of the skew.

- **[NIT — qualified-name reconstruction]** The path-segment reconstruction
  (lines 138-167) walks `path_to_root` and prepends the crate name. There's
  a subtle behavioural difference from `Node.qualified_name` elsewhere in
  the graph: this builder always prepends `crate_name`, but
  `lookup_by_qualified_name` may or may not accept that form depending on
  how the graph stores names. The tests pass (`finds_at_least_one_unsafe_block_in_self_workspace`
  + `enclosing_function_resolves_for_snapshot_unsafe`), so it works in
  practice. Worth comparing against `model::Node.qualified_name`
  construction once to ensure they share a canonicalization rule.

- **[NIT — unused import-style observation]** `attach_db(db, || { ... })`
  wraps the whole loop. Correct, but means any panic inside ends up
  through the salsa-attach guard; fine.

**Verdict.** PASS with a `[MAJOR]` follow-up: either extend the auditor
to cover `unsafe fn|impl|trait` or rename the tool to
`unsafe_block_audit` and document the limitation prominently in the tool
description. The tool description on `search_tool_router.rs:455` is
otherwise excellent.

### 0e30fa9a — `[review] add mut_static_audit and statics extraction (Phase 7 Path B)`

LOC: 444. New module `src/graph/statics.rs` (186), additions to `model.rs`
(`StaticMetadata`), `extract.rs` (call the new pass), `snapshot.rs` (persist),
`storage.rs` (new sub-DB `static_metadata_by_target`), `queries.rs`
(`MutStaticFinding`, `MUT_STATIC_PATTERNS`, `classify_metadata`,
`static_metadata`, `mut_static_audit`), and the usual tool/router wiring.

**What it does.** Build-time: for every `ModuleDefId::StaticId` in
`def_to_node`, render the static's HIR type with `HirDisplay` against
its owning crate's `DisplayTarget`, record `(type_string, is_mut)` into
the new LMDB sub-DB. Query-time: iterate `Item` nodes with
`item_kind == ItemKind::Static`, fetch metadata, classify against four
literal substring patterns (`static mut` via `is_mut`; `LazyLock`,
`OnceLock`, `OnceCell` via type-string substring match), emit one
finding per matched pattern.

**Issues.**

- **[MAJOR — pattern coverage]** The task description's headline question
  ("what about interior mutability through `static X: Mutex<...>` or
  `static X: OnceLock<...>`?") is half-answered. `OnceLock` and `OnceCell`
  are in `MUT_STATIC_PATTERNS`, but `Mutex`, `RwLock`, `RefCell`,
  `AtomicU*`/`AtomicI*`/`AtomicBool`/`AtomicUsize`/`AtomicPtr`,
  `parking_lot::Mutex/RwLock`, `tokio::sync::Mutex`, and `arc_swap::ArcSwap`
  are NOT. These are all common forms of interior-mutable global state.
  The classifier is one `const &[(&str, &str)]` away from covering them —
  the architecture is already there. Two questions for the author:
    1. Is the omission intentional (the doc comment at
       `queries.rs:386-391` lists the "why these four" rationale but does
       NOT explain why `Mutex`/`Atomic*` are excluded)?
    2. If unintentional: just extend `MUT_STATIC_PATTERNS`. If intentional:
       state the rationale in the const's docstring AND in the tool
       description, because the natural user expectation for "mut_static_audit"
       is "find global mutable state", and `static X: Mutex<...>` is
       exactly that.

- **[MEDIUM — naming]** Tool name `mut_static_audit` implies `static mut`
  detection. Three of the four patterns (`LazyLock`/`OnceLock`/`OnceCell`)
  are interior-mutability patterns on `static` — not `static mut`. A
  user who wants to find `static mut` only will get noise; a user who
  wants all global mutable state won't realize the tool's scope is
  capped. Either rename (`global_mutable_state_audit`) or add a `mode`
  param. Documented well in the tool description, so this is a
  presentation issue.

- **[MEDIUM — `lazy_static!` documented limitation]** Acknowledged at
  `queries.rs:393-394` and again in the tool description. Suggesting
  `items_with_attribute` or grep as a workaround is honest — but
  `lazy_static!` produces a `struct` per static, not an item with a
  specific attribute, so `items_with_attribute` won't help directly.
  Mention `search` (keyword) as the right fallback.

- **[MINOR — backward compat across snapshots]** `SCHEMA_VERSION` is
  bumped (see `src/graph/storage.rs:115` — current value is `910` in
  worktree but the diff shows the v9→v10 bump comment). The
  `graph_id_for` hash mixes `SCHEMA_VERSION`, so old snapshots are not
  reused — confirmed at `storage.rs:267-273`. New sub-DB
  `static_metadata_by_target` is opened with `open_or_create_bytes_bincode`
  at build, and `Database::open_database` at read. Snapshot upgrade is
  forced (not lazy), which is the right call.

- **[MINOR — empty-type-string skip]** `statics.rs:49-55` skips statics
  whose `type_string` renders empty. Reasonable defensive skip, but
  silently dropping the static means it won't appear in the audit even
  if it's `static mut FOO: () = ()`. Probably fine; the trace log will
  surface the case if anyone investigates.

- **[MINOR — `static mut` row's `type_string`]** When `is_mut == true`
  and the type isn't one of the named patterns, the row still has the
  static's HIR type in `type_string`. Good — but for combo cases (e.g.
  `static mut FOO: LazyLock<...>`), two findings emit with the SAME
  `type_string`. The `matched_pattern` differentiates. Worth a test
  asserting that combo (the existing `classifier_detects_combo` only
  tests the classifier-level merge, not the audit-level deduplication
  behaviour).

- **[NIT — DisplayTarget cache]** `display_targets: HashMap<Crate, DisplayTarget>`
  in `statics.rs:30` is a nice micro-opt. The `*` deref on the entry
  value (line 41) requires `DisplayTarget: Copy` — fine because it is.

- **[NIT — `_vfs` parameter]** `extract_statics` takes `_vfs: &Vfs` and
  doesn't use it. Either drop the param or document why it's reserved.

**Verdict.** PASS structurally. Strongly suggest extending
`MUT_STATIC_PATTERNS` to cover `Mutex`/`RwLock`/`Atomic*`/`RefCell` —
even a single follow-up commit would close the most-asked gap. Cost is
trivial (one entry per pattern); test cost is one extra unit test per
pattern.

### 22627641 — `[review] offload build_hypergraph and unsafe_audit to spawn_blocking`

LOC: 33. Pure wrapper change in two functions in `src/tools/graph_tools.rs`.

**What it does.** Wraps the call to `build_and_persist` in
`build_hypergraph`, and the `loader::load` + `unsafe_audit` block in
`unsafe_audit`, with `tokio::task::spawn_blocking(move || …).await`.
Adds a join-error mapping (`spawn_blocking join error: {e}`) that
unwraps with `??` so the inner `Result<_, McpError>` flows through.

**Issues.**

- **[MEDIUM — inconsistent application]** Only these two tools are offloaded.
  Looking at the same file *after* this commit, there are roughly 30+
  other `pub async fn` handlers that synchronously hit LMDB read txns
  (`open_workspace_snapshot` → `env.read_txn()` → multi-key scans). LMDB
  reads are typically fast, but `dead_pub_report`,
  `crate_dependency_metric`, `forbidden_dependency_check`, `overlaps`,
  `mut_static_audit` (introduced in the SAME group!) and similar
  whole-workspace iterations can be tens to hundreds of milliseconds.
  More importantly: `mut_static_audit` walks ALL `nodes_by_id` and does
  one extra LMDB lookup per Static — at workspace scale (~5-15k items)
  this is non-trivial and runs in the async handler.
  **Suggestion:** either (a) document a "blocking ceiling" policy (LMDB
  reads under N ms are OK in async; loader::load and full-extract MUST
  be offloaded), or (b) wrap `mut_static_audit` and other heavy LMDB
  scans uniformly. The current state is the worst of both — readers
  have to guess which tools are safe under load.

- **[MINOR — cancellation semantics]** `spawn_blocking` returns a
  `JoinHandle` whose drop does NOT abort the underlying thread. If the
  MCP client cancels the request, `loader::load` continues running on
  the blocking pool until completion. This is the standard tokio
  trade-off — but worth a one-line comment. Effects: a flapping
  client could burn through the default `blocking_threads` budget (512
  by default in tokio) with stuck `loader::load` calls.

- **[MINOR — join error formatting]** `McpError::internal_error(format!("spawn_blocking join error: {e}"), None)`
  formats the `JoinError`. `JoinError` only contains useful info when
  the inner task panicked (`.is_panic()`) or was cancelled. Consider
  `.is_panic()` check + `panic_payload` extraction for a better message,
  or at least call out that this branch is reached only on panic /
  cancel. Not blocking.

- **[NIT — `params.directory.clone()` necessary]** In `unsafe_audit`,
  `directory` is cloned out of `params` before moving into the closure
  because `params.directory` is later used in the `Resp` struct (line 1543).
  Correct, but slightly wasteful — `params` could be moved entirely into
  the closure and the closure could return `(directory, findings)`. Minor.

- **[NIT — `build_hypergraph` capture]** `build_and_persist(&dir, opts)`
  with `dir`/`opts` moved into the closure. Both are owned values. Clean.

**Verdict.** PASS as-is for the two tools it touches. The async-hygiene
inconsistency is a cross-commit concern (see §3).

## 3. Cross-commit observations

- **Cross-cutting: schema-version coordination is correct.** The v9→v10
  bump for `static_metadata_by_target` is the right pattern — it's
  hashed into `graph_id_for`, so old snapshots can't be silently reused
  with a missing sub-DB. The current `SCHEMA_VERSION = 910` in the
  worktree suggests the project later jumped numbering schemes; the
  commit-time delta is consistent with prior practice.

- **Cross-cutting: query module hygiene.** `mut_static_audit` lives as
  a method on `OpenedSnapshot` in `queries.rs` (consistent with other
  audits). `unsafe_audit` lives in its own module (`graph/unsafe_audit.rs`)
  but is invoked via a thin method on `OpenedSnapshot`. The split is
  defensible (unsafe_audit needs RA `Semantics`, mut_static_audit is
  pure LMDB) but it's worth documenting the rule in `queries.rs`:
  "audits that need a live `LoadedWorkspace` live in their own module
  and take it as a parameter; audits that read only persisted data are
  methods on `OpenedSnapshot`."

- **Cross-cutting: spawn_blocking is half-done.** 397efd61 introduces
  `unsafe_audit` doing ~2-3s of synchronous work in an async fn —
  exactly the bug class 22627641 fixes. They're three commits apart in
  the same group: it would have been cleaner to introduce `unsafe_audit`
  with `spawn_blocking` from the start (one-line addition at intro).
  Now there's a 2-commit window where `unsafe_audit` blocks the runtime.
  For history-hygiene, this is fine; for a "feature complete" group
  boundary, the offload should have been part of 397efd61.

- **Cross-cutting: testing parity.** `unsafe_audit` has four tests
  covering happy-path, no-SAFETY case, enclosing-fn resolution, and
  the heuristic edge cases. `statics`/`mut_static_audit` has seven
  tests covering each classifier branch, audit smoke, known-LazyLock
  detection, and metadata round-trip. Symmetric and adequate. Neither
  has a "no findings" empty-workspace test — the project may have one
  elsewhere via the shared snapshot fixture.

- **Cross-cutting: server instructions string.** The big instructions
  block in `search_tool_router.rs:476-484` is hand-maintained and
  appended to with each new tool. By the end of 0e30fa9a it lists 32
  tools. Risk: this is exactly the kind of string that drifts. A
  follow-up to derive the instructions from the `#[tool(description=…)]`
  attributes would eliminate this. Not blocking for this group.

- **Cross-cutting: NodeId rendering.** Both new tools render `NodeId`s
  as hex strings (`n.to_hex()`) in the MCP `Resp` structs, rather than
  the raw 32-byte array `serde_bytes_32` would emit. Consistent with
  other tools, good.

## 4. Overall verdict — PASS (with one MAJOR documentation/coverage follow-up each)

The group is well-engineered, well-tested, and the schema-versioning
discipline is solid. Two follow-ups are worth filing as separate issues:

1. **`unsafe_audit` scope.** Either extend to `unsafe fn`/`unsafe impl`/
   `unsafe trait` (preferred; mechanical change against
   `ast::Fn`/`ast::Impl`/`ast::Trait` `.unsafe_token()`), or rename to
   `unsafe_block_audit` and update the tool description to be explicit
   that the other three forms are out of scope.

2. **`mut_static_audit` pattern coverage.** Add `Mutex`, `RwLock`,
   `RefCell`, `Atomic*` (or a small set: `AtomicBool`, `AtomicUsize`,
   `AtomicI64`, `AtomicU64`, `AtomicPtr`) to `MUT_STATIC_PATTERNS`.
   Trivial addition; closes the obvious "what about `Mutex<...>`?"
   user question. If the omission is intentional, document the rationale
   in `queries.rs:385-394`.

Neither blocks merge. The `spawn_blocking` commit is correct and small;
the inconsistency with other LMDB-heavy tools is a project-wide
async-hygiene policy decision, not a regression introduced here.

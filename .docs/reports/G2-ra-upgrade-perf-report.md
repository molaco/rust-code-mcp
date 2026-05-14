# G2 — RA upgrade + perf infra — review

## 1. Group summary

This group upgrades rust-analyzer from 0.0.313 to 0.0.330, flips the graph loader
from a workspace-only/`no_deps: true` setup to a `no_deps: false` +
`sysroot: Discover` configuration to enable cross-crate resolution, and recovers
the perf cost that came with that flip via two tactics: (a) reducing the default
log level so RA's millions of `tracing::debug!` events don't bottleneck on the
formatter, and (b) sharing a single workspace load across all in-module tests
(plus prefilling DefMap caches on load). Functionally the upgrade is competent —
field renames and API changes (`load_workspace` → `load_workspace_at`,
`minicore` → `ra_fixture`, new `FindAllRefsConfig` fields, new
`LoadCargoConfig` fields) are picked up cleanly. The main caveats are: a
genuine logic change (a bindings dedup that drops the Value-namespace half of
unit/tuple struct bindings) is silently bundled into the test-perf commit, the
e68b2b1c commit briefly regresses `prefill_caches` from `true` to `false`
before 90508cd4 restores it, and stale documentation/tool descriptions still
claim `no_deps=true`.

## 2. Per-commit review

### e68b2b1c — upgrade ra_ap to 0.0.330 and enable cross-crate resolution via sysroot+deps (635 LOC)

**What it does**

- Bumps all `ra_ap_*` crates from `0.0.313` → `0.0.330` (Cargo.toml + lockfile).
- Rewrites `src/graph/loader.rs`:
  - drops the manual `ProjectManifest::discover_single` / `ProjectWorkspace::load`
    two-step in favor of `load_workspace_at`.
  - switches from `no_deps: true` to `no_deps: false`, adds
    `sysroot: Some(RustLibSource::Discover)`, `features: CargoFeatures::All`,
    `all_targets: true`.
  - replaces the cargo-metadata-driven `member_roots` filter with
    `Crate::origin(db).is_local()`.
  - **regresses** `prefill_caches: true → false` (silently — not mentioned in
    the commit message; restored later in 90508cd4).
  - adds the new `num_worker_threads` / `proc_macro_processes` fields required
    by `LoadCargoConfig` in 0.0.330.
- `src/semantic/loader.rs`: adds the new `LoadCargoConfig` fields; leaves
  `sysroot: None, no_deps: true` (intentional asymmetry — semantic ops stay
  fast and single-file).
- `src/semantic/position.rs`: replaces `minicore: Default::default()` with
  `ra_fixture: ra_ap_ide_db::ra_fixture::RaFixtureConfig::default()` in both
  `GotoDefinitionConfig` and `FindAllRefsConfig`; adds new fields
  `exclude_imports: false, exclude_tests: false` (preserves prior "include
  everything" behavior).
- Three new dev examples (`debug_itemscope.rs`, `probe_workspace.rs`,
  `rebuild_burn_default.rs`) plus their `[[example]]` entries.

**Issues**

- **Major — silent perf regression bundled in upgrade**: the diff flips
  `prefill_caches: true → false` in `src/graph/loader.rs`. This is not mentioned
  in the commit message and is at odds with the message's "enable cross-crate
  resolution" framing — it would slow extraction by ~30× per the comment added
  later. The next-but-one commit (90508cd4) restores it, so the head of the
  group is fine, but the intermediate commit is bisect-hostile.
- **Minor — stale tool description**: `src/tools/search_tool_router.rs:231` still
  describes `build_hypergraph` as "HIR-driven, no_deps=true". That string is
  user-facing (MCP tool listing). Should be updated to reflect the new
  cross-crate-resolution behavior.
- **Minor — loader doc-comment is misleading about lockfile env**: the rewritten
  module doc on `src/graph/loader.rs` claims RA ≥ 0.0.328 uses
  `CARGO_RESOLVER_LOCKFILE_PATH` "instead of `--lockfile-path` to avoid mutating
  Cargo.lock". Our code does not set that env var — the comment is describing
  RA-internal behavior but reads as if it documents our code. Either drop the
  paragraph or rephrase to make clear it's a transitive observation, not
  something we configure.
- **Minor — example portability**: `examples/debug_itemscope.rs` and
  `examples/rebuild_burn_default.rs` hard-code
  `/home/molaco/Documents/burn`. Pre-existing convention in this examples/
  directory, but the new probe (`probe_workspace.rs`) is correctly argv-driven
  and is the model the others should follow.
- **Nit — unused `paths` / `vfs` plumbing reduced**: the new `filter_local_crates`
  no longer needs vfs or the member_roots set, so the function signature
  correctly drops them. Worth double-checking that `is_local()` matches the
  old `member_roots` set on the existing snapshot; the kept
  `loads_self_workspace` test gives some confidence but only checks
  non-emptiness + presence of `file_search_mcp`.
- **Nit — bounded-ness of cross-crate resolution**: with `no_deps: false`
  + `sysroot: Discover` + `all_targets: true`, RA loads the full dep graph and
  sysroot. The walk downstream is bounded by `local_crates` (filtered to
  `origin.is_local()`), and `impls.rs:212-215` drops items whose nav file
  resolves outside the workspace. So although RA holds more state, our
  extraction output stays scoped. This is safe but not asserted by any test —
  worth a regression test that confirms `local_crates` count is stable across
  the change.

**Verdict**: correct upgrade; the embedded `prefill_caches: false` regression
is the only real wart. Major.

### b5e7bb44 — reduce default log level from DEBUG to WARN (12 LOC)

**What it does**

Replaces the previous `EnvFilter::from_default_env().add_directive(Level::DEBUG)`
default with `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn,file_search_mcp=info"))`.
Comment explains the 7s → 7+ minute regression that DEBUG-level RA tracing
caused on the formatter+stderr pipeline.

**Issues**

- **Nit — RUST_LOG semantics**: `try_from_default_env()` reads `RUST_LOG`. If
  the env var is set but malformed, `try_from_default_env` returns an `Err`
  and we silently fall through to the default. Previous behavior was the same
  (the explicit `Level::DEBUG.into()` directive would still apply if env
  parsing failed). Net same. No issue.
- **Nit — important-warnings exposure**: our crate's own `info!`/`debug!`
  emissions are still surfaced (`file_search_mcp=info`). `warn!` from any
  crate is preserved. Errors and warnings still surface. The only thing
  silenced relative to the prior default is library-crate `debug!`/`trace!`,
  which is exactly what we want.

**Verdict**: clean, well-motivated, low-risk. Pass.

### 90508cd4 — speed up graph tests via shared workspace cache and prefill DefMap caches (101 LOC)

**What it does**

- `src/graph/loader.rs`: restores `prefill_caches: false → true` and adds a
  comment explaining the ~30× extraction-time impact.
- `src/graph/extract.rs`: introduces `shared_model()` using
  `OnceLock<ExtractionModel>` so all `#[cfg(test)]` cases in the module reuse
  a single load+extract pass.
- `src/graph/queries.rs`: introduces `shared_snapshot()` returning
  `&'static OpenedSnapshot`, with a `SharedSnap` wrapper that keeps the
  `tempfile::TempDir` alive for the process lifetime; all 30+ tests rewritten
  to call it instead of building a fresh snapshot per test.
- `src/graph/bindings.rs`: **adds a post-hoc HashSet-based dedup** at the end of
  `extract_bindings`, dropping any duplicate `(from_module, visible_name,
  target, kind)` rows. The commit message does not mention this.

**Issues**

- **Major — undisclosed semantic change in bindings dedup**: this is not a perf
  change. Before this commit, a unit/tuple struct named `Foo` imported with
  `use other_crate::Foo;` produced two `Binding` rows (one in the Type
  namespace, one in the Value namespace) — both stored, because
  `binding_id_for` (src/graph/snapshot.rs:313) keys on namespace (`"T"`/`"V"`).
  After this commit, the dedup keys on `(from_module, visible_name, target,
  kind)` — namespace is dropped from the key — so the second row is filtered
  out before storage. Downstream queries that surface `binding.namespace`
  (see `namespace_label` in `src/tools/graph_tools.rs:2293-2298`, fed at
  `:2209`) will therefore only ever see `Type` for the deduped pairs and
  never `Value`. Binding counts also drop. This may be the right call (the
  duplication is genuinely redundant), but it deserves its own commit + a
  test that pins down the new contract. As stands, it is a behavior change
  bundled into a "speed up tests" commit.
- **Minor — `SharedSnap` and `OnceLock<ExtractionModel>` leak their TempDir**:
  static `OnceLock` values are not dropped at process exit, so the `TempDir`
  inside is never cleaned up via `Drop`. In practice the OS cleans `/tmp` on
  reboot and the directory is small, so this is operationally fine; mention
  it because the original per-test `let (_, td) = ...` pattern did clean up.
- **Minor — shared snapshot weakens isolation**: heed/LMDB read transactions
  are per-call and short-lived, so concurrent test access is safe. But any
  future test that mutates the snapshot (or relies on a fresh build per run
  to catch ordering bugs in `build_and_persist`) will now silently observe
  whatever the first call cached. That risk should be documented in the
  `shared_snapshot()` comment so future authors don't add destructive tests
  to this module.
- **Nit — `shared_snapshot()` is `pub(crate)`**: making the cache visible to
  other modules invites accidental reuse from suites that may want different
  build options. Worth a comment that only this module's tests should use
  it.

**Verdict**: the perf parts (prefill restore, shared cache) are correct and
well-motivated. The bindings dedup is a separate logic change that should not
have been merged in this commit without disclosure or a test. Major.

## 3. Cross-commit observations

- **Internal regression / restoration of `prefill_caches`**: e68b2b1c switches
  it `true → false`, 90508cd4 switches it back `false → true`. At the head of
  the group the value is correct, but the intermediate commit is significantly
  slower than either neighbor. If commits are ever cherry-picked or bisected,
  this manifests as a confusing transient perf cliff. The cleanest fix is to
  squash the prefill-toggle out of e68b2b1c entirely.
- **API breakage handled completely**: every RA API change visible in the diff
  (new `LoadCargoConfig` fields, renamed `minicore` → `ra_fixture`, new
  `FindAllRefsConfig` exclude flags, `load_workspace` → `load_workspace_at`)
  is mirrored at every call site. `cargo check --lib` succeeds with only
  pre-existing-style warnings (`unreachable pub`).
- **Cross-crate resolution is safe and bounded**: although `no_deps: false`
  pulls the full cargo + sysroot graph into RA, our walks only iterate
  `local_crates` (filtered via `origin.is_local()`), the impl-extraction
  drops items whose nav resolves outside the workspace
  (`src/graph/impls.rs:212-215`), and `semantic/loader.rs` keeps its fast
  single-workspace mode. The blast radius is contained to the hypergraph
  pipeline.
- **Logging change interaction with the upgrade**: the DEBUG→WARN change in
  b5e7bb44 was reactive — RA 0.0.330 with `no_deps: false` is exactly the
  workload that triggers the DEBUG-level overload described in the commit
  message. The two changes together correctly leave the user opt-in to
  verbose RA tracing via `RUST_LOG`.
- **Documentation drift**: the tool description string for `build_hypergraph`
  in `src/tools/search_tool_router.rs:231` still says `no_deps=true`. Visible
  to MCP clients on every tool listing. Worth a tiny follow-up.

## 4. Overall verdict — MINOR

The upgrade itself is correct and the perf instrumentation is well-targeted.
Two issues keep this from a clean pass:

1. The `prefill_caches: true → false` toggle hidden inside e68b2b1c is a
   bisect/cherry-pick footgun even though the head state is right.
2. The bindings post-hoc dedup added by 90508cd4 is a real semantic change
   (collapses the Type+Value namespace pair for ADTs) bundled into a "speed
   up tests" commit, with no test coverage of the new contract and no
   mention in the commit message.

Both are recoverable without reworking the upgrade. Recommend (a) move the
prefill toggle out of e68b2b1c, and (b) split the bindings dedup into its own
commit with a regression test that pins down how downstream
namespace-reporting tools should treat ADT bindings now that only one row
survives. Also worth a one-line fix to the `build_hypergraph` MCP description.

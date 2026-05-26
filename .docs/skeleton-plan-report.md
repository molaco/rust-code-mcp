# Implementation Report: `crate_skeleton`

Date: 2026-05-26

Plan: `.plans/skeleton-plan.md`

Implementation range: `e4a8960e..c47a97d6`

Status: phases 0 through 5 complete. Optional phases 6 through 8 remain
deferred.

## Summary

Implemented `crate_skeleton`, an MCP graph tool that writes a stripped,
mirrored Rust facade tree under `<workspace>/.skeleton/`.

The final v1 contract is deliberately item-file based. It renders retained
items into files mirroring each item's recorded `Node.file`, strips executable
bodies and const/static initializers, preserves item docs/attrs when requested,
and writes parseable Rust facades for the selected crates.

The implementation reuses the existing persisted hypergraph snapshot,
workspace snapshot opening path, list pagination helpers, graph visibility
queries, and router/parameter patterns. It does not introduce a schema bump.

## Commit Sequence

- `e4a8960e` - `docs: narrow skeleton plan for item-file v1`
- `50e49e97` - `docs: record skeleton phase 0 preflight`
- `27c2cb82` - `feat: add skeleton graph stub renderer`
- `49b36805` - `feat: render skeletons from source slices`
- `80295290` - `feat: expose crate skeleton tool`
- `d7e4b2ad` - `docs: document crate skeleton tool`
- `c47a97d6` - `test: harden crate skeleton output`

## Phase 0: Preflight

Actions:

- Ran `jj show --summary` before phase work.
- Re-read `AGENTS.md`.
- Confirmed the `cuda-code` nix dev shell requirement.
- Confirmed the no-formatting rule.
- Checked the working copy before implementation.

Output:

- No production code changed in this phase.
- Plan state was recorded in `.plans/skeleton-plan.md`.

## Phase 1: Graph Skeleton Core

Actions:

- Added `.skeleton/` to `.gitignore`.
- Excluded `.skeleton/` from source fingerprinting and codemap freshness scans.
- Introduced shared graph query helpers in `query/shared.rs`.
- Reused those helpers from module and overlap queries.
- Added the graph skeleton module:
  - `crates/rmc-graph/src/graph/skeleton/mod.rs`
  - `crates/rmc-graph/src/graph/skeleton/model.rs`
  - `crates/rmc-graph/src/graph/skeleton/collect.rs`
  - `crates/rmc-graph/src/graph/skeleton/render.rs`
- Exported `render_crate_skeletons`, `SkeletonOptions`, `SkeletonOutput`,
  `SkeletonFile`, and `SkeletonDiagnostic` from `graph/mod.rs`.

Important behavior:

- Default visibility includes `pub` and `pub(crate)`.
- `include=["all"]` expands to all supported visibility buckets.
- Vendor crates are excluded by default.
- Test items are pruned by v1 heuristics by default.
- V1 stays item-file only; it does not synthesize module declarations or place
  re-exports.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib overlap_scope_filters_examples_and_vendor
```

## Phase 2: Source-Sliced Rendering

Actions:

- Added `crates/rmc-graph/src/graph/skeleton/source.rs`.
- Added `SourceCache` keyed by workspace-relative graph file paths.
- Rendered declarations from source spans when source is available and spans
  are valid.
- Replaced function and trait default bodies with placeholder bodies.
- Replaced const/static initializer expressions with `todo!()`.
- Preserved item docs and attrs from `Node.attributes` according to options.
- Added fallback synthetic rendering plus diagnostics for missing source or bad
  spans.
- Added synthetic inherent impl facades only for associated items whose parent
  is a struct, enum, or union.

Important behavior:

- Trait associated items are not duplicated into synthetic impl blocks.
- Module/crate-root attrs are not preserved in v1 because the current graph
  persists item attrs, not module declaration source locations.
- Source access is conservative: an unreadable or stale span affects the item,
  not the whole skeleton render.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
```

## Phase 3: MCP Endpoint And Filesystem Writer

Actions:

- Added `CrateSkeletonParams` in
  `crates/rmc-server/src/tools/params/graph.rs`.
- Added `crates/rmc-server/src/tools/graph/skeleton.rs`.
- Registered `crate_skeleton` in the graph module and router.
- Opened the existing workspace snapshot with `open_workspace_snapshot`.
- Wrapped render and filesystem writes in `tokio::task::spawn_blocking`.
- Wrote generated files under `<workspace>/.skeleton/` using safe relative
  paths.
- Implemented exact `.skeleton/` cleanup when `clean=true`.
- Added paginated `files_written` summaries.
- Added `summary=true` support using existing `ListMeta` behavior.

Important behavior:

- The endpoint refuses unsafe source paths.
- Cleanup targets only the exact `<directory>/.skeleton` child directory.
- Sibling directories such as `.skeleton-backup` are not removed.
- `files_written` can be paginated or suppressed while totals remain
  authoritative.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib crate_skeleton
```

## Phase 4: Public Documentation

Actions:

- Added `crate_skeleton` to the `TOOLS.md` overview.
- Documented endpoint parameters, defaults, example invocation, response shape,
  and v1 limitations.
- Added `crate_skeleton` to `README.md` tool categories and usage notes.
- Documented `.skeleton/` as the explicit project-write exception.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
```

## Phase 5: End-To-End Quality Pass

Actions:

- Added deterministic ordering tests for collected files and items.
- Added deterministic ordering tests for synthetic impl members.
- Added missing-source diagnostics coverage.
- Added missing-span fallback coverage.
- Strengthened the server endpoint smoke test to build a current workspace
  snapshot and generate real skeleton files for `rmc_server` and `rmc_graph`.
- Asserted generated output contains expected declarations:
  - `.skeleton/crates/rmc-server/src/tools/router.rs` contains
    `fn crate_skeleton`
  - `.skeleton/crates/rmc-graph/src/graph/model.rs` contains
    `pub struct Node`
- Confirmed endpoint cleanup leaves no `.skeleton/` or `.skeleton-backup`
  artifacts behind.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib crate_skeleton
nix develop ../nix-devshells#cuda-code --command cargo check --workspace --lib
```

Result:

- All feature-focused tests passed.
- The workspace library check passed.
- The workspace check still reports existing dead-code and unreachable-pub
  warnings outside the skeleton changes.

## Files Changed

Production:

- `.gitignore`
- `README.md`
- `TOOLS.md`
- `crates/rmc-graph/src/graph/codemap/build.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-graph/src/graph/query/modules.rs`
- `crates/rmc-graph/src/graph/query/overlaps.rs`
- `crates/rmc-graph/src/graph/query/shared.rs`
- `crates/rmc-graph/src/graph/skeleton/collect.rs`
- `crates/rmc-graph/src/graph/skeleton/mod.rs`
- `crates/rmc-graph/src/graph/skeleton/model.rs`
- `crates/rmc-graph/src/graph/skeleton/render.rs`
- `crates/rmc-graph/src/graph/skeleton/source.rs`
- `crates/rmc-graph/src/graph/storage.rs`
- `crates/rmc-server/src/tools/graph/mod.rs`
- `crates/rmc-server/src/tools/graph/skeleton.rs`
- `crates/rmc-server/src/tools/params/graph.rs`
- `crates/rmc-server/src/tools/router.rs`

Tests and plan/docs:

- `.plans/skeleton-plan.md`
- `crates/rmc-server/src/tools/graph/tests.rs`

Cumulative implementation diff after narrowing the plan:

```text
20 files changed, 2114 insertions(+), 136 deletions(-)
```

## Public Interface

Graph API:

- `render_crate_skeletons(snap, opts) -> Result<SkeletonOutput>`
- `SkeletonOptions`
- `SkeletonOutput`
- `SkeletonFile`
- `SkeletonDiagnostic`

MCP tool:

- `crate_skeleton`

Parameters:

- `directory`
- `crates`
- `include`
- `include_docs`
- `include_attrs`
- `include_impls`
- `skip_test_items`
- `exclude_vendor`
- `clean`
- pagination fields from `ListPaginationParams`

Response:

- `skeleton_dir`
- `snapshot_id`
- `page`
- `files_written`
- `total_files`
- `total_items`
- `total_bytes`
- `diagnostics`

## Reuse Decisions

Reused existing infrastructure:

- Persisted hypergraph snapshot loading through `open_workspace_snapshot`.
- Existing graph item model and source file paths.
- Existing visibility/binding data.
- Existing `ListPaginationParams`, `ListMeta`, `list_page`, and `page_list`.
- Existing router method pattern.
- Existing JSON result and MCP error helpers.
- Existing codemap and fingerprint source-walk concepts, with a shared
  generated-directory exclusion helper.
- `ra_ap_syntax` for parse-aware declaration rewriting instead of ad hoc full
  text manipulation.

Avoided duplicate infrastructure:

- No new snapshot format.
- No new persistence tables.
- No new pagination model.
- No parallel tool registry.
- No formatter dependency.

## Known V1 Limits

- Module declarations are not emitted.
- Inline module wrappers are not emitted.
- Re-exports are not placed.
- Files that only contain module declarations or re-exports are not represented
  unless they also contain retained graph items.
- Module and crate-root attrs are not preserved.
- Test filtering is heuristic:
  - module paths containing `::tests::`
  - item-level `#[test]`
  - item-level `#[cfg(test)]`
- Output is intended as a navigable, parseable facade, not a compilable crate.

## Deferred Work

Phase 6 remains the next correctness expansion if needed:

- Recover source locations for module declarations.
- Recover source locations for explicit public re-exports.
- Preserve source-exact `pub use` text.
- Add module/crate-root attr preservation after placement is source-backed.

Phase 7 remains optional:

- Add `.skeleton/manifest.json`.

Phase 8 remains optional:

- Persist additional pure-snapshot metadata if source IO becomes a practical
  problem.

## Independent Review

Date: 2026-05-26

Method:

- Spawned three read-only `gpt-5.5` subagents with `xhigh` reasoning.
- Split review by phase group:
  - Phases 1-2: `rmc-graph` skeleton collection/rendering.
  - Phase 3: `rmc-server` endpoint and filesystem writer.
  - Phases 0, 4, and 5: preflight, docs, and coverage/report accuracy.
- Parent pass inspected the same implementation and consolidated findings.
- No formatting commands were run.

Scores:

| Phase | Score | Review summary |
|-------|-------|----------------|
| Phase 0 | 9/10 | Preflight bookkeeping is sound and records the project rules. |
| Phase 1 | 8/10 | Core graph API, shared helpers, `.skeleton/` exclusion, and collector shape mostly match the plan. |
| Phase 2 | 6/10 | Source-sliced rendering works for happy paths, but important fallback and associated-item cases are incomplete. |
| Phase 3 | 7/10 | Endpoint surface and writer are mostly present; test isolation and symlink-hardening need work. |
| Phase 4 | 9/10 | Public docs are complete and honest about the v1 contract. |
| Phase 5 | 8/10 | Focused coverage improved substantially, but some planned deterministic/parseability checks are still indirect. |

Overall: 7.8/10. The feature is usable as an item-file v1 facade generator, but Phase 2 has correctness gaps that should be fixed before treating the skeleton output as dependable API context under stale-source or default-filter conditions.

### Review Findings

1. Phase 2: public inherent impl items can be omitted by default.

   Associated items are retained through the same visibility filter as
   module-level items. The impl extraction currently records methods,
   associated consts, and associated types with `visibility: None`, and those
   associated items do not have module-scope declared bindings. With the
   default `include = ["pub", "pub(crate)"]`, they bucket as private and can
   be dropped unless callers request `include = ["all"]`.

   References:

   - `crates/rmc-graph/src/graph/impls.rs:253`
   - `crates/rmc-graph/src/graph/skeleton/collect.rs:257`
   - `crates/rmc-graph/src/graph/skeleton/collect.rs:279`
   - `crates/rmc-graph/src/graph/query/shared.rs:36`

2. Phase 2: fallback rendering does not use persisted signature/static metadata.

   The plan requires fallback functions to use `FunctionSignature` and
   fallback statics to use `StaticMetadata`. The implementation currently falls
   back to hardcoded declarations such as `fn name()` and `static name: ()`,
   losing arguments, return types, generics, and static types when source is
   missing or spans are stale.

   References:

   - `crates/rmc-graph/src/graph/skeleton/source.rs:35`
   - `crates/rmc-graph/src/graph/skeleton/source.rs:48`
   - `crates/rmc-graph/src/graph/skeleton/source.rs:324`

3. Phase 2: stale/unparseable source handling is too permissive.

   Parse errors are diagnosed but the parsed tree is still used. Span lookup can
   also select an expected-kind item inside the persisted span without checking
   the item name. After source drift, that can silently render the wrong
   declaration instead of falling back with a stale-span diagnostic.

   References:

   - `crates/rmc-graph/src/graph/skeleton/source.rs:79`
   - `crates/rmc-graph/src/graph/skeleton/source.rs:152`
   - `crates/rmc-graph/src/graph/skeleton/source.rs:160`

4. Phase 3: endpoint tests are not isolated from the real workspace.

   The endpoint smoke test builds against the real workspace root, creates
   `.skeleton/` and `.skeleton-backup`, runs `clean=true`, then removes both
   directories. Because `.skeleton/` is ignored generated output, this can
   delete a developer's pre-existing local skeleton tree or a real
   `.skeleton-backup` directory. The plan called for a temp fixture workspace.

   References:

   - `crates/rmc-server/src/tools/graph/tests.rs:600`
   - `crates/rmc-server/src/tools/graph/tests.rs:609`
   - `crates/rmc-server/src/tools/graph/tests.rs:619`
   - `crates/rmc-server/src/tools/graph/tests.rs:717`

5. Phase 3: writer path validation is lexical only.

   `safe_relative_source_path` rejects absolute paths and `..`, which is good,
   but `clean=false` can still write outside `.skeleton/` through pre-existing
   symlinked directories or symlinked output files because `fs::write` follows
   symlinks.

   References:

   - `crates/rmc-server/src/tools/graph/skeleton.rs:35`
   - `crates/rmc-server/src/tools/graph/skeleton.rs:55`
   - `crates/rmc-server/src/tools/graph/skeleton.rs:64`
   - `crates/rmc-server/src/tools/graph/skeleton.rs:188`

6. Phase 5: deterministic crate ordering is implemented but not directly tested.

   The collector sorts selected crates by crate name before traversal, but the
   added deterministic-order test only asserts sorted file paths and per-file
   item order. It does not directly lock down crate-order behavior.

   References:

   - `crates/rmc-graph/src/graph/skeleton/collect.rs:80`
   - `crates/rmc-graph/src/graph/skeleton/collect.rs:496`

7. Phase 5: current-workspace parseability is not checked end to end.

   Graph-level tests parse generated files from the shared snapshot, but the
   endpoint smoke test that writes real current-workspace `.skeleton/` files
   only checks file existence and selected substrings.

   References:

   - `crates/rmc-graph/src/graph/skeleton/render.rs:199`
   - `crates/rmc-server/src/tools/graph/tests.rs:599`

### Review Verification

Subagent verification:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib overlap_scope_filters_examples_and_vendor
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib crate_skeleton
```

Results reported by subagents:

- `rmc-graph` check passed.
- `rmc-graph` skeleton tests passed, 13 tests.
- `rmc-graph` overlap scope focused test passed.
- `rmc-server` `crate_skeleton` focused test passed when run alone, 1 test.

Parent verification notes:

- `jj status` was clean before the review.
- A parent duplicate `rmc-server` focused test run failed while another graph
  test was running concurrently, with `open_current_for_workspace: snapshot
  exists but databases not initialized` in a pre-skeleton shared graph test
  path. The same `rmc-server` focused test passed in the isolated subagent run.
  Treat this as additional evidence that graph endpoint tests share fragile
  workspace snapshot state, not as a direct `crate_skeleton` endpoint failure.

### Recommended Follow-Up

1. Fix associated-item visibility for skeleton rendering so public inherent
   methods/assoc items survive the default include filter.
2. Make fallback rendering consult `FunctionSignature` and `StaticMetadata`.
3. Tighten source-slice lookup with name validation and stronger stale-span
   fallback behavior.
4. Move endpoint tests to an isolated fixture or preserve/restore any
   pre-existing `.skeleton/` and `.skeleton-backup` paths.
5. Harden writer behavior around symlinked `.skeleton/` contents, especially
   for `clean=false`.
6. Add direct tests for crate ordering and parse the real endpoint-generated
   files in the smoke path.

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

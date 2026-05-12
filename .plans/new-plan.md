# Detailed plan: split `src/graph/queries.rs` safely

## Goal

Split the 3,571-line `src/graph/queries.rs` into smaller topic files while
preserving the existing public module path:

- Keep `crate::graph::queries::*` working.
- Keep the flat `crate::graph::{...}` re-exports in `src/graph/mod.rs`.
- Keep existing query behavior unchanged.
- Avoid combining this with a broad `graph/` directory reshuffle.

This plan intentionally focuses on the one file that is actually too large.
The optional `extract/`, `store/`, and `audit/` grouping should be a separate
refactor after this one is green.

## Constraints

- Use `jj status` before and after major steps. This repo uses jujutsu.
- Do not run `cargo fmt` or any formatting command.
- Do not touch unrelated working-copy changes.
- Prefer small, compile-checkable steps.
- Use compatibility shims when moving public module paths.

## Current risks this plan addresses

1. `src/graph/mod.rs` currently declares `pub mod queries;`, and callers use
   `crate::graph::queries::ItemWithAttribute`. Moving to `graph/query/` would
   break that path.
2. Tests in multiple files depend on
   `crate::graph::queries::tests::shared_snapshot`.
3. Some tests hard-code canonical names under
   `file_search_mcp::graph::queries`.
4. Private helper methods and functions will become inaccessible across
   sibling modules unless their visibility is widened narrowly.

## Target layout

Keep the module name plural:

```text
src/graph/
├── queries/
│   ├── mod.rs
│   ├── lookup.rs
│   ├── scope.rs
│   ├── usages.rs
│   ├── call_graph.rs
│   ├── items.rs
│   ├── reexports.rs
│   ├── crate_graph.rs
│   ├── workspace.rs
│   ├── cursors.rs
│   └── labels.rs
├── test_support.rs
└── mod.rs
```

`src/graph/mod.rs` should still contain:

```rust
pub mod queries;
```

Do not rename the public module to `query`.

## Phase 0: preflight

1. Check the working copy:

   ```sh
   jj status
   ```

2. Note unrelated dirty files and avoid editing them.

3. Confirm the current query file size:

   ```sh
   wc -l src/graph/queries.rs
   ```

4. Optional baseline check:

   ```sh
   cargo check
   ```

   Do not run `cargo fmt`.

## Phase 1: introduce stable shared test support

Do this before splitting `queries.rs`.

1. Add `src/graph/test_support.rs`.

2. Move the current `SharedSnap` struct and `shared_snapshot()` function from
   `queries.rs::tests` into `test_support.rs`.

3. Gate the module in `src/graph/mod.rs`:

   ```rust
   #[cfg(test)]
   pub(crate) mod test_support;
   ```

4. Update these imports:

   - `src/graph/attributes.rs`
   - `src/graph/signatures.rs`
   - `src/graph/statics.rs`
   - `src/graph/unsafe_audit.rs`
   - `src/graph/queries.rs`

   Replace:

   ```rust
   use crate::graph::queries::tests::shared_snapshot;
   ```

   with:

   ```rust
   use crate::graph::test_support::shared_snapshot;
   ```

5. Keep `classify_metadata` imported from `crate::graph::queries` for now.

6. Run targeted tests:

   ```sh
   cargo test shared_snapshot
   cargo test graph::attributes
   cargo test graph::signatures
   cargo test graph::statics
   cargo test graph::unsafe_audit
   ```

   Do not run `cargo fmt`.

## Phase 2: convert `queries.rs` into a directory module

This is a mechanical move with no semantic split yet.

1. Create the directory:

   ```sh
   mkdir -p src/graph/queries
   ```

2. Move the file:

   ```sh
   mv src/graph/queries.rs src/graph/queries/mod.rs
   ```

3. Keep `pub mod queries;` unchanged in `src/graph/mod.rs`.

4. Run:

   ```sh
   cargo check
   cargo test graph::queries
   ```

At this checkpoint, the code should still behave exactly as before, and
`crate::graph::queries::*` should still resolve.

## Phase 3: prepare the `queries` facade

Edit `src/graph/queries/mod.rs` so it becomes the stable facade for the split.

1. Add private submodule declarations at the top:

   ```rust
   mod call_graph;
   mod crate_graph;
   mod cursors;
   mod items;
   mod labels;
   mod lookup;
   mod reexports;
   mod scope;
   mod usages;
   mod workspace;
   ```

2. Keep public type re-exports in `queries/mod.rs`, for example:

   ```rust
   pub use call_graph::{CallGraphNode, EnrichedCallSite, RecursiveCallersCount};
   pub use crate_graph::{
       CrateDeadPub, CrateEdge, CrateMetric, DeadPubFinding, EdgeSymbol,
       ForbiddenDependencyRule, ForbiddenDependencyViolation,
   };
   pub use items::{
       FunctionFilter, FunctionWithSignature, ItemWithAttribute, MutStaticFinding,
       SelfKindFilter, classify_metadata,
   };
   pub use reexports::{
       PubTypeAliasMasqueradingAsReexport, ReExportChain, ReExportLink,
   };
   pub use usages::UsageSummaryRow;
   pub use workspace::{
       CommonFnName, ModuleShadow, ModuleTreeNode, NodeKindCounts, OverlapsReport,
       TypeCollision, TypeLocation, VisibilityCounts, WithinCrateDuplicate,
       WorkspaceStats,
   };
   ```

3. Keep shared constants in the narrowest sensible home:

   - Put `MAX_REEXPORT_HOPS` in `queries/mod.rs` if both lookup and re-export
     logic need it.
   - Put `MUT_STATIC_PATTERNS` in `items.rs`.

4. Any helper used by multiple query submodules should be:

   ```rust
   pub(in crate::graph::queries)
   ```

   Avoid making helpers `pub(crate)` unless a non-query module needs them.

## Phase 4: split helpers first

Move helpers before moving the public query methods.

1. Create `labels.rs`.

   Move:

   - `label_node_kind`
   - `label_item_kind`
   - `label_binding_kind`
   - `usage_category_label`
   - `format_binding_visibility`
   - `match_attribute`

   Use:

   ```rust
   pub(in crate::graph::queries) fn label_item_kind(...)
   ```

2. Move `filter_matches` into `items.rs`, not `labels.rs`, because it only
   supports `functions_with_filter`.

3. Create `cursors.rs`.

   Move the LMDB duplicate-sort helpers:

   - `bindings_for_from_module`
   - `bindings_for_target`
   - `usages_for_target`
   - `usages_for_consumer`
   - `usages_for_consumer_function`

   Because these are used by sibling modules after the split, declare them:

   ```rust
   pub(in crate::graph::queries) fn bindings_for_target(...)
   ```

4. Run:

   ```sh
   cargo check
   cargo test graph::queries
   ```

## Phase 5: split query groups in dependency order

Move one group at a time. After each group, run `cargo check` or the relevant
targeted tests.

### 5.1 `lookup.rs`

Move:

- `OpenedSnapshot::lookup_by_qualified_name`
- `lookup_by_qualified_name_inner`
- `node_by_id`
- `find_root_module_of`
- `module_ancestors`
- `is_visible_from`

Notes:

- `lookup_by_qualified_name_inner` can stay private if only `lookup.rs` uses it.
- `module_ancestors` and `is_visible_from` are likely needed by `scope.rs`, so
  use `pub(in crate::graph::queries)` if needed.

### 5.2 `scope.rs`

Move:

- `imports_of`
- `exports_of`
- `reexports_of`
- `declared_reexports_of`
- `who_imports`

Dependencies:

- `cursors`
- `lookup::is_visible_from`

### 5.3 `usages.rs`

Move:

- `UsageSummaryRow`
- `usages_of`
- `usages_in`
- `who_uses_summary`

Dependencies:

- `cursors`
- `labels::usage_category_label`

### 5.4 `call_graph.rs`

Move:

- `EnrichedCallSite`
- `CallGraphNode`
- `RecursiveCallersCount`
- `who_calls`
- `calls_from`
- `call_graph`
- `call_graph_rec`
- `callers_in_crate`
- `recursive_callers_count`

Dependencies:

- `cursors`
- `labels::usage_category_label`

### 5.5 `items.rs`

Move:

- `ItemWithAttribute`
- `FunctionFilter`
- `SelfKindFilter`
- `FunctionWithSignature`
- `MutStaticFinding`
- `MUT_STATIC_PATTERNS`
- `classify_metadata`
- `filter_matches`
- `enum_variants`
- `item_attributes`
- `items_with_attribute`
- `function_signature`
- `static_metadata`
- `mut_static_audit`
- `functions_with_filter`

Keep this public facade working:

```rust
crate::graph::queries::ItemWithAttribute
crate::graph::queries::FunctionFilter
crate::graph::queries::classify_metadata
```

### 5.6 `reexports.rs`

Move:

- `PubTypeAliasMasqueradingAsReexport`
- `ReExportLink`
- `ReExportChain`
- `pub_use_pub_type_audit`
- `re_export_chain`

Dependencies:

- `MAX_REEXPORT_HOPS`
- `cursors`

### 5.7 `crate_graph.rs`

Move:

- `DeadPubFinding`
- `CrateDeadPub`
- `CrateEdge`
- `EdgeSymbol`
- `ForbiddenDependencyRule`
- `ForbiddenDependencyViolation`
- `CrateMetric`
- `glob_match`
- `crate_dependency_metric`
- `dead_pub_in_crate`
- `dead_pub_report`
- `crate_edges`
- `forbidden_dependency_check`

Keep `glob_match` private unless a test needs it. Move its smoke tests into
`crate_graph.rs`.

### 5.8 `workspace.rs`

Move:

- `OverlapsReport`
- `TypeCollision`
- `TypeLocation`
- `ModuleShadow`
- `WithinCrateDuplicate`
- `CommonFnName`
- `ModuleTreeNode`
- `WorkspaceStats`
- `NodeKindCounts`
- `VisibilityCounts`
- `overlaps`
- `module_tree`
- `build_module_tree`
- `workspace_stats`

Dependencies:

- `labels`

## Phase 6: update hard-coded canonical paths in tests

Some tests currently assume declarations live directly in
`file_search_mcp::graph::queries`. After the split, canonical declaration paths
will include the submodule name.

Update these cases intentionally:

1. `explicit_pub_use_is_marked_on_pub_use_bindings`

   Current intent: prove a non-pub `use` gets `is_explicit_pub_use == false`.

   Change the private-import probe from:

   ```text
   file_search_mcp::graph::queries
   ```

   to a stable submodule that has private imports, such as:

   ```text
   file_search_mcp::graph::queries::lookup
   ```

   or whichever split file contains ordinary `use` statements after the move.

2. `re_export_chain_finds_known_facade`

   Current target:

   ```text
   file_search_mcp::graph::queries::ForbiddenDependencyRule
   ```

   New canonical target should be the declaration path, likely:

   ```text
   file_search_mcp::graph::queries::crate_graph::ForbiddenDependencyRule
   ```

   Keep the assertion that `src/graph/mod.rs` re-exports it through
   `crate::graph::ForbiddenDependencyRule`.

3. Comments in `src/graph/extract.rs`

   Update comments that say methods live in `queries.rs`. The method item path
   remains:

   ```text
   file_search_mcp::graph::snapshot::OpenedSnapshot::usages_of
   ```

   but the source file will now be one of the `queries/*.rs` files.

## Phase 7: test distribution

Do not leave every query test in `queries/mod.rs`.

1. Keep facade-level tests in `queries/mod.rs` only when they test cross-module
   behavior.

2. Move focused tests next to their query group:

   - lookup tests -> `lookup.rs`
   - scope/import/export tests -> `scope.rs`
   - usage summary tests -> `usages.rs`
   - call graph tests -> `call_graph.rs`
   - item/signature/static metadata tests -> `items.rs`
   - re-export chain tests -> `reexports.rs`
   - dead pub/crate edges/forbidden dependency tests -> `crate_graph.rs`
   - overlaps/module tree/workspace stats tests -> `workspace.rs`

3. Every test module should import:

   ```rust
   use crate::graph::test_support::shared_snapshot;
   ```

## Phase 8: public API compatibility check

Before considering the split done, verify these paths still work:

```rust
crate::graph::queries::ItemWithAttribute
crate::graph::queries::FunctionFilter
crate::graph::queries::SelfKindFilter
crate::graph::queries::classify_metadata
crate::graph::CallGraphNode
crate::graph::ForbiddenDependencyRule
crate::graph::OpenedSnapshot
```

Concrete check:

```sh
rg -n "crate::graph::queries|graph::queries" src
cargo check
```

Any remaining direct `crate::graph::queries::...` imports should either keep
working through `queries/mod.rs` re-exports or be intentionally moved to the
flat `crate::graph::...` facade.

## Phase 9: final verification

Run:

```sh
cargo check
cargo test graph::queries
cargo test graph::attributes
cargo test graph::signatures
cargo test graph::statics
cargo test graph::unsafe_audit
```

If time allows, run the full library tests:

```sh
cargo test --lib
```

Do not run `cargo fmt`.

Finish with:

```sh
jj status
```

Confirm only intended files changed.

## Optional later refactor: directory grouping

Only do this after the query split is merged or otherwise stable.

If grouping `extract/`, `store/`, and `audit/` is still desired, preserve the
old public module paths with wrappers:

```rust
mod store {
    pub mod snapshot;
    pub mod storage;
}

pub mod snapshot {
    pub use super::store::snapshot::*;
}

pub mod storage {
    pub use super::store::storage::*;
}
```

Use the same pattern for `docs_audit`, `derive_audit`, `channel_audit`,
`fn_body_audit`, `recursion_check`, and `unsafe_audit` if external or internal
callers still use `crate::graph::<old_module>`.

Do not combine this optional grouping with the first `queries` split. It
changes many imports and canonical declaration paths, so it deserves its own
review and test pass.

## Done criteria

- `src/graph/queries.rs` no longer exists.
- `src/graph/queries/mod.rs` is a small facade.
- No query submodule is larger than roughly 500 lines unless there is a clear
  reason.
- `crate::graph::queries::*` compatibility is preserved.
- Shared snapshot setup lives in `src/graph/test_support.rs`.
- Tests that relied on old canonical `graph::queries` declaration paths have
  been updated deliberately.
- `cargo check` passes.
- Targeted graph tests pass.
- `jj status` shows only intended changes.

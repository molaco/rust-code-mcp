# `graph` module split proposal

## What you actually have

The directory is **22 flat files / ~3,200 LoC + a 3,571-line `queries.rs`**. The
module header already names the architectural layers: *loader → model →
extraction passes → persistence → read path → audits*. Today everything sits
at one level — so naturally, `queries.rs` and the audits both bulge.

**Only one file is actually too big**: `queries.rs` at 144 KB / 3,571 lines
(~28% of the entire module). Everything else is under ~600 lines and reads
fine. So the real recommendation is **split `queries.rs`, then optionally
group the existing files into subdirectories** to make the layering visible at
the filesystem level.

### File sizes today

| File                  | Lines | Status                                        |
|-----------------------|------:|-----------------------------------------------|
| `queries.rs`          | 3571  | **Split — the only urgent one**               |
| `fn_body_audit.rs`    |  793  | Borderline; could split per-pattern matcher   |
| `snapshot.rs`         |  581  | Fine                                          |
| `storage.rs`          |  516  | Fine                                          |
| `bindings.rs`         |  486  | Fine                                          |
| `usages.rs`           |  475  | Fine                                          |
| `extract.rs`          |  461  | Fine                                          |
| `channel_audit.rs`    |  455  | Fine                                          |
| `derive_audit.rs`     |  404  | Fine                                          |
| `impls.rs`            |  363  | Fine                                          |
| `attributes.rs`       |  351  | Fine                                          |
| `signatures.rs`       |  342  | Fine                                          |
| `recursion_check.rs`  |  337  | Fine                                          |
| `unsafe_audit.rs`     |  341  | Fine                                          |
| `model.rs`            |  280  | Fine                                          |
| `docs_audit.rs`       |  269  | Fine                                          |
| `hir_trim.rs`         |  243  | Fine                                          |
| `statics.rs`          |  188  | Fine                                          |
| `ids.rs`              |  167  | Fine                                          |
| `loader.rs`           |  109  | Fine                                          |
| `mod.rs`              |   52  | Fine                                          |
| `ast_resolve.rs`      |   29  | Fine                                          |

## Step 1 — Split `queries.rs` (the only urgent one)

`queries.rs` mixes:

- ~30 result types (lines 25–422)
- ~40 methods on `impl OpenedSnapshot` (lines 424–2350)
- ~10 free helpers (lines 2352–2549)
- ~1,000 lines of tests with a shared snapshot (lines 2552–3571)

`OpenedSnapshot`'s methods cluster cleanly by topic. Split into a `query/`
directory, with one impl block per file (Rust lets you spread
`impl OpenedSnapshot { ... }` across files freely). **Co-locate each result
type with the query that produces it** rather than dumping all types in one
`types.rs` — it makes each file self-contained.

```
graph/query/
├── mod.rs           — declares submodules; hosts shared_snapshot() for tests
├── lookup.rs        — lookup_by_qualified_name, node_by_id, find_root_module_of,
│                      module_ancestors, is_visible_from
├── scope.rs         — imports_of, exports_of, reexports_of, declared_reexports_of,
│                      who_imports
├── usages.rs        — usages_of, usages_in, who_uses_summary + UsageSummaryRow
├── call_graph.rs    — who_calls, calls_from, call_graph (+ rec), callers_in_crate,
│                      recursive_callers_count + EnrichedCallSite, CallGraphNode,
│                      RecursiveCallersCount
├── items.rs         — enum_variants, item_attributes, items_with_attribute,
│                      function_signature, static_metadata, functions_with_filter,
│                      mut_static_audit + ItemWithAttribute, FunctionFilter,
│                      FunctionWithSignature, SelfKindFilter, MutStaticFinding,
│                      classify_metadata, MUT_STATIC_PATTERNS
├── reexports.rs     — pub_use_pub_type_audit, re_export_chain + ReExportLink,
│                      ReExportChain, PubTypeAliasMasqueradingAsReexport,
│                      MAX_REEXPORT_HOPS
├── crate_graph.rs   — crate_edges, dead_pub_in_crate, dead_pub_report,
│                      crate_dependency_metric, forbidden_dependency_check
│                      + DeadPubFinding, CrateDeadPub, CrateEdge, EdgeSymbol,
│                        CrateMetric, ForbiddenDependencyRule/Violation, glob_match
├── workspace.rs     — overlaps, module_tree (+ build_module_tree), workspace_stats
│                      + OverlapsReport (+ all sub-types), ModuleTreeNode,
│                        WorkspaceStats, NodeKindCounts, VisibilityCounts
├── cursors.rs       — bindings_for_from_module, bindings_for_target,
│                      usages_for_target, usages_for_consumer,
│                      usages_for_consumer_function  (the LMDB DUP_SORT helpers)
└── labels.rs        — label_node_kind, label_item_kind, label_binding_kind,
                       usage_category_label, format_binding_visibility,
                       match_attribute, filter_matches
```

Sizes: each ends up at ~150–400 lines. None huge.

**Test infrastructure**: `queries.rs::tests::shared_snapshot` is referenced
from `attributes.rs`, `signatures.rs`, `statics.rs`, and `unsafe_audit.rs`.
Keep `shared_snapshot()` at one stable path — either
`graph::query::tests::shared_snapshot` (and update those 4 callers) or expose
it as `graph::test_support::shared_snapshot` (cleaner). Each `query/foo.rs`
can keep its own `#[cfg(test)] mod tests` that pulls from there.

## Step 2 — Group the existing files (optional but tidy)

The audits and the extraction passes are already cleanly separated by file.
Folding them into named subdirectories makes the layering explicit and
shrinks `mod.rs`:

```
graph/
├── mod.rs              — re-exports (unchanged public API)
├── ids.rs              — kept (167 lines, foundational)
├── model.rs            — kept (280 lines, foundational)
├── hir_trim.rs         — kept (pure utility)
├── ast_resolve.rs      — kept (shared helper used by audits)
├── loader.rs           — kept
│
├── extract/
│   ├── mod.rs          — pub use run::extract; declares submodules
│   ├── run.rs          — current extract.rs body (orchestrator + emit_crate)
│   ├── bindings.rs
│   ├── impls.rs
│   ├── attributes.rs
│   ├── signatures.rs
│   ├── statics.rs
│   └── usages.rs
│
├── store/
│   ├── mod.rs
│   ├── storage.rs      — heed schema, GraphDatabases, GraphPaths, fingerprint
│   └── snapshot.rs     — build_and_persist, OpenedSnapshot, open_current
│
├── query/              — (the split from Step 1)
│   └── ...
│
└── audit/
    ├── mod.rs
    ├── unsafe_audit.rs
    ├── channel_audit.rs
    ├── fn_body_audit.rs       — borderline at 793 lines; see Step 4 below
    ├── derive_audit.rs
    ├── docs_audit.rs
    └── recursion_check.rs
```

## Step 3 — Public-API compatibility

`mod.rs` re-exports a wide flat surface (`pub use queries::{...}`,
`pub use extract::extract`, etc.). Many callers across `src/tools/` reach
into these — keep the existing names working by leaving the `pub use` blocks
in `graph/mod.rs` intact and pointing them at the new locations. No external
caller has to change.

`crate::graph::queries::tests::shared_snapshot` is the one internal path
you'll have to update in the four sibling files (or re-export from the new
location).

## Step 4 — Optional: also split `fn_body_audit.rs` (793 lines)

Eight pattern matchers live in one file. If you touch it anyway, each matcher
(`match_unwrap`, `match_panic_macros`, `match_await_in_guard_scope`, etc.) is
a self-contained `fn body -> Vec<RawFinding>` and could move to
`audit/fn_body/patterns/{unwrap,panic,guard,...}.rs` with
`audit/fn_body/mod.rs` as the orchestrator. Not urgent — but easy if you're
already mid-refactor.

## TL;DR

1. **Must do**: split `queries.rs` (3,571 lines) into a `query/` directory
   along topic seams listed above.
2. **Nice to have**: group existing files into `extract/`, `store/`, `audit/`
   to make the layered comment in `mod.rs` actually reflect the filesystem.
3. **Keep**: `mod.rs`'s flat re-exports — no caller change needed.
4. **One gotcha**: `queries::tests::shared_snapshot` is depended on by 4
   other test modules; move it to a stable path before deleting the old
   `queries.rs`.

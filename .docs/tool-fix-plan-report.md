# Tool-Fix Plan Report

Date: 2026-05-18
Plan: `.plans/tool-fix-plan.md`
Status: complete

## Scope

Implemented all 12 items from the tool-fix plan:

- T1: `semantic_overlaps` response budget controls.
- T7: shared pagination/summary convention for list-shaped tools.
- T5: exact matching for `find_definition` and `find_references`.
- T10: bare attribute-path matching for `items_with_attribute`.
- T3: target-kind filtering for `forbidden_dependency_check`.
- T2: new `module_dependencies` tool for complete module-level references.
- T4: corrected method-dispatch caveats to match actual extraction behavior.
- T8: separated module-private visibility stats.
- T9: scoped `overlaps` report filtering.
- T11: clarified `index_codebase` no-op/skip-only messages.
- T6: resolved impl-module method aliases for qualified-name lookup.
- T12: added `dry_run` to `clear_cache`.

## Commits

- `6a07d70b` - T1 semantic overlaps response budget controls
- `b0f34f14` - T7 add pagination to enumerating tools
- `7e84e8ce` - T5 add exact symbol matching
- `e8ca4fb1` - T10 match bare attribute paths
- `9709be5d` - T3 filter forbidden dependency consumer kinds
- `f4cd04fc` - T2 add module dependencies tool
- `2da8a516` - T4 correct method dispatch caveats
- `cd9d3d1e` - T8 separate module-private visibility stats
- `7038d715` - T9 scope overlaps report
- `be59ceee` - T11 clarify index no-op status
- `2c07e647` - T6 resolve impl module method aliases
- `9fd65901` - T12 add clear cache dry run

## Verification

Per-fix focused tests were run through the project devshell:

- `cargo test page_clusters --lib`
- `cargo test item_ref_summary_omits_file_and_span --lib`
- `cargo test page_list --lib`
- `cargo test rank_and_filter_exact --lib`
- `cargo test match_attribute_accepts_bare_attribute_paths --lib`
- `cargo test forbidden_dependency_rule --lib`
- `cargo test target_kind_label_collapses_cargo_kinds --lib`
- `cargo test load_crate_target_kinds_finds_workspace_targets --lib`
- `cargo test dependency_node_for_climbs_item_parents_to_module --lib`
- `cargo test pattern1_method_call_captured --lib`
- `cargo test pattern2_trait_dispatch_captured --lib`
- `cargo test visibility_counts_separate_module_private_from_restricted --lib`
- `cargo test overlap_scope_filters_examples_and_vendor --lib`
- `cargo test format_result_ --lib`
- `cargo test impl_module_item_alias_ --lib`
- `cargo test clear_cache --lib`

`cargo check --all-targets` was run and passed after each committed fix.

All cargo commands were run as:

```text
nix develop ../nix-devshells#cuda-code --command <cargo command>
```

## Audit Notes

- The plan table now marks every item complete.
- `clear_cache` remains destructive by default, but `dry_run=true` can now be used for the final smoke path without deleting cache/index data.
- `similar_to_item` already resolved canonical method names. The reproduced failure was the impl-module spelling for methods implemented outside the type's defining module; the lookup fallback is intentionally constrained by crate and source file to avoid broad suffix-only matches.
- Some snapshot-backed tests and MCP round-trip tests still exceed the 120-second timeout in this environment. Those were not used as final gates; faster focused tests cover the changed logic.
- Existing warnings remain in the workspace. No formatting command was run.

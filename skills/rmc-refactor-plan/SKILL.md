---
name: rmc-refactor-plan
description: Plan a Rust refactor with evidence.
argument-hint: "<question> [target-symbol]"
allowed-tools: Read, Bash, mcp__rust-code-mcp__*
---

# Rust refactor plan

Practical recipes for specific refactor questions. Each is a short
composition of existing tools. Pick the recipe that matches the question,
run the calls, apply the verdict.

For pure single-symbol forensics, use `rmc-symbol-forensics`. For complete
crate-level audits, use `rmc-crate-audit`. For semantic-similarity-driven
dedupe in depth, use `rmc-semantic-overlaps`. For verifying a refactor by
comparing before/after, use `rmc-snapshot-diff`.

## Scope — task-specific (one symbol, one decision at a time)

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
index_codebase(directory=<absolute-path>)   # required for Recipe 10 (semantic_overlaps)
```

## Recipes

### Recipe 1 — "Should I downgrade X from `pub` to `pub(crate)`?"

```
who_imports(directory=..., target=X)
who_uses(directory=..., target=X)
```

If both are empty cross-crate (all consumers in the same crate as X),
demote. `dead_pub_report` likely already flagged it.

### Recipe 2 — "Is it safe to delete X?"

```
who_uses(directory=..., target=X)
who_imports(directory=..., target=X)
find_references(symbol_name=X)
```

Empty everywhere = delete. `find_references` catches things `who_uses`
doesn't (local var shadows, lifetimes, doc comments).

### Recipe 3 — "Should I move X to a different crate?"

```
who_uses_summary(directory=..., target=X)
```

Look at consumer module distribution. Move X to where most callers live,
OR factor X's deps upstream so no callers need to import sideways.

### Recipe 4 — "Is this `pub use` facade earning its keep?"

```
get_declared_reexports(directory=..., module=<crate_root>)
```

For each item, run `who_imports(target=<canonical_path>)`. If most
importers reach for the canonical path and few or none use the facade,
drop the `pub use`.

### Recipe 5 — "Should I make this trait sealed?"

```
who_uses(directory=..., target=T)
who_imports(directory=..., target=T)
```

Filter to importers outside the defining crate. If implementers are all
internal and external use is purely consumption, seal it.

### Recipe 6 — "Do crate-private types leak through pub APIs?"

```
get_exports(directory=..., module=<crate_root>, consumer=<other>)
module_tree(directory=..., krate=<crate>)   # filter to pub items
```

Diff: items reachable from external consumers vs items declared `pub` at
canonical sites. Mismatches are the leaks.

### Recipe 7 — "What's the minimum viable refactor target?"

```
crate_edges(directory=...)
```

Filter to a target `(consumer, producer)` pair. If `unique_symbols` is
small (1-3), that small set is your refactor target — extract or relocate
just those.

### Recipe 8 — "Test-only helpers I can move to dev-deps?"

```
who_uses_summary(directory=..., target=<helper>)
```

Filter to rows where `category_breakdown` is all Test. Those helpers are
dev-deps candidates.

### Recipe 9 — "Verify a refactor didn't widen the API"

Pre-refactor:

```
get_declared_reexports(directory=..., module=<crate_root>)   → JSON A
dead_pub_report(directory=...)                               → JSON B
```

Post-refactor: same calls. Diff JSON.

- New entries in declared_reexports that weren't there before = API widened.
- Lost entries in `dead_pub_report` = items now used (good).
- New entries in `dead_pub_report` = items now dead (consider removing).

(Full snapshot-diff recipes in `rmc-snapshot-diff`.)

### Recipe 10 — "Find duplicate logic worth extracting"

```
semantic_overlaps(directory=..., crate_name=X, item_kind="Function")
```

For each returned cluster:

1. Inspect `members` — qualified names, files, spans, and
   `avg_similarity` / `min_similarity`.
2. Run `who_uses_summary(target=<member>)` per member to verify they're
   called and to plan migration order.
3. Top clusters by `avg_similarity` are the best extraction targets.

Workspace-scale: drop `crate_name`. Cross-crate-only:
`cross_crate_only=true`. Detail in `rmc-semantic-overlaps`.

### Recipe 11 — "Which complex files have the highest blast radius?"

```
analyze_complexity(file_path=<path>)             → file-level cyclomatic aggregates
get_call_graph(file_path=<path>)                 → high-out-degree fns inside the file
who_uses(directory=..., target=<crate>::<fn>)    → per candidate fn, fan-in
```

Sort files by `Total cyclomatic` desc, then within hot files use
`get_call_graph` to find the dispatch hubs, then weight by fan-in. (See
`rmc-complexity` for the file-level vs per-fn caveat.)

### Recipe 12 — "Find dead facade re-exports" (high leverage)

```
get_declared_reexports(directory=..., module=<crate_root>)
dead_pub_in_crate(directory=..., krate=<crate>)
```

Items appearing in BOTH = dead facade branches. Drop the `pub use` line,
demote source to `pub(crate)`.

Spotted on `tui` in `coding-agent-bad`: `RunState`, `InvalidTransition`,
`RunnerWakeError` were all re-exported AND dead.

### Recipe 13 — "Detect half-finished migrations" (high leverage)

```
overlaps(directory=...)                     → cross_crate_type_collisions
```

For each collision, run `who_uses_summary` on both qualified names. Look
for `consumer_qualified_name` overlap between the two row sets — a
consumer module that imports BOTH versions is converting between them,
usually the trace of a migration that was started by duplicating instead
of moving.

Spotted on `coding-agent-bad`: `AgentConfig` in `agent::config` and
`config` crates, both used by `coding-agent::compose`.

## Decision frames (cross-recipe)

| Refactor goal | Trust signal |
|---|---|
| Demote `pub → pub(crate)` | `dead_pub_report` flag + cross-crate `who_imports` empty |
| Delete | `who_uses` ∪ `who_imports` ∪ `find_references` all empty |
| Move to different crate | `who_uses_summary` clusters in a different crate |
| Seal trait | Outside-crate `who_imports(T)` = 0 |
| Drop facade re-export | Both ends in `dead_pub_in_crate` |
| Extract shared fn | `semantic_overlaps` cluster with high `avg_similarity` + non-empty `who_uses` per member |
| Refactor in place | High complexity × high fan-in |

## Output format

For each refactor question, return:

```
Question: <recipe name>
Target: <symbol or facade>
Evidence: <tool outputs summarized>
Verdict: <go | block | conditional>
Recommended action: <concrete steps>
Risk: <Low | Medium | High>
```

If verdict is `conditional`, list the condition that needs verification
(e.g., "no impl hardcodes the deleted method").

## Limitations

- `find_references` is RA-driven and may include matches in comments and
  doc strings — review each delete candidate by hand.
- `who_uses` excludes macro-expanded refs in some cases; verify with
  `find_references` before deleting.
- `dead_pub_in_crate` is computed from cross-crate consumers only —
  within-crate usage in tests still flags items as dead.
- Method-level fan-in requires Layer 4 (current); pre-Layer-4 these
  queries errored.

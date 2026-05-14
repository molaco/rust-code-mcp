---
name: rmc-snapshot-diff
description: Compare two Rust workspace snapshots.
argument-hint: "<branch-1-path> <branch-2-path>"
allowed-tools: Read, Bash, mcp__rust-code-mcp__*
---

# Rust snapshot / branch comparison

Verify a refactor didn't break invariants, or compare two snapshots over
time. Scope: two snapshots (typically two branches or two timestamps).

For the live "is this safe?" decision on a single refactor, use
`rmc-refactor-plan`. For complexity-only deltas, use `rmc-complexity`.

## Scope — two snapshots

## Prerequisites

```
build_hypergraph(directory=<path_to_branch_1>)
build_hypergraph(directory=<path_to_branch_2>)
```

Independent — issue in parallel.

## Workflow

### Step 1. Build both snapshots

See prereq. Cold builds in parallel; warm reuse is sub-second.

### Step 2. Pull paired data (parallel)

For each metric of interest, call the same tool against both directories:

```
workspace_stats(directory=<path_1>)              → JSON A1
workspace_stats(directory=<path_2>)              → JSON A2

dead_pub_report(directory=<path_1>)              → JSON B1
dead_pub_report(directory=<path_2>)              → JSON B2

crate_edges(directory=<path_1>)                  → JSON C1
crate_edges(directory=<path_2>)                  → JSON C2

get_declared_reexports(directory=<path_1>, module=<root>)  → JSON D1
get_declared_reexports(directory=<path_2>, module=<root>)  → JSON D2
```

### Step 3. Diff

| Diff target | What to compare |
|---|---|
| `workspace_stats` | Item counts, visibility distribution, `pub_crate_share` trend |
| `dead_pub_report` | Per-crate dead-pub count delta |
| `crate_edges` | Per `(consumer, producer)` edge: `unique_symbols` delta, `total_refs` delta |
| `get_declared_reexports` | New entries = API widened; lost entries = API narrowed |
| `module_tree` per crate | Item count delta, depth delta |
| `analyze_complexity` per file | Per-fn cyclomatic delta |

## Recipes

### Recipe — "Verify a refactor didn't widen the API"

```
get_declared_reexports(module=<root>)            → before
... refactor ...
get_declared_reexports(module=<root>)            → after
```

New entries = widened API. Investigate each before merging.

### Recipe — "Dead-pub trend"

`dead_pub_report` per branch; compare counts. Trend up = facades or pub
surface decaying. Trend down = active demotion / cleanup.

### Recipe — "Edge weight changes"

`crate_edges` per branch. Per `(consumer, producer)` compare
`unique_symbols` and `total_refs`. New high-weight edges = new coupling.
Lost edges = cleaned-up coupling.

### Recipe — "Method count by type"

`workspace_stats.items_by_kind.Method` trend. Up = adding methods
(Layer 4 captures the count). Down = removing or consolidating.

### Recipe — "Complexity trend"

`analyze_complexity` per branch on the same files. Per-fn cyclomatic
delta. Negative deltas confirm refactors landed. Positive deltas may be
regressions.

## Decision frames

| Finding | Means |
|---|---|
| `pub_crate_share` increased | Encapsulation discipline improved |
| `dead_pub_report` count decreased | Active demotion / cleanup happening |
| New entries in `get_declared_reexports` post-refactor | API widened — investigate intent |
| `crate_edges` row added with high `total_refs` | New coupling introduced |
| `crate_edges` cycle appeared | Architectural break — block merge |
| Per-fn cyclomatic delta positive | Possible regression |
| `items_by_kind.Method` decreased | Methods consolidated or removed |

## Pattern reference

| Signal | Means |
|---|---|
| Many `dead_pub_report` entries appear together | Refactor removed callers but didn't demote source |
| `workspace_stats.items_by_kind.Method` jumped 100+ in one PR | Big impl-block addition; investigate |
| `crate_edges` added new edge between previously unrelated crates | Architectural change; verify intent |
| Same fn appears in both `before` and `after` with delta=0 cyclomatic | Refactor was structural, not logical |

## Output format

```
Snapshots: <branch_1> vs <branch_2>
ΔItems: <n>; ΔMethods: <m>; ΔStructs: <s>
ΔVisibility: pub <±n> / pub_crate <±n> / Δshare <±r>
ΔDead pubs: <±n> across <m> crates
ΔAPI surface: <±n> declared re-exports
ΔTop edges: <list with deltas>
Cycle status: <pass | new cycle: A → B → A>
Verdict: <safe | review | block>
```

## Limitations

- No `diff_hypergraph` tool — diffing is manual JSON post-processing.
- Comparing two branches of the same workspace requires distinct working
  directories; you can't compare two refs of the same checkout.
- `analyze_complexity` is file-level only; per-fn cyclomatic deltas
  require parser-level walks outside this tool.
- `crate_edges` excludes method-call / trait-dispatch from edge totals —
  coupling deltas undercount for trait-heavy designs.

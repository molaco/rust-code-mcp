---
name: rmc-complexity
description: Rust complexity hotspots by blast radius.
argument-hint: "[crate-name-or-file-path]"
allowed-tools: Read, Bash, mcp__rust-code-mcp__*
---

# Rust complexity-driven prioritization

Find the gnarly code, then rank by blast radius. Scope: workspace or
single crate.

For pure call-graph analysis without complexity scoring, use
`rmc-call-graph`. For composing a refactor decision around a specific
function, use `rmc-refactor-plan`.

## Scope — workspace or single crate

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Find files with high aggregate complexity

```
analyze_complexity(file_path=<path>)
```

Returns **file-level aggregates** (not per-function): total LOC, function/
struct/trait counts, **total cyclomatic**, **avg cyclomatic per function**,
total function-call count. There is no cognitive metric and no per-fn
score in the output.

File-level prioritization heuristics:

- High `Total cyclomatic` (≥ 50 in a single file) → file probably contains
  at least one gnarly fn.
- High `Avg per function` (≥ 5) → branching is spread across the file;
  whole-file refactor candidate.
- High `Function calls` relative to function count → tight intra-file
  coupling; cross-reference with `get_call_graph` to see hubs.

Per-fn thresholds (cyclomatic ≥ 10 / 15 / 25 from rust-guidelines §4) are
NOT directly checkable with this tool — you'd need a parser-level walk.
Use file-level aggregates to triage which files to read.

### Step 2. Identify the actual gnarly fns within a hot file

`analyze_complexity` doesn't tell you which fn is gnarly. To find it:

```
get_call_graph(file_path=<file>)
```

Functions with high out-degree (calling many helpers) are dispatch hubs —
usually where complexity concentrates. Cross-check by reading the source
(`read_file_content`) at those fns.

### Step 3. Cross-reference with usage

For each candidate fn:

```
who_uses_summary(directory=..., target=<crate>::<fn>)
```

Compute `out_degree × total_count` as a rough blast-radius-weighted
refactor priority. For an exact integer blast radius, use
`rmc-call-graph` Step 5 (`recursive_callers_count`).

### Step 4. Pre-/post-snapshot to verify simplifications

Before refactor: `analyze_complexity(file_path=...)` → record total + avg
cyclomatic.
After refactor: same call → compare.

Drops in total cyclomatic or avg-per-fn confirm the refactor reduced
complexity. For broader before/after diffs, use `rmc-snapshot-diff`.

## Decision frames

| Finding | Action |
|---|---|
| File `Total cyclomatic` ≥ 50 + high fan-in on a fn inside it | Top refactor priority |
| File `Avg per function` ≥ 5 + many functions | Whole-file refactor / split file |
| File `Total cyclomatic` ≥ 50 + fan-in concentrated in tests | Probably test-heavy; lower priority |
| `Function calls` >> function count | Tight intra-file coupling — extract helpers |
| Clean call graph + high cyclomatic | Complexity is in `match` arms / nested `if` — read source |

## Pattern reference

| Signal | Means |
|---|---|
| `Avg per function` close to 1 but total very high | Many simple fns — refactor target is file structure, not individual fns |
| One fn with high out-degree in `get_call_graph` + file has high cyclomatic | That fn is likely the gnarl |
| Clean call graph + still high cyclomatic | Complexity is in branching, not dispatch |
| Total LOC high + Total cyclomatic moderate | File is large but logically simple — split for clarity, not complexity |

## Output format

```
File: <path>
Total cyclomatic: <n>
Avg per function: <r>
Top candidate fns (by out-degree from get_call_graph):
  1. <crate>::<fn>  (out_degree=<n>, fan_in=<m>, score=<n*m>)
  2. ...
Recommended action: <split-file | extract-helpers | refactor-fn | leave-alone>
```

Workspace-wide ranking is a table per file sorted by
`Total cyclomatic × avg_fan_in_of_top_fn`.

## Limitations

- No per-function cyclomatic — you must combine `analyze_complexity`
  (file-level) with `get_call_graph` (structural) and visual inspection.
- No cognitive complexity metric — only cyclomatic.
- No type-complexity metric — generics with many params, deeply-nested
  type aliases, etc., are invisible.
- `get_call_graph` is parser-only and file-scoped; cross-file dispatch is
  invisible to it. Use `rmc-call-graph` for HIR-resolved workspace
  reachability.

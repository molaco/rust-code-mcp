---
name: rmc-unsafe-audit
description: Audit Rust unsafe blocks.
argument-hint: "[workspace-path]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust unsafe-block audit

Every `unsafe { ... }` block in the workspace's local crates surfaces
with its enclosing fn, line count, and a `has_safety_comment` heuristic
flag. Live computation; nothing cached. Scope: workspace-wide.

For global-mutable-state audits (`static mut`, `LazyLock`, `OnceLock`),
use `rmc-mut-static-audit`. For blast-radius integers on the enclosing
fns, hand off to `rmc-call-graph`.

## Scope — workspace-wide

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Pull every unsafe block

```
unsafe_audit(directory=...)
```

Returns:

```json
{
  "directory": "...",
  "finding_count": <n>,
  "findings": [
    { "file": "src/foo.rs", "span": [1024, 1100], "line_count": 4,
      "enclosing_function": "<64-char-hex>",
      "enclosing_function_name": "my_crate::do_unsafe_thing",
      "has_safety_comment": true }
  ]
}
```

Sorted by `(file, span)`. Per-invocation cost is dominated by workspace
load (~2-3s).

### Step 2. SAFETY-comment compliance

Filter `has_safety_comment=false`:

```
findings | where has_safety_comment=false
```

The flag is true when `SAFETY` appears as a substring in any of the 5
source lines preceding the `unsafe` keyword. False = undocumented unsafe
— the audit's primary output. Empty-after-filter is the healthy signal.

### Step 3. Block-size distribution

Sort by `line_count` descending → top candidates for breakdown into
smaller annotated blocks. Idiomatic Rust prefers small unsafe blocks with
one-fact-per-block SAFETY comments; a 30-line unsafe block usually mixes
too many invariants under one umbrella note.

### Step 4. Blast-radius weighting

For each block:

```
recursive_callers_count(directory=..., target=<enclosing_function_name>, depth=8)
```

(Detail in `rmc-call-graph`.) The integer answers "how many fns
transitively touch unsafe code via this fn?" A block whose enclosing fn
has `transitive_callers=200` is on the hot path; one with
`transitive_callers=2` is a leaf.

### Step 5. Render context

For each finding worth investigating:

```
read_file_content(file_path=<finding.file>)
```

Slice `[span[0] - 500, span[1] + 200]` for the SAFETY comment and
surrounding fn body. Review whether the comment matches the actual
invariant being upheld.

## Recipes

### Recipe — "Undocumented-unsafe inventory"

Filter `has_safety_comment=false`; sort by `line_count` desc. Top
candidates are the largest undocumented unsafe blocks — the highest-
leverage places to add SAFETY comments first.

### Recipe — "Unsafe blast radius"

For each finding, `recursive_callers_count(target=<enclosing_function_name>, depth=8)`.
Sort by `transitive_callers` desc → unsafe ranked by how many callers
are downstream. Combine with `has_safety_comment=false` to identify
high-blast-radius undocumented unsafe.

### Recipe — "Per-crate unsafe surface"

Group findings by the first path component of `file` (the crate dir).
Crates with disproportionate unsafe density are the targets for FFI /
perf-critical-section review.

## Decision frames

| Finding | Verdict |
|---|---|
| Small undocumented block (`line_count ≤ 2`) using `mem::transmute` between equivalent reprs | Tolerable; idiom |
| Small undocumented block doing pointer arithmetic | Add SAFETY comment |
| Large undocumented block (`line_count ≥ 10`) | Break into smaller blocks each with its own SAFETY |
| Block with documented SAFETY but high blast radius | Re-review comment quality on PR |
| Block with no enclosing fn (e.g. const initializer) | `enclosing_function_name=null`; harder to attribute risk — review case-by-case |

## Pattern reference

| Audit | Invocation |
|---|---|
| Undocumented unsafe | `unsafe_audit` filtered to `has_safety_comment=false` |
| Top by size | `unsafe_audit` sorted by `line_count` desc |
| Top by blast radius | `unsafe_audit` × `recursive_callers_count(target=enclosing_fn)` |
| Per-crate density | `unsafe_audit` grouped by file's first path component |

## Output format

```
Workspace: <path>
Total unsafe blocks: <n>
Undocumented: <m> (<m/n>%)
Top undocumented by line_count: <list>
Top by blast radius: <list with transitive_callers>
Per-crate density:
  <crate_a>: <n> blocks
  <crate_b>: <n> blocks
Verdict: <PASS | k undocumented blocks need SAFETY comments>
```

## Limitations

- `has_safety_comment` is a substring heuristic — checks for `SAFETY` in
  the 5 lines preceding the `unsafe` keyword. It does NOT validate
  comment quality, freshness, or whether the comment matches the actual
  invariant.
- `enclosing_function_name` is null for unsafe in const initializers,
  trait bounds, and closures-without-fn-parent. These cases need manual
  review.
- Live computation per invocation (no caching). Workspace load is ~2-3s;
  subsequent calls in the same session may be faster if RA's incremental
  cache is warm.
- Only counts `unsafe { ... }` blocks — does not surface `unsafe fn`
  declarations as findings (use `rmc-attribute-audit` or
  `rmc-signature-search` for that).

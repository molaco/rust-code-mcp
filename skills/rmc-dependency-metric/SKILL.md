---
name: rmc-dependency-metric
description: Rank Rust crates by Martin metrics.
argument-hint: "[sort_by=afferent|efferent|instability|abstractness|item_count] [top_n=N]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust per-crate dependency metric

Robert Martin's instability/abstractness metric per local crate,
sortable by any of five keys. Surfaces architectural shape at higher
resolution than reading the full `crate_edges` matrix. Scope:
workspace-wide.

For per-edge symbol breakdown (which symbols flow), use
`rmc-imports-exports`. For architectural rule enforcement, use
`rmc-architecture-rules`.

## Scope — workspace-wide

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Pull sorted metric

```
crate_dependency_metric(directory=..., sort_by=<key>, top_n=<n>)
```

Returns one row per local crate:

```json
{
  "crate_count": 12,
  "metrics": [
    { "crate_id": "<64-char-hex>", "crate_name": "my_crate",
      "efferent": 5, "afferent": 2,
      "instability": 0.71, "abstractness": 0.18, "item_count": 142 }
  ]
}
```

- `efferent` (Ce) — distinct outgoing producer crates (fan-out).
- `afferent` (Ca) — distinct incoming consumer crates (fan-in).
- `instability = Ce / (Ce + Ca)` — 0 = max stable (lots of consumers,
  no dependencies); 1 = max unstable.
- `abstractness = (traits + pub_type_aliases) / total_items` — high
  abstractness = trait-and-alias-heavy crate (facades, abstract layers).

`sort_by` accepts `instability`, `item_count`, `afferent`, `efferent`,
`abstractness` (all descending). `top_n` slices the head after sorting.

### Step 2. Cross-reference

Pick the top crate from the metric; pull `crate_edges` filtered to that
crate to see the per-edge breakdown:

```
crate_edges(directory=...)   # filter consumer_crate=<top> OR producer_crate=<top>
```

The metric ranks; `crate_edges` shows the symbols carrying each edge.

## Recipes

### Recipe — "Most-depended-on crate"

```
crate_dependency_metric(sort_by="afferent", top_n=10)
```

Top by `afferent` = "what's the architectural core?" Crates with high
fan-in are the universal-types layer. Matches the `crate_edges`
decomposition but at higher resolution and with the abstractness ratio
attached.

### Recipe — "Most-dependent crate"

```
crate_dependency_metric(sort_by="efferent", top_n=10)
```

Top by `efferent` = "what's the workhorse / orchestrator?" — crates
with high fan-out integrate many services. Often the binary, the
integration crate, or a god-crate candidate.

### Recipe — "Stable-but-concrete vs abstract main-sequence"

Robert Martin's main-sequence: stable crates should be abstract (high
`abstractness`, low `instability`). Crates that are stable AND concrete
(low instability, low abstractness) are rigid concrete cores — refactor
caution. Crates that are abstract AND unstable are facades over volatile
internals — verify intent.

## Decision frames

| Situation | Tool |
|---|---|
| Per-edge analysis (which symbols flow?) | `crate_edges` (via `rmc-imports-exports`) |
| High-level ranking | `crate_dependency_metric` |
| Find architectural core | `sort_by="afferent"` |
| Find workhorses | `sort_by="efferent"` |
| Find facades / abstract layers | `sort_by="abstractness"` |
| Find god-crates | `sort_by="item_count"` |
| Find unstable cores | `sort_by="instability"` |

## Pattern reference

| Audit | Invocation |
|---|---|
| Top 10 by fan-in | `crate_dependency_metric(sort_by="afferent", top_n=10)` |
| Top 10 by fan-out | `crate_dependency_metric(sort_by="efferent", top_n=10)` |
| Top 10 by item count | `crate_dependency_metric(sort_by="item_count", top_n=10)` |
| Top 10 by abstractness | `crate_dependency_metric(sort_by="abstractness", top_n=10)` |
| Top 10 most unstable | `crate_dependency_metric(sort_by="instability", top_n=10)` |

## Output format

```
Crates: <n>
Sorted by: <key>

Top crates:
  1. <crate_a>  efferent=<e>  afferent=<a>  instability=<i>  abstractness=<b>  items=<n>
  2. ...

Notes:
  - Architectural core: <crate with highest afferent>
  - Workhorse: <crate with highest efferent>
  - Off main-sequence: <list (stable + concrete OR abstract + unstable)>
```

## Limitations

- Counts edges, not symbols. For symbol-level breakdown of any
  (consumer, producer) pair, drop to `crate_edges` and filter.
- NaN-guarded: degenerate counts (zero items, zero edges) return 0.0
  for both metrics.
- Unknown `sort_by` values produce an `invalid_params` error.
- `abstractness` counts `traits + pub_type_aliases` — it does NOT count
  `pub use` re-exports (which are bindings, not items). A facade that's
  all `pub use` will have low `abstractness` despite being structurally
  a facade.

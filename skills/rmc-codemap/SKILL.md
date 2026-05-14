---
name: rmc-codemap
description: Task-conditioned workspace subgraph — nodes, edges, hierarchy.
argument-hint: "<task-prompt-or-seed-list>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust codemap — task-conditioned workspace subgraph

`build_codemap` returns a focused subgraph of the workspace hypergraph keyed off either a natural-language `task_prompt` or explicit `seed_qualified_names`. One MCP call composes BFS expansion + relevance scoring + hierarchy projection + optional snippet extraction into a single artifact: nodes + edges + filtered module tree + stats.

Use this when "what code interacts with X?" is too coarse for `rmc-call-graph`'s single-fn shape. For workspace-wide duplicate detection, use `rmc-semantic-overlaps`. For exhaustive one-symbol analysis (uses + imports + bindings), use `rmc-symbol-forensics`. For refactor planning around the result, hand off to `rmc-refactor-plan`.

## Scope — one or many seeds, depth-bounded subgraph

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

If the snapshot is older than the newest `.rs` file in the workspace, `build_codemap` returns a diagnostic asking for `build_hypergraph(directory=..., force_rebuild=true)`. The check runs automatically.

`task_prompt` mode also requires the BM25 / vector indices: run `index_codebase(directory=...)` once.

## Two modes, two jobs

| Mode | Seed | When to use |
|---|---|---|
| `seed_qualified_names=[...]` | Explicit list of `pub` / `pub(crate)` qualified names | You know the entry point. Fast, deterministic. |
| `task_prompt="..."` | Natural-language query → HybridSearch (BM25 + vector) | Exploratory. Best against code with rich doc comments. |

Pass at least one. Both together: seeds take precedence; prompt ignored.

## Workflow

### Step 1. The minimum call

```
build_codemap(directory=..., seed_qualified_names=[<crate>::Y])
```

Defaults: `max_nodes=80`, `depth=3`, `max_incoming_per_node=8`, `embedding_policy="no_rerank"`, `format="json"`, `include_snippets=false`.

### Step 2. Tune the budget

| Param | Default | Cap | Effect |
|---|---|---|---|
| `max_nodes` | 80 | 500 | Total retained nodes. Seeds always survive; only non-seeds prune. |
| `depth` | 3 | 5 | BFS depth from each seed, both directions. |
| `max_incoming_per_node` | 8 | — | Per-node incoming-edge fanout cap (ranked by BM25 hit score). |

Focused: `max_nodes=15, depth=1`. Sweeping: `max_nodes=200, depth=3`.

### Step 3. Pick a format

| `format` | Best for |
|---|---|
| `json` (default) | LLM ingestion / dashboard / programmatic consumer |
| `outline` | Terminal / chat reading |
| `mermaid` | Diagram for docs |
| `all` | One call → three views (~3× token cost) |

`include_snippets=true` adds first 5 lines / 400 bytes of each item's source. Aligned to line boundaries; doc-comment included.

### Step 4. (Optional) Better ranking

| `embedding_policy` | Cost | Quality |
|---|---|---|
| `no_rerank` (default) | Free | `0.60 × bm25_norm + 0.40 × graph_prox` |
| `cached_only` | Free | Adds cosine for items already cached |
| `compute_missing` | ~10 ms / node first call | `0.40 × emb_sim + 0.35 × bm25 + 0.25 × graph_prox` |

Cache is shared with `rmc-semantic-overlaps` — running either warms both.

### Step 5. Read the diagnostics

`Codemap.diagnostics` is a Vec<String>, never errors. Surface to the user when non-empty.

| Diagnostic | Meaning |
|---|---|
| `"unresolved seed: X"` | Override-seed name doesn't resolve, parent path doesn't either. Typo or wrong crate. |
| `"unresolved seed: X (parent module resolves; leaf likely private or not indexed)"` | Parent module is real; leaf isn't indexed. Likely private. Make `pub(crate)` if you want it navigable. |
| `"no search hits resolved to graph items"` | Prompt path: all hits dropped. Refine the prompt or use override seeds. |
| `"{n} search hits dropped: {a} path-norm, {b} line-resolve, {c} kind-filter"` | Funnel counters. High `line-resolve` → prompt too broad; hits landed in import lines or comments. |
| `"snapshot is older than newest .rs file ..."` | Rebuild the snapshot. |

## Recipes

### Recipe — "Show me what touches fn X"

```
build_codemap(directory=..., seed_qualified_names=[<crate>::X], depth=2)
```

One seed, both directions, depth 2. Returns the cohort of callers + callees + types X interacts with. ~30-60 nodes typical.

### Recipe — "Map an unfamiliar subsystem"

```
build_codemap(directory=..., task_prompt="how does <subsystem> work",
              depth=3, format="outline")
```

Exploratory. Check `diagnostics` for drop counts — high `line-resolve` means refine the prompt. Promising hits can become explicit `seed_qualified_names` for a follow-up call.

### Recipe — "Visualize a 3-function flow for docs"

```
build_codemap(directory=..., seed_qualified_names=[A, B, C],
              depth=1, format="mermaid", max_nodes=30)
```

Paste output into mermaid.live, render via `npx -y @mermaid-js/mermaid-cli`, or commit the `.mmd` to `.docs/`.

### Recipe — "Programmatic consumer needs structured data"

```
build_codemap(directory=..., seed_qualified_names=[...],
              format="json", include_snippets=true)
```

Read `nodes[].id` (32-byte array) ↔ `edges.from/to`. Use `qualified_name`, `file`, `line` for display. `snippet` carries first ~5 source lines.

### Recipe — "Best ranking, willing to pay once"

```
build_codemap(directory=..., task_prompt=..., embedding_policy="compute_missing")
```

First call seconds-to-minutes depending on retained-node count. Cache fills; subsequent calls are fast. Cache is shared with `semantic_overlaps`.

### Recipe — "Quick subgraph diff after edits"

```
build_hypergraph(directory=..., force_rebuild=true)
build_codemap(directory=..., seed_qualified_names=[<edited-fn>], depth=2)
```

`build_codemap` automatically fires the staleness diagnostic if you forget the rebuild. `compute_fingerprint` flips on any `.rs` or `Cargo.toml` byte change.

## Decision frames

| Goal | Tool |
|---|---|
| Single fn caller/callee list | `rmc-call-graph` (not codemap) |
| Subgraph around one fn | `build_codemap(seed_qualified_names=[X], depth=1)` |
| Exploratory subgraph | `build_codemap(task_prompt=..., depth=3)` |
| Duplicate-detection / clones | `rmc-semantic-overlaps` (not codemap) |
| One-symbol forensics | `rmc-symbol-forensics` (not codemap) |
| Diagram for docs | `format="mermaid"` |
| LLM ingestion | `format="json"` |
| Terminal-readable | `format="outline"`, often with `include_snippets=true` |
| Quality over speed | `embedding_policy="compute_missing"` |
| Speed over quality | `embedding_policy="no_rerank"` (default) |

## Pattern reference

| Pattern | Invocation |
|---|---|
| Override-seed, default | `build_codemap(seed_qualified_names=[X])` |
| Multi-seed cohort | `build_codemap(seed_qualified_names=[A, B, C], depth=1)` |
| Prompt-driven | `build_codemap(task_prompt="...", depth=2)` |
| Narrow (single concept) | `max_nodes=15, depth=1` |
| Broad (subsystem map) | `max_nodes=200, depth=3` |
| Visual | `format="mermaid"` |
| Annotated text | `format="outline", include_snippets=true` |
| All views | `format="all"` |
| Cached cosine rerank | `embedding_policy="cached_only"` |
| Compute + cache | `embedding_policy="compute_missing"` |

## Output format

```
Codemap: <seeds-summary>
Snapshot: <graph_id>  (<diagnostic about staleness if any>)
Nodes: <n>  Edges: <m>  Seeds: <s>  ms: <ms>
Embeddings: <cached / computed / none>

Diagnostics:
  - <line per diagnostic, or "none">

Hierarchy (top items by relevance):
  * <seed>  [<ItemKind>]  <file>:<line>
    <neighbor1>  [<ItemKind>]  <file>:<line>
    <neighbor2>  [<ItemKind>]  <file>:<line>
  ...

Edges (sampled by weight desc): <e1>, <e2>, ...
Verdict: <leaf cluster | hub | midstream flow | exploratory>
Followups: <up-to-3 specific suggestions>
```

Use `*` prefix for seeds. Group by parent module path in the hierarchy block. For `format="mermaid"` outputs, return the raw `flowchart LR ...` block (no wrapping).

## Limitations

- Hypergraph indexes only `pub` / `pub(crate)` items. Module-local private fns can't be seeded by qualified name. The diagnostic distinguishes "not indexed" from "typo" by checking whether the parent module resolves.
- HybridSearch weighs token frequency in doc comments, so verbose-doc public surfaces rank highest. Prompts naming rare identifiers (e.g. `is_callable`) may not rank those identifiers' definitions; the result is dominated by code that *talks about* the concept. Use override seeds for pinpoint navigation.
- ~65% of search hits typically fail line-resolve when prompts are too broad — chunks span line ranges that don't align with item bodies. The dropped-hit diagnostic surfaces this.
- Edge weight is always 1 in v1; underlying adapters dedupe by NodeId. Call-site multiplicity will surface if/when adapters expose counts.
- Trait dispatch / `dyn ExternalTrait` / generic `F: Fn(..)` indirect calls are inherited blind spots from `Usage` extraction. Local trait dispatch IS captured (resolves to the trait declaration's Item).
- `compute_missing` first-call cost: model load is ~80 MB on disk, per-node embedding is ~10 ms; subsequent calls hit the cached vector and are sub-ms per node.
- Span byte offsets are snapshot-pinned. After source edits without rebuilding, snippets and line numbers may drift; the staleness diagnostic catches the common case.

# `build_codemap` — usage guide

A task-conditioned subgraph of the indexed workspace: nodes + edges + hierarchy + stats, focused on whatever you point it at. Returns a compact, structured artifact that an agent or human can read end-to-end without opening source files.

## Prerequisites

The tool reads from a persisted hypergraph snapshot. Before first use against a workspace:

```
build_hypergraph(directory = "/path/to/your/cargo/workspace")
```

That call walks the workspace via rust-analyzer, extracts items, bindings, usages, then writes a snapshot under `<data_dir>/graphs/<workspace_hash>/`. `build_codemap` will refuse to run otherwise.

If the snapshot is older than your most recent `.rs` edit, `build_codemap` returns a diagnostic asking you to `build_hypergraph(force_rebuild = true)`. The check runs automatically on every call.

## Parameters

| Parameter | Type | Default | Purpose |
|---|---|---|---|
| `directory` | string | **required** | Absolute path to the workspace root (the dir containing `Cargo.toml`). |
| `task_prompt` | string | — | Natural-language query. Required unless `seed_qualified_names` is given. |
| `seed_qualified_names` | string[] | — | Explicit seed list by qualified name. Required unless `task_prompt` is given. |
| `max_nodes` | int | 80 (cap 500) | Maximum retained nodes total. Seeds are unconditional; only non-seed nodes are pruned. |
| `depth` | int | 3 (cap 5) | BFS expansion depth from each seed in both directions. |
| `max_incoming_per_node` | int | 8 | Per-node cap on incoming-edge neighbors during BFS, sorted by BM25 hit score. |
| `embedding_policy` | string | `"no_rerank"` | One of `no_rerank` \| `cached_only` \| `compute_missing`. |
| `format` | string | `"json"` | One of `json` \| `mermaid` \| `outline` \| `all`. |
| `include_snippets` | bool | `false` | Read source per retained node and include 5-line / 400-byte snippet in JSON + outline. |

You must supply at least one of `task_prompt` or `seed_qualified_names`. The MCP layer returns `invalid_params` otherwise.

## Two ways to seed

### A) Override seeds (precise, fast)

Use when you know what you want.

```
build_codemap(
  directory = "/home/molaco/Documents/rust-code-mcp-final",
  seed_qualified_names = ["file_search_mcp::tools::graph_tools::ensure_embeddings_for"],
  depth = 1,
  format = "outline"
)
```

Returns the seed plus everything it directly calls / uses, and everyone who directly calls / uses it.

Constraint: only `pub` and `pub(crate)` items are indexed. Module-local private functions can't be referenced. If a name fails to resolve and the parent module *does* exist, the diagnostic says `"... (parent module resolves; leaf likely private or not indexed)"`. If the parent path also doesn't resolve, you get the terse `"unresolved seed: <name>"` — likely a typo.

### B) `task_prompt` (exploratory)

Use when you don't yet know the entry point.

```
build_codemap(
  directory = "/home/molaco/Documents/rust-code-mcp-final",
  task_prompt = "how does the workspace hypergraph get persisted to disk",
  format = "outline"
)
```

Driven by `HybridSearch` (BM25 + LanceDB vectors over `all-MiniLM-L6-v2` embeddings). Best when the target code has rich doc comments. The semantic-similarity model is trained on natural English — it weighs verbose documentation heavily and tokenizes Rust identifiers imprecisely, so pinpoint navigation through code-only blocks works less well than override seeds.

Hits that fail to resolve to an indexed Item span are counted in `Codemap.diagnostics`:
```
"42 search hits dropped: 1 path-norm, 41 line-resolve, 0 kind-filter"
```
- **path-norm**: hit's file path didn't strip cleanly to a workspace-relative form.
- **line-resolve**: hit's line range didn't fall inside any indexed Item.
- **kind-filter**: resolved item failed the `is_callable() || is_type()` filter.

A high `line-resolve` count is the most common signal that your prompt is too broad — hits land in import lists or comment regions outside item bodies.

## Embedding policies

How the relevance score is computed.

| Policy | What it does | When to use |
|---|---|---|
| `no_rerank` (default) | Skip embeddings entirely. Relevance = `0.60 × bm25_norm + 0.40 × graph_prox`. | Fast, deterministic, works without a warm embedding cache. The default. |
| `cached_only` | Use cosine similarity against cached embeddings only; skip nodes that have no cached entry. Relevance = `0.40 × emb_sim + 0.35 × bm25 + 0.25 × graph_prox`. | When `semantic_overlaps` has been run before and warmed the cache. Free quality bump. |
| `compute_missing` | Compute embeddings for retained nodes that don't have them, persist into the cache for future calls. Slow first call (model load ~80 MB + per-node ~10 ms), free thereafter. | Highest quality. Pay once. |

The cache lives in the same heed sub-DB as `semantic_overlaps`'s embedding cache (`embeddings_by_target`), so warming via either tool benefits both.

## Output formats

### `format = "json"` (default)

```json
{
  "prompt": "...",
  "snapshot_id": "6e7c42b1565a3dccbca561c5af158b22",
  "generated_at_unix": 1778613715,
  "seeds": [[0x0a, 0xf4, ...]],
  "nodes": [
    {
      "id": [0x0a, 0xf4, ...],
      "qualified_name": "file_search_mcp::graph::codemap::build_codemap",
      "kind": "Item",
      "item_kind": "Function",
      "file": "src/graph/codemap.rs",
      "span": [7033, 21606],
      "line": 200,
      "relevance": 0.4,
      "is_seed": true,
      "snippet": "..."
    }
  ],
  "edges": [
    { "from": [...], "to": [...], "kind": "Calls", "weight": 1 }
  ],
  "hierarchy": { /* filtered ModuleTreeNode */ },
  "stats": {
    "seed_count": 1, "node_count": 8, "edge_count": 7,
    "embedded_nodes": 0, "embeddings_computed": 0, "total_ms": 4
  },
  "diagnostics": []
}
```

Canonical for downstream consumers. `NodeId` serializes as a 32-byte JSON array — cross-reference `edges.from` / `edges.to` against `nodes[].id`. `snippet` is `null` unless `include_snippets=true` was passed.

### `format = "outline"`

```
      file_search_mcp::embeddings::EmbeddingGenerator  [Struct]  src/embeddings/mod.rs:26
        | /// Embedding generator using fastembed
        | #[derive(Clone)]
        | pub struct EmbeddingGenerator {
      * file_search_mcp::graph::codemap::build_codemap  [Function]  src/graph/codemap.rs:200
```

Human-readable, navigable. `* ` prefix marks seeds. Indent reflects the filtered hierarchy tree depth. Lines are `<qualified_name>  [<ItemKind>]  <file>:<line>`. With `include_snippets=true`, source lines appear under each item with `| ` prefix. Best for "look at this in a chat or terminal."

### `format = "mermaid"`

```
flowchart LR
  subgraph m_file_search_mcp__graph__codemap ["mod file_search_mcp::graph::codemap"]
    n_0af479ba["build_codemap"]:::seed
  end
  n_0af479ba -->|calls| n_1f0ebe02
  n_0af479ba -.->|uses| n_a7d4fcb2
  classDef seed fill:#fde68a,stroke:#92400e
```

Pure flowchart — flat per-module subgraphs grouped by parent qualified name. Seeds styled via the `:::seed` class. Solid arrows for `Calls`, dotted for `Uses`. Edge weights always 1 in v1.

Render in:
- mermaid.live (paste contents)
- VS Code Mermaid preview extensions
- `npx -y @mermaid-js/mermaid-cli -i in.mmd -o out.svg`
- Any chat UI that renders Mermaid fenced code blocks

### `format = "all"`

Returns a JSON object `{ "json": <Codemap>, "mermaid": "...", "outline": "..." }`. Useful when you want one call to feed multiple downstream consumers. Triples the token cost.

## Diagnostics catalog

Non-erroring messages surfaced in `Codemap.diagnostics`:

| Message | What it means |
|---|---|
| `"unresolved seed: <qn>"` | Override seed didn't resolve. Parent path doesn't exist either — likely a typo or wrong crate. |
| `"unresolved seed: <qn> (parent module resolves; leaf likely private or not indexed)"` | Parent module exists in the index, but the leaf doesn't. Most often: the function is module-private. Use `pub(crate)` if you want it navigable. |
| `"no search hits resolved to graph items"` | Prompt path returned hits, but every one was dropped at path-norm / line-resolve / kind-filter. Either no resolvable code matches the prompt, or your snapshot is missing items in the relevant region. |
| `"{n} search hits dropped: {a} path-norm, {b} line-resolve, {c} kind-filter"` | Counters explaining where the search-hit funnel narrowed. High `line-resolve` typically means chunks span line ranges that don't align with item boundaries. |
| `"snapshot is older than newest .rs file; consider build_hypergraph(force_rebuild=true) (snapshot is N seconds older)"` | Staleness signal. Snippets and line numbers may not match current source. Rebuild the snapshot. |

## Capability limits

What `build_codemap` can and cannot find:

- **Captured:** direct calls, non-import references, local trait dispatch (`x.method()` where `method` is declared in a workspace-local trait — resolves to the trait declaration's `Item`).
- **NOT captured:** dispatch through `dyn ExternalTrait` over foreign traits, generic `F: Fn(..)` indirect calls, resolution to specific impl-method NodeIds via `<T as Trait>::method()`. These are limits of the underlying Usage extraction in rust-analyzer.
- **NOT indexed:** module-local private items. Make them `pub(crate)` to navigate them via override seeds.

## Validation errors

The MCP layer returns `INVALID_PARAMS` (JSON-RPC `-32602`) for:

- Missing both `task_prompt` and `seed_qualified_names`.
- `format` not in `{json, mermaid, outline, all}`.
- `embedding_policy` not in `{no_rerank, cached_only, compute_missing}`.
- `directory` doesn't resolve to a workspace with a hypergraph snapshot.

## Maintenance

| Operation | Tool call |
|---|---|
| Rebuild the hypergraph snapshot | `build_hypergraph(directory, force_rebuild=true)` |
| Wipe ALL caches (BM25, vector, metadata) plus hypergraph | `clear_cache(directory, include_hypergraph=true)` |
| Wipe just search caches, keep hypergraph | `clear_cache(directory)` |

After any of those, the next `build_codemap` call regenerates whatever it needs.

## Recommended usage patterns

**"Show me everything that touches function X."**
```
build_codemap(directory, seed_qualified_names=["crate::path::X"], depth=2)
```

**"Help me explore an unfamiliar area."**
```
build_codemap(directory, task_prompt="how does <subsystem> work", depth=3, format="outline")
```
Then refine with override seeds based on what came back.

**"Generate a diagram for documentation."**
```
build_codemap(directory, seed_qualified_names=[...], format="mermaid", max_nodes=30)
```
Paste the output into mermaid.live or check it into your repo's `.docs/`.

**"Get a quick architectural map of a 3-function flow."**
```
build_codemap(
  directory,
  seed_qualified_names=["mod::entry_point", "mod::middle", "mod::sink"],
  depth=1,
  include_snippets=true,
  format="outline"
)
```

**"Best-quality semantic ranking, willing to wait."**
```
build_codemap(directory, task_prompt="...", embedding_policy="compute_missing")
```
Run once; subsequent calls hit the warm cache and are fast.

**"Programmatic consumer (LLM, frontend, dashboard)."**
```
build_codemap(directory, ..., format="json")
```
Read `nodes` and `edges` into your data structures. Use `id` for cross-reference; `qualified_name` / `file` / `line` for display.

## Quick reference

| Want | Set |
|---|---|
| Fastest possible response | `embedding_policy=no_rerank` (default), `depth=1` |
| Compact for a terminal | `format=outline`, no snippets |
| Annotated for a chat | `format=outline`, `include_snippets=true` |
| Visual | `format=mermaid` |
| Machine-readable | `format=json` (default) |
| Best ranking quality | `embedding_policy=compute_missing` |
| Narrow focus | `max_nodes=15`, `depth=1` |
| Sweeping view | `max_nodes=200`, `depth=3` |

## Related documents

- `.plans/codemaps-proposal.md` — design rationale, algorithm specification, revision history.
- `.docs/codemap-smoke-test.{md,json}` — reproduction recipe + canonical capture for regression checks.
- `.docs/codemap-phase-{1..6}.md` — implementation phase reports.
- `.docs/codemap-pass-{1..3}.md` — post-implementation polish reports.
- `.docs/codemap-a3-fingerprint-investigation.md` — findings on `compute_fingerprint` correctness.
- `.docs/codemap-final-report.md` — overall feature shipping report.

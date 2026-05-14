# Codemap smoke-test artifact

A canonical end-to-end run of `build_codemap` against this repository, captured
post pass-1. Use this both as a sanity baseline ("did I break it?") and as a
reference for the JSON shape downstream consumers should expect.

## Reproducing

```bash
# 1. Ensure the hypergraph is current for this workspace.
#    (Once per snapshot; cheap when no source changed.)
nix develop ../nix-devshells#code --command \
    cargo run --bin file-search-mcp -- # (start MCP server)
#    Then via your MCP client:  build_hypergraph(directory=…)

# 2. Build a codemap. From an MCP client:
build_codemap(
    directory = "/home/molaco/Documents/rust-code-mcp-final",
    seed_qualified_names = ["file_search_mcp::graph::codemap::build_codemap"],
    max_nodes = 25,
    depth = 2,
    include_snippets = true,
    format = "json"
)
```

The captured JSON sits at `.docs/codemap-smoke-test.json` (~70 KB).

## Captured state (snapshot `ff4cd727a7c935f0ddbcd526d46249e7`)

| Metric | Value |
|---|---|
| `stats.seed_count` | 1 |
| `stats.node_count` | 25 |
| `stats.edge_count` | 30 |
| `stats.total_ms` | 33 |
| `stats.embedded_nodes` | 0 (default `embedding_policy: no_rerank`) |
| `stats.embeddings_computed` | 0 |
| `diagnostics` | `[]` |
| nodes with `line` populated | 25 / 25 |
| nodes with `snippet` populated | 25 / 25 |

## Verification checklist

When investigating "did I break codemap?", a fresh capture should match these
properties (numeric values can drift as the source code evolves; shapes
should not):

- [ ] **Seed resolves.** `cm.seeds` has exactly one entry whose 32-byte
      `NodeId` matches the seed node in `cm.nodes` (`is_seed: true`,
      `qualified_name: file_search_mcp::graph::codemap::build_codemap`).
- [ ] **Seed relevance is highest.** Seed `relevance` should be the max
      in `cm.nodes`. Default formula with no embedding: `0.40` for seed
      (`graph_prox = 1.0`), `0.20` for depth-1, `0.13̄` for depth-2.
- [ ] **`graph_prox` is positive for direct neighbors.** The BFS-distance
      fix (commit `71f88332`) ensures depth-1 nodes score 0.20, not 0.0.
      Regression would show all non-seed `relevance` at zero.
- [ ] **Both edge kinds present.** `cm.edges[].kind` contains both
      `"Calls"` and `"Uses"`. `Calls` for callable targets; `Uses` for
      type/data targets.
- [ ] **Edge endpoints map back.** Every `edges[].from` and `edges[].to`
      `NodeId` appears in `cm.nodes[].id`.
- [ ] **Hierarchy is filtered.** `cm.hierarchy` is a `ModuleTreeNode`
      tree containing exactly the modules that house retained items —
      no empty branches.
- [ ] **`line` populated for items with `file` + `span`.** Matches the
      1-indexed source line where the item begins (consistent with
      `ChunkContext.line_start`).
- [ ] **`snippet` populated when `include_snippets: true`.** First ~5
      lines / 400 bytes from each node's source, trimmed.
- [ ] **Empty diagnostics on the happy path.** Override-seed mode with a
      resolvable name should yield `diagnostics: []`.
- [ ] **`total_ms < 200`** for this size of codemap on a warm snapshot.
      The smoke run took 33 ms.

## Diagnostic strings the codemap can emit

These are not errors. The MCP call still returns a valid `Codemap`.

- `"unresolved seed: <qualified_name>"` — one per override seed that
  `lookup_by_qualified_name` couldn't find. Usually a typo or stale name.
- `"no search hits resolved to graph items"` — pushed once if the prompt
  path produced `hits.is_some()` but resolution dropped every hit.
- `"{n} search hits dropped: {a} path-norm, {b} line-resolve, {c} kind-filter"`
  — pushed once when at least one hit was dropped. Reads as:
  - `path-norm`: hit's file path couldn't be canonicalized and stripped
    against the workspace root (usually because the hit came from a path
    not under the snapshot's `workspace_root`).
  - `line-resolve`: `enclosing_item_for_line_range` couldn't find an
    Item span enclosing the hit's line range.
  - `kind-filter`: a resolved Item was neither callable nor a type, so
    it wasn't admitted as a seed.

## MCP validation errors

These DO come back as `McpError(code=-32602, INVALID_PARAMS)`:

- Missing both `task_prompt` AND `seed_qualified_names`.
- `format` not in `{json, mermaid, outline, all}`.
- `embedding_policy` not in `{no_rerank, cached_only, compute_missing}`.

## When to recapture

- After landing changes to `src/graph/codemap.rs`, `src/graph/queries.rs`
  (adapters), `src/tools/graph_tools.rs::ensure_embeddings_for`, or
  `src/tools/graph_tools.rs::handle_build_codemap`.
- After bumping the hypergraph schema (`SCHEMA_VERSION`).
- After changing `ItemKind::is_callable` / `is_type` predicates.

# Codemap Phase 6 — Mermaid/outline renderers + MCP tool wiring

**Status:** complete. The codemap feature is fully wired into the MCP server.

## Files changed

| File | LOC delta | Detail |
|---|---|---|
| `src/graph/codemap.rs` | +275 (1087 → 1362) | `render_mermaid`, `render_outline` + 2 smoke tests |
| `src/tools/graph_tools.rs` | +135 (3668 → 3803) | `handle_build_codemap` (snapshot open → search → build_codemap → format response) |
| `src/tools/search_tool_router.rs` | +55 | `build_codemap` `#[tool]` method (delegates to `handle_build_codemap`) |
| `src/tools/search_tool.rs` | +32 | `BuildCodemapParams` struct (Deserialize + JsonSchema) |

## Renderer choices

### Mermaid

Flat per-module `subgraph` blocks (not nested). Group key is `qualified_name.rsplit_once("::")` → parent module qn, sanitized to `[A-Za-z0-9_]`. Node IDs are `n_` + first 8 hex chars of `NodeId.as_bytes()`.

Arrow conventions:
- `EdgeKind::Calls` → `-->|calls|` (solid)
- `EdgeKind::Uses` → `-.->|uses|` (dotted)
- `Imports`/`Contains` → suppressed (not produced by current algorithm)
- Edge weight > 1: `(×N)` appended to the label
- Seeds get `:::seed` class; one `classDef seed fill:#fde68a,stroke:#92400e` at the bottom

### Outline

Flat sorted-by-qualified-name list. Two-space indent per `::` segment. Seeds prefixed with `* `. Format: `<indent><qualified_name>  [<item_kind>]  <file>@<byte_start>`.

`<file>@<byte_offset>` instead of `<file>:<line_number>` because the snapshot is not in scope at render time — byte offsets are still navigable; line conversion would require re-reading source.

## MCP tool surface

`#[tool]` method `build_codemap` on the router, delegating to `handle_build_codemap` in `graph_tools.rs`. Description (first 5 lines):

```
Build a task-conditioned subgraph (codemap) of the indexed workspace.

Returns nodes/edges/hierarchy focused on the prompt. Edges come from the
HIR-driven hypergraph: direct calls and non-import uses. Local trait
dispatch (`x.method()` where `method` is declared in a workspace-local
```

(Full description includes the trait-dispatch nuance per v2.4 §6 and the tunable-defaults summary.)

### Format dispatch

| `format` | Response shape |
|---|---|
| `"json"` (default) | Pretty-printed `Codemap` JSON via `Content::text` |
| `"mermaid"` | Raw `flowchart LR ...` text |
| `"outline"` | Raw indented outline text |
| `"all"` | JSON object `{ "json": <Codemap>, "mermaid": "...", "outline": "..." }` |

Unknown values → `McpError::invalid_params`.

## Build verification

`cargo check --lib` → 0.20s. **17 warnings** (pre-codemap baseline). All 14 codemap-related `dead_code` warnings consumed:
- Phase 1–4: `span_index` (field+method), `line_to_byte`, `enclosing_item_for_line_range`, `canonicalize_and_strip`, `callees_of`, `referrers_of`, `min_call_distance`.
- Phase 5: `build_codemap`, `resolve_override_seeds`, `resolve_search_seeds`, `build_bm25_by_node`, `rank_referrer`, `node_qualified_name`, `project_hierarchy`, `filter_module_tree`.

(`ensure_embeddings_for` was already live from Phase 4 via `semantic_overlaps`.)

The 17 residual warnings are all pre-existing (unused RA helpers in `semantic/position.rs`, `unreachable_pub` lint hits in `ids.rs`, dead `build_type_references*` in `parser/`) — unchanged by Phases 1–6.

## Tests

Two renderer smoke tests against the synthetic Phase 2 fixture:
- `render_mermaid_smoke`
- `render_outline_smoke`

The proposed `end_to_end_against_self` (build_codemap against the live file-search-mcp snapshot) was **skipped** — it would couple CI to a pre-built snapshot at a specific path. The synthetic-fixture path already exercises the full chain. End-to-end validation against the live workspace is left to manual MCP-server runs.

## Notes on existing MCP infrastructure

- `open_workspace_snapshot` and `internal_error` are file-private in `graph_tools.rs`, not `pub(crate)`. New tool helpers must live in that file or get re-exports. `handle_build_codemap` lives there for this reason.
- Existing tools all use `Content::text` with pretty-printed JSON strings — `Content::json` is not used anywhere. Convention preserved.
- `HybridSearch` construction is not a one-liner: needs `ProjectPaths::from_directory`, BM25 via `TantivyAdapter`, and `query_tools::create_hybrid_search`. The codemap wiring inlines this (with a best-effort fallback for missing BM25 indices) rather than introducing a new shared helper.
- `Parameters<T>` destructuring at the `#[tool]` signature works for arbitrary field counts — no need for a `params:` intermediate binding.

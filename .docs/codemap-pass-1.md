# Codemap Pass-1 polish

**Status:** complete.
**Scope:** five surgical fixes from the post-implementation review (items 3, 4, 5, 8, 9). No architectural change; all in known files.

## Files changed

| File | LOC delta | Detail |
|---|---|---|
| `src/graph/codemap.rs` | ~+135 | `CodemapNode.line`; zero-seed + dropped-hit diagnostics; `line_of_byte` + `extract_snippet` helpers; outline line-format + snippet rendering; Mermaid snippet-exclusion doc |
| `src/tools/graph_tools.rs` | ~+85 | three `mod tests` validation tests; `handle_build_codemap` validation order confirmed (already correct) |
| `src/tools/search_tool.rs` | +1 | `include_snippets` schema description updated |

## What was added

### Diagnostics (items 3 + 4)

`Codemap.diagnostics` now carries two new strings when warranted:

- `"no search hits resolved to graph items"` — pushed once when `hits.is_some()` and the resolved seed set is empty.
- `"{total} search hits dropped: {a} path-norm, {b} line-resolve, {c} kind-filter"` — pushed once when total drops > 0. Counters:
  - `dropped_path_norm`: `canonicalize_and_strip` returned `None`.
  - `dropped_line_resolve`: `enclosing_item_for_line_range` returned `None`.
  - `dropped_kind_filter`: resolved item failed `is_callable() || is_type()`.

Verified live: a nonsense prompt produced `"36 search hits dropped: 0 path-norm, 36 line-resolve, 0 kind-filter"`. The "zero seeds" diagnostic is rarer in practice because the vector store usually returns *some* resolvable hit — but the dropped-hit counter is the more useful signal anyway.

### `CodemapNode.line: Option<u32>` (item 5)

- Populated during the ASSEMBLE step when `node.file.is_some() && node.span.is_some()`.
- Lookup: `snap.line_to_byte(file).ok()?`, then `table.partition_point(|&off| off <= byte_start)` → 1-indexed line `idx + 1`.
- Stays `None` when file/span absent or the line→byte table fails.
- `span` retained (machine consumers still need byte ranges).

### Outline rendering (item 5)

`render_outline` now prints `<file>:<line>` when `node.line` is `Some`, falls back to `<file>@<byte_offset>` when only `span` is present, and `<file>` when neither.

### Snippets (item 9)

When `opts.include_snippets`:
- Per retained node with `file` + `span`, read source from disk (cached per call by workspace-relative path).
- Slice from `byte_start`, capped at **5 newlines OR 400 bytes** (first hit wins). Walked back to nearest UTF-8 char boundary, `trim_end()`'d.
- Skipped silently if file unreadable, span past EOF, start not on a char boundary, or trimmed result empty.

Outline renders snippets indented under each item with `| ` prefix per source line. Mermaid deliberately does NOT render snippets — node labels stay compact (doc-comment added explaining).

`BuildCodemapParams.include_snippets` description updated from "reserved" to:
> "Include the first ~5 lines of source per node in the JSON/outline output. Default false."

### MCP parameter tests (item 8)

Three new `#[tokio::test]`s in `src/tools/graph_tools.rs::mod tests`:

1. `build_codemap_requires_prompt_or_seeds` — asserts error message mentions both knobs.
2. `build_codemap_rejects_bad_format` — asserts `INVALID_PARAMS` (-32602) with valid-options list.
3. `build_codemap_rejects_bad_embedding_policy` — same shape.

All three pass `/tmp` as `directory`. Validation runs *before* `open_workspace_snapshot`, so no fixture needed. Order was already correct; no `handle_build_codemap` restructuring required.

## Build verification

- `cargo check --lib` → 17 warnings (unchanged baseline).
- `cargo check --lib --tests` → clean; pre-existing 17 lib + 16 test warnings unchanged.

## End-to-end smoke

`/tmp/codemap-pass1-check.py` exercises all five items through the live MCP server:

- Outline + snippets render correctly with `:line` format and 5-line source under each item.
- JSON contains `line: 2436` for `cosine` (matches actual file line); `snippet` is the doc comment + signature.
- Bad format returns `INVALID_PARAMS` with the message `"unknown format \`graphviz\`; expected \`json\` | \`mermaid\` | \`outline\` | \`all\`"`.
- Dropped-hit counter fires on the nonsense-prompt case.

## Items deferred to Pass 2

- Item 2 (make codemap tests runnable — fixture isolation).
- Item 6 (formal smoke test artifact in `.docs/`).
- Item 7 (regression tests for pruning + BFS distance).
- Item 10 (final verification regimen).

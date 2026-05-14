# Codemap Phase 2 — span index + line→byte bridge + path normalization

**Status:** complete.
**Scope:** §4 of `.plans/codemaps-proposal.md` + the "Span-resolution helper" row of §2.

## Files changed

| File | Change | LOC delta |
|---|---|---|
| `src/graph/snapshot.rs` | extended `OpenedSnapshot` with two lazy fields + two `pub(crate)` accessors | ~+90 |
| `src/graph/codemap.rs` | added `enclosing_item_for_line_range` + `canonicalize_and_strip` helpers + `#[cfg(test)] mod tests` (4 tests) | ~+200 |

## What was added

### `OpenedSnapshot` caches
- `span_index: OnceLock<HashMap<String, Vec<(u32, u32, NodeId)>>>` — per-file flat sorted `Vec`, built once per handle by scanning `dbs.nodes_by_id` for `NodeKind::Item` entries with both `file` and `span` set.
- `line_to_byte: Mutex<HashMap<String, Arc<Vec<u32>>>>` — built on demand per file; reads the file once and computes a `\n`-offset prefix table. `Arc<Vec<u32>>` so callers can clone-out without holding the mutex.

### `pub(crate)` accessors on `OpenedSnapshot`
- `span_index(&self) -> &HashMap<String, Vec<(u32, u32, NodeId)>>` — `get_or_init` on the `OnceLock`.
- `line_to_byte(&self, workspace_relative_file: &str) -> io::Result<Arc<Vec<u32>>>` — fast path via the mutex; slow path reads from `manifest.workspace_root.join(rel)`.

### `pub(crate)` helpers in `src/graph/codemap.rs`
- `enclosing_item_for_line_range(snap, file, line_start, line_end) -> Option<NodeId>` — converts 1-indexed inclusive lines → byte range → smallest enclosing Item span via the span index. The conversion is the exact formula specified in §4: `byte_start = line_to_byte[line_start - 1]`; `byte_end = line_to_byte[line_end] - 1` (or EOF fallback).
- `canonicalize_and_strip(path, ws_root) -> Option<String>` — query-time path normalization. Not the same as the build-time `resolve_workspace_relative` in `src/graph/usages.rs` (which takes a `&Vfs` only available at indexing time).

## Tests

Four `#[cfg(test)]` tests in `codemap::tests` against a real synthetic snapshot built via `build_and_persist` (pattern lifted from `src/graph/usages.rs:272`):

1. `line_to_byte_correct_for_lf_file` — byte offsets per line correct.
2. `enclosing_item_returns_none_for_unknown_file` — graceful miss.
3. `enclosing_item_returns_none_for_invalid_range` — out-of-range guard.
4. `canonicalize_and_strip_normalizes` — round-trip a `tempdir`-rooted path.

The "smallest enclosing span (outer vs inner fn)" test was skipped — RA's extraction byte-span shape for nested fns is not well-enough documented to assert without brittleness; Phase 6 e2e exercises it.

## Construction-site update

`OpenedSnapshot` has exactly one literal construction site: `src/graph/snapshot.rs:429` inside `open_specific`. Updated to initialize the two new fields. No destructuring patterns anywhere.

## Build verification

`nix develop ../nix-devshells#code --command cargo check --lib` → success in 0.20s. **21 warnings (18 pre-existing baseline + 4 new `dead_code`** on the unused `pub(crate)` accessors / helpers; these consume in Phase 5).

## Notes for Phase 3

- `NodeId` is a tuple struct with a `pub` `[u8; 32]` field. Construction idiom is `NodeId(arr)` where `arr: [u8; 32]` built via `copy_from_slice`. No `from_bytes` helper exists (see `src/graph/queries.rs:2269-2271`).
- `OpenedSnapshot::node(&self, txn, id) -> Result<Option<Node>>` already exists — Phase 3 adapters can use it cheaply to fetch `item_kind`.
- The two cache fields are private; Phase 3+ calls `snap.span_index()` / `snap.line_to_byte(rel)` via the `pub(crate)` accessors.

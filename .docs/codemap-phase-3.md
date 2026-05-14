# Codemap Phase 3 — Raw-ID graph adapters + `min_call_distance`

**Status:** complete.
**Scope:** Raw-ID adapters from §5 + the "Raw-ID graph adapters" row of §2.

## Files changed

| File | Change | LOC delta |
|---|---|---|
| `src/graph/queries.rs` | added `OpenedSnapshot::callees_of` + `referrers_of` (`pub(crate)`) right after `usages_for_consumer_function` | +30 |
| `src/graph/codemap.rs` | added `min_call_distance` + 3 tests (extended fixture with `caller`/`callee` pair) | +82 |

## What was added

### Adapters in `src/graph/queries.rs` (lines 2329–2358)

```rust
pub(crate) fn callees_of(&self, caller_fn: NodeId) -> Result<Vec<NodeId>>
pub(crate) fn referrers_of(&self, target: NodeId)  -> Result<Vec<NodeId>>
```

Both wrap the existing private `usages_for_consumer_function` / `usages_for_target` iterators and dedupe via `HashSet`. The query layer stays feature-agnostic — no `EdgeKind` here. Edge classification (Calls vs Uses) happens later in `codemap.rs` by reading endpoint `Node.item_kind`.

### `min_call_distance` in `src/graph/codemap.rs` (lines 178–217)

```rust
pub(crate) fn min_call_distance(
    snap: &OpenedSnapshot,
    node: NodeId,
    seeds: &HashSet<NodeId>,
    max_depth: u32,
) -> u32
```

Forward BFS over `callees_of`, returning shortest distance to any seed or `u32::MAX` if no seed is reachable within `max_depth`. `0` when `node` is itself a seed. Mirrors the frontier+visited template of `recursive_callers_count` (`src/graph/queries.rs:852+`) but in the forward direction and returning depth instead of count.

## Tests

Extended Phase 2's `FIXTURE_LIB_RS` with `pub fn callee() {}` / `pub fn caller() { callee(); }`. Added 3 tests to `mod tests`:

1. `callees_of_includes_called_function` — asserts `caller`'s callees set contains `callee`.
2. `referrers_of_includes_caller` — symmetric: `callee`'s referrers set contains `caller`.
3. `min_call_distance_zero_when_seed_is_self` — degenerate case, no graph traversal needed.

## Build verification

`cargo check --lib` → 0.27s. 23 warnings total:
- 17 pre-existing (untouched).
- 4 from Phase 2 (`span_index`, `line_to_byte`, `enclosing_item_for_line_range`, `canonicalize_and_strip`) — still dead, consumed in Phase 5.
- 2 new from Phase 3 (`callees_of`/`referrers_of` combined, `min_call_distance`) — also consumed in Phase 5.

`cargo check --lib --tests` also passes; new tests compile.

## Notes for Phase 4

- `src/tools/graph_tools.rs:717-1100` (`semantic_overlaps`) is ~400 LOC with established async/locking semantics around `embed_batch_async`.
- The factored signature is `ensure_embeddings_for(snap, nids: &[NodeId]) -> Result<HashMap<NodeId, Vec<f32>>>`. Must follow the three-phase txn pattern from §5 SCORE: pure-sync collect → drop `rtxn` → async batched embed → reopen `rwxn` to persist. No `RoTxn` may be held across the embed call.

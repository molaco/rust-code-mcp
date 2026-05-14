# Codemap Phase 4 — Extract `ensure_embeddings_for`

**Status:** complete.
**Scope:** Refactor only — extract the cache+batch-embed body of `semantic_overlaps` into a reusable `pub(crate)` helper. No new feature work.

## Files changed

| File | LOC delta | Detail |
|---|---|---|
| `src/tools/graph_tools.rs` | net +79 (3589 → 3668) | `semantic_overlaps` body shrank ~400 → ~265 (-135); new helper + types ~+210 |

Single-file change as required.

## What was extracted

```rust
pub(crate) const EMBEDDER_VERSION: &str = "fastembed:all-MiniLM-L6-v2:dim384:v1";
pub(crate) const EMBED_CHUNK: usize = 64;

#[derive(Debug, Clone)]
pub(crate) struct ResolvedEmbedding {
    pub vector: Vec<f32>,
    pub content_hash: [u8; 16],
}

pub(crate) async fn ensure_embeddings_for(
    snap: &OpenedSnapshot,
    nids: &[NodeId],
) -> anyhow::Result<HashMap<NodeId, ResolvedEmbedding>>
```

Three-phase async hygiene:
- **Phase A** — open one short `RoTxn`, classify each NodeId into (cached-and-fresh, needs-compute), slice source bytes for the latter, drop `RoTxn`.
- **Phase B** — if anything needs computing, lazy-construct `EmbeddingGenerator` (80MB model load), batch via `embed_batch_async(EMBED_CHUNK=64)`.
- **Phase C** — open one short `RwTxn`, persist new `EmbeddingRecord`s, commit.

### Deviations from the brief's hint

1. **Returns `ResolvedEmbedding { vector, content_hash }` instead of `Vec<f32>`.** Lets `semantic_overlaps` keep its identical-source short-circuit (which compares 16-byte hashes, not full vectors) without re-reading files. Phase 5 ignores the hash field and just uses `.vector`.
2. **No `&EmbeddingGenerator` parameter.** Model init is 80MB; constructing it inside the helper *after* Phase A preserves the cheap all-cache-hit path.

Both deviations preserve `semantic_overlaps` performance characteristics.

## Behavior-preserving refactor

`semantic_overlaps` post-refactor:
1. Opens snapshot, resolves crate scope.
2. Enumerates seed Items (crate / item_kind / file+span / test filters).
3. Collects `NodeId`s, calls `ensure_embeddings_for(&snap, &seed_nids).await`.
4. Rebuilds `Vec<SeedCtx>` from the returned map (silently drops items the helper skipped).
5. Runs the unchanged short-circuit + pairwise-cosine + edge-dedup + JSON-response pipeline.

## Nuances for Phase 5

- **Locking**: callers must NOT hold any `RoTxn` across the call. Helper opens/drops its own.
- **Hash**: SHA-256 of the trimmed UTF-8 source slice, truncated to first 16 bytes.
- **Skip rules** (silent — no error, no log): NodeId with no file, no span, unreadable file, out-of-range span bytes, or empty/whitespace-only source.
- **Dedup**: helper de-duplicates input slice internally.
- **Path resolution**: helper joins `Node.file` onto `snap.manifest.workspace_root` — not any user-supplied directory.
- **Error type**: `anyhow::Result`. MCP layer maps via `internal_error("ensure_embeddings_for")`.

## Build verification

`cargo check --lib` → 0.20s. **23 warnings, unchanged from Phase 3**. No new warnings introduced.

# Qwen3 Embedding GPU Optimization Proposal

## Summary

Indexing this workspace with `Qwen/Qwen3-Embedding-0.6B` is embedding-bound. A
full local benchmark on May 16, 2026 indexed 118 files and 1833 chunks in 71.93s.
The measured embedding phase was 71.57s, or 99.5% of the total runtime. Parse,
Tantivy indexing, and LanceDB writes were effectively noise at this scale.

The current path is already using the GPU and saturating it. During the run, an
RTX 3090 stayed near 100% SM utilization and roughly 300-349W. The observed
throughput is therefore not caused by idle GPU time, disk writes, metadata cache
work, or a CPU parse bottleneck. The bottleneck is the Qwen3 forward pass as
implemented by fastembed's Candle backend.

The target is to move effective embedding throughput from roughly 9k model
tokens/sec toward 20k tokens/sec while preserving retrieval quality.

## Current Measurements

Benchmark command used:

```sh
./target/release/examples/index_codebase
```

Observed result:

| Metric | Value |
|---|---:|
| Files indexed | 118 |
| Chunks generated | 1833 |
| Total wall time | 71.93s |
| Embedding time | 71.57s |
| Embedding share | 99.5% |
| Parse time | 0.07s |
| Index/write time | 0.07s |
| Effective chunks/sec | 25.5 |

Token distribution from `./target/release/examples/chunk_token_stats`:

| Metric | Value |
|---|---:|
| Tokenized chunks | 1852 |
| Raw total tokens | 642,855 |
| Model input tokens, capped at 1024 | 555,894 |
| Padded model tokens, batch size 32 | 640,160 |
| Mean raw tokens/chunk | 347 |
| Median raw tokens/chunk | 212 |
| p90 | 660 |
| p95 | 958 |
| p99 | 1767 |
| Max raw chunk | 17,862 |

The batch-padded total explains the current ~9k tokens/sec:

```text
640,160 padded tokens / 71.57s = ~8,945 padded tokens/sec
```

## Current Code Path

The indexing path batches chunks in `UnifiedIndexer::process_and_index_batch`,
then delegates to `IndexerCore::generate_embeddings_batched`.

Relevant code:

- `src/indexing/unified.rs`: collects chunks, measures embedding time, then
  writes Tantivy and LanceDB records.
- `src/indexing/embedding_batcher.rs`: formats chunks, sorts by character
  length, and embeds fixed-size sub-batches.
- `src/config/indexer.rs`: sets `gpu_batch_size = 32` for all codebase sizes
  and for the default MCP path.
- `src/embeddings/qwen3.rs`: constructs fastembed `Qwen3TextEmbedding` with
  `DType::F16`, `max_len = 1024`, and a CUDA device.

The current batching strategy is reasonable but coarse:

```text
format chunks -> sort by char length -> chunks(32) -> fastembed embed()
```

The important limitation is inside fastembed 5.13.4:

```rust
let kt = k.transpose(2, 3)?;
let mut attn = q.matmul(&kt)?; // [B, Nh, T, T]
attn = candle_nn::ops::softmax(&attn, D::Minus1)?;
let out = attn.matmul(&v)?;
```

This is full materialized attention over `[batch, heads, seq, seq]`. It does
not use flash attention. Because cost grows with sequence length squared, long
chunks are disproportionately expensive even when the simple token/sec metric
looks moderate.

## Diagnosis

### Not the bottleneck

- File discovery and parsing are fast. The benchmark measured ~0.07s parse time.
- Tantivy and LanceDB writes are already batched. The benchmark measured ~0.07s
  write/index time.
- The GPU is not waiting on CPU work during embedding. SM utilization is pinned
  near 100%.
- Model initialization is not part of the 70s steady-state indexing cost.

### Bottleneck

The Qwen3 Candle backend is doing many expensive full-attention forwards with a
fixed max batch size of 32. Since Qwen3 pads each batch to the longest sequence,
effective work is driven by:

```text
sum over batches: batch_size * max_seq_len_in_batch^2
```

The current character-length sorting reduces padding waste, but it does not:

- use real token lengths,
- choose batch size by token budget,
- isolate or split pathological long chunks,
- reduce the `T^2` attention cost,
- use fused attention kernels.

## Recommendations

### 1. Add runtime tuning for embedding batch size

Make `gpu_batch_size` configurable by environment variable and MCP/indexer
options. Keep 32 as the conservative default, but allow local tuning to 48, 64,
and possibly 96 on 24 GB GPUs.

Suggested environment variables:

```text
RUST_CODE_MCP_EMBED_BATCH_SIZE=64
RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=32768
```

Expected impact:

- Low implementation risk.
- Possible 5-25% speedup if current batches underutilize GEMM shapes.
- May hit OOM on mixed long batches unless paired with token-budget batching.

Implementation notes:

- Extend `IndexerCoreConfig` with env override parsing.
- Log the resolved batch size at startup.
- Reject zero.
- Keep the configured value in the embedding backend identity only if it changes
  vector semantics. Batch size does not change embeddings, so it should not
  invalidate caches.

### 2. Replace char-length batching with token-budget batching

Current code sorts by `text.len()`. This is a rough proxy. Qwen3 cost is based on
token count after truncation and batch padding.

Proposal:

1. Load or share the Qwen3 tokenizer in the embedding path.
2. Compute `token_len = min(raw_token_len, backend.max_len)` per formatted chunk.
3. Sort by `token_len`.
4. Pack batches by both item count and padded-token budget:

```text
batch_cost = batch_len * max_token_len_in_batch
batch_cost <= max_tokens_per_batch
batch_len <= max_batch_size
```

For full-attention Qwen3, also consider a quadratic budget:

```text
attention_cost = batch_len * max_token_len_in_batch^2
attention_cost <= max_attention_tokens
```

Expected impact:

- Medium implementation risk.
- Reduces padding waste and OOM risk.
- Lets small chunks batch much larger while long chunks stay in small batches.
- Should improve throughput more reliably than only raising `gpu_batch_size`.

### 3. Split long chunks before embedding

The current chunk distribution has long impl and test-module chunks that get
truncated to 1024 tokens but still pay near-worst-case attention cost. The top
raw chunks were:

| Raw tokens | Source |
|---:|---|
| 17,862 | `src/graph/queries.rs`: `impl OpenedSnapshot` |
| 9,577 | `src/tools/search_tool_router.rs`: `impl SearchToolRouter` |
| 9,351 | `src/graph/queries.rs`: `tests` module |
| 7,268 | `src/graph/codemap.rs`: `tests` module |
| 6,480 | `src/tools/graph_tools.rs`: `tests` module |

Proposal:

- Add a secondary split for chunks whose formatted text exceeds a token target.
- Start with `target_tokens = 512` or `768`, `hard_max_tokens = 1024`.
- Prefer semantic split points:
  - functions inside impl blocks,
  - methods inside impl blocks,
  - test functions inside test modules,
  - blank-line or item boundaries as fallback.
- Carry parent context in the formatted text:
  - file path,
  - parent symbol,
  - child symbol,
  - line range.

Expected impact:

- Medium to high implementation risk.
- Likely the biggest quality plus speed improvement inside the current backend.
- Reduces truncation loss.
- Reduces the number of near-1024-token forwards.

Important tradeoff:

- Splitting increases chunk count, but the attention cost reduction can still
  win because Qwen3 full attention is quadratic in sequence length.
- Retrieval may return narrower chunks. This is usually good, but result
  presentation may need parent context.

### 4. Patch fastembed Qwen3 hot spots

There are two low-level fastembed/Candle improvements worth testing.

First, use `softmax_last_dim` instead of generic `softmax` in Qwen3 attention:

```rust
let attn = candle_nn::ops::softmax_last_dim(&attn)?;
```

Candle has a CUDA custom op for last-dim softmax. The current Qwen3 code calls
the generic path.

Second, use Candle's fused RMSNorm op rather than manual RMSNorm. The current
fastembed Qwen3 RMSNorm does multiple tensor operations:

```text
to f32 -> powf -> mean -> add eps -> sqrt -> recip -> multiply -> cast -> multiply
```

Candle has `candle_nn::ops::rms_norm`, including CUDA support.

Expected impact:

- Medium risk because this requires vendoring or upstreaming fastembed changes.
- Potentially meaningful kernel-launch and memory-bandwidth savings.
- Needs embedding parity tests against the current implementation.

### 5. Consider an optimized inference backend for the 20k target

If the target is strict 20k tokens/sec on Qwen3-Embedding-0.6B, the current
fastembed Candle backend may not be the right ceiling. It lacks flash attention
for Qwen3. A backend such as Hugging Face Text Embeddings Inference, ONNX
Runtime with an optimized exported model, TensorRT, or another fused-attention
runtime is more likely to reach that target.

Expected impact:

- Highest implementation and deployment risk.
- Highest chance of a 2x throughput jump.
- Requires a clean abstraction so local embedded mode remains available.

## Proposed Implementation Plan

### Phase 1: Make performance tunable and measurable

Deliverables:

- Add env-configurable `gpu_batch_size`.
- Add optional token-budget batching config.
- Improve logs to report:
  - chunk count,
  - raw token total,
  - capped token total,
  - padded token total,
  - embedding seconds,
  - padded tokens/sec,
  - max VRAM if available.
- Add an ignored benchmark that prints comparable results for batch sizes
  16, 32, 48, 64.

Acceptance:

- Existing default behavior remains batch size 32.
- `RUST_CODE_MCP_EMBED_BATCH_SIZE=64` changes only batch shape, not embeddings.
- Benchmark output makes padded tokens/sec visible.

### Phase 2: Token-budget batching

Deliverables:

- Token-count or tokenizer-backed batch planner.
- Batch planner unit tests over synthetic token lengths.
- Guardrails for `max_len = 1024`.
- Fallback to char length only if tokenizer metadata cannot be loaded.

Acceptance:

- No OOM at default settings.
- Padded token total falls below the current ~640k on this workspace.
- Full index is faster than 71.9s on the same machine.

### Phase 3: Long-chunk splitting

Deliverables:

- Secondary splitting for chunks above a configurable token threshold.
- Parent-child chunk metadata.
- Retrieval smoke tests showing parent context remains visible.
- Token distribution report before and after splitting.

Acceptance:

- p95 model input length drops below the selected target, ideally <=768.
- Raw truncation loss is reduced.
- Search quality does not regress in existing integration tests.

### Phase 4: Backend/kernel improvement

Deliverables:

- Experiment branch with vendored fastembed Qwen3 changes:
  - `softmax_last_dim`,
  - fused RMSNorm.
- Embedding parity check:
  - compare cosine similarity for a fixed corpus,
  - record max absolute delta and mean cosine similarity.
- Throughput benchmark against baseline.

Acceptance:

- No material retrieval-quality regression.
- Clear speedup, or the branch is dropped.

### Phase 5: Optional optimized backend

Deliverables:

- Add an `EmbeddingRuntime` abstraction if needed:
  - `fastembed-candle` default,
  - optional external TEI or ONNX/TensorRT runtime.
- Keep embedder identity stable and explicit.
- Add version mismatch protection for runtime/model changes.

Acceptance:

- Default local embedded mode still works.
- Optional optimized runtime can be selected explicitly.
- Throughput moves materially closer to 20k tokens/sec.

## Experiment Matrix

Run each experiment on the same machine, same git revision, same cache state,
and same codebase path.

| Experiment | Batch policy | Long chunk split | Backend patch | Expected result |
|---|---|---|---|---|
| Baseline | fixed 32 | no | no | ~71.9s |
| A | fixed 48 | no | no | measure OOM and speed |
| B | fixed 64 | no | no | measure OOM and speed |
| C | token budget | no | no | less padding, safer large batches |
| D | token budget | 768 | no | lower attention cost |
| E | token budget | 512 | no | lower attention cost, more chunks |
| F | best D/E | yes | softmax/RMSNorm | kernel-level delta |
| G | best D/E | yes | optimized backend | target-path validation |

Metrics to record:

- total wall time,
- embedding wall time,
- chunks/sec,
- raw tokens/sec,
- capped tokens/sec,
- padded tokens/sec,
- peak VRAM,
- GPU SM utilization,
- LanceDB/Tantivy write time,
- top 10 longest chunks.

## Risks

- Larger fixed batches can OOM because Qwen3 attention stores `[B, heads, T, T]`
  tensors.
- Tokenization during batch planning adds CPU work, though it should be small
  relative to 70s of GPU inference.
- Splitting chunks can change retrieval ranking and result granularity.
- Backend patches may change floating-point results. This requires parity checks.
- External optimized runtimes increase installation complexity.

## Recommendation

Do not start with LanceDB, Tantivy, or parser work. The measured bottleneck is
embedding.

The highest-confidence path is:

1. Add tunable batch size and benchmark logs.
2. Implement token-budget batching.
3. Add long-chunk splitting around 512-768 tokens.
4. Benchmark fastembed Qwen3 hot-path patches.
5. Only then evaluate a runtime swap if 20k tokens/sec remains out of reach.

This sequence keeps the first changes small, gives immediate benchmark data, and
improves retrieval quality by reducing truncation before taking on a larger
backend replacement.

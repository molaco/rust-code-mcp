# Qwen3 Embedding GPU Optimization Implementation Report

Date: May 16, 2026

## Summary

The implementation moved local indexing for `Qwen/Qwen3-Embedding-0.6B` from an
embedding-bound ~75s path to a ~37s path on the RTX 3090 test machine while
keeping the embedded `fastembed`/Candle runtime.

Final clean benchmark:

| Metric | Value |
|---|---:|
| Total Rust files discovered | 122 |
| Files indexed | 121 |
| Files skipped | 1 sensitive file |
| Indexed chunks | 1982 |
| Total wall time | 36.58s |
| Embedding time | 34.64s |
| Embedding share | 94.69% |
| Chunks/sec | 54.2 |
| Raw token total in embedding batches | 568,943 |
| Capped token total in embedding batches | 568,943 |
| Padded token total in embedding batches | 615,268 |
| Effective padded tokens/sec | ~17,763 |
| Peak observed GPU memory | ~6.1 GB framebuffer |
| Observed GPU utilization during embedding | mostly 97-100% SM |
| Observed GPU power during embedding | mostly 304-350W |

Compared with the locked Phase 0 baseline in `.plans/gpu-opti-plan.md`:

| Metric | Phase 0 | Final |
|---|---:|---:|
| Indexed chunks | 1833 | 1982 |
| Wall time | 74.87s | 36.58s |
| Embedding time | 74.44s | 34.64s |
| Padded tokens | 639,936 | 615,268 |
| Padded tokens/sec | ~8,596 | ~17,763 |
| Chunks/sec | 24.5 | 54.2 |

That is roughly a 2.0x wall-time improvement and a 2.1x embedding-time
improvement. The result clears the plan's strong target of sub-45s wall time and
at least 14k padded tokens/sec. The aspirational 20k padded tokens/sec target is
close enough that a second runtime was deferred until there is a measured,
maintained alternative.

## Final Recommended Configuration

The defaults are now the recommended RTX 3090 configuration:

| Setting | Value |
|---|---|
| Runtime | repo-local `fastembed` 5.13.4 patch over Candle |
| Embedder identity | `fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2` |
| Model | `Qwen/Qwen3-Embedding-0.6B` |
| Vector dimension | 1024 |
| Max sequence length | 1024 |
| GPU batch size | 32 |
| Max padded tokens per batch | 32,768 |
| Chunk target | 768 tokens |
| Chunk hard max | 1024 tokens |

No environment variables are required for the recommended path:

```sh
unset RUST_CODE_MCP_EMBED_BATCH_SIZE
unset RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH
unset RUST_CODE_MCP_CHUNK_TARGET_TOKENS
unset RUST_CODE_MCP_CHUNK_HARD_MAX_TOKENS
```

For smaller GPUs, first use runtime-only throttles:

```sh
RUST_CODE_MCP_EMBED_BATCH_SIZE=16
RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=16384
```

Only if those still OOM, reduce chunk shape. This changes indexed document
content and intentionally changes the chunking cache salt:

```sh
RUST_CODE_MCP_CHUNK_TARGET_TOKENS=512
RUST_CODE_MCP_CHUNK_HARD_MAX_TOKENS=768
```

## Implemented Phases

### Phase 0: Baseline

The baseline confirmed that indexing was dominated by embedding, not parsing or
storage writes.

Recorded baseline:

| Metric | Value |
|---|---:|
| Max sequence length | 1024 |
| GPU batch size | 32 |
| Indexed chunks | 1833 |
| Raw token total | 642,901 |
| Capped token total | 555,939 |
| Padded token total | 639,936 |
| Wall time | 74.87s |
| Embedding time | 74.44s |
| Embedding share | 99.43% |
| Padded tokens/sec | ~8,596 |
| Peak observed GPU memory | ~9.6 GB |

GPU sampling showed the RTX 3090 mostly at 97-100% SM utilization and roughly
330-350W during embedding. That ruled out parser, Tantivy, LanceDB, and CPU
feeding as the primary bottlenecks.

### Phase 1: Batch Size Override

Implemented `RUST_CODE_MCP_EMBED_BATCH_SIZE` with:

- default `32`,
- rejection of zero and non-integer values,
- clamp above `256`,
- startup logging,
- no embedder identity/cache invalidation.

Benchmarking fixed batch sizes showed that larger batches regressed the current
workload:

| Batch size | Indexed chunks | Wall time | Embedding time | Chunks/sec |
|---:|---:|---:|---:|---:|
| 16 | 1841 | 70.04s | 69.69s | 26.3 |
| 32 | 1841 | 73.23s | 72.87s | 25.1 |
| 48 | 1841 | 80.49s | 80.17s | 22.9 |
| 64 | 1841 | 87.56s | 87.24s | 21.0 |

Conclusion: tuning fixed batch size is useful as a guardrail but not the main
speed path.

### Phase 2: Throughput Instrumentation

Added structured embedding logs for:

- chunks,
- sub-batches,
- configured max batch size,
- elapsed seconds,
- chunks/sec,
- formatted character range,
- token metrics availability.

Also added `examples/gpu_batch_matrix.rs` to run comparable batch-size sweeps
from clean temporary directories.

### Phase 3: Token-Length Measurement

Added `EmbeddingTokenCounter` using the same Qwen3 tokenizer path and special
token behavior as fastembed. Embedding logs and `chunk_token_stats` now report:

- raw token total,
- capped token total,
- padded token total,
- padded tokens/sec,
- min/max token length.

The first token-aware benchmark at batch size 16 produced ~9,570 padded
tokens/sec, which made later comparisons less dependent on chunk count alone.

### Phase 4: Token-Length Sorting

Changed embedding batch ordering from formatted character length to capped token
length, with original index as a deterministic tie-breaker. Output order remains
the caller's original chunk order.

Default batch-size 32 run:

| Metric | Value |
|---|---:|
| Indexed chunks | 1865 |
| Wall time | 65.68s |
| Embedding time | 65.37s |
| Padded token total | 616,128 |
| Padded tokens/sec | ~9,426 |

This reduced padding from the Phase 3 char-sorted report and improved wall time
without changing vector semantics.

### Phase 5: Token-Budget Packing

Implemented `RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH` with default `32 * 1024`
and planned sub-batches by:

```text
batch_len <= max_batch_size
batch_len * max_capped_token_len <= max_tokens_per_batch
```

Benchmark matrix:

| Batch size | Token budget | Indexed chunks | Wall time | Embedding time | Padded tokens/sec |
|---:|---:|---:|---:|---:|---:|
| 32 | 32,768 | 1875 | 71.56s | 71.22s | ~8,714 |
| 64 | 32,768 | 1875 | 78.36s | 78.03s | ~8,438 |
| 64 | 49,152 | 1875 | 83.28s | 82.96s | ~8,191 |
| 96 | 49,152 | 1875 | 81.50s | 81.15s | ~8,642 |

The planner works and prevents accidental oversized batches, but larger token
budgets did not help this workload. The conservative default remained `32 /
32768`.

### Phase 6: Quadratic Budget

No code was added. Phase 5 showed that larger token budgets and item-count
ceilings regressed wall time, so carrying a disabled quadratic attention budget
would have added complexity without measured benefit.

### Phase 7: Oversized Chunk Splitting

Implemented token-aware oversized chunk splitting:

- `RUST_CODE_MCP_CHUNK_TARGET_TOKENS`, default `768`,
- `RUST_CODE_MCP_CHUNK_HARD_MAX_TOKENS`, default `1024`,
- `ChunkSplitConfig`,
- parent context in `ChunkContext`,
- split-part metadata for leaf splits,
- `chunk-split:v1:target{target}:hard{hard}` metadata-cache salt,
- production splitter applied before embedding and in token stats.

Token stats after splitting:

| Metric | Value |
|---|---:|
| Raw parsed chunks | 1922 |
| Split chunks | 1991 |
| Raw token total | 569,777 |
| Capped token total | 569,777 |
| Max raw tokens | 792 |
| p95 | 767 |
| p99 | 776 |
| Chunks above 1024 | 0 |

Full benchmark:

| Metric | Value |
|---|---:|
| Indexed chunks | 1973 |
| Wall time | 61.95s |
| Embedding time | 60.05s |
| Padded tokens | 612,947 |
| Padded tokens/sec | ~10,208 |

This removed truncation loss on the workspace and improved retrieval input
quality, but did not yet reach the sub-60s target.

### Phase 8: fastembed Qwen3 Patch

Vendored `fastembed` 5.13.4 under `vendor/fastembed` and patched only Qwen3
hot spots:

- attention now calls `candle_nn::ops::softmax_last_dim(&attn)`,
- RMSNorm now calls `candle_nn::ops::rms_norm` for contiguous tensors,
- the original manual RMSNorm remains as a non-contiguous fallback.

Added `examples/qwen3_parity_probe.rs` to compare a fixed upstream snapshot
against the patched runtime.

Parity results:

| Corpus | Vectors | Min cosine | Mean cosine | Max abs delta | Mean abs delta |
|---|---:|---:|---:|---:|---:|
| Documents | 30 | 0.999993464 | 0.999995310 | 0.000549316 | 0.000074970 |
| Queries | 5 | 0.999990430 | 0.999992007 | 0.000518799 | 0.000098410 |

Phase 8 benchmark before the final identity bump:

| Metric | Value |
|---|---:|
| Indexed chunks | 1974 |
| Wall time | 37.36s |
| Embedding time | 35.43s |
| Padded tokens | 613,036 |
| Padded tokens/sec | ~17,301 |

The backend patch produced the largest single speedup, cutting embedding time
from 60.05s to 35.43s on the same chunk shape.

### Phase 9: Optimized Runtime Decision

No external runtime abstraction was added. The embedded Candle path reached the
strong-result band after Phase 8, and the remaining gap to 20k padded tokens/sec
did not justify adding TEI, ONNX Runtime, TensorRT, deployment config, and
runtime identity handling before proving a concrete win.

Follow-up trigger: reopen this phase if the repo grows enough that wall time
returns above 45s or if a maintained Qwen3 runtime can beat the Phase 8 path
with acceptable operational complexity.

### Phase 10: Final Documentation and Identity Protection

Updated `.docs/gpu-opti-proposal.md` with final defaults, benchmark numbers,
fallback settings, and cache behavior.

Added this report.

Also bumped the embedder identity suffix from `v1` to `v2`:

```text
fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2
```

The backend patch has very small numerical drift but is still a different
vector-producing implementation. The `v2` identity prevents vector stores built
with upstream fastembed Qwen3 from being silently reused with the patched path.

## Cache Invalidation

Runtime-only, non-semantic knobs:

- `RUST_CODE_MCP_EMBED_BATCH_SIZE`
- `RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH`

These only alter batch shape and scheduling. They are not included in embedder
identity.

Chunk-shape semantic knobs:

- `RUST_CODE_MCP_CHUNK_TARGET_TOKENS`
- `RUST_CODE_MCP_CHUNK_HARD_MAX_TOKENS`

These change formatted chunk content and therefore use the metadata-cache salt:

```text
chunk-split:v1:target{target}:hard{hard}
```

Vector-producing semantic identity:

- Qwen3 variant,
- vector dimension,
- max sequence length,
- backend implementation identity suffix.

The final identity is `v2` because the repo now carries a patched Qwen3
fastembed implementation.

## Verification

Commands run during the final phase:

```sh
jj show --summary
RUSTFLAGS='-C link-arg=-fuse-ld=bfd' cargo build --release --example index_codebase --example chunk_token_stats
./target/release/examples/chunk_token_stats
nvidia-smi dmon -s pucm -d 1 -c 70
/home/molaco/Documents/rust-code-mcp-final/target/release/examples/index_codebase
```

Final benchmark command was run from a clean temporary directory:

```text
/tmp/rust-code-mcp-gpu-bench-final-h2WFJI
```

The release build used the bfd linker override because the default `rust-lld`
path repeatedly failed in this repo with a corrupted `.eh_frame` error. No
formatting command was run.

`cargo check --lib` passed after this report update and is recorded in the
Phase 10 plan entry.

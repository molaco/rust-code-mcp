# Step-by-step plan: optimize Qwen3 embedding GPU throughput

## Goal

Move `Qwen/Qwen3-Embedding-0.6B` indexing throughput from the current
~9k padded model tokens/sec toward 20k tokens/sec without changing embedding
semantics by accident.

The current benchmark in `.docs/gpu-opti-proposal.md` shows the indexing run is
embedding-bound:

- 118 files indexed.
- 1833 chunks embedded.
- 71.93s total wall time.
- 71.57s embedding time.
- 99.5% of runtime spent in embedding.
- RTX 3090 near 100% SM utilization during embedding.

This plan focuses first on batch shape and chunk shape, then moves to backend
kernel work only if the simpler changes do not get close enough to the target.

## Constraints

- Use `jj status` before and after major steps.
- Do not run `cargo fmt` or any formatting command.
- Keep changes small and compile-checkable.
- Do not invalidate existing embedding caches unless embedding semantics change.
- Treat batch size, token-budget packing, and instrumentation as non-semantic.
- Treat model variant, max sequence length, instruction format, runtime backend,
  and vector dimension as semantic cache identity inputs.

## Baseline commands

Use these commands before the first implementation step and after each
performance-related phase:

```sh
jj status
./target/release/examples/chunk_token_stats
./target/release/examples/index_codebase
```

For GPU monitoring during `index_codebase`, run this in a second terminal:

```sh
nvidia-smi dmon -s pucm -d 1
```

If existing binaries are stale or missing, build only the needed target. Do not
run formatting.

## Phase 0: lock down the baseline

Status: completed on May 16, 2026.

Evidence:

- `jj show --summary` ran before the phase.
- `jj status` showed a clean working copy before benchmark changes.
- `./target/release/examples/chunk_token_stats` completed in 955.26ms.
- `./target/release/examples/index_codebase` completed successfully.
- `nvidia-smi dmon -s pucm -d 1 -c 90` captured GPU utilization during the
  indexing run.

Recorded baseline:

| Metric | Value |
|---|---:|
| Qwen3 variant | `Qwen3-Embedding-0.6B` |
| Embedder identity | `fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v1` |
| Max sequence length | 1024 |
| GPU batch size | 32 |
| CUDA device | RTX 3090 |
| Vector dimension | 1024 |
| Parsed chunks | 1852 |
| Indexed chunks | 1833 |
| Raw token total | 642,901 |
| Capped token total | 555,939 |
| Padded token total, batch 32 | 639,936 |
| Index wall time | 74.87s |
| Embedding time | 74.44s |
| Embedding share | 99.43% |
| Parse time | 0.075s |
| Index/write time | 0.142s |
| Effective chunks/sec | 24.5 |
| Effective padded tokens/sec | ~8,596 |
| Peak observed GPU memory | ~9.6 GB |
| Observed GPU SM utilization | mostly 97-100% during embedding |
| Observed GPU power | mostly 330-350W during embedding |

1. Confirm the working copy state:

   ```sh
   jj status
   ```

2. Record current embedding configuration:

   - Qwen3 variant.
   - `max_len`.
   - `gpu_batch_size`.
   - CUDA device name.
   - vector dimension.

3. Run the current token distribution report:

   ```sh
   ./target/release/examples/chunk_token_stats
   ```

4. Run a full local indexing benchmark:

   ```sh
   ./target/release/examples/index_codebase
   ```

5. Save the baseline values in a local note or benchmark artifact:

   - wall time,
   - embedding time,
   - chunks/sec,
   - raw token total,
   - capped token total,
   - padded token total,
   - padded tokens/sec,
   - peak GPU memory,
   - average GPU SM utilization.

6. Do not change production code in this phase.

Acceptance:

- Baseline numbers are reproducible and comparable with the proposal numbers.
- We can calculate padded tokens/sec for every later run.

## Phase 1: make embedding batch size tunable

Status: completed on May 16, 2026.

Implemented:

- Added `RUST_CODE_MCP_EMBED_BATCH_SIZE`.
- Kept the default batch size at `32`.
- Added parsing that rejects non-integer values and `0`.
- Added a conservative maximum of `256`; larger requested values are clamped.
- Applied the override in `IndexerCoreConfig::with_env_overrides()` during
  `IndexerCore` construction.
- Left batch size out of embedder identity/cache identity.
- Added parser unit coverage for valid, clamped, zero, and non-integer values.

Verification:

- `jj show --summary` ran before the phase.
- `cargo check --lib` passed with existing warnings.
- `cargo build --release --example index_codebase` passed with existing
  warnings.
- The release benchmark logs showed the override being applied, for example
  `gpu_batch_size=64` and `EmbeddingBatcher configured with GPU embedding batch
  size: 64`.
- `cargo test test_gpu_batch_size_override_parser --lib` was attempted, but the
  test-profile cargo process hung with a defunct `rustc` child and was killed
  after it produced no useful output. The parser test remains in the code.

Benchmark matrix from a temp working directory:

| `RUST_CODE_MCP_EMBED_BATCH_SIZE` | Indexed chunks | Wall time | Embedding time | Chunks/sec | Result |
|---:|---:|---:|---:|---:|---|
| 16 | 1841 | 70.04s | 69.69s | 26.3 | fastest in this run |
| 32 | 1841 | 73.23s | 72.87s | 25.1 | current default |
| 48 | 1841 | 80.49s | 80.17s | 22.9 | slower |
| 64 | 1841 | 87.56s | 87.24s | 21.0 | slower |

Conclusion:

- Runtime batch-size tuning works.
- Larger fixed batches do not help this workload; they make the current
  char-length batching worse.
- Phase 2/3/5 should focus on measuring and reducing padded-token waste rather
  than raising fixed batch size globally.

Target files:

- `src/config/indexer.rs`
- `src/indexing/embedding_batcher.rs`
- any MCP/indexer option plumbing that constructs `IndexerCoreConfig`

Steps:

1. Add an env-var override for the existing fixed batch size:

   ```text
   RUST_CODE_MCP_EMBED_BATCH_SIZE
   ```

2. Keep the current default at `32`.

3. Reject invalid values:

   - missing env var: use default,
   - non-integer: return a config error or log and use default, matching existing
     config style,
   - `0`: reject or clamp away from zero,
   - extremely high values: cap to a conservative upper bound such as `256`.

4. Log the resolved embedding batch size at indexer startup.

5. Do not include batch size in the embedder identity or cache version.

6. Add unit coverage for env parsing if config code already has similar tests.

7. Run checks:

   ```sh
   cargo check --lib
   ```

8. Benchmark these values:

   ```sh
   RUST_CODE_MCP_EMBED_BATCH_SIZE=16 ./target/release/examples/index_codebase
   RUST_CODE_MCP_EMBED_BATCH_SIZE=32 ./target/release/examples/index_codebase
   RUST_CODE_MCP_EMBED_BATCH_SIZE=48 ./target/release/examples/index_codebase
   RUST_CODE_MCP_EMBED_BATCH_SIZE=64 ./target/release/examples/index_codebase
   ```

Acceptance:

- Default behavior remains batch size 32.
- Larger batch sizes change runtime only, not generated vectors.
- At least one benchmark result shows whether fixed larger batches help on the
  RTX 3090.

Rollback criteria:

- Any non-default batch size causes frequent OOM.
- Larger fixed batches do not improve throughput and create noisy failures.

## Phase 2: add embedding throughput instrumentation

Status: completed on May 16, 2026.

Implemented:

- Added structured `Embedding batch plan` logs from `EmbeddingBatcher` with:
  - chunks,
  - sub-batches,
  - configured max embedding batch size,
  - min/max formatted character length,
  - `token_metrics_available=false`.
- Added structured `Embedding batcher completed document embeddings` logs with:
  - chunks,
  - sub-batches,
  - configured max embedding batch size,
  - elapsed seconds,
  - chunks/sec,
  - min/max formatted character length,
  - `token_metrics_available=false`.
- Added `examples/gpu_batch_matrix.rs`, a helper that runs the release
  `index_codebase` sibling binary from temporary directories, applies
  `RUST_CODE_MCP_EMBED_BATCH_SIZE`, parses the metrics summary, and prints a
  compact comparison table.

Verification:

- `jj show --summary` ran before the phase.
- `cargo check --lib` passed with existing warnings.
- `cargo build --release --example index_codebase --example gpu_batch_matrix`
  passed with existing warnings.
- `./target/release/examples/gpu_batch_matrix 16` completed one full helper
  benchmark and printed:
  - 1848 chunks,
  - 66.71s index wall time,
  - 66.35s embedding time,
  - 27.7 chunks/sec,
  - 67.61s child process wall time.
- A direct `RUST_CODE_MCP_EMBED_BATCH_SIZE=16 index_codebase` run showed the new
  structured logs. Example first batch:
  - chunks: 599,
  - sub-batches: 38,
  - configured max batch size: 16,
  - min/max chars: 147..27327,
  - elapsed: 19.57s,
  - chunks/sec: 30.6.

Notes:

- Token-level metrics are intentionally marked unavailable here. Phase 3 wires
  tokenizer-backed token lengths into the hot path.
- The new benchmark helper supports the default `16 32 48 64` matrix, but the
  phase verification used a one-size smoke run to avoid repeating the full
  Phase 1 matrix.

Target files:

- `src/indexing/unified.rs`
- `src/indexing/embedding_batcher.rs`
- `src/embeddings/qwen3.rs`
- existing benchmark examples under `examples/`

Steps:

1. Extend embedding logs to report:

   - chunks embedded,
   - number of embedding sub-batches,
   - configured max batch size,
   - embedding seconds,
   - chunks/sec.

2. Add token accounting where token lengths are available:

   - raw token total,
   - capped token total,
   - padded token total,
   - padded tokens/sec,
   - longest sequence per sub-batch.

3. If tokenizer access is not available yet, add the logging fields behind an
   optional path and leave them as unavailable until Phase 3.

4. Add a small benchmark helper or ignored test that prints a comparable table
   for batch sizes `16`, `32`, `48`, and `64`.

5. Run checks:

   ```sh
   cargo check --lib
   ```

6. Run the benchmark helper and one full `index_codebase` pass.

Acceptance:

- Every benchmark can report padded tokens/sec, not only chunks/sec.
- The instrumentation does not change embedding output or cache identity.
- Full benchmark output is enough to compare runs without manual log scraping.

## Phase 3: implement token-length measurement

Target files:

- `src/indexing/embedding_batcher.rs`
- `src/embeddings/qwen3.rs`
- a new helper module if needed, such as `src/embeddings/token_lengths.rs`

Steps:

1. Find the least invasive way to access the same tokenizer used by fastembed.

2. Add a helper that returns both raw and capped token length:

   ```rust
   struct EmbeddingTextLen {
       raw_tokens: usize,
       capped_tokens: usize,
   }
   ```

3. Use the active backend `max_len` for the cap.

4. Keep the helper deterministic and independent from GPU execution.

5. Add unit tests with representative inputs:

   - empty string,
   - short code snippet,
   - long repeated snippet,
   - unicode text if the tokenizer path already supports it.

6. Do not change batching behavior yet.

7. Run checks:

   ```sh
   cargo check --lib
   ```

8. Run:

   ```sh
   ./target/release/examples/chunk_token_stats
   ```

Acceptance:

- Token counts match the model tokenizer path used for Qwen3.
- Phase 2 instrumentation can print raw, capped, and padded token totals.
- No change in batch ordering or embeddings yet.

Risk notes:

- If fastembed does not expose tokenizer access cleanly, prefer a small local
  tokenizer wrapper over reflection or source parsing.
- If loading a second tokenizer is necessary, cache it once and keep it off the
  hot GPU path.

## Phase 4: replace char-length sorting with token-length sorting

Target file:

- `src/indexing/embedding_batcher.rs`

Steps:

1. Replace the current character-length sort key with capped token length.

2. Preserve deterministic ordering for equal token lengths by keeping the
   original chunk index as a tie-breaker.

3. Preserve the final output order expected by callers.

4. Keep fixed-size batches for this phase:

   ```text
   sort by capped_token_len -> chunks(gpu_batch_size)
   ```

5. Add unit tests for:

   - stable output order,
   - equal-length tie behavior,
   - padded-token reduction on synthetic input,
   - empty input.

6. Run checks:

   ```sh
   cargo check --lib
   ```

7. Benchmark:

   ```sh
   ./target/release/examples/index_codebase
   ```

Acceptance:

- Embedding results are returned in the same order as input chunks.
- Padded token total is less than or equal to the current char-sort baseline.
- Full index time does not regress.

Rollback criteria:

- Tokenization overhead is visible enough to erase batching gains.
- Ordering bugs appear in vector/document pairing tests.

## Phase 5: add token-budget batch packing

Target files:

- `src/config/indexer.rs`
- `src/indexing/embedding_batcher.rs`

Steps:

1. Add config for a padded-token budget:

   ```text
   RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH
   ```

2. Keep fixed max item count from `RUST_CODE_MCP_EMBED_BATCH_SIZE`.

3. Start with a conservative default equivalent to the current behavior:

   ```text
   32 * 1024 = 32768 padded tokens
   ```

4. Implement a planner that packs sorted chunks into sub-batches by:

   ```text
   batch_len <= max_batch_size
   batch_len * max_capped_token_len <= max_tokens_per_batch
   ```

5. Keep each individual chunk embeddable even if it alone exceeds the budget
   after capping.

6. Return a planning summary:

   - sub-batch count,
   - max batch length,
   - max padded tokens in a batch,
   - total padded tokens.

7. Add unit tests over synthetic token lengths:

   - many short chunks pack above 32 only if max batch size allows it,
   - one long chunk stays alone when needed,
   - mixed short and long chunks do not exceed budget,
   - zero or missing budget falls back to default.

8. Run checks:

   ```sh
   cargo check --lib
   ```

9. Benchmark matrix:

   ```sh
   RUST_CODE_MCP_EMBED_BATCH_SIZE=32 RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=32768 ./target/release/examples/index_codebase
   RUST_CODE_MCP_EMBED_BATCH_SIZE=64 RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=32768 ./target/release/examples/index_codebase
   RUST_CODE_MCP_EMBED_BATCH_SIZE=64 RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=49152 ./target/release/examples/index_codebase
   RUST_CODE_MCP_EMBED_BATCH_SIZE=96 RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=49152 ./target/release/examples/index_codebase
   ```

Acceptance:

- No OOM at default settings.
- Padded token total drops below the current ~640k baseline.
- Full index time improves over 71.9s on the same machine.
- Best config is documented in `.docs/gpu-opti-proposal.md` or a follow-up
  benchmark report.

## Phase 6: optional quadratic attention budget

Target file:

- `src/indexing/embedding_batcher.rs`

Steps:

1. Add an optional internal planner limit:

   ```text
   batch_len * max_capped_token_len * max_capped_token_len <= max_attention_budget
   ```

2. Keep it disabled by default until benchmarked.

3. Add tests showing that pathological long batches split earlier than the
   linear token budget would split them.

4. Benchmark on the same workspace with the best Phase 5 settings.

Acceptance:

- The quadratic budget improves wall time or prevents OOM for mixed long-batch
  inputs.
- If it does not help, leave the code out rather than carrying unused config.

## Phase 7: split oversized chunks before embedding

Target areas:

- AST chunk creation code.
- chunk metadata structs.
- chunk formatting code used before embedding.
- search result presentation if parent context needs to be shown.

Steps:

1. Add config values:

   ```text
   RUST_CODE_MCP_CHUNK_TARGET_TOKENS=768
   RUST_CODE_MCP_CHUNK_HARD_MAX_TOKENS=1024
   ```

2. Start with target `768` and hard max `1024`.

3. Detect chunks whose formatted embedding text exceeds the hard max or target.

4. Split oversized chunks at semantic boundaries first:

   - methods inside impl blocks,
   - functions inside modules,
   - test functions inside test modules,
   - item boundaries inside large modules.

5. Use line or blank-line boundaries only as fallback.

6. Preserve parent context in child chunks:

   - file path,
   - parent symbol,
   - child symbol,
   - line range,
   - chunk kind.

7. Add metadata to connect child chunks back to the parent symbol.

8. Update formatting so the embedded text includes enough parent context for
   retrieval.

9. Add tests for:

   - large impl split into method chunks,
   - large test module split into test-function chunks,
   - child chunks retain parent symbol metadata,
   - no split for chunks under target,
   - stable chunk IDs if the surrounding file did not change materially.

10. Run checks:

    ```sh
    cargo check --lib
    ```

11. Run token stats before and after:

    ```sh
    ./target/release/examples/chunk_token_stats
    ```

12. Run a full benchmark:

    ```sh
    ./target/release/examples/index_codebase
    ```

Acceptance:

- p95 model input length drops to <=768 if that target is selected.
- Raw truncation loss is materially reduced.
- Full index time improves despite a possible increase in chunk count.
- Search results still show enough parent context to be useful.

Rollback criteria:

- Chunk count grows enough to erase attention-cost savings.
- Search output becomes too fragmented without parent context.
- Stable chunk identity becomes too noisy for incremental indexing.

## Phase 8: patch fastembed Qwen3 hot spots

Target area:

- vendored or patched fastembed Qwen3 implementation.

Steps:

1. Create an experiment branch or jj change dedicated to backend patching.

2. Vendor or patch only the Qwen3 implementation needed for measurement.

3. Replace generic softmax with last-dim softmax:

   ```rust
   candle_nn::ops::softmax_last_dim(&attn)
   ```

4. Replace manual RMSNorm math with Candle's fused RMSNorm op if the signature
   and dtype behavior match.

5. Add a parity benchmark using a fixed corpus:

   - 10 short code snippets,
   - 10 medium snippets,
   - 10 long snippets,
   - representative natural-language queries.

6. Compare patched vs baseline vectors:

   - cosine similarity,
   - max absolute delta,
   - mean absolute delta,
   - top-k retrieval overlap on a small fixed index.

7. Run checks:

   ```sh
   cargo check --lib
   ```

8. Run full indexing benchmarks with the best batch planner from earlier
   phases.

Acceptance:

- Mean cosine similarity against baseline is effectively unchanged.
- Top-k retrieval overlap is unchanged or explainably equivalent.
- Throughput improves enough to justify carrying the patch.

Rollback criteria:

- Speedup is marginal.
- Numerical drift changes retrieval results in a meaningful way.
- The patch makes fastembed upgrades too expensive.

## Phase 9: evaluate an optimized runtime only if needed

Target area:

- embedding backend abstraction.
- deployment docs.
- cache identity and version checks.

Steps:

1. Decide whether the Phase 1-8 result is close enough to 20k tokens/sec.

2. If not, prototype one optimized runtime behind an explicit selection:

   - Hugging Face Text Embeddings Inference,
   - ONNX Runtime with an optimized Qwen3 export,
   - TensorRT,
   - another fused-attention runtime.

3. Add an `EmbeddingRuntime` abstraction only if the prototype proves faster.

4. Keep `fastembed-candle` as the default local embedded runtime.

5. Include runtime choice in embedder identity.

6. Add startup checks that refuse to open indexes built with a different
   runtime identity.

7. Benchmark the optimized runtime against the best in-process Candle path.

Acceptance:

- Default local mode still works without external services.
- Optimized runtime is selected explicitly.
- Runtime identity prevents accidental cross-runtime cache reuse.
- Throughput moves materially closer to, or past, 20k tokens/sec.

## Phase 10: document final settings

Target files:

- `.docs/gpu-opti-proposal.md`
- a new benchmark report under `.docs/reports/` if useful
- README or user-facing config docs if these knobs become supported

Steps:

1. Record the best measured configuration:

   - batch size,
   - token budget,
   - chunk target,
   - backend/runtime,
   - wall time,
   - embedding time,
   - padded tokens/sec,
   - chunks/sec,
   - GPU memory.

2. Document recommended env vars.

3. Document fallback settings for smaller GPUs.

4. Document which knobs do and do not invalidate caches.

5. Run final checks:

   ```sh
   jj status
   cargo check --lib
   ```

6. Do not run `cargo fmt`.

Acceptance:

- The final benchmark can be repeated by another developer.
- The docs identify the best default and the best RTX 3090 tuning.
- Cache invalidation behavior is explicit.

## Suggested implementation order

1. Phase 0: baseline.
2. Phase 1: env-tunable batch size.
3. Phase 2: throughput instrumentation.
4. Phase 3: token-length measurement.
5. Phase 4: token-length sorting.
6. Phase 5: token-budget packing.
7. Phase 7: long-chunk splitting.
8. Phase 8: fastembed hot-spot patch.
9. Phase 9: optimized runtime only if still needed.
10. Phase 10: final docs.

Phase 6 is optional and should be added only if the Phase 5 planner still forms
expensive long-sequence batches.

## Success criteria

Minimum useful result:

- Full index time drops below 60s on the same RTX 3090.
- Padded token waste is lower than the current ~640k baseline.
- Existing retrieval tests keep passing.

Strong result:

- Full index time drops below 45s.
- Throughput reaches at least 14k padded tokens/sec.
- Long chunks are no longer heavily truncated.

Target result:

- Throughput reaches roughly 20k padded tokens/sec.
- Full index time approaches 30-35s for this workspace.
- The path to the result is documented and reproducible.

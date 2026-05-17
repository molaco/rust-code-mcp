# API Batch Implementation Plan

## Goal

Improve `openrouter-qwen3-8b` bulk indexing throughput by reducing sequential OpenRouter request overhead while preserving the local GPU and local CPU behavior.

Current benchmark baseline from `.docs/api-batch-proposal.md`:

| profile | chunks | embedding time | padded tokens/sec | request count |
|---|---:|---:|---:|---:|
| `local-gpu-small` | 2084 | 33.17s | 19365.1 | local |
| `openrouter-qwen3-8b` | 2084 | 138.05s | 4573.1 | about 68 |

The target is to move OpenRouter toward `35s` to `60s` embedding time by using larger remote batches, limited concurrency, and better request metrics.

## Investigation Summary

External API checks:

- OpenRouter embeddings accepts batch input arrays, `dimensions`, `encoding_format`, `input_type`, and `provider` in `POST /api/v1/embeddings`.
- `encoding_format` allows `float` and `base64`.
- Provider routing supports `sort` values including `throughput` and `latency`.
- `preferred_min_throughput` and `preferred_max_latency` are preferences, not guarantees.

Sources:

- https://openrouter.ai/docs/api/api-reference/embeddings/create-embeddings
- https://openrouter.ai/docs/api/reference/embeddings
- https://openrouter.ai/docs/guides/routing/provider-selection

Local code findings:

- `src/embeddings/openrouter.rs` sends one HTTP request at a time from `embed_with_split`.
- `src/embeddings/openrouter.rs` hardcodes `encoding_format: "float"`.
- `src/indexing/embedding_batcher.rs` applies the local GPU-oriented planner to every backend using `gpu_batch_size` and `max_tokens_per_batch`.
- `src/config/indexer.rs` only exposes the shared local batch env vars:
  - `RUST_CODE_MCP_EMBED_BATCH_SIZE`
  - `RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH`
- `examples/gpu_batch_matrix.rs` only varies `RUST_CODE_MCP_EMBED_BATCH_SIZE`, so it cannot test OpenRouter request sizing or concurrency.
- Existing dependencies already include `tokio`, `reqwest`, `serde`, `serde_json`, and `futures`; float-only batching should not require Cargo or Nix changes.

## Guardrails

1. Do not change local GPU batch behavior except for a small branch that bypasses local pre-splitting when the backend is OpenRouter.
2. Do not change Nix shell files for the float batching work.
3. Do not add dependencies for Phase 1-5.
4. Do not run `cargo fmt` or any formatter.
5. Before any build or test command, ask which shell to use and run it as:

```sh
nix develop ../nix-devshells#<shell> --command <command>
```

6. Never log API keys. Error bodies must remain snippets only.

## Phase 1: Add OpenRouter Runtime Config

Status: Implemented.

Implementation notes:

- Added OpenRouter-specific runtime config in `src/embeddings/openrouter.rs`.
- Added env vars:
  - `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS`
  - `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS`
  - `RUST_CODE_MCP_OPENROUTER_CONCURRENCY`
  - `RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT`
- Defaults are `128` inputs, `131072` tokens, concurrency `4`, and `float` encoding.
- Invalid numeric values fall back to defaults.
- Over-large numeric values clamp to configured caps.
- Unsupported encoding values currently fall back to `float`; `base64` remains reserved for Phase 8.
- `OpenRouterEmbedder::new` logs the resolved config once and `request_batch` now reads `encoding_format` from the config.
- Added unit coverage for defaults, valid overrides, invalid values, unsupported encoding, and clamping.

Verification notes:

- Source review completed.
- Cargo tests were not run in this phase because the Nix build shell has not been confirmed for this execution.

Files:

- `src/embeddings/openrouter.rs`
- tests in `src/embeddings/openrouter.rs`
- optionally `src/embeddings/mod.rs` if a small internal type must be re-exported inside the crate

Implementation steps:

1. Add constants:
   - `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS`
   - `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS`
   - `RUST_CODE_MCP_OPENROUTER_CONCURRENCY`
   - `RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT`
2. Add `OpenRouterRuntimeConfig`.
3. Parse config during `OpenRouterEmbedder::new`.
4. Use conservative defaults:
   - `max_batch_inputs = 128`
   - `max_batch_tokens = 131072`
   - `concurrency = 4`
   - `encoding_format = float`
5. Clamp invalid high values and warn on invalid values.
6. Treat unsupported `encoding_format` values as invalid and fall back to `float`.
7. Log the resolved OpenRouter config once at backend initialization.

Suggested caps:

| field | default | minimum | maximum |
|---|---:|---:|---:|
| `max_batch_inputs` | 128 | 1 | 512 |
| `max_batch_tokens` | 131072 | 1 | 1048576 |
| `concurrency` | 4 | 1 | 16 |

Acceptance criteria:

- Invalid env values do not panic.
- Local batch env vars remain unchanged.
- Unit tests cover default config, valid overrides, zero/invalid overrides, and clamping.

## Phase 2: Add Remote Batch Planning

Status: Implemented.

Implementation notes:

- Added `OpenRouterInput`, `OpenRouterBatchPlan`, and `OpenRouterInputBatch` in `src/embeddings/openrouter.rs`.
- Added remote planner logic that sorts inputs by estimated token length and original index.
- Remote plans now respect `max_batch_inputs` and padded `max_batch_tokens`.
- Oversized single inputs are kept as single-item batches so payload-too-large handling can produce the final split/error behavior.
- Added tokenizer-backed token length estimation through `EmbeddingTokenCounter`.
- Added deterministic fallback estimation based on text length when tokenizer loading or counting is unavailable.
- Added `restore_original_embedding_order` helper for Phase 3 request execution.
- Added planner unit coverage for sorting, input-count limits, token-budget limits, oversized inputs, original order restoration, benchmark-shaped request count, and fallback token estimation.

Verification notes:

- Source review completed.
- Cargo tests were not run in this phase because the Nix build shell has not been confirmed for this execution.

Files:

- `src/embeddings/openrouter.rs`
- tests in `src/embeddings/openrouter.rs`

Implementation steps:

1. Add an internal remote input type:

```rust
struct OpenRouterInput {
    original_index: usize,
    text: String,
    token_len: usize,
}
```

2. Add a planner that sorts by `token_len` and original index.
3. Build request plans using:
   - `max_batch_inputs`
   - `max_batch_tokens`
4. Preserve stable original ordering in final output.
5. Estimate token length with the Qwen tokenizer when available.
6. Fall back to a deterministic text-length estimate if tokenizer loading fails.
7. Keep oversized single inputs as single-item requests so existing payload-too-large handling can produce the final error.

Acceptance criteria:

- Planner reduces request count for the benchmark shape from about 68 to roughly 8-20 before split retries.
- Planner tests verify count limits, token limits, sorting, oversized single inputs, and final order restoration.

## Phase 3: Execute Remote Requests Concurrently

Status: Implemented.

Implementation notes:

- Replaced OpenRouter's top-level sequential request loop with the Phase 2 remote planner plus bounded `futures::stream::buffer_unordered`.
- `RUST_CODE_MCP_OPENROUTER_CONCURRENCY` now controls the number of planned OpenRouter requests in flight.
- Added `request_batch_with_split`, which preserves existing retry behavior through `request_batch`.
- Payload-too-large handling still recursively splits only the offending request batch.
- Completed request results are collected as `(original_index, embedding)` pairs and restored to the caller's input order.
- Empty input still returns an empty embedding list.
- Added an OpenRouter request-plan info log with input count, planned request count, concurrency, and batch limits.

Verification notes:

- Source review completed.
- Cargo tests were not run in this phase because the Nix build shell has not been confirmed for this execution.

Files:

- `src/embeddings/openrouter.rs`

Implementation steps:

1. Replace the sequential `VecDeque` top-level loop in `embed_with_split` with planned request execution.
2. Use `futures::stream::iter(...).buffer_unordered(config.concurrency)` for request concurrency.
3. Keep the existing retry policy for `429`, `5xx`, and `529`.
4. Keep payload-too-large splitting inside each planned request.
5. Make each completed request return `(original_index, embedding)` pairs.
6. Fill a `Vec<Option<Embedding>>` by original index.
7. Fail if any original index is missing.

Acceptance criteria:

- OpenRouter requests are in flight concurrently up to the configured limit.
- Result order matches input order exactly.
- Existing response parsing checks still pass.
- Payload-too-large still recursively splits and retries smaller requests.

## Phase 4: Bypass Local GPU Pre-Splitting For OpenRouter

Status: Planned.

Files:

- `src/indexing/embedding_batcher.rs`

Implementation steps:

1. In `EmbeddingBatcher::generate_embeddings_batched`, branch on:

```rust
self.embedding_generator.backend().runtime == EmbeddingRuntime::OpenRouter
```

2. For OpenRouter, format all chunks and call `embedding_generator.embed_documents(chunk_texts)` once for the current indexing batch.
3. Let `OpenRouterEmbedder` handle remote sub-batching and concurrency.
4. Keep local GPU and local CPU on the existing `plan_embedding_batches` path.
5. Keep the current token summary log for all providers where possible.
6. Rename log labels so OpenRouter is not reported as `Embedding GPU sub-batch`.

Acceptance criteria:

- `local-gpu-small` still uses `RUST_CODE_MCP_EMBED_BATCH_SIZE` and `RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH`.
- `openrouter-qwen3-8b` does not depend on `RUST_CODE_MCP_EMBED_BATCH_SIZE`.
- OpenRouter request count is controlled by `RUST_CODE_MCP_OPENROUTER_*` config.

## Phase 5: Add OpenRouter Request Metrics

Status: Planned.

Files:

- `src/embeddings/openrouter.rs`
- `examples/index_codebase.rs`
- optionally `src/metrics/mod.rs` only if metrics need to be surfaced outside logs

Implementation steps:

1. Track per OpenRouter embedding call:
   - request count
   - retry count
   - payload-too-large split count
   - failed request count
   - total request latency
   - min/max/average request latency
   - input count per request
   - estimated tokens per request
   - response vector count
   - response dimension
2. Emit a single structured `tracing::info!` summary after `embed_with_split`.
3. Emit per-request debug logs with request index, input count, estimated tokens, latency, status, and retry attempt.
4. Add stable `key=value` fields so benchmark examples can parse them from stderr.
5. Update `examples/index_codebase.rs` to print resolved OpenRouter config values in machine-readable form when the active profile is OpenRouter.
6. Keep API keys out of all logs and machine metrics.

Initial machine metrics:

```text
openrouter_max_batch_inputs=128
openrouter_max_batch_tokens=131072
openrouter_concurrency=4
openrouter_encoding_format=float
openrouter_request_count=<n>
openrouter_retry_count=<n>
openrouter_split_count=<n>
openrouter_avg_request_latency_secs=<n>
```

Acceptance criteria:

- A benchmark run explains whether time is dominated by request count, per-request latency, retries, or throttling.
- `examples/gpu_batch_matrix.rs` can keep working for local GPU.
- API keys are never printed.

## Phase 6: Add OpenRouter Batch Matrix Benchmark

Status: Planned.

Files:

- new `examples/openrouter_batch_matrix.rs`
- `examples/index_codebase.rs`

Implementation steps:

1. Add a separate benchmark example instead of overloading `gpu_batch_matrix.rs`.
2. The benchmark should run `index_codebase --profile openrouter-qwen3-8b` with a temp working directory.
3. If no OpenRouter key is present, print `openrouter_benchmark=skipped_missing_api_key` and exit successfully.
4. Sweep:

```text
max_batch_inputs: 32, 64, 128
max_batch_tokens: 32768, 65536, 131072
concurrency: 1, 2, 4, 8
encoding_format: float
```

5. Parse machine metrics from `index_codebase`.
6. Print a markdown table with:
   - batch inputs
   - batch tokens
   - concurrency
   - chunks
   - embedding time
   - request count
   - retry count
   - split count
   - average request latency
   - padded tokens/sec
   - child wall time
7. Support CLI filters so smaller runs can be done first:

```sh
openrouter_batch_matrix --inputs 64,128 --tokens 65536,131072 --concurrency 2,4
```

Acceptance criteria:

- Benchmark is OpenRouter-specific.
- Missing API key does not fail local developer checks.
- Results are directly comparable to the current `138.05s` baseline.

## Phase 7: Optional Provider Routing Preferences

Status: Planned after Phase 1-6.

Files:

- `src/embeddings/openrouter.rs`
- tests in `src/embeddings/openrouter.rs`
- `examples/index_codebase.rs`

Implementation steps:

1. Add optional env vars:
   - `RUST_CODE_MCP_OPENROUTER_PROVIDER_SORT`
   - `RUST_CODE_MCP_OPENROUTER_PREFERRED_MIN_THROUGHPUT`
   - `RUST_CODE_MCP_OPENROUTER_PREFERRED_MAX_LATENCY`
2. Support `sort` values:
   - `price`
   - `throughput`
   - `latency`
3. Add `provider` to `EmbeddingRequest` only when at least one provider preference is configured.
4. Prefer simple raw API JSON:

```json
{
  "provider": {
    "sort": "throughput",
    "preferred_min_throughput": 5000,
    "preferred_max_latency": 2.0
  }
}
```

5. Log only whether provider preferences were used, not sensitive headers.
6. Benchmark with and without `sort=throughput`.

Acceptance criteria:

- Provider preferences are opt-in.
- Defaults do not force a provider route.
- Benchmark output records the provider preference state.

## Phase 8: Optional Base64 Encoding Benchmark

Status: Planned after float batching is stable.

Files:

- `Cargo.toml` only if a direct `base64` dependency is needed.
- `src/embeddings/openrouter.rs`
- tests in `src/embeddings/openrouter.rs`
- `examples/openrouter_batch_matrix.rs`

Implementation steps:

1. Keep `float` as the default.
2. Add `EmbeddingResponseEmbedding` as an untagged enum:

```rust
enum EmbeddingResponseEmbedding {
    Float(Vec<f32>),
    Base64(String),
}
```

3. Decode base64 response bytes into little-endian `f32` values.
4. Validate decoded vector dimensions exactly like float vectors.
5. Add tests for valid base64, invalid base64, and dimension mismatch.
6. Add `encoding_format=base64` as a separate benchmark mode.

Acceptance criteria:

- Base64 is only enabled when explicitly configured.
- Float behavior remains the default and remains covered by existing tests.
- If base64 does not improve end-to-end time, leave it documented as unsupported for default tuning.

## Phase 9: Verification

Status: Planned.

Before running any command, confirm the Nix shell with the user.

Suggested build/check commands after confirmation:

```sh
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo check --lib'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo check --tests'
```

Suggested focused tests:

```sh
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo test embeddings::openrouter --lib'
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo test indexing::embedding_batcher --lib'
```

Suggested examples:

```sh
nix develop ../nix-devshells#<shell> --command zsh -lc 'cargo build --release --example index_codebase --example openrouter_batch_matrix'
nix develop ../nix-devshells#<shell> --command zsh -lc './target/release/examples/openrouter_batch_matrix --inputs 64,128 --tokens 65536,131072 --concurrency 2,4'
```

Acceptance criteria:

- `cargo check --lib` passes.
- `cargo check --tests` passes.
- Missing OpenRouter key path is clean.
- Live OpenRouter benchmark completes when a key is configured.
- Benchmark result includes request count, retry count, split count, and latency metrics.

## Phase 10: Tune Defaults And Record Results

Status: Planned after live benchmark.

Implementation steps:

1. Compare each benchmark row against the current baseline:
   - `embedding time: 138.05s`
   - `padded tokens/sec: 4573.1`
   - `requests: about 68`
2. Pick a conservative default that improves speed without hitting throttling.
3. Record an aggressive preset separately if concurrency `8` or larger payloads are stable.
4. Update `.docs/api-batch-proposal.md` or add a short follow-up report with final benchmark numbers.
5. Keep local GPU benchmark numbers unchanged as the regression guard.

Recommended decision rule:

- If concurrency `4` with `128` inputs and `131072` token budget is stable, make it the default.
- If retries or `429` responses appear, reduce concurrency before reducing batch size.
- If payload-too-large splits appear often, reduce `max_batch_tokens` before reducing `max_batch_inputs`.
- If provider routing improves throughput without increasing failures, document it as an opt-in preset first.

## Expected Code Change Scope

Likely touched files:

- `src/embeddings/openrouter.rs`
- `src/indexing/embedding_batcher.rs`
- `examples/index_codebase.rs`
- `examples/openrouter_batch_matrix.rs`
- `.docs/api-batch-proposal.md` or a follow-up report after measurement
- `.plans/api-batch-plan.md`

Possible later touched files:

- `Cargo.toml` if `base64` support is implemented with a direct dependency.
- `Cargo.lock` if `Cargo.toml` changes.

Not expected:

- No Nix devshell change for float batching/concurrency.
- No local Qwen3 CUDA dependency change.
- No ONNX Runtime dependency change.
- No vector store schema change.

## Completion Criteria

This work is complete when:

1. OpenRouter remote batching and concurrency are implemented behind OpenRouter-specific env vars.
2. Local GPU and local CPU profiles retain their current behavior.
3. Request metrics explain OpenRouter performance.
4. A live benchmark identifies the fastest stable OpenRouter config.
5. The final benchmark result is recorded without exposing the API key.

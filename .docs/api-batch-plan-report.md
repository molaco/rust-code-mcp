# API Batch Plan Implementation Report

## Summary

Implemented `.plans/api-batch-plan.md` phase by phase for the OpenRouter `openrouter-qwen3-8b` embedding path.

The implementation adds OpenRouter-specific request sizing, bounded request concurrency, provider-aware API request options, base64 response support, request metrics, and a dedicated OpenRouter benchmark matrix. Local GPU and local CPU embedding behavior remain on the existing local batch path.

No formatting command was run.

## Commits

| phase | commit | description |
|---|---|---|
| 1 | `cc21c1b1` | `api batch phase 1 openrouter config` |
| 2 | `630e0172` | `api batch phase 2 remote planner` |
| 3 | `f8c50f65` | `api batch phase 3 concurrent openrouter requests` |
| 4 | `5a60e79a` | `api batch phase 4 openrouter index batching` |
| 5 | `70f42ca6` | `api batch phase 5 openrouter metrics` |
| 6 | `944c604f` | `api batch phase 6 openrouter benchmark matrix` |
| 7 | `08f1f2f7` | `api batch phase 7 provider routing prefs` |
| 8 | `f3b7d597` | `api batch phase 8 base64 encoding option` |
| 9 | `c5927cf3` | `api batch phase 9 verification` |
| 10 | `223b6cfd` | `api batch phase 10 default tuning notes` |

Each phase began with `jj show --summary`; each phase updated `.plans/api-batch-plan.md` and was committed separately.

## Files Changed

- `.plans/api-batch-plan.md`
- `src/embeddings/openrouter.rs`
- `src/embeddings/mod.rs`
- `src/indexing/embedding_batcher.rs`
- `examples/index_codebase.rs`
- `examples/openrouter_batch_matrix.rs`

No Nix files were changed.

No Cargo dependency changes were made.

## Implemented Behavior

### OpenRouter Runtime Config

Added OpenRouter-specific env vars:

```text
RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS
RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS
RUST_CODE_MCP_OPENROUTER_CONCURRENCY
RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT
```

Current defaults:

```text
max_batch_inputs=128
max_batch_tokens=131072
concurrency=4
encoding_format=float
```

Invalid numeric values fall back to defaults. Over-large values are clamped. `float` remains the default encoding.

### Remote Planner

OpenRouter now builds remote request batches with a provider-specific planner instead of inheriting local GPU batch sizing.

Planner behavior:

- Estimates token length with the embedding tokenizer when available.
- Falls back to deterministic text-length estimates when tokenizer setup fails.
- Sorts by estimated token length and original input index.
- Respects max input count and padded token budget.
- Keeps oversized single inputs as single-item requests.
- Restores final embedding order to match caller input order.

### Concurrent Requests

The OpenRouter backend now executes planned requests with bounded concurrency through `buffer_unordered`.

`RUST_CODE_MCP_OPENROUTER_CONCURRENCY` controls in-flight request count.

Payload-too-large handling still splits only the offending request batch and retries smaller sub-batches.

### Index Batcher Split

`EmbeddingBatcher` now has a narrow OpenRouter branch:

- OpenRouter receives the full current indexing batch once.
- `OpenRouterEmbedder` handles remote sub-batching and concurrency.
- Local GPU and local CPU retain the existing length-bucketed local planner.

This keeps `RUST_CODE_MCP_EMBED_BATCH_SIZE` and `RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH` local-profile controls.

### Request Metrics

OpenRouter now records structured request metrics:

```text
openrouter_request_count
openrouter_retry_count
openrouter_split_count
openrouter_failed_request_count
openrouter_total_request_latency_secs
openrouter_min_request_latency_secs
openrouter_avg_request_latency_secs
openrouter_max_request_latency_secs
openrouter_total_request_inputs
openrouter_max_request_inputs
openrouter_total_estimated_tokens
openrouter_max_estimated_tokens
openrouter_response_vector_count
openrouter_response_dim
openrouter_embedding_count
openrouter_elapsed_secs
openrouter_padded_tokens_per_sec
```

Per-request debug logs include request index, retry attempt, input count, estimated tokens, latency, HTTP status when available, and response shape on success.

API keys are not logged.

### Benchmark Tool

Added `examples/openrouter_batch_matrix.rs`.

Default sweep:

```text
max_batch_inputs: 32, 64, 128
max_batch_tokens: 32768, 65536, 131072
concurrency: 1, 2, 4, 8
encoding_format: float
```

Supported filters:

```sh
openrouter_batch_matrix --inputs 64,128 --tokens 65536,131072 --concurrency 2,4
openrouter_batch_matrix --encoding base64 --inputs 128 --tokens 131072 --concurrency 4
```

If no OpenRouter API key is present, the benchmark exits successfully with:

```text
openrouter_benchmark=skipped_missing_api_key
```

### Provider Routing

Added opt-in provider routing env vars:

```text
RUST_CODE_MCP_OPENROUTER_PROVIDER_SORT
RUST_CODE_MCP_OPENROUTER_PREFERRED_MIN_THROUGHPUT
RUST_CODE_MCP_OPENROUTER_PREFERRED_MAX_LATENCY
```

Supported sort values:

```text
price
throughput
latency
```

The request includes `provider` only when at least one provider preference is configured.

Defaults do not force provider routing.

### Base64 Encoding

Added opt-in base64 response support without adding a direct Cargo dependency.

Behavior:

- `float` remains default.
- `base64` can be selected with `RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT=base64` or `openrouter_batch_matrix --encoding base64`.
- Response parsing accepts either float arrays or base64 strings.
- Base64 bytes decode into little-endian `f32` values.
- Dimension validation is shared with float response parsing.

## Verification

All verification was run through:

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc '<command>'
```

Commands run:

```sh
cargo test embeddings::openrouter --lib
cargo test indexing::embedding_batcher --lib
cargo check --lib
cargo check --tests
cargo build --release --example index_codebase --example openrouter_batch_matrix
env -u RUST_CODE_MCP_OPENROUTER_API_KEY -u OPENROUTER_API_KEY ./target/release/examples/openrouter_batch_matrix --inputs 128 --tokens 131072 --concurrency 4
```

Results:

- `cargo test embeddings::openrouter --lib`: passed, 20 tests.
- `cargo test indexing::embedding_batcher --lib`: passed, 8 tests.
- `cargo check --lib`: passed.
- `cargo check --tests`: passed.
- Release build for `index_codebase` and `openrouter_batch_matrix`: passed.
- Missing-key benchmark path: passed, printed `openrouter_benchmark=skipped_missing_api_key`.

Existing warnings remain. No new formatting command was run.

## Live Benchmark Status

A live OpenRouter matrix was not run because neither `RUST_CODE_MCP_OPENROUTER_API_KEY` nor `OPENROUTER_API_KEY` was configured in the shell.

No API key was written to source files, docs, plans, or command output.

The previous baseline remains the comparison point:

```text
embedding time: 138.05s
padded tokens/sec: 4573.1
requests: about 68
```

Recommended first live run:

```sh
./target/release/examples/openrouter_batch_matrix --inputs 64,128 --tokens 65536,131072 --concurrency 2,4
```

## Default Decision

Defaults remain conservative:

```text
RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS=128
RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS=131072
RUST_CODE_MCP_OPENROUTER_CONCURRENCY=4
RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT=float
```

No aggressive preset was selected because live benchmark evidence is required.

Provider routing and base64 are opt-in until measured.

## Residual Risk

The compile and unit coverage verify the new request planning, metrics, provider config, base64 parsing, and benchmark plumbing. The main remaining risk is provider-specific live behavior:

- OpenRouter payload limits for larger batches.
- Rate limiting or throttling under concurrency.
- Whether `base64` improves end-to-end time.
- Whether provider routing improves throughput without reducing availability.

These require a configured API key and live benchmark run.

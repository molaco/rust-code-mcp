# API Batch Proposal

## Goal

Improve remote embedding throughput for `openrouter-qwen3-8b` without changing the local GPU path.

The current OpenRouter benchmark completed successfully, but it was much slower than local GPU:

| profile | model | dim | chunks | embedding time | padded tokens/sec |
|---|---|---:|---:|---:|---:|
| `local-gpu-small` | Qwen3-Embedding-0.6B local CUDA | 1024 | 2084 | 33.17s | 19365.1 |
| `openrouter-qwen3-8b` | `qwen/qwen3-embedding-8b` remote API | 4096 | 2084 | 138.05s | 4573.1 |

OpenRouter worked functionally. The problem is throughput for bulk indexing.

## Diagnosis

The remote run was dominated by embedding time:

- Total duration: `139.628191s`
- Embedding duration: `138.053058s`
- Chunks: `2084`
- Padded tokens: approximately `631335`
- Effective padded tokens/sec: approximately `4573.1`

The current remote path reused the local batching shape:

- `RUST_CODE_MCP_EMBED_BATCH_SIZE=32`
- `RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=32768`
- Requests were sent sequentially.

The run produced four indexing batches and about `68` OpenRouter embedding sub-requests:

- Batch 1: `17` requests
- Batch 2: `26` requests
- Batch 3: `17` requests
- Batch 4: `8` requests

`138s / 68` is about `2.0s` per request, which explains almost all the wall time.

This does not look like a build, Nix, ONNX, or correctness issue. It looks like a remote API batching/concurrency issue.

## API vs Local Difference

The API profile is not equivalent to the local GPU profile:

- Local GPU uses Qwen3-Embedding-0.6B.
- OpenRouter uses Qwen3-Embedding-8B.
- Local GPU returns 1024-dimensional vectors.
- OpenRouter returns 4096-dimensional vectors.
- Local GPU has no network round trips.
- OpenRouter sends and receives JSON over HTTP.
- Local GPU can use VRAM-aware batch planning.
- OpenRouter needs request-count, payload-size, and provider-limit-aware batch planning.

The expected behavior is therefore different. OpenRouter can still be valuable for quality and zero-local-VRAM access, but it needs a remote-specific execution path for bulk indexing.

## Proposal

Add OpenRouter-specific batching, concurrency, and metrics.

Do not change the local GPU batcher behavior unless explicitly needed later.

## Phase 1: Add Remote Batch Config

Add OpenRouter-specific configuration values:

```text
RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS=128
RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS=131072
RUST_CODE_MCP_OPENROUTER_CONCURRENCY=4
RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT=float
```

Initial defaults:

| config | default | reason |
|---|---:|---|
| `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS` | `128` | Reduces HTTP request count without starting too aggressively. |
| `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS` | `131072` | Lets remote batches be larger than local GPU batches. |
| `RUST_CODE_MCP_OPENROUTER_CONCURRENCY` | `4` | Hides per-request latency while staying conservative. |
| `RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT` | `float` | Preserve current behavior first; benchmark `base64` separately. |

Acceptance criteria:

- Existing local GPU config remains unchanged.
- OpenRouter config has validation and clear warnings on invalid values.
- Defaults are documented in the proposal and CLI/benchmark output.

## Phase 2: Split Remote Batching From Local GPU Batching

The current local batch planner is VRAM-oriented. That is correct for CUDA, but too conservative for remote APIs.

Update the embedding path so:

- `LocalQwen3CandleCuda` keeps the existing local batch logic.
- `LocalFastembedOnnxCpu` keeps current behavior unless profiling shows a need to change it.
- `OpenRouter` uses a remote batch planner based on max inputs, max tokens, and concurrency.

Remote planner behavior:

1. Estimate token length for each formatted input.
2. Sort or group inputs to reduce padding waste if useful.
3. Build API requests up to `max_batch_inputs` and `max_batch_tokens`.
4. Send up to `concurrency` requests in flight.
5. Preserve original embedding order.
6. On payload-too-large errors, split the offending request and retry.

Acceptance criteria:

- OpenRouter no longer depends on `RUST_CODE_MCP_EMBED_BATCH_SIZE`.
- OpenRouter no longer creates dozens of tiny requests for medium-sized repos.
- Original embedding order remains stable.
- Payload-too-large handling keeps current safety behavior.

## Phase 3: Add OpenRouter Request Metrics

Add metrics for:

- request count
- retry count
- payload-too-large split count
- per-request latency
- input count per request
- token estimate per request
- response vector count
- response dimensions
- HTTP status on failure
- effective padded tokens/sec

Expose metrics through benchmark logs and machine-readable output.

Acceptance criteria:

- A benchmark run can explain whether time was spent in API latency, retries, provider throttling, or local parsing.
- No API keys are logged.
- HTTP error bodies are still summarized safely.

## Phase 4: Benchmark Remote Batch Matrix

Benchmark OpenRouter separately from local GPU.

Matrix:

```text
max_batch_inputs: 32, 64, 128
max_batch_tokens: 32768, 65536, 131072
concurrency: 1, 2, 4, 8
encoding_format: float
```

After float is stable, test:

```text
encoding_format: base64
```

Measurements:

- wall time
- embedding time
- request count
- retry count
- split count
- chunks indexed
- padded tokens
- padded tokens/sec
- API cost estimate

Acceptance criteria:

- Find the fastest stable OpenRouter config.
- Identify provider/payload limits if they exist.
- Record a recommended default and an aggressive tuning preset.

## Phase 5: Optional Provider Routing Preferences

OpenRouter supports provider routing preferences, including latency and throughput preferences. These are preferences, not hard guarantees.

Add optional env/config:

```text
RUST_CODE_MCP_OPENROUTER_PROVIDER_SORT=throughput
RUST_CODE_MCP_OPENROUTER_PREFERRED_MIN_THROUGHPUT=5000
RUST_CODE_MCP_OPENROUTER_PREFERRED_MAX_LATENCY=2.0
```

Request shape:

```json
{
  "provider": {
    "sort": { "by": "throughput" },
    "preferred_min_throughput": 5000,
    "preferred_max_latency": 2.0
  }
}
```

Acceptance criteria:

- Provider routing preferences are optional.
- Defaults do not force a routing behavior that might reduce availability.
- Benchmark output records whether provider preferences were used.

## Risks

Larger requests may hit provider payload limits.

Mitigation:

- Keep recursive split-on-too-large behavior.
- Start with conservative defaults.
- Add request metrics before raising defaults further.

Concurrency may trigger throttling.

Mitigation:

- Start with concurrency `4`.
- Log retry and HTTP status counts.
- Make concurrency configurable.

Base64 may not improve end-to-end time.

Mitigation:

- Keep `float` as the initial default.
- Benchmark `base64` only after float batching/concurrency is measured.

Provider routing may be inconsistent.

Mitigation:

- Keep routing preferences opt-in.
- Treat OpenRouter preferences as hints, not guarantees.

## Expected Outcome

Current OpenRouter result:

```text
embedding time: 138.05s
padded tokens/sec: 4573.1
requests: about 68
```

Reasonable target after remote batching/concurrency:

```text
embedding time: 35s to 60s
padded tokens/sec: 10000 to 18000
requests: about 8 to 20
```

This depends on OpenRouter provider limits and payload handling.

## Recommendation

Implement this as an isolated OpenRouter API batching project.

Do not change local GPU batching in the same patch. The local path is already near the original target at roughly `19.4k` padded tokens/sec, and mixing local GPU tuning with API batching would make regressions harder to diagnose.

# Qwen3 Follow-Up Embedding Profiles Plan

## Goal

Add three production-ready embedding profiles:

- `local-gpu-small`: local Qwen3-Embedding-0.6B on CUDA.
- `local-cpu-small`: local quantized BGE-small-en-v1.5 via `BGESmallENV15Q`.
- `openrouter-qwen3-8b`: remote OpenRouter `qwen/qwen3-embedding-8b`.

The implementation must preserve the existing optimized Qwen3-Embedding-0.6B path, keep old `model` arguments working, and make switching profiles cache-safe.

## Non-Goals

- Do not replace Qwen3-Embedding-0.6B as the preferred local GPU model.
- Do not make Qwen3-Embedding-8B a local target for 8 GB GPUs.
- Do not run formatting commands.
- Do not change unrelated VCS state.

## Phase 1: Fix Cache Correctness

Status: Complete.

Multiple embedding profiles are unsafe until incremental state is keyed by the embedding profile.

Current risk:

- Vector collection names include `backend.identity()`.
- Incremental Merkle snapshots are primarily keyed by codebase path.
- Switching profiles can create a fresh vector collection while the incremental snapshot says all files are unchanged.

Implementation steps:

1. Locate the incremental snapshot path generation in `src/indexing/incremental.rs`.
2. Locate vector collection identity construction in `src/tools/project_paths.rs`.
3. Add a shared indexing identity that includes:
   - canonical codebase path
   - embedding backend identity
   - chunking identity
4. Use that identity for incremental snapshot paths.
5. Preserve existing snapshots as old data; do not migrate them in place.
6. Add tests proving different embedding profiles produce different snapshot paths.

Acceptance criteria:

- Indexing the same directory with two different profiles cannot reuse the same Merkle snapshot.
- Existing default indexing still works.

Completed work:

- Added `src/indexing/identity.rs` as the shared source for indexing identities.
- Merkle snapshots now key off canonical codebase path, active embedder identity, and chunking identity.
- `ProjectPaths` now exposes the resolved indexing identity, chunking identity, and snapshot path.
- Force reindex and stale-index cleanup now delete the profile-aware snapshot path.
- Added tests proving snapshot paths differ by backend identity and chunking identity.

## Phase 2: Define First-Class Embedding Profiles

Status: Complete.

Replace the current model-only selection with profile-aware configuration.

Target profiles:

| Profile | Runtime | Model | Dimension | Max Tokens |
| --- | --- | --- | --- | --- |
| `local-gpu-small` | local Candle CUDA | `Qwen/Qwen3-Embedding-0.6B` | 1024 | 1024 current runtime cap |
| `local-cpu-small` | fastembed ONNX CPU | `BGESmallENV15Q` | 384 | 512 |
| `openrouter-qwen3-8b` | OpenRouter API | `qwen/qwen3-embedding-8b` | 4096 | provider limit |

Implementation steps:

1. Refactor `src/embeddings/backend.rs` around:
   - `EmbeddingProfile`
   - `EmbeddingRuntime`
   - `EmbeddingModelSpec`
2. Make each profile expose:
   - stable `identity()`
   - `dimension()`
   - `max_tokens()`
   - default chunk target
   - default hard chunk cap
   - query/document prefix behavior
3. Keep compatibility aliases:
   - `qwen3-0.6b` maps to `local-gpu-small`
   - `qwen3-4b` remains valid if supported by the current local path
   - `qwen3-8b` remains valid if supported by the current local path or is explicitly rejected with a clear message
4. Make the default profile `local-gpu-small`.

Acceptance criteria:

- Existing callers using `model: "qwen3-0.6b"` continue to work.
- New callers can use explicit profile names.
- Profile identities are unique and stable.

Completed work:

- Refactored `src/embeddings/backend.rs` around `EmbeddingProfile`, `EmbeddingRuntime`, and `EmbeddingModelSpec`.
- Added stable metadata for `local-gpu-small`, `local-cpu-small`, `openrouter-qwen3-8b`, plus compatibility local Qwen3 4B/8B profiles.
- Kept the existing default Qwen3-Embedding-0.6B identity string compatible: `fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2`.
- Added profile dimensions, default token caps, default chunking metadata, and query formatting behavior.
- Updated local Qwen3 initialization and token counting to use the new model spec.
- Added unit coverage for profile parsing, identity uniqueness, dimension values, query formatting, and identity round-trips.

Verification:

- `CUDARC_CUDA_VERSION=12080 cargo check --lib` passed with pre-existing warnings.
- `CUDARC_CUDA_VERSION=12080 cargo test embeddings::backend --lib` compiled but could not link because CUDA libraries (`cuda`, `nvrtc`, `curand`, `cublas`, `cublasLt`, `cudart`) are not visible in this shell.

## Phase 3: Expose Profile Selection in MCP Tools

Status: Complete.

Add an explicit MCP argument while keeping the old argument.

Implementation steps:

1. Add optional `embedding_profile` to the index tool input in `src/tools/index_tool.rs`.
2. Keep optional `model` for compatibility.
3. Resolution order:
   - `embedding_profile` wins if present.
   - otherwise resolve `model`.
   - otherwise use the default profile.
4. Return the resolved profile and backend identity in the index response.
5. Make error messages name accepted profiles and legacy model aliases.

Acceptance criteria:

- `index_codebase(..., embedding_profile: "local-gpu-small")` works.
- `index_codebase(..., model: "qwen3-0.6b")` still works.
- Invalid profile names fail before indexing starts.

Completed work:

- Added optional `embedding_profile` to `IndexCodebaseParams`.
- Implemented resolution order where `embedding_profile` wins over legacy `model`.
- Kept legacy model aliases for `qwen3-0.6b`, `qwen3-4b`, and `qwen3-8b`.
- Included resolved profile name in `index_codebase` result text.
- Added tests for legacy model resolution, profile precedence, and invalid profile errors.

Verification:

- `CUDARC_CUDA_VERSION=12080 cargo check --lib` passed with pre-existing warnings.

## Phase 4: Implement OpenRouter Qwen3-Embedding-8B

Status: Complete.

Add a remote embedding backend for quality-first API embeddings.

Configuration:

- `RUST_CODE_MCP_OPENROUTER_API_KEY`
- `OPENROUTER_API_KEY`
- optional `RUST_CODE_MCP_OPENROUTER_BASE_URL`

Default endpoint:

```text
https://openrouter.ai/api/v1/embeddings
```

Model:

```text
qwen/qwen3-embedding-8b
```

Implementation steps:

1. Add an `OpenRouterEmbedder` backend.
2. Use `reqwest` with JSON request/response types.
3. Preserve input order across batches.
4. Validate response vector dimensions against the profile dimension.
5. Add retry handling for transient `429` and `5xx` responses.
6. Split or fail clearly on payload size errors.
7. Include useful request context in errors without logging API keys.

Query/document behavior:

- Prefer provider-supported request fields if OpenRouter accepts them.
- Otherwise use Qwen3 instruction-style query prefixes.
- Do not add document prefixes unless required.

Acceptance criteria:

- Missing API key returns a clear configuration error.
- Mocked OpenRouter responses parse correctly.
- Dimension mismatches fail loudly.
- Network retries do not reorder embeddings.

Completed work:

- Added `src/embeddings/openrouter.rs` with an async OpenRouter embeddings backend.
- Added direct `reqwest` dependency for JSON API calls.
- Wired `EmbeddingGenerator` to dispatch by runtime between local Qwen3 and OpenRouter.
- Implemented `RUST_CODE_MCP_OPENROUTER_API_KEY`, `OPENROUTER_API_KEY`, and optional `RUST_CODE_MCP_OPENROUTER_BASE_URL`.
- Sends `model`, `input`, `encoding_format`, `dimensions`, and `input_type` to `/api/v1/embeddings`.
- Preserves response ordering by `index`, validates vector dimensions, retries transient `429`/`5xx`/`529` responses, and splits payload-too-large batches.
- Added unit coverage for OpenRouter response parsing, dimension mismatch, and missing API key messaging.

Verification:

- Confirmed OpenRouter embeddings API supports `dimensions`, `encoding_format`, and `input_type`.
- `CUDARC_CUDA_VERSION=12080 cargo check --lib` passed with pre-existing warnings.
- Live OpenRouter indexing was not run because no API key is configured in this session.

## Phase 5: Implement Local CPU Small with BGESmallENV15Q

Status: Complete.

Add a quantized CPU embedding backend for machines without useful CUDA.

Model:

```rust
EmbeddingModel::BGESmallENV15Q
```

Identity:

```text
fastembed-onnx-cpu:BGESmallENV15Q:dim384:max512:v1
```

Implementation steps:

1. Add a fastembed ONNX CPU backend behind a feature or dependency configuration that does not break the existing Qwen3 CUDA path.
2. Use `BGESmallENV15Q`, not `AllMiniLML6V2Q`, as the CPU default.
3. Set profile defaults:
   - dimension `384`
   - max tokens `512`
   - chunk target around `384`
   - hard chunk cap `512`
4. Add BGE query prefix support:

```text
Represent this sentence for searching relevant passages: 
```

5. Keep documents unprefixed.

Dependency warning:

- The current project patches fastembed for Qwen3/CUDA and uses an `ort` setup intended to avoid normal ONNX linking.
- This phase may require feature-gating the ONNX path or adjusting `ort` features carefully.

Acceptance criteria:

- `embedding_profile: "local-cpu-small"` initializes without CUDA.
- Generated embeddings are 384-dimensional.
- CPU profile has a distinct vector collection and Merkle snapshot.

Completed work:

- Added `src/embeddings/fastembed_cpu.rs` backed by `fastembed::TextEmbedding`.
- Wired `EmbeddingGenerator` to dispatch `local-cpu-small` to `BGESmallENV15Q`.
- Enabled fastembed's ONNX Runtime download feature and removed the direct `ort alternative-backend` override that would prevent ONNX execution.
- Set `local-cpu-small` to 384 dimensions and 512 max tokens through the profile metadata.
- Added BGE query prefix behavior through profile-aware query formatting.
- Applied profile-specific chunk defaults before env overrides, so `local-cpu-small` uses `target384/hard512` by default.
- Updated token counting to load the tokenizer for the active model spec, not only Qwen3.
- Added unit coverage for the CPU profile chunking salt.

Verification:

- `CUDARC_CUDA_VERSION=12080 cargo check --lib` passed with pre-existing warnings.
- Runtime CPU indexing was not executed in this shell because the binary still links the CUDA-enabled Candle stack; the CPU backend itself does not construct a CUDA device.

## Phase 6: Make Query Embedding Profile-Aware

Status: Complete.

Search must use the same profile semantics as indexing.

Implementation steps:

1. Thread the selected embedding profile into query embedding code.
2. Ensure vector search opens the collection matching the profile identity.
3. Apply the profile's query formatting.
4. Prevent querying a collection with the wrong embedding dimension.
5. Include active profile and identity in diagnostics or debug logs.

Acceptance criteria:

- Qwen3-indexed collections use Qwen3 query formatting.
- BGE-indexed collections use BGE query formatting.
- OpenRouter-indexed collections use OpenRouter/Qwen3 query formatting.
- Cross-profile dimension mistakes fail clearly.

Completed work:

- Added optional `embedding_profile` to `search` and `get_similar_code` MCP parameters.
- Query paths now derive `ProjectPaths` from the requested embedding backend instead of always using the default backend.
- Auto-indexing from search now initializes the requested backend and collection.
- `create_hybrid_search` now accepts the configured backend, falls back to it when metadata is absent, and still reconciles against on-disk metadata when present.
- Query embedding formatting is profile-aware through `EmbeddingBackend::format_query`.
- Added profile/embedder/collection tracing for query initialization.
- Updated graph-side callers to pass the default backend explicitly.

Verification:

- `CUDARC_CUDA_VERSION=12080 cargo check --lib` passed with pre-existing warnings.

## Phase 7: Add Tests

Add focused tests before benchmarking.

Test coverage:

- profile parsing
- legacy model alias parsing
- identity uniqueness
- snapshot path uniqueness by profile
- profile dimension values
- query prefix behavior
- OpenRouter JSON response parsing
- OpenRouter dimension mismatch error
- missing API key error

Acceptance criteria:

- `cargo check --lib` passes.
- Unit tests cover the new profile resolution behavior.
- No formatting command is run.

## Phase 8: Benchmark and Tune

Benchmark each profile separately.

Local GPU small:

```text
embedding_profile=local-gpu-small
```

Recommended 8 GB-safe overrides:

```text
RUST_CODE_MCP_EMBED_BATCH_SIZE=16
RUST_CODE_MCP_EMBED_MAX_TOKENS_PER_BATCH=16384
```

Optional tighter overrides:

```text
RUST_CODE_MCP_CHUNK_TARGET_TOKENS=512
RUST_CODE_MCP_CHUNK_HARD_MAX_TOKENS=768
```

Local CPU small:

```text
embedding_profile=local-cpu-small
```

OpenRouter:

```text
embedding_profile=openrouter-qwen3-8b
```

Measurements to collect:

- total indexing wall time
- embedding wall time
- chunks indexed
- padded tokens
- effective padded tokens per second
- vector dimension
- peak GPU memory for CUDA profile
- API request count and retry count for OpenRouter

Acceptance criteria:

- The default local GPU profile remains near current optimized performance.
- CPU profile is usable on machines without CUDA.
- OpenRouter profile works when credentials are configured.

## Rollout Order

1. Cache correctness fix.
2. Profile model/config refactor.
3. MCP argument plumbing.
4. OpenRouter Qwen3-Embedding-8B.
5. Local CPU `BGESmallENV15Q`.
6. Query profile awareness.
7. Tests and checks.
8. Benchmarks.

## Final Expected User-Facing API

Example local GPU:

```json
{
  "directory": "/path/to/codebase",
  "force_reindex": true,
  "embedding_profile": "local-gpu-small"
}
```

Example local CPU:

```json
{
  "directory": "/path/to/codebase",
  "force_reindex": true,
  "embedding_profile": "local-cpu-small"
}
```

Example OpenRouter:

```json
{
  "directory": "/path/to/codebase",
  "force_reindex": true,
  "embedding_profile": "openrouter-qwen3-8b"
}
```

Legacy compatibility:

```json
{
  "directory": "/path/to/codebase",
  "force_reindex": true,
  "model": "qwen3-0.6b"
}
```

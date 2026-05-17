# embeddings - Architecture

## Overview

The `embeddings` module provides profile-driven text embedding for indexing and search. A profile describes the runtime, provider model id, vector dimension, tokenizer source, query handling, and chunk defaults. Local Candle/ONNX models remain code-bound because they require loader code, model variants, and native runtime setup. OpenRouter API models can be added as data through TOML configuration.

The main public types are:

- `EmbeddingBackend`: resolved runtime/profile configuration used by indexers, search, vector stores, and sync.
- `EmbeddingProfile`: owned profile data for built-in and user-defined profiles.
- `EmbeddingGenerator`: runtime dispatcher for local Qwen3/Candle, local fastembed ONNX CPU, or OpenRouter.
- `QueryPolicy`: runtime-aware query handling. Local models use instruction prefixes; OpenRouter uses `input_type`.

## Built-In Profiles

| profile | runtime | model id | dim | max_len | query handling |
|---|---|---:|---:|---:|---|
| `local-gpu-small` | local Qwen3 Candle CUDA | `Qwen/Qwen3-Embedding-0.6B` | 1024 | 1024 | Qwen3 code-search prefix |
| `local-qwen3-4b` | local Qwen3 Candle CUDA | `Qwen/Qwen3-Embedding-4B` | 2560 | 1024 | Qwen3 code-search prefix |
| `local-qwen3-8b` | local Qwen3 Candle CUDA | `Qwen/Qwen3-Embedding-8B` | 4096 | 1024 | Qwen3 code-search prefix |
| `local-cpu-small` | local fastembed ONNX CPU | `Qdrant/bge-small-en-v1.5-onnx-Q` | 384 | 512 | BGE search prefix |
| `openrouter-qwen3-8b` | OpenRouter API | `qwen/qwen3-embedding-8b` | 4096 | 32768 | `search_document` / `search_query` |

Legacy aliases remain accepted:

- `qwen3-local-gpu-small` -> `local-gpu-small`
- `bge-small-cpu` -> `local-cpu-small`
- `qwen3-8b-openrouter` -> `openrouter-qwen3-8b`

## User-Defined OpenRouter Profiles

Dynamic profiles are loaded per request. Resolution order:

1. `RUST_CODE_MCP_EMBEDDING_PROFILES` points to a TOML file.
2. `embedding_profiles.toml` in the indexed project root, if present.
3. Built-in profile names and aliases.

The TOML file contains metadata only. Secrets never belong in TOML; the OpenRouter key still comes from `RUST_CODE_MCP_OPENROUTER_API_KEY` or `OPENROUTER_API_KEY`. Unknown TOML keys are rejected, so accidental fields like `api_key` fail parsing instead of being stored.

Example:

```toml
[[profile]]
name = "openrouter-e5-large"
model_id = "intfloat/multilingual-e5-large"
dim = 1024
max_len = 512
query_document = "search_document"
query_input = "search_query"
chunk_target_tokens = 384
chunk_hard_max_tokens = 768
```

Minimal required fields:

```toml
[[profile]]
name = "openrouter-custom"
model_id = "provider/model-name"
dim = 1536
max_len = 8192
```

`runtime` is optional and defaults to `openrouter`. If present, it must be `openrouter`; TOML cannot define local Qwen3 or ONNX profiles. Adding a local model still requires code because the loader must know the concrete model variant and native runtime behavior.

Use the profile through MCP tool parameters:

```json
{
  "directory": "/path/to/project",
  "embedding_profile": "openrouter-custom"
}
```

The same profile name can be passed to search tools that accept `embedding_profile`.

## Identity Format

New vector stores record a v2 embedder identity in `metadata.json`:

```text
emb;v=2;rt=<runtime>;model=<percent-encoded>;dim=<n>;max=<n>;query=<percent-encoded>
```

Runtime values:

- `local-qwen3-candle-cuda`
- `local-fastembed-onnx-cpu`
- `openrouter`

The `model` and `query` fields are percent-encoded byte strings using a filesystem-safe alphabet. This allows provider ids and query policies containing `/`, `:`, `=`, `;`, spaces, or newlines without reintroducing the old colon-splitting ambiguity.

`query` stores a `QueryPolicy` tag:

- `prefix:<encoded-prefix>` for local instruction-prefix models.
- `input-type:<encoded-document>:<encoded-query>` for OpenRouter request `input_type` values.
- `none` for profiles with no query transformation.

`EmbeddingBackend::from_identity()` still accepts the legacy colon-delimited identities for existing indexes:

```text
fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max1024:v2
fastembed-onnx-cpu:BGESmallENV15Q:dim384:max512:v1
openrouter:qwen/qwen3-embedding-8b:dim4096:max32768:v1
```

Search and background sync preserve the exact stored identity string when opening existing vector stores. That keeps legacy `metadata.json` values usable even though new writes use v2 identities.

## Runtime Flow

1. Tool input resolves to an `EmbeddingBackend` from `embedding_profile`, legacy `model`, or the default profile.
2. `ProjectPaths` derives profile-specific vector collection names from the active indexing identity.
3. Indexing creates `EmbeddingGenerator::with_backend(backend)` and writes vector metadata with the backend identity.
4. Search reads vector metadata back through `EmbeddingBackend::from_identity()` so query embeddings match the stored vectors.
5. Background sync enumerates existing per-profile vector indexes and syncs each with its recorded backend. It does not create a default-profile index as a side effect of syncing a non-default profile.

## OpenRouter Behavior

OpenRouter requests send the resolved profile's `model_id` directly. There is no OpenRouter model allow-list in code. The request also sends:

- `dimensions` from the profile.
- `input_type` from `QueryPolicy::InputType`.
- runtime batching/concurrency settings from OpenRouter environment variables.
- optional provider routing preferences when configured.

API keys are never logged.

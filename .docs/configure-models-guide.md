# Configuring Embedding Models

How to select, add, and tune embedding models — both local and API-backed.

## The model of configuration

A **profile** is the unit of configuration. It bundles a model id, runtime,
output dimension, max input length, query handling, and chunk sizes. Two axes:

- **Built-in vs dynamic** — built-ins are a compiled-in table; dynamic profiles
  are loaded from a TOML file at runtime.
- **Local vs API** — local profiles run on this machine (Candle/CUDA or
  ONNX/CPU); API profiles call OpenRouter.

The rule that drives everything:

> **API models are pure data (config). Local models need code.**

OpenRouter's API is uniform, so any API model is just a few fields. Local
models each need their own weights, tokenizer, and runtime loader.

## 1. Using an existing (built-in) model

Five built-in profiles ship today:

| Profile name | Alias | Model | Runtime | Dim |
|---|---|---|---|---:|
| `local-gpu-small` | `qwen3-local-gpu-small` | Qwen3-Embedding-0.6B | local CUDA | 1024 |
| `local-qwen3-4b` | — | Qwen3-Embedding-4B | local CUDA | 2560 |
| `local-qwen3-8b` | — | Qwen3-Embedding-8B | local CUDA | 4096 |
| `local-cpu-small` | `bge-small-cpu` | BGE-small-en-v1.5 | local ONNX/CPU | 384 |
| `openrouter-qwen3-8b` | `qwen3-8b-openrouter` | qwen/qwen3-embedding-8b | OpenRouter | 4096 |

Select one by **name**:

- **MCP tools** — pass the `embedding_profile` argument to the `index_codebase`
  and search tools.
- **`index_codebase` example** — `--profile <name>`.

Switching between existing models requires nothing more than the name.

## 2. Adding a new API model — config only, no recompile

Create an **`embedding_profiles.toml`**. It is discovered two ways; the
environment variable wins when both are present:

1. `RUST_CODE_MCP_EMBEDDING_PROFILES` — an explicit path to a TOML file
   (global, applies to every indexed project).
2. `embedding_profiles.toml` in the root of the project being indexed
   (per-project).

### TOML schema

One `[[profile]]` block per model:

```toml
[[profile]]
name                  = "openrouter-e5-large"               # required
model_id              = "intfloat/multilingual-e5-large"    # required
dim                   = 1024     # required — MUST equal the model's real output dimension
max_len               = 512      # required — model's max input length in tokens
runtime               = "openrouter"      # optional; defaults to "openrouter", must be "openrouter"
query_document        = "search_document" # optional, default shown
query_input           = "search_query"    # optional, default shown
chunk_target_tokens   = 768      # optional, default 768
chunk_hard_max_tokens = 1024     # optional, default 1024
```

### Loader rules (enforced)

- **API-only.** `runtime` must be `openrouter`. A local runtime in TOML is
  rejected — local models need code (see section 3).
- **Unknown keys are rejected.** You cannot put `api_key`, `token`, or any
  other credential in this file; an unknown field is a hard parse error.
- A `name` that collides with a built-in profile or alias is rejected.
- A duplicate `name` within the TOML is rejected.
- `dim` and `max_len` must be present and greater than zero.

### Credentials

The API key comes only from the environment, never from the TOML:

```sh
OPENROUTER_API_KEY=sk-or-...              # or RUST_CODE_MCP_OPENROUTER_API_KEY
RUST_CODE_MCP_OPENROUTER_BASE_URL=...     # optional; override the endpoint
```

### Example `embedding_profiles.toml`

```toml
# API-only. Secrets stay in env vars, never here.

[[profile]]
name     = "openrouter-text-embedding-3-small"
model_id = "openai/text-embedding-3-small"
dim      = 1536
max_len  = 8191

[[profile]]
name     = "openrouter-text-embedding-3-large"
model_id = "openai/text-embedding-3-large"
dim      = 3072
max_len  = 8191
```

## 3. Adding a new local model — this needs code

A local model is **not** a config change. It needs a loader spec — real
weights, a tokenizer, and a runtime path. Two cases:

- **Another Qwen3 size** — add a `Qwen3Variant` and a `BUILT_IN_PROFILES` entry
  in `src/embeddings/backend.rs`. Mostly data, but still a recompile.
- **A genuinely different local model** (new ONNX model or new architecture) —
  also needs loader support in `src/embeddings/fastembed_cpu.rs` (ONNX) or
  `src/embeddings/qwen3.rs` (Candle), because the ONNX path is pinned to a
  specific fastembed model. This is real code work.

This asymmetry is deliberate: the OpenRouter API is uniform, so an API model is
just data; each local model needs its own loader.

## 4. Tuning OpenRouter throughput

All optional, all environment variables, all with safe defaults:

| Variable | Default | Purpose |
|---|---|---|
| `RUST_CODE_MCP_OPENROUTER_CONCURRENCY` | 4 | concurrent requests in flight |
| `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_INPUTS` | 128 | max inputs per request |
| `RUST_CODE_MCP_OPENROUTER_MAX_BATCH_TOKENS` | 131072 | max padded tokens per request |
| `RUST_CODE_MCP_OPENROUTER_ENCODING_FORMAT` | float | `float` or `base64` |
| `RUST_CODE_MCP_OPENROUTER_PROVIDER_SORT` | unset | `price`, `throughput`, or `latency` |
| `RUST_CODE_MCP_OPENROUTER_PREFERRED_MIN_THROUGHPUT` | unset | provider routing preference |
| `RUST_CODE_MCP_OPENROUTER_PREFERRED_MAX_LATENCY` | unset | provider routing preference |

Note: for a large compute-bound model, raising concurrency can make indexing
*slower* by overloading the provider. Measure before committing a value.

## 5. Things that will bite you

1. **`dim` must be exact.** If a TOML `dim` does not match the model's true
   output dimension, the vector store rejects the vectors. Look up the real
   number per model (e.g. `text-embedding-3-small` = 1536,
   `text-embedding-3-large` = 3072, `multilingual-e5-large` = 1024).

2. **Changing the model means re-indexing.** Each profile has a distinct
   identity string; caches and vector stores are keyed by it. Switching
   profiles builds a *separate* index — old vectors are not reused, and search
   must use the same profile the index was built with. Background sync handles
   this per-profile automatically.

3. **Secrets stay in environment variables.** The TOML carries model metadata
   only, by design and by enforcement.

4. **Query handling differs by runtime.** Local Qwen3 profiles prepend a
   code-search instruction prefix; OpenRouter profiles send `input_type`
   values (`search_document` / `search_query`). A general-purpose API model
   that is not code-tuned will index fine but may return lower-quality results
   for code search than a code-tuned model — verify retrieval quality, not
   just indexing speed, before switching a default.

5. **Higher dimensions cost more.** A profile's `dim` is the vector size: a
   4096-dim model (Qwen3-8B) stores ~10x the bytes per vector and costs ~10x
   the per-query search compute of a 384-dim model (BGE). Vector search is
   exact brute-force KNN — fast at workspace scale (thousands of chunks)
   regardless of dimension — but for very large monorepos, prefer a
   lower-dimension profile.

## Quick reference

| Task | What to do |
|---|---|
| Use a built-in model | Pass its profile name (`embedding_profile` arg or `--profile`) |
| Add an API model | Add a `[[profile]]` block to `embedding_profiles.toml` |
| Add a local model | Code change in `src/embeddings/` + recompile |
| Set the API key | `OPENROUTER_API_KEY` environment variable |
| Tune OpenRouter speed | `RUST_CODE_MCP_OPENROUTER_*` environment variables |

# embeddings - Detailed Logic

## `EmbeddingBackend::identity() -> String`

**Steps:**

1. Read runtime, model id, dimension, max length, and `QueryPolicy` from the active profile.
2. Serialize the query policy with `QueryPolicy::encode_tag()`.
3. Build an `EmbeddingIdentity` and encode it as v2:

   ```text
   emb;v=2;rt=<runtime>;model=<encoded>;dim=<n>;max=<n>;query=<encoded>
   ```

4. Percent-encode string fields so model ids and query policies can contain provider separators and newlines safely.

## `EmbeddingBackend::from_identity(s: &str) -> Result<Self, EmbeddingError>`

**Steps:**

1. If `s` starts with `emb;`, parse the v2 identity with `EmbeddingIdentity::decode()`.
2. Decode the `query` field with `QueryPolicy::decode_tag()`.
3. For `OpenRouter`, create a usable API backend directly from identity fields. If the model matches a built-in API profile, reuse built-in metadata such as tokenizer id and chunk defaults.
4. For local runtimes, resolve `(runtime, model_id)` against the built-in registry to recover `LocalLoaderSpec`.
5. Reject unknown local model ids with a `clear_cache`-oriented error because TOML cannot define local loaders.
6. If the identity is not v2, parse the three supported legacy identities for existing vector stores.

## `resolve_profile(name, project_root)`

**Steps:**

1. Load profiles from the path in `RUST_CODE_MCP_EMBEDDING_PROFILES`, if set.
2. Load profiles from `project_root/embedding_profiles.toml`, if present.
3. Parse TOML with `deny_unknown_fields`; unknown keys such as `api_key` are hard errors.
4. Require `name`, `model_id`, `dim`, and `max_len`.
5. Default `runtime` to `openrouter`; reject every other runtime.
6. Reject profile names that collide with built-ins or aliases.
7. Return a user profile if the requested name matches; otherwise fall back to built-in profile parsing.

## `EmbeddingGenerator::with_backend(backend)`

**Steps:**

1. Inspect `backend.runtime`.
2. For `LocalQwen3CandleCuda`, construct `Qwen3Embedder` from `backend.require_qwen3_variant()`.
3. For `LocalFastembedOnnxCpu`, construct `FastembedCpuEmbedder` from `backend.require_fastembed_cpu_model()`.
4. For `OpenRouter`, construct `OpenRouterEmbedder` and require `QueryPolicy::InputType`.
5. Store the backend next to the runtime-specific generator so index and search callers can inspect the active configuration.

## OpenRouter Dynamic Model Path

**Steps:**

1. Read the API key only from `RUST_CODE_MCP_OPENROUTER_API_KEY` or `OPENROUTER_API_KEY`.
2. Send `backend.model_id()` as the request `model` without an allow-list.
3. Send `backend.dim()` as `dimensions`.
4. Send `search_document` or `search_query` from the profile's `QueryPolicy::InputType`.
5. Split, retry, and meter remote batches with the existing OpenRouter planner.

## Search Metadata Resolution

**Steps:**

1. Compute the requested profile's current vector path.
2. If that path has metadata, use it.
3. Otherwise enumerate existing per-profile vector indexes for the directory and select one whose decoded backend matches the request.
4. Read `metadata.json` and parse its `embedder_version` with `EmbeddingBackend::from_identity()`.
5. Create query embeddings with the decoded backend.
6. Open the vector store with the exact stored identity string. This preserves legacy vector stores whose metadata predates v2 identities.

## Background Sync

**Steps:**

1. Enumerate existing vector indexes for the tracked directory.
2. For each `metadata.json`, decode `embedder_version` into an `EmbeddingBackend`.
3. Reopen the incremental indexer with that backend and the stored identity string.
4. Sync each existing profile index independently.
5. If no vector indexes exist, skip sync; do not create a default-profile index.

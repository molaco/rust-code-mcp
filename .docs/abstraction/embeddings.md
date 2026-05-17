# embeddings - Abstract Logic

## Module: embeddings/backend.rs

**Purpose:** Owns embedding profile data, runtime selection, query policies, and stable identities.

1. **Resolve built-in profiles and aliases** -> `EmbeddingProfile::parse()`
2. **Construct a backend from profile data** -> `EmbeddingBackend::from_profile()`
3. **Expose model metadata** -> `dim()`, `model_id()`, `tokenizer_model_id()`, `model_display_name()`
4. **Recover local loader details for built-in local profiles** -> `require_qwen3_variant()`, `require_fastembed_cpu_model()`
5. **Apply runtime-specific query handling** -> `QueryPolicy::format_query()`, `QueryPolicy::input_types()`
6. **Encode and decode query policy identity tags** -> `QueryPolicy::encode_tag()`, `QueryPolicy::decode_tag()`
7. **Emit v2 backend identities for new vector stores** -> `EmbeddingBackend::identity()`
8. **Load v2 and legacy identities from metadata** -> `EmbeddingBackend::from_identity()`

## Module: embeddings/identity.rs

**Purpose:** Provides the v2 filesystem-safe identity codec.

1. **Encode identity fields as key/value records** -> `EmbeddingIdentity::encode()`
2. **Decode order-independent v2 identities** -> `EmbeddingIdentity::decode()`
3. **Percent-encode string fields** -> `percent_encode()`
4. **Reject malformed or unsupported identities without panics** -> `decode()`

## Module: embeddings/profile_registry.rs

**Purpose:** Loads API-only user profiles for OpenRouter from TOML configuration.

1. **Resolve per-request profile names** -> `resolve_profile(name, project_root)`
2. **Load global TOML from `RUST_CODE_MCP_EMBEDDING_PROFILES`**
3. **Load project-root `embedding_profiles.toml`**
4. **Reject built-in name collisions and duplicate user names**
5. **Reject unknown fields, missing dimensions, invalid paths, and local runtimes**
6. **Build `EmbeddingProfile` values with `runtime = OpenRouter` and `local_loader = None`**

## Module: embeddings/mod.rs

**Purpose:** Dispatches embedding generation to the backend selected by `EmbeddingBackend`.

1. **Construct the default backend** -> `EmbeddingGenerator::new()`
2. **Construct a generator for an explicit backend** -> `EmbeddingGenerator::with_backend()`
3. **Embed documents without query transformation** -> `embed_documents()`
4. **Embed queries with runtime-specific query handling** -> `embed_queries()`
5. **Format and embed code chunks** -> `embed_chunks()`

## Runtime Modules

- `qwen3.rs`: local Candle/CUDA Qwen3 loader; local models are code-bound by `Qwen3Variant`.
- `fastembed_cpu.rs`: local ONNX CPU loader; local model is code-bound by `FastembedCpuModel`.
- `openrouter.rs`: remote OpenRouter embedding client; dynamic API profiles send `profile.model_id` directly.
- `token_lengths.rs`: tokenizer-backed text length estimation keyed by backend tokenizer metadata.

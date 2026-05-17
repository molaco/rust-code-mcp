# Multi-Provider Embedding Profiles Implementation Report

## Summary

Implemented `.plans/multi-provider-plan.md` phase by phase. The embedding stack now treats OpenRouter embedding models as data-backed profiles while keeping local Candle/ONNX models code-bound. Built-in profiles are table-driven, identities use a v2 filesystem-safe codec, dynamic OpenRouter profiles can be loaded from TOML, and background sync/search reopen indexes with the profile recorded in vector-store metadata.

No formatting command was run.

## Commits

| phase | commit | description |
|---|---|---|
| 1 | `c2bdcbbe` | `phase 1 identity codec` |
| 2 | `49a09646` | `phase 2 table driven profiles` |
| 3 | `0fe22e3f` | `phase 3 query policy` |
| 4 | `d40636dd` | `phase 4 v2 backend identity` |
| 5 | `73bf2238` | `phase 5 dynamic api profiles` |
| 6 | `ce9b2805` | `phase 6 openrouter model ids` |
| 7 | `79fd605e` | `phase 7 profile aware sync` |
| 8a | `ffd455c9` | `phase 8 test coverage` |
| 8b | `0a100e01` | `phase 8 verification` |
| 9 | this change | `phase 9 documentation` |

Each implementation phase began with `jj show --summary`, updated `.plans/multi-provider-plan.md`, and was committed separately. Phase 8 needed a second commit after the Nix shell was confirmed and final verification could run.

## Files Changed

- `.plans/multi-provider-plan.md`
- `.docs/architecture/embeddings.md`
- `.docs/abstraction/embeddings.md`
- `.docs/logic/embeddings.md`
- `.docs/multi-provider-plan-report.md`
- `Cargo.toml`
- `Cargo.lock`
- `src/embeddings/backend.rs`
- `src/embeddings/identity.rs`
- `src/embeddings/profile_registry.rs`
- `src/embeddings/mod.rs`
- `src/embeddings/qwen3.rs`
- `src/embeddings/fastembed_cpu.rs`
- `src/embeddings/openrouter.rs`
- `src/embeddings/token_lengths.rs`
- `src/config/indexer.rs`
- `src/indexing/identity.rs`
- `src/indexing/indexer_core.rs`
- `src/indexing/unified.rs`
- `src/mcp/sync.rs`
- `src/tools/index_tool.rs`
- `src/tools/project_paths.rs`
- `src/tools/query_tools.rs`

No Nix files were changed.

## Implemented Behavior

### Identity Codec

Added `src/embeddings/identity.rs` with `EmbeddingIdentity` and v2 encoding:

```text
emb;v=2;rt=<runtime>;model=<encoded>;dim=<n>;max=<n>;query=<encoded>
```

String-valued fields are percent-encoded with a filesystem-safe alphabet, so provider model ids and query policy tags can contain `/`, `:`, `=`, `;`, spaces, and newlines without breaking parsing or cache paths.

### Table-Driven Built-Ins

Replaced the profile enum/match-arm model with owned `EmbeddingProfile` data. `EmbeddingBackend` is now `Clone` instead of `Copy`, and built-ins live in a `LazyLock<Vec<EmbeddingProfile>>`.

Built-in profiles now carry:

- profile name and aliases
- runtime
- provider model id
- tokenizer model id
- dimension and max length
- query policy
- chunk defaults
- optional local loader spec

### Query Policy

Replaced static query formatting with `QueryPolicy`:

- `InstructionPrefix` for local embedding models.
- `InputType { document, query }` for OpenRouter.
- `None` for profiles with no query handling.

OpenRouter now reads `search_document` / `search_query` from profile data instead of hardcoding those values separately from the profile.

### Backend Identity Wiring

`EmbeddingBackend::identity()` writes v2 identities for new indexes.

`EmbeddingBackend::from_identity()` now handles:

- v2 API identities, including arbitrary OpenRouter model ids
- v2 local identities by recovering `LocalLoaderSpec` from the built-in registry
- legacy Candle Qwen3 identity
- legacy ONNX CPU identity
- legacy OpenRouter Qwen3 identity

Unknown local v2 model ids fail clearly and suggest clearing stale or foreign indexes.

### Dynamic API Profiles

Added `src/embeddings/profile_registry.rs` and the `toml` dependency for user profile loading.

Profile resolution is per request:

1. `RUST_CODE_MCP_EMBEDDING_PROFILES`
2. project-root `embedding_profiles.toml`
3. built-ins and aliases

TOML profiles are API-only. Local runtimes are rejected because local models require loader code. Unknown TOML fields are rejected, so credentials such as `api_key` cannot be silently stored.

### OpenRouter Model Generalization

`OpenRouterEmbedder::new` now sends `backend.model_id()` directly. Dynamic OpenRouter profiles can use any provider/model id accepted by OpenRouter without recompilation.

API keys still come only from environment variables and are not logged.

### Profile-Aware Search And Sync

Search now resolves vector metadata before constructing query embeddings. If an existing vector store uses a legacy identity string, the vector store is reopened with that exact stored identity while the query backend is rebuilt from it.

Background sync now enumerates existing per-profile vector indexes and syncs each with the backend recorded in its `metadata.json`. It skips unindexed directories and does not create a default-profile index while syncing a non-default profile.

### Verification Fixes

The final verification pass found one remaining `EmbeddingBackend` move after `Copy` was removed. `UnifiedIndexer::for_embedded_with_backend` now clones the backend when constructing `IndexerCore`, preserving the backend for the `UnifiedIndexer` struct.

Added a query-tool unit test that proves a legacy vector-store `embedder_version` is preserved at search-time metadata resolution.

## Documentation

Updated embedding docs to cover:

- `embedding_profiles.toml`
- `RUST_CODE_MCP_EMBEDDING_PROFILES`
- required and optional TOML fields
- built-in profile list
- v2 identity format
- legacy identity compatibility
- local models remaining code-bound by design

The main user-facing guide is `.docs/architecture/embeddings.md`.

## Verification

All verification was run through:

```sh
nix develop ../nix-devshells#cuda-code --command zsh -lc '<command>'
```

Commands run:

```sh
cargo check --lib
cargo check --tests
cargo test embeddings:: --lib
cargo test indexing::embedding_batcher --lib
cargo test tools::query_tools::tests::resolve_query_backend_preserves_legacy_vector_identity --lib
```

Results:

- `cargo check --lib`: passed.
- `cargo check --tests`: passed.
- `cargo test embeddings:: --lib`: 53 passed.
- `cargo test indexing::embedding_batcher --lib`: 8 passed.
- Legacy query metadata test: 1 passed.

Warnings remain in unrelated pre-existing areas such as graph, semantic, and older test modules. No verification failure remains.

The live OpenRouter smoke check was not run because it requires a real OpenRouter API key.

## Completion Notes

- Adding an OpenRouter embedding model is now a TOML/config change.
- Built-in profiles are centralized data rather than scattered enum match arms.
- New indexes use v2 identity strings.
- Existing legacy identities still parse and are preserved when opening old vector stores.
- Local Candle/ONNX models still require code changes by design.
- Dynamic profiles are restricted to OpenRouter API runtimes with clear validation.
- Background sync uses the profile each existing index was built with.

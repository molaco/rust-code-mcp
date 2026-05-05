# Rust Code MCP Workspace Split Plan

This plan splits the current monolithic `file-search-mcp` package into focused
`rust-code-mcp-*` crates while keeping the system buildable through a temporary
legacy `file-search-mcp` compatibility facade.

Hard rules:

- Do not run `cargo fmt`.
- Use `jj` first for VCS checks; fall back to `git` only if `jj` is unavailable.
- Keep `rmcp` only in `rust-code-mcp-server`.
- Keep `tokio` out of `rust-code-mcp-model` and `rust-code-mcp-syntax`.
- Keep graph as a real crate, not a server-only module.

## Final Crate Set

```text
crates/rust-code-mcp-model
crates/rust-code-mcp-syntax
crates/rust-code-mcp-embeddings
crates/rust-code-mcp-bm25
crates/rust-code-mcp-vector-store
crates/rust-code-mcp-search
crates/rust-code-mcp-indexing
crates/rust-code-mcp-graph
crates/rust-code-mcp-ra-analysis
crates/rust-code-mcp-server
```

Rust import names use underscores. For example, package
`rust-code-mcp-model` is imported as `rust_code_mcp_model`.

## Dependency DAG

```text
model
syntax -> model
embeddings -> model
bm25 -> model
vector-store -> model
search -> model + embeddings + bm25 + vector-store
indexing -> model + syntax + embeddings + bm25 + vector-store
graph -> rust-analyzer HIR/IDE stack + heed
ra-analysis -> rust-analyzer IDE stack
server -> all feature crates + rmcp
legacy file-search-mcp facade -> all feature crates temporarily
```

## Phase 0: Baseline

1. Run `jj status`.
2. Run `cargo check --lib --bins --tests`.
3. Run `cargo check --all-targets` as a known-failing baseline, not as a gate.
   Current all-target failures are in stale rust-analyzer examples using old
   `LoadCargoConfig` and `GotoDefinitionConfig` fields.
4. Record ignored/heavy tests:
   GPU embeddings, full incremental flow, burn performance, burn incremental,
   GPU JSON-RPC, and benchmark tests.
5. Do not run `cargo fmt`.

## Phase 1: Workspace Skeleton And Naming

1. Add `[workspace]`, `[workspace.package]`, and `[workspace.dependencies]`.
2. Use edition 2024.
3. Create empty crates under `crates/`.
4. Keep the current root package as a temporary legacy `file-search-mcp`
   compatibility facade to reduce churn in tests and examples.
5. New package names use `rust-code-mcp-*`.
6. Re-export new crate APIs through the legacy facade while extraction is in
   progress. Remove this facade in final cleanup once call sites are migrated.
7. Run `cargo check --lib --bins --tests`.

## Phase 2: Extract rust-code-mcp-model

Move shared domain types:

- `ChunkId`
- `ChunkContext`
- `CodeChunk`
- `CodeChunk::format_for_embedding`
- `Embedding = Vec<f32>`
- `EMBEDDING_DIM`

Dependencies:

- `serde`
- `uuid`

Rules:

- Do not move `anyhow` wrappers here.
- Do not move `ChunkWithEmbedding`; it belongs in embeddings.
- No `tokio`.
- Re-export through the temporary root facade.

Validation:

```text
cargo check -p rust-code-mcp-model --all-targets
cargo check --lib --bins --tests
```

## Phase 3: Extract rust-code-mcp-syntax

Move:

- `src/parser/*`
- `Chunker` from `src/chunker/mod.rs`

Public API:

- `RustParser`
- `ParseResult`
- `Symbol`
- `SymbolKind`
- `Visibility`
- `Range`
- `Import`
- `CallGraph`
- `TypeReference`
- `Chunker`

Dependencies:

- `rust-code-mcp-model`
- `ra_ap_syntax`

Rules:

- No `tokio`.
- No embeddings, vector store, BM25, or indexing dependencies.
- Tighten visibility only for clearly internal syntax helpers.

Validation:

```text
cargo check -p rust-code-mcp-syntax --all-targets
cargo check --lib --bins --tests
```

## Phase 4: Extract rust-code-mcp-embeddings

Move:

- `src/embeddings/*`

Keep here:

- `ChunkWithEmbedding`
- `EmbeddingGenerator`
- `EmbeddingPipeline`
- `EmbeddingError`

Dependencies:

- `rust-code-mcp-model`
- `fastembed`
- `ort`
- `tokio`
- `thiserror`
- `tracing`

Rules:

- Must not depend on vector store.
- Uses `Embedding`, `CodeChunk`, and `ChunkId` from model.

Validation:

```text
cargo check -p rust-code-mcp-embeddings --all-targets
cargo check --lib --bins --tests
```

## Phase 5: Extract rust-code-mcp-bm25

Move:

- `src/search/bm25.rs`
- `src/indexing/tantivy_adapter.rs`
- `TantivyConfig` from `src/config/indexer.rs`
- Chunk Tantivy schema from `src/schema.rs`

Decision point:

- If moving all of `src/schema.rs`, export both `ChunkSchema` and `FileSchema`.
- If keeping `FileSchema` server-side, move only `ChunkSchema` to BM25.

Boundary decision:

- Move Tantivy-specific helper logic from `src/tools/indexing_tools.rs` into
  BM25 or indexing before server extraction if those helpers are still needed.
- Preferred boundary: BM25 owns Tantivy index opening/schema helpers, so server
  does not depend on Tantivy.
- Fallback boundary: if `tools/indexing_tools.rs` remains server-side with
  direct `tantivy::Index` or `FileSchema` imports, server must list `tantivy`
  as a direct dependency.

Dependencies:

- `rust-code-mcp-model`
- `tantivy`
- `serde_json`
- `anyhow`
- `tracing`

Rules:

- `TantivyAdapter::create_bm25_search` should return the BM25 crate's
  `Bm25Search`, not reach through the old monolith path.
- Do not introduce server or `rmcp` dependencies.

Validation:

```text
cargo check -p rust-code-mcp-bm25 --all-targets
cargo check --lib --bins --tests
```

## Phase 6: Extract rust-code-mcp-vector-store

Move:

- `src/vector_store/*`

Public API:

- `VectorStore`
- `VectorStoreConfig`
- `VectorStoreBackend`
- `LanceDbBackend`
- `VectorStoreError`
- Current vector search result type

Optional rename:

- `SearchResult` to `VectorHit`, if ambiguity becomes painful.

Dependencies:

- `rust-code-mcp-model`
- `lancedb`
- `arrow-array`
- `arrow-schema`
- `async-trait`
- `futures`
- `directories`
- `serde`
- `serde_json`
- `thiserror`
- `tracing`

Rules:

- Do not depend on the embeddings implementation crate.
- Use the model crate's `Embedding` type alias only.

Validation:

```text
cargo check -p rust-code-mcp-vector-store --all-targets
cargo check --lib --bins --tests
```

## Phase 7: Extract rust-code-mcp-search

Move:

- `src/search/mod.rs`
- `src/search/error.rs`
- `src/search/resilient.rs`
- `src/search/rrf_tuner.rs`

Do not move:

- `src/search/bm25.rs`

Public API:

- `HybridSearch`
- `HybridSearchConfig`
- `VectorSearch`
- `SearchResult`
- `ResilientHybridSearch`
- RRF tuning types
- `SearchError`

Optional rename:

- `SearchResult` to `HybridHit`, if ambiguity becomes painful.

Dependencies:

- `rust-code-mcp-model`
- `rust-code-mcp-embeddings`
- `rust-code-mcp-bm25`
- `rust-code-mcp-vector-store`
- `tokio`
- `serde`
- `thiserror`
- `anyhow`
- `tracing`

Dev-dependencies:

- `serde_json`

Validation:

```text
cargo check -p rust-code-mcp-search --all-targets
cargo check --lib --bins --tests
```

## Phase 8: Extract rust-code-mcp-indexing

Move:

- `src/indexing/*`, except `tantivy_adapter.rs`
- `src/metadata_cache.rs`
- `src/security/*`
- `src/metrics/*`
- `IndexerConfig`
- `IndexerCoreConfig`
- `src/monitoring/backup.rs`

Keep for server:

- `src/monitoring/health.rs`

Public API:

- `UnifiedIndexer`
- `IncrementalIndexer`
- `IndexStats`
- `IndexFileResult`
- `IndexerCore`
- `ProcessedFile`
- `get_snapshot_path`

Dependencies:

- `rust-code-mcp-model`
- `rust-code-mcp-syntax`
- `rust-code-mcp-embeddings`
- `rust-code-mcp-bm25`
- `rust-code-mcp-vector-store`
- `sled`
- `sha2`
- `rs_merkle`
- `walkdir`
- `directories`
- `rayon`
- `regex`
- `glob`
- `sysinfo`
- `tokio`
- `anyhow`
- `serde`
- `bincode`
- `thiserror`
- `tracing`
- `num_cpus`

Direct `tantivy` dependency:

- Preferred path: hide Tantivy behind `rust-code-mcp-bm25` APIs.
- Current code has direct Tantivy touch points in consistency checks and
  `UnifiedIndexer`; if those remain after extraction, list `tantivy` as a
  direct indexing dependency.
- Remove that direct dependency later once the public surface no longer exposes
  or accepts raw Tantivy types.

Validation:

```text
cargo check -p rust-code-mcp-indexing --all-targets
cargo check --lib --bins --tests
```

## Phase 9: Extract rust-code-mcp-graph

Move:

- All of `src/graph/*`

Dependencies:

- `heed`
- `serde`
- `serde_json`
- `serde_bytes`
- `bincode`
- `sha2`
- `walkdir`
- `directories`
- `num_cpus`
- `anyhow`
- `tracing`
- `ra_ap_hir`
- `ra_ap_hir_def`
- `ra_ap_ide`
- `ra_ap_ide_db`
- `ra_ap_load-cargo`
- `ra_ap_project_model`
- `ra_ap_syntax`
- `ra_ap_vfs`

Rules:

- No `rmcp`.
- Keep graph one crate initially.
- Move graph examples/tests where practical.

Validation:

```text
cargo check -p rust-code-mcp-graph --all-targets
cargo check --lib --bins --tests
```

## Phase 10: Extract rust-code-mcp-ra-analysis

Move:

- `SemanticService`
- `Location`
- `semantic/loader.rs`
- `semantic/position.rs`

Do not move:

- The global `SEMANTIC` singleton wiring. Keep that in server.

Dependencies:

- `ra_ap_ide`
- `ra_ap_ide_db`
- `ra_ap_load-cargo`
- `ra_ap_project_model`
- `ra_ap_vfs`
- `anyhow`
- `num_cpus`
- `tracing`

Validation:

```text
cargo check -p rust-code-mcp-ra-analysis --all-targets
cargo check --lib --bins --tests
```

## Phase 11: Extract rust-code-mcp-server

Move:

- `src/tools/*`
- `src/mcp/*`
- `src/main.rs`
- `src/monitoring/health.rs`
- Top-level server `Config`
- `SEMANTIC` singleton wiring

Dependencies:

- All feature crates
- `rmcp`
- `tokio`
- `tracing`
- `tracing-subscriber`
- `serde`
- `serde_json`
- `directories`
- `sha2`
- `tantivy`, only if Tantivy-specific `tools/indexing_tools.rs` helpers remain
  server-side instead of moving behind BM25/indexing APIs

Rules:

- This is the only crate that depends on `rmcp`.
- Final binary name is `rust-code-mcp`.
- Server tools should import feature crate APIs directly.
- Prefer keeping Tantivy-specific helper code out of server; server should call
  BM25/indexing APIs instead of opening Tantivy indexes directly.

Validation:

```text
cargo check -p rust-code-mcp-server --all-targets
cargo check --lib --bins --tests
```

## Phase 12: Cleanup

1. Convert root to a virtual workspace, or keep a minimal legacy compatibility
   crate only if downstream callers still need it.
2. Remove the temporary legacy `file-search-mcp` facade when call sites are
   migrated.
3. Update tests and examples from `file_search_mcp::...` to exact new crates.
4. Fix, gate, or retire stale rust-analyzer examples so all-target checks become
   meaningful.
5. Remove unused dependencies from each crate.
6. Re-run architecture checks and dead-public review after extraction.

Final verification:

```text
cargo check --workspace --all-targets
cargo test --workspace --no-fail-fast
```

Do not run `cargo fmt`.

# Step-by-step plan: refactor module boundaries and prepare graph extraction

## Goal

Reduce the maintenance risk in the largest mixed-responsibility files without
changing runtime behavior, MCP tool schemas, cache formats, or public import
paths by accident.

The current high-risk files are:

| File | Current problem |
|---|---|
| `src/tools/graph_tools.rs` | MCP adapter, response enrichment, graph calls, semantic overlap, codemap orchestration, embedding cache helpers, and tests are mixed in one large module. |
| `src/graph/queries.rs` | Query result models and unrelated query families are mixed into one large `OpenedSnapshot` impl. |
| `src/graph/codemap.rs` | Codemap models, build logic, rendering, source utilities, and embedding/rerank orchestration are mixed. It also calls back into `tools::graph_tools`, which blocks clean graph extraction. |
| `src/chunker/mod.rs` | Types, formatting, chunking, and oversized split logic are mixed. |
| `src/parser/mod.rs` | Types, parser facade, AST symbol extraction, and helper logic are mixed. |
| `src/search/mod.rs` | Public types, vector search, hybrid search, and RRF logic are mixed. |

The desired end state is still one crate for now, with cleaner internal modules
and stable compatibility facades. Crate extraction should happen only after the
internal dependencies prove that the boundary is real.

## Constraints

- Use `jj status` before and after each phase.
- Do not run `cargo fmt` or any formatting command.
- Do not change MCP tool names, parameter schemas, or response JSON shapes in
  this refactor.
- Do not change embedding semantics, vector dimensions, model identities, graph
  snapshot schema, or index cache paths in this refactor.
- Preserve existing public imports through compatibility facades where practical.
- Prefer mechanical moves plus minimal import fixes.
- Keep each phase independently compile-checkable.
- If a phase becomes too large, stop and split it before continuing.

## Verification Commands

Run these after every code-moving phase:

```sh
jj status
cargo check --lib
```

Run these after graph-facing phases:

```sh
cargo check --examples
```

Run these after MCP tool-router or graph-tool phases:

```sh
cargo check --tests
```

Do not run formatting commands.

## Acceptance Criteria

- `cargo check --lib` passes after every phase.
- Public compatibility paths keep working, especially:
  - `file_search_mcp::graph::queries::*`
  - `file_search_mcp::tools::search_tool::*`
  - `file_search_mcp::tools::graph_tools::*` during the transition
  - `file_search_mcp::chunker::{Chunker, CodeChunk, ChunkId, ChunkSplitConfig}`
  - `file_search_mcp::parser::{RustParser, ParseResult, Symbol, SymbolKind}`
  - `file_search_mcp::search::{HybridSearch, SearchResult, HybridSearchConfig}`
- `src/graph/codemap.rs` no longer imports or calls `crate::tools::*`.
- `src/graph` has no dependency on `tools`.
- `graph_tools.rs`, `queries.rs`, and `codemap.rs` are reduced to facades or
  small compatibility modules.
- Crate extraction is not started until a dependency audit proves the boundary.

## Phase 0: Baseline and Safety Checks

Status: pending.

Purpose: make sure the refactor starts from a known state and that later
failures are attributable to the current phase.

Steps:

1. Confirm the working copy:

   ```sh
   jj status
   ```

2. Record current file sizes:

   ```sh
   wc -l src/tools/graph_tools.rs src/graph/queries.rs src/graph/codemap.rs src/chunker/mod.rs src/parser/mod.rs src/search/mod.rs src/embeddings/mod.rs
   ```

3. Rebuild or reuse the graph snapshot for structural checks:

   ```sh
   # Via MCP: build_hypergraph(directory, force_rebuild=false)
   # Via MCP: workspace_stats(directory)
   ```

4. Run the baseline compile check:

   ```sh
   cargo check --lib
   ```

5. Do not edit production code in this phase.

Acceptance:

- Working copy state is known.
- Baseline compile result is recorded.
- Current visibility and module-shape stats are recorded.

## Phase 1: Introduce Compatibility Facade Rules

Status: pending.

Purpose: define the compatibility pattern before moving code.

Steps:

1. Keep existing top-level module names during the first split:

   - `src/tools/graph_tools.rs`
   - `src/graph/queries.rs`
   - `src/graph/codemap.rs`
   - `src/search/mod.rs`
   - `src/chunker/mod.rs`
   - `src/parser/mod.rs`
   - `src/embeddings/mod.rs`

2. Convert each old large file into either:

   - a facade that declares focused submodules and `pub use`s the old surface, or
   - a transitional module that forwards to new focused modules.

3. Do not rename public result types in this phase.

4. Do not move MCP parameter structs out of `tools::search_tool` yet unless
   `tools::search_tool` continues to re-export the exact old names.

Acceptance:

- A reader can still find the old public entry points.
- Later phases can move implementation details without changing public imports.

## Phase 2: Split `tools::graph_tools` Into Tool Families

Status: pending.

Purpose: remove the worst MCP adapter bottleneck first while preserving the tool
router surface.

Target layout:

```text
src/tools/
  graph_tools.rs              # facade and compatibility exports
  graph_common.rs             # open snapshot, resolve nodes, JSON helpers, labels
  graph_response.rs           # enriched response structs and enrichment helpers
  graph_core_tools.rs         # imports, exports, reexports, uses, calls
  graph_crate_tools.rs        # crate_edges, dependency metrics, forbidden deps, overlaps
  graph_surface_tools.rs      # dead pub, docs, derives, attrs, re-export chain
  graph_audit_tools.rs        # unsafe, mut static, recursion, fn body, channels
  graph_similarity_tools.rs   # similar_to_item, semantic_overlaps
  graph_codemap_tool.rs       # handle_build_codemap adapter
  graph_embedding_cache.rs    # embedder_version, ensure_embeddings_for, cosine if still needed
```

Steps:

1. Move generic helpers first:

   - `open_workspace_snapshot`
   - `resolve_required_node`
   - `json_result`
   - `internal_error`
   - label helpers
   - item-kind parsing helpers

2. Move response structs and enrichment helpers:

   - enriched bindings
   - enriched usages
   - enriched dead-pub rows
   - common file/span response structs

3. Move simple tool families in small batches:

   - core graph lookup tools
   - crate/dependency tools
   - public-surface tools
   - audit tools
   - similarity tools
   - codemap adapter

4. Leave `src/tools/graph_tools.rs` as a facade:

   ```rust
   pub use graph_core_tools::*;
   pub use graph_crate_tools::*;
   pub use graph_surface_tools::*;
   pub use graph_audit_tools::*;
   pub use graph_similarity_tools::*;
   pub use graph_codemap_tool::*;
   ```

5. Update `src/tools/mod.rs` to declare the new internal modules.

6. Keep `src/tools/search_tool_router.rs` calling through
   `crate::tools::graph_tools::*` until the phase is stable.

7. Run:

   ```sh
   cargo check --lib
   cargo check --tests
   ```

Acceptance:

- `graph_tools.rs` becomes mostly a compatibility facade.
- `search_tool_router.rs` does not require a broad rewrite.
- All MCP graph tools still compile under their existing names.

## Phase 3: Fix the Codemap Boundary

Status: pending.

Purpose: make `graph::codemap` stop depending on `tools`, and preferably stop
depending directly on `embeddings` and `search`.

Current problem:

- `src/graph/codemap.rs` calls `crate::tools::graph_tools::embedder_version`.
- `src/graph/codemap.rs` calls `crate::tools::graph_tools::ensure_embeddings_for`.
- `src/graph/codemap.rs` calls `crate::tools::graph_tools::cosine`.
- `src/graph/codemap.rs` constructs `EmbeddingGenerator`.
- `src/graph/codemap.rs` accepts `crate::search::SearchResult` for seed hits.

Target split:

```text
src/graph/codemap.rs          # compatibility facade
src/graph/codemap_model.rs    # Codemap, CodemapNode, CodemapEdge, options
src/graph/codemap_build.rs    # pure graph expansion and scoring
src/graph/codemap_render.rs   # JSON-independent mermaid/outline renderers
src/graph/codemap_source.rs   # source mtime, snippets, line/byte helpers
src/tools/graph_codemap_tool.rs
src/tools/graph_embedding_cache.rs
```

Steps:

1. Move serializable codemap types into `codemap_model.rs`.

2. Move render functions into `codemap_render.rs`.

3. Move source utilities into `codemap_source.rs`.

4. Introduce graph-owned input structs so `graph::codemap` does not need
   `crate::search::SearchResult`:

   ```rust
   pub struct CodemapSeedHit {
       pub file: String,
       pub line_start: u32,
       pub line_end: u32,
       pub score: f32,
   }

   pub struct CodemapRerankInput {
       pub prompt_embedding: Option<Vec<f32>>,
       pub node_embeddings: HashMap<NodeId, Vec<f32>>,
       pub embeddings_computed: usize,
   }
   ```

5. Move conversion from `SearchResult` to `CodemapSeedHit` into
   `tools::graph_codemap_tool`.

6. Move prompt embedding generation and missing embedding-cache computation into
   `tools::graph_codemap_tool` or `tools::graph_embedding_cache`.

7. Move `cosine` to either:

   - `src/graph/codemap_build.rs` if it is only codemap scoring, or
   - `src/semantic/similarity.rs` if semantic overlap also needs it.

8. Keep `graph::codemap::build_codemap` focused on:

   - resolving seed IDs,
   - expanding graph neighbors,
   - ranking with already-provided rerank data,
   - pruning,
   - building the serializable codemap.

9. Ensure `src/graph/codemap.rs` does not import `crate::tools`.

10. Run:

   ```sh
   cargo check --lib
   cargo check --tests
   ```

Acceptance:

- `rg -n "crate::tools" src/graph` returns no production dependency from graph
  to tools.
- `build_codemap` remains available from `crate::graph::codemap`.
- Codemap MCP tool behavior is unchanged.

## Phase 4: Split `graph::queries` by Query Family

Status: pending.

Purpose: reduce the 3600-line query file while preserving the old
`graph::queries::*` import path.

Target layout:

```text
src/graph/
  queries.rs                  # compatibility facade
  query_model.rs              # shared result structs and filters
  query_lookup.rs             # lookup_by_qualified_name, root module lookup
  query_usage.rs              # imports, exports, uses, calls, call graph
  query_surface.rs            # dead pub, attrs, enum variants, re-export chain
  query_crates.rs             # crate_edges, dependency metric, forbidden deps
  query_functions.rs          # signatures and function filtering
  query_stats.rs              # workspace_stats, overlaps, module_tree
```

Steps:

1. Move shared structs and enums to `query_model.rs`.

2. Keep `queries.rs` re-exporting all moved public types:

   ```rust
   pub use query_model::*;
   pub use query_usage::*;
   pub use query_surface::*;
   pub use query_crates::*;
   pub use query_functions::*;
   pub use query_stats::*;
   ```

3. Split the `impl OpenedSnapshot` methods by family. Multiple impl blocks in
   different modules are acceptable.

4. Keep private helpers close to their owning query family where possible.

5. Move truly shared helpers into `query_model.rs` or a small
   `query_helpers.rs`.

6. Run:

   ```sh
   cargo check --lib
   cargo check --examples
   cargo check --tests
   ```

Acceptance:

- `src/graph/queries.rs` is a facade.
- Existing references to `crate::graph::queries::TypeName` still compile.
- No query behavior changes.

## Phase 5: Split `search::mod`

Status: pending.

Purpose: make search internals easier to change without disturbing the public
facade.

Target layout:

```text
src/search/
  mod.rs                      # facade only
  types.rs                    # SearchResult, HybridSearchConfig
  vector.rs                   # VectorSearch
  hybrid.rs                   # HybridSearch
  rrf.rs                      # reciprocal rank fusion core
  bm25.rs
  resilient.rs
  rrf_tuner.rs
  error.rs
```

Steps:

1. Move `HybridSearchConfig` and `SearchResult` to `types.rs`.

2. Move `VectorSearch` to `vector.rs`.

3. Move `HybridSearch` to `hybrid.rs`.

4. Move `reciprocal_rank_fusion_core` to `rrf.rs`.

5. Keep `search/mod.rs` re-exporting the old public surface:

   ```rust
   pub use types::{HybridSearchConfig, SearchResult};
   pub use vector::VectorSearch;
   pub use hybrid::HybridSearch;
   ```

6. Run:

   ```sh
   cargo check --lib
   cargo check --tests
   ```

Acceptance:

- `file_search_mcp::search::HybridSearch` still compiles.
- `file_search_mcp::search::SearchResult` still compiles.
- Search behavior is unchanged.

## Phase 6: Split `chunker::mod`

Status: pending.

Purpose: isolate types and oversized chunk splitting from the chunker facade.

Target layout:

```text
src/chunker/
  mod.rs                      # facade only
  types.rs                    # ChunkId, ChunkContext, CodeChunk, ChunkSplitConfig
  chunker.rs                  # Chunker and symbol-based chunking
  split.rs                    # oversized split/token-limit logic
```

Steps:

1. Move `ChunkId`, `ChunkContext`, `CodeChunk`, and `ChunkSplitConfig` to
   `types.rs`.

2. Move `Chunker` and symbol-based chunk construction to `chunker.rs`.

3. Move oversized line-splitting logic to `split.rs`.

4. Keep `chunker/mod.rs` re-exporting:

   ```rust
   pub use types::{ChunkId, ChunkContext, CodeChunk, ChunkSplitConfig};
   pub use chunker::Chunker;
   ```

5. Run:

   ```sh
   cargo check --lib
   cargo check --examples
   ```

Acceptance:

- Existing chunker imports still compile.
- Chunk IDs, formatting, and split metadata remain unchanged.

## Phase 7: Split `parser::mod`

Status: pending.

Purpose: isolate parser types from AST symbol extraction implementation.

Target layout:

```text
src/parser/
  mod.rs                      # facade only
  types.rs                    # Symbol, SymbolKind, Visibility, Range, ParseResult
  rust_parser.rs              # RustParser
  symbols.rs                  # extract_symbols_recursive and helpers
  imports.rs
  call_graph.rs
  type_references.rs
```

Steps:

1. Move public parser data types into `types.rs`.

2. Move `RustParser` into `rust_parser.rs`.

3. Move recursive AST symbol extraction and helper functions into `symbols.rs`.

4. Keep `parser/mod.rs` re-exporting:

   ```rust
   pub use rust_parser::RustParser;
   pub use types::{ParseResult, Range, Symbol, SymbolKind, Visibility};
   ```

5. Run:

   ```sh
   cargo check --lib
   cargo check --examples
   ```

Acceptance:

- Existing parser imports still compile.
- Parser output shapes are unchanged.

## Phase 8: Split `embeddings` Only Around the Multi-Backend Work

Status: pending.

Purpose: avoid premature churn, but prepare the module shape for OpenRouter,
local GPU, and local CPU profiles.

Recommended target layout when backend work starts:

```text
src/embeddings/
  mod.rs                      # facade only
  types.rs                    # Embedding, ChunkWithEmbedding
  generator.rs                # EmbeddingGenerator public API
  pipeline.rs                 # EmbeddingPipeline
  backend.rs                  # model/profile identity
  token_lengths.rs
  error.rs
  providers/
    mod.rs
    qwen3_candle.rs
    openrouter.rs
    fastembed_onnx.rs
```

Steps:

1. Do not split `embeddings/mod.rs` before the model-profile implementation
   unless it becomes a blocker.

2. When implementing profiles, first move only public types and generator API:

   - `Embedding`
   - `ChunkWithEmbedding`
   - `EmbeddingGenerator`
   - `EmbeddingPipeline`

3. Keep `embeddings/mod.rs` re-exporting the old public names.

4. Keep provider modules private until the runtime/profile design is stable.

5. Run:

   ```sh
   cargo check --lib
   cargo check --examples
   ```

Acceptance:

- Existing embedding imports still compile.
- No backend behavior changes happen as part of pure file movement.

## Phase 9: Visibility Cleanup Pass

Status: pending.

Purpose: reduce accidental public surface after the file layout makes ownership
clearer.

Current verified stats:

| Metric | Value |
|---|---:|
| `pub` items | 508 |
| `pub(crate)` items | 25 |
| `pub_crate_share` | 0.0469 |

Steps:

1. Rebuild the hypergraph:

   ```text
   build_hypergraph(directory, force_rebuild=true)
   ```

2. Run:

   ```text
   workspace_stats(directory)
   dead_pub_in_crate(directory, "file_search_mcp")
   ```

3. Convert candidates from `pub` to `pub(crate)` only when they are not part of:

   - MCP tool public API,
   - documented library examples,
   - tests/examples that intentionally use the library externally,
   - public facade compatibility paths.

4. Work module by module:

   - graph internals,
   - tools internals,
   - parser internals,
   - chunker internals,
   - search internals,
   - embeddings internals.

5. Run:

   ```sh
   cargo check --lib
   cargo check --examples
   cargo check --tests
   ```

6. Re-run `workspace_stats` and record the new `pub_crate_share`.

Acceptance:

- Public facade imports still compile.
- Internal helpers are not unnecessarily `pub`.
- `pub_crate_share` improves materially without breaking external examples.

## Phase 10: Dependency Boundary Audit

Status: pending.

Purpose: decide whether `graph` is ready to become a crate.

Steps:

1. Check production imports:

   ```sh
   rg -n "crate::(tools|embeddings|search|indexing|vector_store|chunker|parser)::" src/graph
   ```

2. Expected result before crate extraction:

   - no `crate::tools` imports,
   - no `crate::embeddings` imports,
   - no `crate::search` imports,
   - no `crate::indexing` imports,
   - no `crate::vector_store` imports,
   - parser/rust-analyzer dependencies only where extraction/audit code truly
     needs them.

3. Use graph tooling to inspect crate edges once the split is stable:

   ```text
   build_hypergraph(directory, force_rebuild=true)
   crate_edges(directory)
   crate_dependency_metric(directory)
   ```

4. Document remaining graph dependencies and whether they are acceptable crate
   dependencies.

Acceptance:

- There is a written go/no-go decision for graph crate extraction.
- Any remaining graph dependency has a clear reason.

## Phase 11: Optional `rmc-graph` Crate Extraction

Status: blocked until Phase 10 passes.

Purpose: lift only the strongest real boundary after the in-crate split has
proven stable.

Target layout:

```text
crates/rmc-graph/
  Cargo.toml
  src/lib.rs
  src/ids.rs
  src/model.rs
  src/storage.rs
  src/snapshot.rs
  src/loader.rs
  src/extract/
  src/query/
  src/audit/
  src/codemap/
```

Steps:

1. Create the crate with the same graph modules.

2. Move graph code mechanically.

3. Keep `src/graph.rs` or `src/graph/mod.rs` in the main crate as a
   compatibility re-export:

   ```rust
   pub use rmc_graph::*;
   ```

4. Move only graph-specific tests with the graph crate.

5. Update main crate imports to use `crate::graph::*` compatibility first, then
   optionally convert to `rmc_graph::*` in a later cleanup.

6. Run:

   ```sh
   cargo check --workspace
   cargo check --examples
   cargo check --tests
   ```

Acceptance:

- Main crate public graph imports still work.
- `rmc-graph` builds independently.
- No MCP tool schemas change.

## Phase 12: Optional Engine and MCP Crate Split

Status: future work only.

Purpose: split larger runtime containers only after graph extraction and
embedding-profile work settle.

Do not split `indexing`, `search`, `embeddings`, `vector_store`, `chunker`, and
`parser` into separate crates individually yet. They form one dense engine
cluster.

Possible later layout:

```text
crates/rmc-engine/
  src/chunker/
  src/parser/
  src/embeddings/
  src/vector_store/
  src/indexing/
  src/search/

crates/rmc-mcp/
  src/router.rs
  src/tools/
  src/sync.rs
  src/project_paths.rs
```

Acceptance before this phase:

- `rmc-graph` is already stable.
- embedding profiles have stable cache identity behavior.
- engine boundary has a clear API.
- MCP layer is already thin and adapter-only.

## Rollback Strategy

Use small changesets. If a phase fails in a confusing way:

1. Stop editing.
2. Inspect:

   ```sh
   jj status
   jj diff
   ```

3. Revert only the current phase's own edits if needed.
4. Do not revert unrelated user changes.
5. Split the phase into smaller mechanical moves.

## Suggested Commit Boundaries

Use one commit per phase or subphase:

1. `refactor plan 99`
2. `split graph tool common helpers`
3. `split graph tool families`
4. `untangle codemap tool dependencies`
5. `split graph query modules`
6. `split search facade`
7. `split chunker facade`
8. `split parser facade`
9. `tighten visibility after module split`
10. `audit graph crate boundary`

## Final Done Definition

The refactor is complete when:

- the largest files are split behind compatibility facades,
- `graph` no longer depends on `tools`,
- codemap embedding orchestration lives outside graph core,
- public imports and MCP tool schemas are unchanged,
- visibility is materially tighter,
- and there is a documented decision on whether to extract `rmc-graph`.

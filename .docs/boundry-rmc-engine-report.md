# rmc-engine Boundary Report

## Status

- Crate: `rmc-engine`
- Graph qualified name: `rmc_engine`
- Analysis order: 1 of 4
- Current phase: Phase 3 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | c35970b1 | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Complete | b393df22 | Root exposes six domain modules; submodule facades reexport public API types. |
| Phase 2: Dependency boundary | Complete | 549dbff6 | No outgoing `rmc_*` dependency violations; one outgoing edge to `fastembed`. |
| Phase 3: Import and usage coupling | Complete | Pending commit | Coupling centers on embedding, chunk, vector-store, parser, and search boundary types. |
| Phase 4: Internal cohesion | Pending | Not started |  |
| Phase 5: Targeted source reads and recommendations | Pending | Not started |  |

## Phase 0: Snapshot Readiness And Baseline

### Required VCS Check

Before Phase 0, `jj show --summary` reported:

```text
Commit ID: 37c0699ac2940e3010afa3973ea8f32009998b8e
Change ID: zmuwpyquxtoxnswyvmylqnxquwxwztku
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
build_hypergraph(directory="/home/molaco/Documents/rust-code-mcp-refactor", force_rebuild=false)
workspace_stats(directory="/home/molaco/Documents/rust-code-mcp-refactor")
crate_edges(directory="/home/molaco/Documents/rust-code-mcp-refactor", summary=true, limit=200)
crate_dependency_metric(directory="/home/molaco/Documents/rust-code-mcp-refactor", sort_by="instability", summary=true, limit=200)
```

Snapshot result:

```text
graph_id: 4fc200b6ab2a6d0ef4162f4fec31da5f
fingerprint: a2800cb435de19d32f27bf58901fd5efb037e85565033279dd50611589501073
node_count: 3040
binding_count: 5371
usage_count: 7963
reused: true
```

Workspace baseline:

```text
crates: 45
modules: 296
items: 2448
external symbols: 250
declared bindings: 1830
named imports: 1958
glob imports: 1583
pub items: 339
pub(crate) items: 298
module-private items: 1095
pub_crate_share: 0.46781789638932497
```

Item mix:

```text
functions: 1096
methods: 676
structs: 350
enums: 41
enum variants: 190
traits: 7
type aliases: 10
consts: 70
statics: 5
```

`rmc_engine` baseline from `crate_dependency_metric`:

```text
crate_name: rmc_engine
item_count: 555
efferent: 1
afferent: 14
instability: 0.06666666666666667
abstractness: 0.0018018018018018018
```

### Phase 0 Interpretation

`rmc_engine` currently behaves like a stable foundation crate: many crates depend
on it, while it has only one outgoing producer edge in the hypergraph metrics.
That is consistent with the expected boundary hypothesis that engine should own
low-level parser, chunker, embedding, search, and vector-store primitives.

The low abstractness value is expected for this codebase because most of the
engine surface appears to be concrete implementation types rather than traits.
This is not automatically a smell for a primitive crate, but later phases need
to check whether concrete internals are exposed as public API.

### Phase 0 Findings

- No snapshot rebuild was required; the existing graph snapshot was reusable.
- `rmc_engine` is the most stable of the four target crates by instability
  score: `0.06666666666666667`.
- `rmc_engine` has the largest item count among the four target crates at this
  baseline: `555`.
- The high afferent count (`14`) means facade quality matters: many examples,
  tests, and sibling crates can form dependencies on whatever engine exposes.

### Open Questions For Later Phases

- What is the single outgoing dependency from `rmc_engine`, and is it expected?
- Does the root module provide a deliberate facade, or do callers import deep
  modules directly?
- Are parser/chunker/embedding/search/vector-store responsibilities cleanly
  separated inside the crate?
- Are public implementation types needed by downstream crates, or are they
  boundary leaks?

## Phase 1: Public Surface

### Required VCS Check

Before Phase 1, `jj show --summary` reported:

```text
Commit ID: 98bf747d3c4ff4c748323ac411d9754d9969e312
Change ID: ostmmnwwkryozrtxrtswqmttplqswxyr
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
module_tree(directory, krate="rmc_engine")
get_exports(directory, module="rmc_engine", consumer="rmc_engine", summary=true, limit=300)
get_declared_reexports(directory, module="rmc_engine", summary=false, limit=300)
pub_use_pub_type_audit(directory, crate_name="rmc_engine", summary=true, limit=300)

get_exports(directory, module="rmc_engine::chunker", consumer="rmc_engine", summary=true, limit=300)
get_exports(directory, module="rmc_engine::embeddings", consumer="rmc_engine", summary=true, limit=300)
get_exports(directory, module="rmc_engine::vector_store", consumer="rmc_engine", summary=true, limit=300)
get_exports(directory, module="rmc_engine::parser", consumer="rmc_engine", summary=true, limit=300)
get_exports(directory, module="rmc_engine::schema", consumer="rmc_engine", summary=true, limit=300)
get_exports(directory, module="rmc_engine::search", consumer="rmc_engine", summary=true, limit=300)

get_declared_reexports(directory, module=<same six submodules>, summary=false, limit=300)
```

Root exports:

```text
rmc_engine::chunker
rmc_engine::embeddings
rmc_engine::parser
rmc_engine::schema
rmc_engine::search
rmc_engine::vector_store
```

Root declared reexports:

```text
count: 0
```

`pub_use_pub_type_audit` findings:

```text
count: 0
```

Submodule export counts:

```text
chunker: 5
embeddings: 21
vector_store: 9
parser: 9
schema: 2
search: 11
```

Declared reexport counts by submodule:

```text
chunker: 5
embeddings: 16
vector_store: 3
parser: 7
schema: 0
search: 3
```

Important public submodule facades:

```text
chunker:
  Chunker -> rmc_engine::chunker::chunker::Chunker
  CodeChunk, ChunkContext, ChunkId, ChunkSplitConfig -> rmc_engine::chunker::types::*

embeddings:
  EmbeddingGenerator, ChunkWithEmbedding, Embedding
  EmbeddingBackend, EmbeddingRuntime
  EmbeddingProfile, Qwen3Variant, FastembedCpuModel, LocalLoaderSpec, QueryPolicy
  OpenRouterRuntimeConfig, OpenRouterProviderPreferences, OpenRouterProviderSort
  OpenRouterEncodingFormat, openrouter_runtime_config
  EmbeddingError, EmbeddingTokenCounter, EmbeddingTextLen, resolve_profile

vector_store:
  VectorStore, VectorStoreConfig, VectorSearchResult
  LanceDbBackend, VectorStoreBackend, VectorStoreError
  public modules: lancedb, traits, error

parser:
  RustParser
  ParseResult, Range, Symbol, SymbolKind, Visibility
  public modules: call_graph, imports, type_references

schema:
  FileSchema, ChunkSchema

search:
  HybridSearch, HybridSearchConfig, SearchResult
  Bm25Search, ResilientHybridSearch, SearchError
  public modules: bm25, resilient, rrf_tuner, error
```

### Phase 1 Interpretation

`rmc_engine` has a deliberate module-level root facade: the root only exposes
the six engine domains and does not reexport individual types. The more useful
facades are one level down, where `chunker`, `embeddings`, `vector_store`,
`parser`, and `search` reexport key types from deeper implementation modules.

This is mostly a clean primitive-crate shape. The main boundary risk is that
some implementation modules are public alongside their facade reexports:
`vector_store::lancedb`, `vector_store::traits`, `search::bm25`,
`search::resilient`, `search::rrf_tuner`, `search::error`, and parser helper
modules are visible as modules. That may be intentional for advanced callers,
but Phase 3 needs to check whether downstream crates import those deep modules
instead of the intended one-level facades.

The `embeddings` facade is broad. It exports runtime config, provider
preferences, backend/runtime selection, profile registry, token length helpers,
and the generator API. That breadth may be justified because embeddings are the
most configurable engine subsystem, but it is also the area most likely to leak
provider-specific policy upward.

### Phase 1 Findings

- The root facade is narrow and intentional: six public domain modules, no
  individual root-level type reexports.
- There are no `pub type` alias reexport-smell findings from
  `pub_use_pub_type_audit`.
- `chunker` has the tightest facade: five public types, all directly related to
  chunk production and chunk metadata.
- `schema` is minimal: `FileSchema` and `ChunkSchema`.
- `embeddings` is the largest public surface with 21 visible exports and 16
  explicit reexports from deeper modules.
- `vector_store` and `search` both expose facade types and implementation
  modules; usage analysis must determine whether those modules are real
  extension points or leaked internals.

### Open Questions For Later Phases

- Do sibling crates import `rmc_engine::chunker::Chunker`, or do they rely on
  `rmc_engine::chunker::chunker::Chunker`?
- Does any downstream code import `rmc_engine::vector_store::lancedb`
  directly?
- Are OpenRouter-specific embedding config types used outside engine, or should
  they be hidden behind `EmbeddingProfile` or `EmbeddingBackend`?
- Is `parser` still a true low-level engine primitive now that graph owns
  rust-analyzer/HIR extraction?

## Phase 2: Dependency Boundary

### Required VCS Check

Before Phase 2, `jj show --summary` reported:

```text
Commit ID: 96b5a6778c2e940325d6550e88ae2d5481637d25
Change ID: utlyyskvnwzyvulnvzvxlmpqpxszrnls
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
crate_dependency_metric(directory, sort_by="efferent", summary=true, limit=200)
crate_dependency_metric(directory, sort_by="afferent", summary=true, limit=200)
forbidden_dependency_check(directory, rules=[{ consumer="rmc_engine", producer="*" }], summary=false, limit=100)
forbidden_dependency_check(directory, rules=[{ consumer="*", producer="rmc_engine" }], summary=true, limit=200)
forbidden_dependency_check(directory, rules=<expected layering rules>, summary=false, limit=300)
```

`rmc_engine` dependency metric:

```text
efferent: 1
afferent: 14
instability: 0.06666666666666667
abstractness: 0.0018018018018018018
item_count: 555
```

Outgoing edge filter:

```text
consumer: rmc_engine
producer: fastembed
sample_symbol: fastembed::models::qwen3::Qwen3TextEmbedding
unique_symbols: 11
total_refs: 18
```

Incoming production/library edge filter from `forbidden_dependency_check`:

```text
rmc_config -> rmc_engine
  sample_symbol: rmc_engine::embeddings::profile::EmbeddingProfile
  unique_symbols: 4
  total_refs: 7

rmc_graph -> rmc_engine
  sample_symbol: rmc_engine::embeddings::backend::EmbeddingBackend
  unique_symbols: 7
  total_refs: 11

rmc_indexing -> rmc_engine
  sample_symbol: rmc_engine::embeddings::backend::EmbeddingBackend
  unique_symbols: 46
  total_refs: 198

rmc_server -> rmc_engine
  sample_symbol: rmc_engine::embeddings::backend::EmbeddingBackend
  unique_symbols: 38
  total_refs: 153
```

Expected-layering rule result:

```text
violation_count: 0
```

### Phase 2 Interpretation

The dependency boundary is healthy. `rmc_engine` does not depend on `rmc_server`,
`rmc_graph`, `rmc_indexing`, or any other `rmc_*` crate in the MCP dependency
graph. Its single outgoing edge is to `fastembed`, represented by Qwen3
embedding model usage, which fits the engine role because embedding backends are
owned by the engine layer.

The inbound production/library edges show that `rmc_engine` is a genuine
foundation crate. `rmc_indexing` and `rmc_server` are the heaviest production
consumers by symbol/reference count. Both consume `EmbeddingBackend` heavily,
so the embedding backend/profile API is the most important engine boundary to
keep stable and intentional.

`rmc_config` depending on `rmc_engine::embeddings::profile::EmbeddingProfile` is
not an immediate violation of the stated layering hypothesis, but it is worth
watching. Config depending on engine profile types means embedding profile
configuration is not purely external to engine; that can be acceptable if
engine owns the canonical profile model.

### Phase 2 Findings

- No expected-layering violations were reported.
- `rmc_engine` has no outgoing dependency to another `rmc_*` crate.
- The only outgoing dependency edge is `rmc_engine -> fastembed`.
- Production/library incoming edges are from `rmc_config`, `rmc_graph`,
  `rmc_indexing`, and `rmc_server`.
- The highest incoming production usage is from `rmc_indexing` with 46 unique
  engine symbols and 198 total references.
- `EmbeddingBackend` is the central boundary type for graph, indexing, and
  server consumers.

### Open Questions For Later Phases

- Are consumers using the engine embedding facade or deep provider-specific
  modules?
- Is `EmbeddingBackend` carrying too much cross-layer policy?
- Should `rmc_config` depend on engine profile types, or should profile config
  live in a smaller shared configuration model?
- Are the many `rmc_indexing` references mostly through stable facade types?

## Phase 3: Import And Usage Coupling

### Required VCS Check

Before Phase 3, `jj show --summary` reported:

```text
Commit ID: 21f5ca5e005118b37c65a8779cea6ef9399efd52
Change ID: ztqywnttnzlsmzzqwoktowusoqtzyupw
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
get_imports(directory, module="rmc_engine", summary=true, limit=300)
module_dependencies(directory, module="rmc_engine", summary=true, limit=300)

who_imports(directory, target=<key engine type>, summary=true, limit=100)
who_uses_summary(directory, target=<key engine type>, summary=true, limit=100)

get_imports(directory, module=<MCP-identified consumer module>, summary=true, limit=200)
```

Root coupling:

```text
get_imports(rmc_engine): 0
module_dependencies(rmc_engine): 0
```

Key import counts:

```text
Chunker: 8 import bindings
CodeChunk: 25 import bindings
EmbeddingBackend: 30 import bindings
EmbeddingGenerator: 15 import bindings
EmbeddingProfile: 11 import bindings
VectorStore: 14 import bindings
LanceDbBackend: 3 import bindings
Bm25Search: 8 import bindings
HybridSearch: 9 import bindings
RustParser: 9 import bindings
```

Key usage summary:

```text
EmbeddingBackend:
  total consumer modules: 26
  highest external production consumers:
    rmc_server::mcp::project_paths: 10
    rmc_server::tools::endpoints::query: 10
    rmc_indexing::indexing::incremental: 7
    rmc_indexing::indexing::identity: 6
    rmc_indexing::indexing::unified: 6

VectorStore:
  total consumer modules: 9
  external production consumers:
    rmc_indexing::indexing::unified: 3
    rmc_indexing::indexing::consistency: 2
    rmc_indexing::monitoring::health: 2
    rmc_server::tools::endpoints::health: 1
    rmc_server::tools::endpoints::query: 1

CodeChunk:
  total consumer modules: 14
  external production consumers:
    rmc_indexing::indexing::unified: 3
    rmc_indexing::indexing::embedding_batcher: 2
    rmc_indexing::indexing::indexer_core: 2
    rmc_indexing::indexing::tantivy_adapter: 2

EmbeddingGenerator:
  total consumer modules: 12
  external production consumers:
    rmc_indexing::indexing::embedding_batcher: 3
    rmc_indexing::indexing::indexer_core: 2
    rmc_graph::graph::codemap::build: 1
    rmc_graph::graph::embedding_cache: 1
    rmc_indexing::indexing::unified: 1
    rmc_server::tools::endpoints::query: 1
```

Consumer-module import samples:

```text
rmc_indexing::indexing::indexer_core imports:
  rmc_engine::embeddings::backend::EmbeddingBackend
  rmc_engine::chunker::chunker::Chunker
  rmc_engine::chunker::types::ChunkSplitConfig
  rmc_engine::parser::rust_parser::RustParser
  rmc_engine::embeddings::Embedding
  rmc_engine::embeddings::EmbeddingGenerator
  rmc_engine::chunker::types::CodeChunk

rmc_indexing::indexing::unified imports:
  rmc_engine::embeddings::backend::EmbeddingBackend
  rmc_engine::chunker::types::CodeChunk
  rmc_engine::embeddings::EmbeddingGenerator
  rmc_engine::chunker::types::ChunkId
  rmc_engine::vector_store::VectorStore

rmc_server::tools::endpoints::query imports:
  rmc_engine::embeddings::backend::EmbeddingBackend
  rmc_engine::vector_store::VectorStore
  rmc_engine::embeddings::EmbeddingGenerator
  rmc_engine::search::HybridSearch

rmc_server::tools::endpoints::health imports:
  rmc_engine::search::bm25::Bm25Search
  rmc_engine::embeddings::backend::EmbeddingBackend
  rmc_engine::vector_store::VectorStore

rmc_config::config::indexer imports:
  rmc_engine::embeddings::profile::EmbeddingProfile
```

### Phase 3 Interpretation

`rmc_engine` itself has no root imports or module dependencies, which is
consistent with a low-level crate root that only declares the public module
tree.

The important external coupling is concentrated in a small set of stable engine
concepts:

- embeddings: `EmbeddingBackend`, `EmbeddingGenerator`, `EmbeddingProfile`
- chunking: `CodeChunk`, `Chunker`, `ChunkId`, `ChunkSplitConfig`
- vector storage: `VectorStore`
- search: `HybridSearch`, `Bm25Search`
- parser: `RustParser`

This is mostly coherent for an engine crate, but the import targets show a
boundary sharpness issue: many canonical targets live in deep implementation
modules such as `embeddings::backend`, `chunker::types`, `chunker::chunker`,
`parser::rust_parser`, and `search::bm25`. Because the MCP import target is the
canonical declaration, this does not prove callers wrote deep paths in source;
some may import through one-level facade reexports. Phase 5 should verify the
actual source paths for `rmc_server::tools::endpoints::health`,
`rmc_indexing::indexing::indexer_core`, and `rmc_config::config::indexer`.

`LanceDbBackend` looks contained: importers are the `vector_store` facade and
engine tests, with no external production consumer found in this pass. That
suggests the public `lancedb` module is not currently causing external
coupling, even though it remains part of the public surface.

### Phase 3 Findings

- `EmbeddingBackend` is the largest cross-crate boundary type and is used by
  server, indexing, graph, config/test utilities, and engine internals.
- `CodeChunk` is shared across chunking, embeddings, search, vector store, and
  indexing; it is a true engine data model boundary.
- `VectorStore` is used by indexing and server endpoints, not just engine
  search internals.
- `Bm25Search` is imported by server health and indexing health modules; this
  deserves source-path verification because it may represent direct dependence
  on a concrete search backend.
- `RustParser` is imported by indexing and server analysis paths; this is
  expected for syntax-level parsing but should stay separate from graph's HIR
  extraction ownership.
- `EmbeddingProfile` is imported by `rmc_config`, confirming the config/engine
  coupling observed in Phase 2.

### Open Questions For Later Phases

- Do source imports use `rmc_engine::embeddings::EmbeddingBackend` or
  `rmc_engine::embeddings::backend::EmbeddingBackend`?
- Should server health depend on concrete `Bm25Search`, or on an indexing/search
  health abstraction?
- Is `RustParser` still used by server only for file-local analysis, rather
  than graph-level HIR analysis?
- Can `EmbeddingProfile` be kept as an engine-owned canonical config model
  without making `rmc_config` depend too heavily on engine?

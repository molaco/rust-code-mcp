# rmc-engine Boundary Report

## Status

- Crate: `rmc-engine`
- Graph qualified name: `rmc_engine`
- Analysis order: 1 of 4
- Current phase: Phase 1 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | c35970b1 | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Complete | Pending commit | Root exposes six domain modules; submodule facades reexport public API types. |
| Phase 2: Dependency boundary | Pending | Not started |  |
| Phase 3: Import and usage coupling | Pending | Not started |  |
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

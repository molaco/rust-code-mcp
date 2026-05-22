# rmc-indexing Boundary Report

## Status

- Crate: `rmc-indexing`
- Graph qualified name: `rmc_indexing`
- Analysis order: 3 of 4
- Current phase: Phase 5 complete
- Report state: complete

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | e3004234 | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Complete | 1a332a1a | Root is narrow, but public submodules expose indexing internals directly. |
| Phase 2: Dependency boundary | Complete | 07e23561 | Outgoing edges are only to `rmc_config` and `rmc_engine`; expected layering rules have no violations. |
| Phase 3: Import and usage coupling | Complete | df3f0b8c | Server uses unified/index stats APIs, but also reaches into incremental and Tantivy adapter modules. |
| Phase 4: Internal cohesion | Complete | e86b1df6 | Cohesive indexing crate; overlap findings are small variant/helper pairs. |
| Phase 5: Targeted source reads and recommendations | Complete | Pending commit | Final score and recommendations recorded. |

## Phase 0: Snapshot Readiness And Baseline

### Required VCS Check

Before Phase 0, `jj show --summary` reported:

```text
Commit ID: 3b74a3b6f2d948ce39fc29550c8654f0530eef37
Change ID: vkquonkwvzzmypxkywwtolmpryplwlsm
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

`rmc_indexing` baseline from `crate_dependency_metric`:

```text
crate_name: rmc_indexing
item_count: 308
efferent: 2
afferent: 14
instability: 0.125
abstractness: 0.003246753246753247
```

### Phase 0 Interpretation

`rmc_indexing` behaves like a stable middle-layer crate. It is not as large as
`rmc_graph` or `rmc_engine`, but it is still a substantial crate with `308`
items. Its low instability score (`0.125`) comes from a small outgoing
dependency count and many incoming consumers. That matches the expected role:
indexing should coordinate code chunking, BM25/vector indexing, incremental
state, cache synchronization, and security filtering without knowing about MCP
server request/response concerns.

The two outgoing dependencies need phase 2 validation. The expected shape is
`rmc_indexing -> rmc_engine` and `rmc_indexing -> rmc_config`, with no
`rmc_indexing -> rmc_graph` or `rmc_indexing -> rmc_server` edge.

### Phase 0 Findings

- No snapshot rebuild was required; the existing graph snapshot was reusable.
- `rmc_indexing` has `308` items, making it a substantial middle-layer crate.
- `rmc_indexing` has two outgoing producer crates and fourteen incoming
  consumers.
- The instability score is low (`0.125`), which fits a reusable service crate
  rather than an edge-only adapter.
- Phase 2 must verify that the two outgoing edges point only to expected lower
  or config layers.

### Open Questions For Later Phases

- Does `rmc_indexing` expose a clean root facade, or are internals like
  incremental indexing, Merkle snapshots, Tantivy adapters, and monitoring
  public directly?
- Do server and examples use stable indexing APIs or deep implementation
  modules?
- Does indexing stay independent from persisted graph internals?
- Are security, monitoring, and sync responsibilities cohesive inside indexing,
  or should some be server-owned?

## Phase 1: Public Surface

### Required VCS Check

Before Phase 1, `jj show --summary` reported:

```text
Commit ID: 3d2e3eb7d1d4dd4585428bee2f4cb4da789f4edd
Change ID: kupyymnxlwuxwmmynqwsuxnyzvxupyro
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
module_tree(directory, krate="rmc_indexing", depth=2)
get_exports(directory, module="rmc_indexing", consumer="rmc_indexing", summary=true, limit=300)
get_declared_reexports(directory, module="rmc_indexing", summary=false, limit=300)
pub_use_pub_type_audit(directory, crate_name="rmc_indexing", summary=true, limit=300)
module_tree(directory, krate="rmc_indexing", depth=3)
get_exports(directory, module="rmc_indexing::indexing", consumer="rmc_indexing", summary=true, limit=500)
get_declared_reexports(directory, module="rmc_indexing::indexing", summary=false, limit=500)
get_exports(directory, module=<important public submodules>, consumer="rmc_indexing", summary=true, limit=300)
```

Crate root:

```text
exports: 5
declared reexports: 0

public root modules:
  indexing
  metadata_cache
  metrics
  monitoring
  security
```

`pub_use_pub_type_audit` findings:

```text
count: 0
```

`rmc_indexing::indexing` surface:

```text
exports: 20
declared reexports: 7

public modules:
  consistency
  error
  error_collection
  identity
  incremental
  indexer_core
  merkle
  retry
  tantivy_adapter
  unified

pub(crate) modules visible inside rmc_indexing:
  backup
  embedding_batcher
  file_processor

public reexports:
  UnifiedIndexer -> indexing::unified::UnifiedIndexer
  IndexStats -> indexing::unified::IndexStats
  IndexFileResult -> indexing::unified::IndexFileResult
  IncrementalIndexer -> indexing::incremental::IncrementalIndexer
  get_snapshot_path -> indexing::incremental::get_snapshot_path
  TantivyAdapter -> indexing::tantivy_adapter::TantivyAdapter
```

Other public submodule surfaces:

```text
metadata_cache:
  MetadataCache is public.
  FileMetadata and FileStat are pub(crate).

metrics:
  IndexingMetrics is public.
  memory module is public.
  PhaseTimer is pub(crate).

metrics::memory:
  MemoryMonitor is public.

monitoring:
  health and backup modules are public.

monitoring::health:
  ComponentHealth, HealthMonitor, HealthStatus, Status are public.

security:
  SensitiveFileFilter and secrets module are public.

security::secrets:
  SecretMatch and SecretsScanner are public.

indexing::unified:
  UnifiedIndexer, IndexStats, IndexFileResult are public.

indexing::incremental:
  IncrementalIndexer, get_snapshot_path, get_snapshot_path_for_identity are public.
  get_snapshot_path_for_backend is pub(crate).
```

### Phase 1 Interpretation

The crate root is intentionally narrow: it exposes five responsibility modules
and no root-level reexports. The main API is one level down, especially under
`rmc_indexing::indexing`.

`rmc_indexing::indexing` is both facade and implementation namespace. It
reexports the likely primary APIs (`UnifiedIndexer`, `IncrementalIndexer`,
`IndexStats`, `IndexFileResult`) while also exposing implementation modules
such as `tantivy_adapter`, `merkle`, `retry`, `identity`, `consistency`, and
`indexer_core`. That mirrors the graph crate pattern, but the surface is
smaller.

Security, metadata cache, metrics, and monitoring are also public API groups.
Some of that is probably intentional because server and tests need health,
memory, sensitive-file, and secret-scanning primitives. Phase 3 must check
whether consumers use the high-level facade types or import deep modules.

### Phase 1 Findings

- Root facade is narrow: five public modules and no root reexports.
- `rmc_indexing::indexing` has a curated facade layer, but it also exposes
  implementation modules directly.
- `UnifiedIndexer`, `IncrementalIndexer`, `IndexStats`, and `IndexFileResult`
  appear to be primary public APIs.
- `TantivyAdapter`, `merkle`, `retry`, `identity`, `consistency`, and
  `indexer_core` are public implementation surfaces.
- `metadata_cache::MetadataCache`, `metrics::IndexingMetrics`,
  `metrics::memory::MemoryMonitor`, monitoring health types,
  `SensitiveFileFilter`, `SecretsScanner`, and `SecretMatch` are public.
- No `pub type` masquerading findings were reported.

### Open Questions For Later Phases

- Which external crates import `TantivyAdapter`, `FileSystemMerkle`,
  `get_snapshot_path`, or monitoring/security internals directly?
- Is `UnifiedIndexer` sufficient as the preferred indexing facade?
- Should `metadata_cache`, `monitoring`, and `metrics` be public indexing APIs,
  or are some of them only server operational support?
- Are examples/tests the only consumers of lower-level modules such as
  `merkle` and `tantivy_adapter`?

## Phase 2: Dependency Boundary

### Required VCS Check

Before Phase 2, `jj show --summary` reported:

```text
Commit ID: a50bc01a787d357a81fa8d49e29b565c7a2d76fb
Change ID: monpqutyvrvktromorlrrpuklrqzxryt
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
crate_dependency_metric(directory, sort_by="efferent", summary=true, limit=200)
forbidden_dependency_check(directory, rules=[rmc_indexing -> *], consumer_kinds=[lib, bin, example, test, bench], summary=true, limit=200)
forbidden_dependency_check(directory, rules=[* -> rmc_indexing], consumer_kinds=[lib, bin, example, test, bench], summary=true, limit=200)
forbidden_dependency_check(directory, rules=[expected layering rules], consumer_kinds=[lib, bin, example, test, bench], summary=true, limit=200)
```

`rmc_indexing` dependency metric:

```text
crate_name: rmc_indexing
item_count: 308
efferent: 2
afferent: 14
instability: 0.125
abstractness: 0.003246753246753247
```

Outgoing edge inventory:

```text
rmc_indexing -> rmc_config
  sample_symbol: rmc_config::config::indexer::IndexerCoreConfig
  unique_symbols: 10
  total_refs: 30

rmc_indexing -> rmc_engine
  sample_symbol: rmc_engine::embeddings::backend::EmbeddingBackend
  unique_symbols: 46
  total_refs: 198
```

Incoming edge inventory:

```text
bench_incremental_performance -> rmc_indexing
  sample_symbol: IncrementalIndexer::index_with_change_detection
  unique_symbols: 3
  total_refs: 13

benchmark_gpu_performance -> rmc_indexing
  sample_symbol: IncrementalIndexer
  unique_symbols: 8
  total_refs: 21

benchmark_phases -> rmc_indexing
  sample_symbol: IncrementalIndexer
  unique_symbols: 15
  total_refs: 17

embedding_profile_smoke -> rmc_indexing
  sample_symbol: IncrementalIndexer
  unique_symbols: 4
  total_refs: 5

evaluation -> rmc_indexing
  sample_symbol: UnifiedIndexer
  unique_symbols: 6
  total_refs: 7

index_codebase -> rmc_indexing
  sample_symbol: IncrementalIndexer
  unique_symbols: 6
  total_refs: 7

quick_bench -> rmc_indexing
  sample_symbol: IncrementalIndexer
  unique_symbols: 6
  total_refs: 8

rmc_server -> rmc_indexing
  sample_symbol: rmc_indexing::indexing::unified::IndexStats
  unique_symbols: 21
  total_refs: 55

test_full_incremental_flow -> rmc_indexing
  sample_symbol: IncrementalIndexer::index_with_change_detection
  unique_symbols: 4
  total_refs: 10

test_gpu_index_jsonrpc -> rmc_indexing
  sample_symbol: MemoryMonitor::usage_percent
  unique_symbols: 4
  total_refs: 6

test_hybrid_search -> rmc_indexing
  sample_symbol: UnifiedIndexer
  unique_symbols: 6
  total_refs: 14

test_incremental_indexing -> rmc_indexing
  sample_symbol: IncrementalIndexer::index_with_change_detection
  unique_symbols: 4
  total_refs: 24

test_mcp_stdio_transport -> rmc_indexing
  sample_symbol: get_snapshot_path
  unique_symbols: 1
  total_refs: 2

test_merkle_standalone -> rmc_indexing
  sample_symbol: FileSystemMerkle
  unique_symbols: 11
  total_refs: 69
```

Expected layering check:

```text
rule_count: 5
violation_count: 0
```

Checked rules:

```text
rmc_engine should not depend on rmc_* crates.
rmc_graph should not depend on rmc_server.
rmc_graph should not depend on rmc_indexing.
rmc_indexing should not depend on rmc_server.
rmc_indexing should not depend on rmc_graph.
```

### Phase 2 Interpretation

`rmc_indexing` has the expected dependency direction. It depends on engine
primitives and configuration, and it does not depend on graph or server. That
keeps indexing as a sibling of graph rather than a consumer of persisted graph
internals.

The incoming edge inventory shows two consumer categories. Production server
code uses indexing with `21` unique symbols and `55` refs. Most other incoming
consumers are benchmarks, examples, integration tests, or standalone tools.
Those tools often use lower-level indexing APIs such as `IncrementalIndexer`,
`FileSystemMerkle`, and `get_snapshot_path`, which phase 3 should separate from
production server usage.

### Phase 2 Findings

- `rmc_indexing` has two outgoing crate edges: `rmc_engine` and `rmc_config`.
- No `rmc_indexing -> rmc_server` edge exists.
- No `rmc_indexing -> rmc_graph` edge exists.
- The expected cross-layer rules returned zero violations.
- `rmc_server` is the main production consumer of indexing, with `21` unique
  symbols and `55` refs.
- Lower-level indexing internals are heavily used by tests, benchmarks, and
  standalone tools, especially `IncrementalIndexer`, `FileSystemMerkle`, and
  `get_snapshot_path`.

### Open Questions For Later Phases

- Which `rmc_server` modules account for the 21 indexing symbols?
- Does server use `UnifiedIndexer` as the stable facade, or does it reach into
  incremental, monitoring, metadata cache, and security modules directly?
- Should benchmark/test-only lower-level surfaces be public, or can they move
  behind narrower dev/test APIs?

## Phase 3: Import And Usage Coupling

### Required VCS Check

Before Phase 3, `jj show --summary` reported:

```text
Commit ID: 6c77a89326d4be12e7e2bc8f3e43d33c8cd61bb7
Change ID: vmroopnkxunvvopzstsmlqrvqkmkssnx
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
get_imports(directory, module="rmc_indexing", summary=true, limit=300)
module_dependencies(directory, module="rmc_indexing", summary=true, limit=300)
get_imports(directory, module="rmc_indexing::indexing", summary=true, limit=300)
module_dependencies(directory, module="rmc_indexing::indexing", summary=true, limit=300)
who_imports(directory, target=<key indexing symbols>, summary=true, limit=200)
who_uses_summary(directory, target=<key indexing symbols>, summary=true, limit=200)
module_dependencies(directory, module=<server indexing consumer modules>, summary=true, limit=200)
```

Crate root imports/dependencies:

```text
rmc_indexing imports: 0
rmc_indexing module dependencies: 0
```

`rmc_indexing::indexing` facade imports:

```text
total imports: 7
dependency modules: 4

rmc_indexing::indexing::unified
  import_count: 3

rmc_indexing::indexing::incremental
  import_count: 2

rmc_indexing::indexing::tantivy_adapter
  import_count: 1

rmc_indexing::indexing::error
  import_count: 1
```

Key symbol import/usage rollups:

```text
UnifiedIndexer
  who_imports total: 6
  who_uses_summary total modules: 5
  production/server use:
    rmc_server::tools::endpoints::query
  non-server external use:
    evaluation, test_hybrid_search

IncrementalIndexer
  who_imports total: 14
  who_uses_summary total modules: 11
  production/server use:
    rmc_server::mcp::sync
    rmc_server::tools::endpoints::index
  non-server external use:
    benchmarks, tests, index_codebase, quick_bench, embedding_profile_smoke

IndexStats
  who_imports total: 9
  who_uses_summary total modules: 6
  production/server use:
    rmc_server::tools::endpoints::index
    rmc_server::tools::endpoints::query

TantivyAdapter
  who_imports total: 4
  importers:
    rmc_indexing::indexing
    rmc_indexing::indexing::unified
    rmc_indexing::indexing::unified::tests
    rmc_indexing::indexing::tantivy_adapter::tests

FileSystemMerkle
  who_imports total: 8
  external importer:
    test_merkle_standalone
  internal/importing modules include:
    indexing::incremental
    indexing::backup
    monitoring::backup
    tests

get_snapshot_path
  who_imports total: 4
  external importers:
    test_full_incremental_flow
    test_mcp_stdio_transport
  plus indexing facade and incremental tests

MemoryMonitor
  who_imports total: 3
  importers are internal/tests:
    indexing::embedding_batcher
    indexing::embedding_batcher::tests
    metrics::memory::tests

SensitiveFileFilter
  who_imports total: 4
  external importer:
    benchmark_phases
  internal/importing modules include:
    indexing::file_processor
    tests

SecretsScanner
  who_imports total: 4
  external importer:
    benchmark_phases
  internal/importing modules include:
    indexing::file_processor
    tests
```

Server module dependency rollups:

```text
rmc_server::tools::endpoints::index
  rmc_indexing::indexing::incremental: imports 1, usages 4
  rmc_indexing::indexing::unified: imports 1, usages 4

rmc_server::mcp::sync
  rmc_indexing::indexing::incremental: imports 1, usages 3

rmc_server::tools::endpoints::query
  rmc_indexing::indexing::unified: imports 0, usages 5
  rmc_indexing::indexing::tantivy_adapter: imports 0, usages 3
```

### Phase 3 Interpretation

Consumers use two different indexing surfaces. `UnifiedIndexer` and
`IndexStats` look like intended high-level APIs and are used by server query
and index endpoints. However, server also imports `IncrementalIndexer` directly
from `indexing::incremental` for sync and index endpoints. The query endpoint
also has inline usage of `indexing::tantivy_adapter`, which means server is not
fully insulated from indexing implementation details.

The deepest APIs are mostly not production-server dependencies. `FileSystemMerkle`
is primarily internal and test/standalone-tool facing. `get_snapshot_path` is
used by tests and transport integration checks. `SensitiveFileFilter` and
`SecretsScanner` are internal to file processing plus benchmarks. `MemoryMonitor`
is internal/test-only in the MCP evidence despite being public.

The practical boundary concern is therefore focused: keep `UnifiedIndexer` and
`IndexStats` as the preferred server contract, and evaluate whether server
should call `IncrementalIndexer` and Tantivy adapter details directly.

### Phase 3 Findings

- The crate root has no imports/dependencies; coupling is under public
  submodules.
- `rmc_indexing::indexing` reexports seven items from four internal modules.
- Server uses both high-level indexing APIs and lower-level implementation
  modules.
- `rmc_server::tools::endpoints::index` imports both `incremental` and
  `unified`.
- `rmc_server::mcp::sync` imports `incremental`.
- `rmc_server::tools::endpoints::query` uses both `unified` and
  `tantivy_adapter`.
- `FileSystemMerkle`, `get_snapshot_path`, security scanners, and memory
  monitoring are mostly used by internal modules, tests, benchmarks, or
  standalone tools rather than production server paths.

### Open Questions For Later Phases

- Should sync/index server flows depend on `IncrementalIndexer`, or should that
  be wrapped by a higher-level indexing service API?
- Should query create/use Tantivy search through `UnifiedIndexer` rather than
  touching `TantivyAdapter`?
- Are public monitoring/security modules deliberately reusable, or just public
  because examples and benchmarks import them?

## Phase 4: Internal Cohesion

### Required VCS Check

Before Phase 4, `jj show --summary` reported:

```text
Commit ID: 58f2e97329480920d423fbfdbf060d2b10f7a328
Change ID: olrwqlvrzqpswnzwyxnzmlmuwyupvwpq
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
functions_with_filter(directory, krate="rmc_indexing", summary=true, limit=300)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="rmc_engine", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="EmbeddingBackend", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="IndexerCoreConfig", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="CodeChunk", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="VectorStore", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="UnifiedIndexer", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="IncrementalIndexer", summary=true, limit=100)
overlaps(directory, scope="local_no_vendor")
semantic_overlaps(directory, crate_name="rmc_indexing", item_kind="Struct", summary=true, max_pairs=25)
semantic_overlaps(directory, crate_name="rmc_indexing", item_kind="Enum", summary=true, max_pairs=25)
semantic_overlaps(directory, crate_name="rmc_indexing", item_kind="Function", summary=true, max_pairs=25)
```

Function inventory:

```text
total indexed rmc_indexing functions: 259
returned first page: 259
```

Boundary-signature filters:

```text
has_param_type="rmc_engine"
  total: 0

has_param_type="EmbeddingBackend"
  total: 6
  identity::active_chunking_identity_for_backend
  identity::indexing_identity
  incremental::IncrementalIndexer::with_backend
  incremental::get_snapshot_path_for_backend
  indexer_core::IndexerCore::with_backend
  unified::UnifiedIndexer::for_embedded_with_backend

has_param_type="IndexerCoreConfig"
  total: 2
  indexer_core::IndexerCore::new
  indexer_core::IndexerCore::with_backend

has_param_type="CodeChunk"
  total: 5
  embedding_batcher::EmbeddingBatcher::count_chunk_raw_tokens
  embedding_batcher::EmbeddingBatcher::generate_embeddings_batched
  indexer_core::IndexerCore::generate_embeddings_batched
  tantivy_adapter::TantivyAdapter::index_chunk
  tantivy_adapter::TantivyAdapter::index_chunks

has_param_type="VectorStore"
  total: 2
  consistency::ConsistencyChecker::new
  monitoring::health::HealthMonitor::new

has_param_type="UnifiedIndexer"
  total: 0

has_param_type="IncrementalIndexer"
  total: 0
```

Name-overlap context:

```text
cross_crate_type_collisions: 0
module_shadows: 0
common_fn_names: 0
within_crate_type_duplicates:
  none for rmc_indexing
```

Semantic overlap, structs:

```text
seed_count: 31
total_pair_count: 1
total_cluster_count: 1

cluster:
  FileSystemMerkle / MerkleSnapshot
```

Semantic overlap, enums:

```text
seed_count: 4
total_pair_count: 0
total_cluster_count: 0
```

Semantic overlap, functions:

```text
seed_count: 19
total_pair_count: 4
total_cluster_count: 4

clusters:
  retry::retry_sync_with_backoff / retry::retry_with_backoff
  embedding_batcher::summarize_token_lengths /
    embedding_batcher::summarize_unsorted_token_lengths
  identity::active_chunking_identity /
    identity::active_chunking_identity_for_backend
  incremental::get_snapshot_path_for_backend /
    incremental::get_snapshot_path
```

### Phase 4 Interpretation

`rmc_indexing` is internally cohesive. Its function set clusters around
indexing work: file processing, metadata cache, embedding batching, incremental
indexing, Merkle snapshots, Tantivy writes, unified indexing, consistency,
monitoring, and security filtering. The boundary signatures show expected
dependencies on engine primitives (`EmbeddingBackend`, `CodeChunk`,
`VectorStore`) and config (`IndexerCoreConfig`), without any signature-level
dependency on graph or server types.

The semantic overlap findings are small and understandable. The async/sync
retry pair and default/backend-specific identity/path helpers are variant
pairs. The Merkle struct pair reflects one module's core data model rather than
duplicated ownership. There are no indexing-specific name-collision findings.

The main cohesion question remains API layering rather than duplicated logic:
`UnifiedIndexer`, `IncrementalIndexer`, `IndexerCore`, `TantivyAdapter`, and
supporting modules are all valid pieces of indexing, but only some should be
production server-facing.

### Phase 4 Findings

- `rmc_indexing` has 259 indexed functions.
- No function signature directly contains `rmc_engine`; concrete engine types
  appear as `EmbeddingBackend`, `CodeChunk`, and `VectorStore`.
- No function signature takes `UnifiedIndexer` or `IncrementalIndexer`, so
  those types are construction/use APIs rather than callback/service inputs.
- Config coupling is localized to `IndexerCore` constructors.
- No rmc-indexing name collisions or module shadows were reported.
- Semantic overlap found only one struct cluster and four function clusters,
  all explainable as paired variants or local data-model relationships.

### Open Questions For Phase 5

- Should the public API guide server toward `UnifiedIndexer` and away from
  `IncrementalIndexer`/`TantivyAdapter`?
- Are `IndexerCore` and `TantivyAdapter` intended public extension points or
  implementation details of `UnifiedIndexer`?
- Should default/backend-specific helper pairs remain public, or be kept
  internal behind backend-aware constructors?

## Phase 5: Targeted Source Reads And Recommendations

### Required VCS Check

Before Phase 5, `jj show --summary` reported:

```text
Commit ID: 24bce29423f9b09b30b0f7423d9e992ca3b3612c
Change ID: kvpvtozqwwnwxnsxmtvnlmqournnulxm
Description: (no description set)
```

### Source Reads

Source reads were limited to files and symbols identified by the MCP evidence:

```text
crates/rmc-indexing/src/lib.rs
crates/rmc-indexing/src/indexing/mod.rs
crates/rmc-indexing/src/indexing/unified.rs
crates/rmc-indexing/src/indexing/incremental.rs
crates/rmc-indexing/src/indexing/tantivy_adapter.rs
crates/rmc-indexing/src/indexing/indexer_core.rs
crates/rmc-server/src/tools/endpoints/index.rs
crates/rmc-server/src/tools/endpoints/query.rs
crates/rmc-server/src/mcp/sync.rs
crates/rmc-server/src/mcp/project_paths.rs
crates/rmc-server/src/tools/graph/codemap.rs
crates/rmc-server/src/tools/endpoints/health.rs
crates/rmc-server/src/tools/endpoints/indexing_support.rs
```

Key source observations:

```text
rmc_indexing::lib
  exposes five public root modules:
  indexing, metadata_cache, metrics, monitoring, security.

rmc_indexing::indexing
  exposes public implementation modules:
  consistency, error, error_collection, identity, incremental,
  indexer_core, merkle, retry, tantivy_adapter, unified.

rmc_indexing::indexing
  reexports likely facade APIs:
  UnifiedIndexer, IndexStats, IndexFileResult,
  IncrementalIndexer, get_snapshot_path, TantivyAdapter.

UnifiedIndexer
  owns the high-level Tantivy + vector indexing flow.
  It constructs IndexerCore, TantivyAdapter, and VectorStore internally.
  It exposes search/index support methods including index_directory,
  create_bm25_search, tantivy accessors, vector store cloning, and
  embedding generator cloning.

IncrementalIndexer
  wraps UnifiedIndexer and Merkle change detection.
  Server index and background sync paths use it directly.

TantivyAdapter
  is a focused BM25 adapter over Tantivy index/writer/schema.
  Server query and graph codemap paths open it directly to obtain BM25
  search handles.

IndexerCore
  is pub(crate) despite the public module path.
  It orchestrates FileProcessor, Chunker, and EmbeddingBatcher internally.

Server callers
  endpoints/index.rs imports incremental::IncrementalIndexer and
  unified::IndexStats.
  mcp/sync.rs imports incremental::IncrementalIndexer.
  endpoints/query.rs imports tantivy_adapter::TantivyAdapter inline and
  uses unified::UnifiedIndexer for ensure-indexed behavior.
  mcp/project_paths.rs imports identity helpers and
  get_snapshot_path_for_identity.
  health.rs imports monitoring::health::HealthMonitor and names
  monitoring health statuses directly.
```

### Final Boundary Assessment

Boundary score: `8/10`.

`rmc_indexing` has a strong directional boundary. It depends only on engine and
configuration crates, has no graph/server dependency, and cleanly owns indexing
responsibilities: parsing/chunking orchestration, embedding generation, Tantivy
index writes, vector index coordination, incremental/Merkle state, metadata
cache, security filtering, metrics, and health support.

The reason it is not `10/10` is API shape. The crate has a useful high-level
facade in `UnifiedIndexer` and a legitimate incremental facade in
`IncrementalIndexer`, but it also makes several implementation modules public.
Server production code imports those internals directly in a few places,
especially `TantivyAdapter`, identity/path helpers, and monitoring health
status types. That means some indexing internals are effectively part of the
server contract even where a narrower indexing service API could hide them.

This is still cleaner than the graph boundary because indexing's dependency
direction is correct and the public leakage is concentrated. Most deep
consumer usage outside server is test, benchmark, or standalone-tool oriented.

### Recommendations

1. Keep `rmc_indexing -> rmc_engine` and `rmc_indexing -> rmc_config` as the
   only outgoing production dependencies.

2. Treat `UnifiedIndexer`, `IndexStats`, and `IndexFileResult` as the preferred
   general indexing facade. Document that server-facing indexing should start
   there unless it specifically needs incremental change detection.

3. Decide whether `IncrementalIndexer` is an official public facade. If yes,
   expose it deliberately as a sync/indexing API and document the use cases. If
   no, wrap the server index and background sync paths behind a narrower
   service function that owns Merkle change detection inside indexing.

4. Reduce direct server dependency on `TantivyAdapter`. Query and codemap only
   need "open BM25 search for these paths"; that can be exposed as a small
   indexing-level function or method without making the adapter itself a
   server contract.

5. Reconsider public module visibility for `identity`, `tantivy_adapter`,
   `merkle`, `retry`, `consistency`, and `indexer_core`. If production server
   only needs a few helper functions, expose those helpers through a smaller
   facade and make implementation modules `pub(crate)` over time.

6. Keep `metadata_cache`, `metrics`, `monitoring`, and `security` public only
   where they are intentionally reusable operational APIs. The MCP evidence
   showed several of these are mostly internal/test/benchmark consumers.

7. Leave the semantic-overlap pairs alone for now. The reported pairs are
   understandable variants: async/sync retry, sorted/unsorted token summaries,
   default/backend-aware identity helpers, and default/backend-aware snapshot
   helpers.

### Final Findings

- Directional layering is correct: indexing does not depend on graph or server.
- `UnifiedIndexer` is a coherent high-level facade over indexing internals.
- `IncrementalIndexer` is a real facade, but server imports it from a deep
  module path.
- `TantivyAdapter` is focused and useful, but server query/codemap usage makes
  it part of the practical public API.
- Several implementation modules are public even when their primary consumers
  are internal modules, tests, benchmarks, or server support helpers.
- The crate is cohesive; the main boundary improvement is facade tightening,
  not moving responsibilities across crates.

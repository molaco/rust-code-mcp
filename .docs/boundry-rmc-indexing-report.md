# rmc-indexing Boundary Report

## Status

- Crate: `rmc-indexing`
- Graph qualified name: `rmc_indexing`
- Analysis order: 3 of 4
- Current phase: Phase 2 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | e3004234 | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Complete | 1a332a1a | Root is narrow, but public submodules expose indexing internals directly. |
| Phase 2: Dependency boundary | Complete | Pending commit | Outgoing edges are only to `rmc_config` and `rmc_engine`; expected layering rules have no violations. |
| Phase 3: Import and usage coupling | Pending | Not started |  |
| Phase 4: Internal cohesion | Pending | Not started |  |
| Phase 5: Targeted source reads and recommendations | Pending | Not started |  |

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

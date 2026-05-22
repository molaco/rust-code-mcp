# rmc-server Boundary Report

## Status

- Crate: `rmc-server`
- Graph qualified name: `rmc_server`
- Analysis order: 4 of 4
- Current phase: Phase 4 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | 98e49844 | Graph snapshot reused; workspace and server dependency baseline captured. |
| Phase 1: Public surface | Complete | aedffaec | Root is narrow, but server exposes public implementation namespaces. |
| Phase 2: Dependency boundary | Complete | 4da7a3c2 | Outgoing edges match expected top-layer dependencies; no lower-layer rule violations. |
| Phase 3: Import and usage coupling | Complete | c8f88d50 | Coupling is concentrated in router, endpoints, and graph tool modules. |
| Phase 4: Internal cohesion | Complete | Pending commit | Cohesive server crate, with expected MCP boilerplate and a few helper overlaps. |
| Phase 5: Targeted source reads and recommendations | Pending | Not started |  |

## Phase 0: Snapshot Readiness And Baseline

### Required VCS Check

Before Phase 0, `jj show --summary` reported:

```text
Commit ID: ad1f14435d771c00cc018c1ccb893f84633b3c0f
Change ID: owqtpyynknxzlmqvzsuswmmuqpvpqsqo
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

`rmc_server` baseline from `crate_dependency_metric`:

```text
crate_name: rmc_server
item_count: 370
efferent: 4
afferent: 6
instability: 0.4
abstractness: 0.0
```

### Phase 0 Interpretation

`rmc_server` is a large edge/orchestration crate. Its `370` items put it below
`rmc_graph` and `rmc_engine`, but above `rmc_indexing`. Its instability score
(`0.4`) is the highest among the four target library crates, which is expected
for the MCP-facing layer: server should depend on graph, indexing, engine, and
configuration while lower layers should avoid depending back on server.

The six incoming consumer crates are expected to be mostly tests and
integration probes. Phase 2 should confirm that no core lower layer depends on
`rmc_server`, and that the four outgoing edges match the intended layer shape:
`rmc_server -> rmc_graph`, `rmc_server -> rmc_indexing`, `rmc_server ->
rmc_engine`, and `rmc_server -> rmc_config`.

### Phase 0 Findings

- No snapshot rebuild was required; the existing graph snapshot was reusable.
- `rmc_server` has `370` indexed items.
- `rmc_server` has four outgoing producer crates and six incoming consumer
  crates.
- The instability score is `0.4`, which is plausible for a top-level MCP
  server/orchestration crate.
- Phase 2 must verify whether the outgoing edges are only to expected lower
  layers and whether incoming edges are only tests/examples/tools.

### Open Questions For Later Phases

- Is there a narrow crate-root public facade, or are server internals exposed
  as public modules?
- Does server mostly orchestrate graph/indexing/engine APIs, or does it own
  reusable logic that belongs lower?
- Which server modules import graph and indexing deep implementation modules?
- Are MCP request/response DTOs separated from graph/indexing domain types?
- Are test/integration crates the only meaningful incoming consumers?

## Phase 1: Public Surface

### Required VCS Check

Before Phase 1, `jj show --summary` reported:

```text
Commit ID: 04efed21b7164dcda88bc8ff32a8cf463b585988
Change ID: yqkqprqnwwkrwtqtmpktmrmwwxlkuktk
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
module_tree(directory, krate="rmc_server", depth=2)
get_exports(directory, module="rmc_server", consumer="rmc_server", summary=true, limit=500)
get_declared_reexports(directory, module="rmc_server", summary=false, limit=500)
pub_use_pub_type_audit(directory, crate_name="rmc_server", summary=true, limit=500)
module_tree(directory, krate="rmc_server", depth=4)
get_exports(directory, module=<important server modules>, consumer="rmc_server", summary=true, limit=500)
get_declared_reexports(directory, module=<important server modules>, summary=false, limit=500)
```

Crate root:

```text
exports: 3
declared reexports: 0

public root modules:
  mcp
  semantic
  tools
```

`pub_use_pub_type_audit` findings:

```text
count: 0
```

Top-level module surfaces:

```text
rmc_server::mcp
  exports: 3
  declared reexports: 1
  public modules:
    sync
    project_paths
  public reexport:
    SyncManager -> mcp::sync::SyncManager

rmc_server::tools
  exports: 5
  declared reexports: 4
  public module:
    project_paths
  public reexports:
    IndexCodebaseParams -> tools::endpoints::index::IndexCodebaseParams
    index_codebase -> tools::endpoints::index::index_codebase
    SearchTool -> tools::router::SearchToolRouter
    SearchToolRouter -> tools::router::SearchToolRouter

rmc_server::semantic
  externally public module, but exported symbols are pub(crate):
    Location
    SEMANTIC
    SemanticService
    RenamePreview
```

Important submodule surfaces:

```text
rmc_server::mcp::project_paths
  public:
    ProjectPaths
    IndexedProfilePaths
  pub(crate):
    data_dir
    dir_hash
    read_embedder_identity
    resolve_embedding_backend

rmc_server::mcp::sync
  public:
    SyncManager

rmc_server::tools::router
  public:
    SearchToolRouter

rmc_server::tools::params
  exports: 50
  declared reexports: 49
  all declared reexports are pub(crate) glob imports from audit, graph,
  indexing, and search parameter modules.

rmc_server::tools::endpoints
  exports: 0
  declared reexports: 0

rmc_server::tools::graph
  exports: 0
  declared reexports: 0
```

### Phase 1 Interpretation

The crate root is intentionally small, but it is not a strict facade. It
exports three public namespaces: `mcp`, `semantic`, and `tools`. The public
facade items live below those namespaces rather than at the crate root.

The clearest external APIs are `SyncManager`, `SearchToolRouter`,
`index_codebase`, and `IndexCodebaseParams`. `mcp::project_paths::ProjectPaths`
and `IndexedProfilePaths` are also public, which exposes storage/path identity
details as part of the crate's public surface. That may be intentional for
integration tests and transport tooling, but it is server infrastructure rather
than a clean MCP tool facade.

`semantic` is public as a module, but the surfaced types are `pub(crate)`, so
it behaves more like an internal server service namespace. `tools::params`
centralizes a large internal parameter facade with 49 `pub(crate)` reexports,
which is appropriate for router implementation but not an external API.
`tools::endpoints` and `tools::graph` do not reexport their child modules from
the container modules.

### Phase 1 Findings

- Root exports are narrow: `mcp`, `semantic`, and `tools`.
- Root has no declared reexports.
- No `pub type` masquerading findings were reported.
- `mcp` publicly exposes `sync`, `project_paths`, and reexports
  `SyncManager`.
- `tools` publicly reexports `SearchToolRouter`, `SearchTool`, `index_codebase`,
  and `IndexCodebaseParams`.
- `mcp::project_paths` publicly exposes `ProjectPaths` and
  `IndexedProfilePaths`, which makes server storage identity/path computation a
  public API.
- `semantic` is a public module but its meaningful service/export types are
  `pub(crate)`.
- `tools::params` has a large internal `pub(crate)` reexport facade for tool
  parameter structs.

### Open Questions For Later Phases

- Do external crates actually depend on `ProjectPaths`, `SyncManager`,
  `index_codebase`, or `SearchToolRouter`, or are they mostly test-only?
- Should server expose a crate-root facade for its real public API instead of
  making implementation namespaces public?
- Are `mcp::project_paths` and `tools::project_paths` duplicate path surfaces?
- Do tool endpoints depend on graph/indexing internals directly or through
  stable lower-crate facades?

## Phase 2: Dependency Boundary

### Required VCS Check

Before Phase 2, `jj show --summary` reported:

```text
Commit ID: fd8d990f44252f6cc0416f57053ef53a07ec0568
Change ID: lkxknrmzkomyztlyzlulwvztrrvxtusv
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
crate_dependency_metric(directory, sort_by="efferent", summary=true, limit=200)
crate_dependency_metric(directory, sort_by="afferent", summary=true, limit=200)
forbidden_dependency_check(directory, rules=[rmc_server -> *], consumer_kinds=[lib, bin, test, bench, example], summary=true, limit=300)
forbidden_dependency_check(directory, rules=[* -> rmc_server], consumer_kinds=[lib, bin, test, bench, example], summary=true, limit=300)
forbidden_dependency_check(directory, rules=[expected layering rules], consumer_kinds=[lib, bin, test, bench, example], summary=true, limit=300)
```

`rmc_server` dependency metric:

```text
crate_name: rmc_server
item_count: 370
efferent: 4
afferent: 6
instability: 0.4
abstractness: 0.0
```

Outgoing edge inventory:

```text
rmc_server -> rmc_config
  sample_symbol: rmc_config::config::indexer::TantivyConfig
  unique_symbols: 2
  total_refs: 4

rmc_server -> rmc_engine
  sample_symbol: rmc_engine::embeddings::backend::EmbeddingBackend
  unique_symbols: 38
  total_refs: 153

rmc_server -> rmc_graph
  sample_symbol: rmc_graph::graph::model::NodeKind
  unique_symbols: 108
  total_refs: 334

rmc_server -> rmc_indexing
  sample_symbol: rmc_indexing::indexing::unified::IndexStats
  unique_symbols: 21
  total_refs: 55
```

Incoming edge inventory:

```text
rust-code-mcp -> rmc_server
  sample_symbol: rmc_server::mcp::sync::SyncManager
  unique_symbols: 5
  total_refs: 6

test_burn_performance -> rmc_server
  sample_symbol: rmc_server::tools::endpoints::index::IndexCodebaseParams
  unique_symbols: 2
  total_refs: 2

test_gpu_index_jsonrpc -> rmc_server
  sample_symbol: rmc_server::tools::endpoints::index::IndexCodebaseParams
  unique_symbols: 2
  total_refs: 2

test_index_tool_integration -> rmc_server
  sample_symbol: rmc_server::tools::endpoints::index::index_codebase
  unique_symbols: 5
  total_refs: 30

test_mcp_stdio_transport -> rmc_server
  sample_symbol: rmc_server::mcp::project_paths::ProjectPaths
  unique_symbols: 2
  total_refs: 3

test_sync_manager_integration -> rmc_server
  sample_symbol: rmc_server::mcp::sync::SyncManager::track_directory
  unique_symbols: 9
  total_refs: 36
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

`rmc_server` is correctly positioned as the top layer. Its four outgoing edges
match the expected dependency direction: config, engine, graph, and indexing.
No MCP evidence shows `rmc_graph` or `rmc_indexing` depending back on server.

The volume of the graph edge is notable: `108` unique graph symbols and `334`
refs. That does not violate layering, but it means server likely imports graph
implementation details heavily rather than only a small graph facade. The
indexing and engine edges are also substantial, but smaller. Phase 3 should
identify whether these edges are concentrated in a few orchestration modules or
spread across many tool endpoints.

Incoming edges are almost entirely test/integration oriented. The one
non-test-looking consumer is `rust-code-mcp`, which appears to be the binary
entrypoint and uses server APIs such as `SyncManager`. That is appropriate for
an executable depending on the server library.

### Phase 2 Findings

- Server has exactly the expected four outgoing local-crate edges:
  `rmc_config`, `rmc_engine`, `rmc_graph`, and `rmc_indexing`.
- The expected lower-layer dependency rules returned zero violations.
- No graph or indexing crate depends on server.
- The strongest server dependency is graph: `108` unique symbols and `334`
  refs.
- Server also depends significantly on engine: `38` unique symbols and `153`
  refs.
- Incoming dependencies are the binary crate plus tests/integration probes.
- `test_mcp_stdio_transport` uses `ProjectPaths`, confirming that the public
  path surface is externally consumed by tests.

### Open Questions For Later Phases

- Which server modules account for the heavy `rmc_graph` edge?
- Does server mostly call graph facade functions, or does it directly use graph
  storage/model/snapshot internals?
- Are engine uses low-level search/vector primitives that should be hidden
  behind indexing or graph services?
- Should `ProjectPaths` stay public for tests, or move behind test helpers?

## Phase 3: Import And Usage Coupling

### Required VCS Check

Before Phase 3, `jj show --summary` reported:

```text
Commit ID: d84807e5124ccc42d81d88da24061e6478d8b6e3
Change ID: yvlqvoxxroqpultznpxmpnslrmvpnwmm
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
get_imports(directory, module="rmc_server", summary=true, limit=500)
module_dependencies(directory, module="rmc_server", summary=true, limit=500)
get_imports(directory, module=<server top modules>, summary=true, limit=500)
module_dependencies(directory, module=<server top modules>, summary=true, limit=500)
module_dependencies(directory, module=<key endpoint and graph modules>, summary=true, limit=500)
who_imports(directory, target=<key public server symbols>, summary=true, limit=200)
who_uses_summary(directory, target=<key public server symbols>, summary=true, limit=200)
```

Crate root imports/dependencies:

```text
rmc_server imports: 0
rmc_server module dependencies: 0
```

Top-level module rollups:

```text
rmc_server::tools
  imports:
    IndexCodebaseParams
    index_codebase
    SearchTool
    SearchToolRouter
  module dependencies:
    tools::endpoints::index import_count 2
    tools::router import_count 2

rmc_server::mcp
  imports:
    SyncManager
  module dependencies:
    mcp::sync import_count 1

rmc_server::semantic
  local server dependencies:
    semantic::loader usage_count 1
    semantic::position import_count 1, usage_count 6
    semantic::rename import_count 1, usage_count 2
  external rust-analyzer dependencies include:
    ra_ap_ide::AnalysisHost
    ra_ap_vfs::Vfs
```

Router dependency rollup:

```text
rmc_server::tools::router
  dispatches to:
    endpoints::analysis usage_count 6
    endpoints::cache usage_count 2
    endpoints::health usage_count 2
    endpoints::index usage_count 2
    endpoints::query usage_count 3
    graph::audits usage_count 5
    graph::codemap usage_count 1
    graph::core usage_count 16
    graph::crates usage_count 3
    graph::similarity usage_count 2
    graph::surface usage_count 12
  parameter modules:
    params::graph usage_count 32
    params::search import_count 9, usage_count 18
    params::audit usage_count 7
    params::indexing usage_count 1
```

Graph tool module lower-crate dependencies:

```text
tools::graph::core
  rmc_graph::graph::labels: imports 4, usages 3
  rmc_graph::graph::model: imports 4, usages 12
  rmc_graph::graph::query::model: imports 8, usages 10
  rmc_graph::graph::snapshot: imports 3, usages 35

tools::graph::surface
  rmc_graph::graph::derive_audit: usages 3
  rmc_graph::graph::docs_audit: usages 3
  rmc_graph::graph::ids: imports 1, usages 4
  rmc_graph::graph::labels: imports 1
  rmc_graph::graph::model: imports 4, usages 19
  rmc_graph::graph::query::model: imports 9, usages 12
  rmc_graph::graph::snapshot: imports 1, usages 26

tools::graph::audits
  rmc_graph::graph::channel_audit: usages 3
  rmc_graph::graph::fn_body_audit: usages 4
  rmc_graph::graph::ids: imports 1, usages 8
  rmc_graph::graph::loader: usages 3
  rmc_graph::graph::model: imports 1, usages 6
  rmc_graph::graph::recursion_check: usages 4
  rmc_graph::graph::snapshot: usages 5
  rmc_graph::graph::unsafe_audit: usages 1

tools::graph::response
  rmc_graph::graph::ids: imports 1, usages 8
  rmc_graph::graph::labels: imports 1
  rmc_graph::graph::model: imports 4, usages 25
  rmc_graph::graph::query::model: imports 1, usages 4
  rmc_graph::graph::snapshot: imports 2, usages 9
  rmc_graph::graph::storage: imports 2, usages 3

tools::graph::codemap
  rmc_graph::graph::codemap::{build, model, render, seeds}: usages 13 total
  rmc_config::config::indexer: usages 2
  rmc_engine::{embeddings::backend, search}: usages 4 total
  rmc_indexing::indexing::tantivy_adapter: usages 3

tools::graph::similarity
  rmc_graph::graph::embedding_cache: usages 1
  rmc_graph::graph::ids: imports 1, usages 15
  rmc_graph::graph::labels: imports 1
  rmc_graph::graph::math: usages 1
  rmc_graph::graph::model: imports 2, usages 6
  rmc_graph::graph::snapshot: usages 2
  rmc_engine::{embeddings::backend, search}: usages 3 total
```

Endpoint lower-crate dependencies:

```text
tools::endpoints::index
  rmc_engine::embeddings::backend: imports 1, usages 11
  rmc_engine::embeddings::profile: imports 1, usages 12
  rmc_engine::vector_store::error: imports 1, usages 2
  rmc_indexing::indexing::incremental: imports 1, usages 4
  rmc_indexing::indexing::unified: imports 1, usages 4
  rmc_server::mcp::project_paths: imports 2, usages 3

tools::endpoints::query
  rmc_config::config::indexer: usages 2
  rmc_engine::embeddings: imports 1, usages 3
  rmc_engine::embeddings::backend: imports 1, usages 17
  rmc_engine::embeddings::profile: usages 3
  rmc_engine::search: imports 1, usages 6
  rmc_engine::search::bm25: usages 2
  rmc_engine::vector_store: imports 1, usages 2
  rmc_indexing::indexing::tantivy_adapter: usages 3
  rmc_indexing::indexing::unified: usages 5
  rmc_server::mcp::project_paths: imports 3, usages 15

tools::endpoints::health
  rmc_engine::embeddings::backend: imports 1, usages 8
  rmc_engine::search::bm25: imports 1, usages 2
  rmc_engine::vector_store: imports 1, usages 2
  rmc_indexing::monitoring::health: imports 1, usages 6
  rmc_server::mcp::project_paths: imports 3, usages 5

tools::endpoints::analysis
  rmc_engine::parser::rust_parser: imports 1, usages 9
  rmc_engine::parser::call_graph: usages 6
  rmc_engine::parser::types: usages 3
  rmc_server::semantic: imports 1, usages 6

tools::endpoints::cache
  rmc_graph::graph::storage: usages 3
  rmc_server::mcp::project_paths: imports 2, usages 3
```

MCP support module lower-crate dependencies:

```text
mcp::project_paths
  rmc_engine::embeddings::backend: imports 1, usages 22
  rmc_engine::embeddings::profile: usages 3
  rmc_engine::embeddings::profile_registry: imports 1, usages 1
  rmc_indexing::indexing::identity: imports 3, usages 5
  rmc_indexing::indexing::incremental: imports 1, usages 2

mcp::sync
  rmc_engine::embeddings::backend: usages 1
  rmc_indexing::indexing::incremental: imports 1, usages 3
  rmc_server::mcp::project_paths: usages 2
```

Key public server API usage:

```text
SyncManager
  who_imports total: 5
  external importers:
    rust-code-mcp
    test_sync_manager_integration
    test_index_tool_integration
  internal import/reexport:
    rmc_server::mcp
    rmc_server::mcp::sync::tests
  who_uses_summary includes:
    rmc_server::tools::router total_count 3
    rmc_server::tools::endpoints::query total_count 2
    rmc_server::tools::endpoints::index total_count 1
    rust-code-mcp total_count 1

ProjectPaths
  who_imports total: 8
  external importer:
    test_mcp_stdio_transport
  internal importers include:
    tools::project_paths
    endpoints::query
    endpoints::health
    endpoints::index
    tests
  who_uses_summary includes:
    endpoints::query total_count 11
    mcp::sync, endpoints::health, endpoints::index,
    graph::codemap, graph::similarity, test_mcp_stdio_transport

index_codebase
  who_imports total: 3
  external importer:
    test_index_tool_integration
  internal import/reexport:
    rmc_server::tools
    endpoints::index::tests
  who_uses_summary includes:
    test_index_tool_integration total_count 16
    tools::router total_count 1
    test_burn_performance total_count 1
    test_gpu_index_jsonrpc total_count 1

IndexCodebaseParams
  who_imports total: 3
  external importer:
    test_index_tool_integration
  internal import/reexport:
    rmc_server::tools
    endpoints::index::tests
  who_uses_summary includes:
    test_index_tool_integration total_count 7
    tools::router total_count 1
    test_burn_performance total_count 1
    test_gpu_index_jsonrpc total_count 1

SearchToolRouter
  who_imports total: 4
  external importer:
    rust-code-mcp, under the alias SearchTool
  internal imports/reexports:
    rmc_server::tools as SearchTool and SearchToolRouter
    router tests
  who_uses_summary:
    rmc_server::tools::router total_count 5
```

### Phase 3 Interpretation

Server coupling is concentrated in the expected places: router dispatch,
endpoint modules, graph-tool modules, `mcp::project_paths`, and `mcp::sync`.
The crate root itself has no imports, and the top-level `tools`/`mcp` modules
mostly reexport implementation items.

The strongest architectural concern is graph coupling. Server graph modules do
not just call a small graph facade; they reference graph model, query model,
snapshot, storage, ids, labels, loader, audit, codemap, embedding cache, math,
and render/build modules directly. That matches the high edge volume from
phase 2 and means the server response layer knows a lot about graph internals.

Indexing coupling is narrower but still deep. Index and sync use
`IncrementalIndexer`; query and codemap use `TantivyAdapter`; project path
logic uses indexing identity and snapshot helpers. These are legitimate server
tasks, but the imports confirm the indexing boundary is still partly
implementation-facing.

Engine coupling appears in query/search, health, project path resolution, and
analysis endpoints. Some of it is server-specific orchestration, but the
analysis endpoint directly uses parser/call-graph primitives from engine,
which may be reusable logic that belongs below the MCP endpoint layer if it
grows.

### Phase 3 Findings

- Root module imports are empty; coupling lives below public namespaces.
- `tools::router` is the central dispatcher and references most endpoint and
  graph tool modules.
- Graph tool modules depend directly on graph internals, especially
  `graph::model`, `graph::query::model`, `graph::snapshot`, `graph::storage`,
  `graph::ids`, and audit/codemap modules.
- The response formatting module also imports graph model/snapshot/storage
  types, so response shaping is coupled to graph internals.
- Query and codemap depend on `rmc_indexing::indexing::tantivy_adapter`
  directly.
- Index and sync depend on `rmc_indexing::indexing::incremental` directly.
- `mcp::project_paths` depends on indexing identity/snapshot helpers and engine
  embedding profile/backend details.
- External incoming usage is mostly binary/test oriented: `rust-code-mcp`
  imports `SearchTool` and `SyncManager`; tests import `ProjectPaths`,
  `index_codebase`, `IndexCodebaseParams`, and `SyncManager`.

### Open Questions For Later Phases

- Should graph expose server-ready DTO/query functions so server response code
  does not need graph model/snapshot/storage internals?
- Should indexing expose a "open BM25 search" helper to remove server's direct
  `TantivyAdapter` usage?
- Should project path identity live in indexing rather than server, since it
  combines engine embedding identity with indexing identity and snapshot paths?
- Is `tools::endpoints::analysis` an MCP adapter over semantic/engine logic, or
  is it growing reusable analysis behavior that should move down?

## Phase 4: Internal Cohesion

### Required VCS Check

Before Phase 4, `jj show --summary` reported:

```text
Commit ID: a5795b6a998e4ef5e4550c2db16f44efd04fbb95
Change ID: nqkxsutrzskxkqpmymynklyonnvxovpm
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
functions_with_filter(directory, krate="rmc_server", summary=true, limit=500)
functions_with_filter(directory, krate="rmc_server", has_param_type=<boundary type>, summary=true, limit=100)
functions_with_filter(directory, krate="rmc_server", returns_type_pattern="CallToolResult", summary=true, limit=200)
functions_with_filter(directory, krate="rmc_server", is_async=true, summary=true, limit=200)
overlaps(directory, scope="local_no_vendor")
semantic_overlaps(directory, crate_name="rmc_server", item_kind="Struct", summary=true, max_pairs=60)
semantic_overlaps(directory, crate_name="rmc_server", item_kind="Enum", summary=true, max_pairs=25)
semantic_overlaps(directory, crate_name="rmc_server", item_kind="Function", summary=true, max_pairs=60)
```

Function inventory:

```text
total indexed rmc_server functions: 260
async functions: 149
functions returning CallToolResult: 105
functions returning McpError pattern: 0
```

Boundary-signature filters:

```text
has_param_type="OpenedSnapshot"
  total: 7
  graph::core::enrich_bindings
  graph::core::enrich_usages
  graph::response::resolve_chunk_to_item
  graph::response::resolve_required_node
  graph::response::visibility_label
  graph::surface::enrich_crate_dead_pub
  graph::surface::enrich_dead_pub

has_param_type="GraphPaths"
  total: 0

has_param_type="ProjectPaths"
  total: 6
  endpoints::query::clean_stale_index
  endpoints::query::create_hybrid_search
  endpoints::query::ensure_indexed
  endpoints::query::resolve_query_backend
  endpoints::query::try_open_bm25
  endpoints::query::vector_metadata_exists

has_param_type="SyncManager"
  total: 4
  endpoints::index::index_codebase
  endpoints::query::ensure_indexed
  endpoints::query::search
  router::SearchToolRouter::with_sync_manager

has_param_type="EmbeddingBackend"
  total: 8
  mcp::project_paths::ProjectPaths::from_directory
  mcp::project_paths::ProjectPaths::from_directory_with_chunking_identity
  mcp::project_paths::ProjectPaths::from_existing_collection_name
  endpoints::query::backend_matches_request
  endpoints::query::create_hybrid_search
  endpoints::query::ensure_indexed
  endpoints::query::resolve_query_backend
  endpoints::query::select_index_paths

has_param_type="IncrementalIndexer"
  total: 0

has_param_type="CallToolResult"
  total: 1
  tools::graph::tests::first_text
```

Name-overlap context:

```text
cross_crate_type_collisions: 0
module_shadows: 0
common_fn_names: 0
within_crate_type_duplicates:
  none for rmc_server
  one unrelated rmc_graph test duplicate: SharedSnap
```

Semantic overlap, structs:

```text
seed_count: 106
total_pair_count: 26
total_cluster_count: 10

high-signal clusters:
  ItemsWithAttributeParams / ItemAttributesParams
  GraphDeclaredReexportsParams / GraphReexportsParams / GraphExportsParams
  DeadPubResponse / DeadPubReportResponse / DeadPubReportParams /
    DeadPubParams / EnrichedCrateDeadPub
  CallsFromParams / WhoCallsParams / WhoUsesSummaryParams /
    WhoUsesParams / CallersInCrateParams / WhoImportsParams /
    GraphImportsParams / CallGraphParams / RecursiveCallersCountParams
  MutStaticAuditParams / UnsafeAuditParams / PubUsePubTypeAuditParams
  ItemAttributesResponse / ItemsWithAttributeResponse
  FindDefinitionParams / FindReferencesParams
  ItemRef / SeedItemRef
  OverlapsParams / SemanticOverlapsParams
  MissingDocsAuditParams / DeriveAuditParams
```

Semantic overlap, enums:

```text
seed_count: 0
total_pair_count: 0
total_cluster_count: 0
```

Semantic overlap, functions:

```text
seed_count: 112
total_pair_count: 22
total_cluster_count: 11

clusters:
  mcp::project_paths::data_dir /
    tools::endpoints::indexing_support::data_dir
  endpoints::analysis::find_definition /
    endpoints::analysis::find_definition_with_options
  graph::core::get_declared_reexports /
    graph::core::get_reexports / graph::core::get_exports
  semantic::position::symbol_search /
    semantic::position::symbol_search_with_exact /
    semantic::position::find_references_by_name_with_exact /
    semantic::position::find_references_by_name /
    endpoints::analysis::find_references_with_options /
    endpoints::analysis::find_references
  graph::core::who_uses_summary / graph::core::who_uses
  graph::core::get_imports / graph::core::who_imports
  mcp::project_paths::resolve_embedding_backend /
    endpoints::query::resolve_requested_backend /
    graph::similarity::resolve_graph_tool_backend /
    endpoints::index::resolve_backend
  graph::surface::dead_pub_report / graph::surface::dead_pub_in_crate
  graph::core::who_calls / graph::core::calls_from /
    graph::core::callers_in_crate
  semantic::rename::rename_by_name /
    endpoints::analysis::rename_symbol
  graph::surface::item_attributes /
    graph::surface::items_with_attribute
```

### Phase 4 Interpretation

`rmc_server` is cohesive as an MCP-facing crate. The high counts of async
functions and `CallToolResult` return signatures confirm that much of the
crate is endpoint/router surface. The modules cluster around MCP tool routing,
graph tool adapters, search/index endpoints, cache/health operations, project
path resolution, background sync, and semantic editor operations.

The overlap findings are mostly expected MCP boilerplate: many parameter DTOs
differ only by the specific graph query they represent, and many router or
endpoint functions are shape-similar because they validate params, call a lower
service, and return `CallToolResult`.

There are a few real consolidation candidates:

- `mcp::project_paths::data_dir` and
  `tools::endpoints::indexing_support::data_dir` are near-duplicate helpers.
- Embedding backend resolution exists in four shape-similar functions across
  project paths, query, graph similarity, and index endpoints.
- `find_definition`/`find_definition_with_options`, reference search pairs,
  and exact/non-exact semantic helper pairs are intentional variants but should
  stay thin wrappers.

Boundary-signature filters reinforce phase 3. Graph snapshot types appear in
server graph response/core/surface helpers, and project-path and embedding
backend types appear in query/search path selection. No signatures take
`IncrementalIndexer`, which means indexing internals are constructed locally
rather than passed through server APIs.

### Phase 4 Findings

- `rmc_server` has 260 indexed functions.
- `149` functions are async, consistent with MCP endpoint/router work.
- `105` functions return `CallToolResult`, confirming a large request/response
  surface.
- No server-specific name collisions or module shadows were reported.
- Struct overlaps are mostly parameter/response DTO families for graph tools.
- Function overlaps are mostly endpoint/router wrappers and variant helper
  pairs.
- `data_dir` and embedding backend resolver duplication are worth considering
  for cleanup.
- Server cohesion is acceptable; the main issue remains coupling direction
  within the top-layer adapter, especially graph and indexing internals.

### Open Questions For Phase 5

- Should project path and embedding backend resolution be centralized into one
  server support module or moved into indexing?
- Should graph query/response DTO shaping move into graph to reduce server's
  direct dependency on snapshot/model/storage internals?
- Which public server modules should remain external API after tests are
  accounted for?

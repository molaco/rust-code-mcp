# rmc-server Boundary Report

## Status

- Crate: `rmc-server`
- Graph qualified name: `rmc_server`
- Analysis order: 4 of 4
- Current phase: Phase 2 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | 98e49844 | Graph snapshot reused; workspace and server dependency baseline captured. |
| Phase 1: Public surface | Complete | aedffaec | Root is narrow, but server exposes public implementation namespaces. |
| Phase 2: Dependency boundary | Complete | Pending commit | Outgoing edges match expected top-layer dependencies; no lower-layer rule violations. |
| Phase 3: Import and usage coupling | Pending | Not started |  |
| Phase 4: Internal cohesion | Pending | Not started |  |
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

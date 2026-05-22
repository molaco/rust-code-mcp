# rmc-graph Boundary Report

## Status

- Crate: `rmc-graph`
- Graph qualified name: `rmc_graph`
- Analysis order: 2 of 4
- Current phase: Phase 3 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | 7a9aa8f4 | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Complete | 2a193829 | Crate root is narrow, but `graph` exports a broad internal API surface. |
| Phase 2: Dependency boundary | Complete | ff821ccd | Outgoing edge is only to `rmc_engine`; expected layering rules have no violations. |
| Phase 3: Import and usage coupling | Complete | Pending commit | Server uses the graph facade path, but that facade exposes deep graph modules and DTOs. |
| Phase 4: Internal cohesion | Pending | Not started |  |
| Phase 5: Targeted source reads and recommendations | Pending | Not started |  |

## Phase 0: Snapshot Readiness And Baseline

### Required VCS Check

Before Phase 0, `jj show --summary` reported:

```text
Commit ID: 5a2b4f089d7b2d0ad80d8813bc9e339795950851
Change ID: tnloztrzrzzxrxpxuxumunrnnnqvsuqx
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

`rmc_graph` baseline from `crate_dependency_metric`:

```text
crate_name: rmc_graph
item_count: 585
efferent: 1
afferent: 11
instability: 0.08333333333333333
abstractness: 0.0
```

### Phase 0 Interpretation

`rmc_graph` behaves like a stable core crate. It has the largest item count
among the four target crates (`585`) and a low instability score
(`0.08333333333333333`). That matches the expected role: persisted graph model,
rust-analyzer extraction, storage, and graph query logic should be reusable by
the server/tool layer without depending back on it.

The baseline shows one outgoing producer edge and eleven incoming consumer
crates. Phase 2 must verify that the outgoing edge is only to the intended lower
engine layer and not to server or indexing.

### Phase 0 Findings

- No snapshot rebuild was required; the existing graph snapshot was reusable.
- `rmc_graph` is highly stable by dependency metric: low efferent count, high
  afferent count, low instability.
- `rmc_graph` has the largest item count of the target crates at this baseline:
  `585`.
- The crate is likely a central model/query boundary, so public API and facade
  quality are important.

### Open Questions For Later Phases

- Does `rmc_graph` expose a narrow facade, or are internals such as storage,
  loader, extraction, and model public to downstream crates?
- Is the one outgoing dependency only `rmc_engine`?
- Are examples/tests importing graph internals that should not be public?
- Does server depend on graph through stable query APIs or deep storage/model
  internals?

## Phase 1: Public Surface

### Required VCS Check

Before Phase 1, `jj show --summary` reported:

```text
Commit ID: 869ffc144a67910310ac8d968156a79e5b2695c0
Change ID: tmsklpzpqkyupqmrsvtzpxqzwwryqpwz
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
module_tree(directory, krate="rmc_graph", depth=2)
get_exports(directory, module="rmc_graph", consumer="rmc_graph", summary=true, limit=300)
get_declared_reexports(directory, module="rmc_graph", summary=false, limit=300)
pub_use_pub_type_audit(directory, crate_name="rmc_graph", summary=true, limit=300)
module_tree(directory, krate="rmc_graph", depth=3)
get_exports(directory, module="rmc_graph::graph", consumer="rmc_graph", summary=true, limit=500)
get_declared_reexports(directory, module="rmc_graph::graph", summary=false, limit=500)
```

Crate root:

```text
exports: 1
declared reexports: 0
public export:
  graph -> rmc_graph::graph
```

`pub_use_pub_type_audit` findings:

```text
count: 0
```

`rmc_graph::graph` surface:

```text
exports: 66
declared reexports: 43
```

Public modules under `rmc_graph::graph` from the module tree/export set:

```text
ast_resolve
attributes
bindings
channel_audit
codemap
derive_audit
docs_audit
extract
fn_body_audit
hir_trim
ids
impls
labels
loader
model
recursion_check
signatures
snapshot
statics
storage
unsafe_audit
usages
```

Representative root reexports from `rmc_graph::graph`:

```text
identity/model:
  NodeId, BindingId, Node, NodeKind, ItemKind, Namespace
  Binding, BindingVisibility, Usage, FunctionSignature

snapshot/storage:
  BuildOptions, OpenedSnapshot, build_and_persist, open_current
  GraphPaths, GraphEnvOptions

query/model rows:
  CallGraphNode, CrateEdge, CrateMetric, DeadPubFinding
  ForbiddenDependencyRule, ForbiddenDependencyViolation
  FunctionFilter, FunctionWithSignature, ItemWithAttribute
  ModuleDependency, ModuleDependencySymbol, ModuleTreeNode
  OverlapsReport, OverlapScope, ReExportChain
  RecursiveCallersCount, SelfKindFilter, UsageSummaryRow
  WorkspaceStats

loader/embedding/math:
  LoadedWorkspace, load, ensure_embeddings_for, cosine
```

### Phase 1 Interpretation

The crate root is clean but shallow: `rmc_graph` only exports the `graph`
module. The actual public boundary is `rmc_graph::graph`, and that boundary is
large. It exposes both facade-level query/snapshot types and implementation
modules such as `loader`, `extract`, `storage`, `model`, `ids`, `bindings`,
`usages`, and multiple audit modules.

That broad surface may be partly intentional because the server and examples
need query result types, snapshot build/open functions, and IDs. However, this
does not look like a tightly curated facade. It exposes internals that are
probably useful for examples, tests, diagnostics, and development tools, but
also become public API once downstream crates import them.

The absence of `pub_use_pub_type_audit` findings is good: the broad surface is
from explicit modules/reexports, not disguised type-alias reexports.

### Phase 1 Findings

- Root crate facade is narrow: one public module, `graph`.
- `rmc_graph::graph` is broad: 66 visible exports and 43 explicit reexports.
- Many implementation modules are public, including `loader`, `extract`,
  `storage`, `model`, `ids`, `bindings`, and `usages`.
- Query result DTOs are intentionally exposed through `graph`.
- Snapshot/storage primitives are exposed as public API.
- The main surface risk is not missing facade coverage; it is too much facade
  coverage, including internals.

### Open Questions For Later Phases

- Which external crates import `loader`, `extract`, `storage`, `model`, or
  `ids` directly?
- Does `rmc_server` use only high-level snapshot/query APIs, or does it depend
  on graph internals?
- Are public audit modules intended API, or should the server own the MCP-level
  audit wrappers while graph keeps lower-level helpers private?
- Should examples/debug binaries be allowed to depend on deeper graph internals
  while production crates use a smaller facade?

## Phase 2: Dependency Boundary

### Required VCS Check

Before Phase 2, `jj show --summary` reported:

```text
Commit ID: 8e35a9941c6a33633218cac43227b72910f53297
Change ID: zxysnyryuouqoqnvtnnwtnksrxlqxous
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
crate_dependency_metric(directory, sort_by="efferent", summary=true, limit=200)
forbidden_dependency_check(directory, rules=[rmc_graph -> *], consumer_kinds=[lib, bin, example, test, bench], summary=true, limit=200)
forbidden_dependency_check(directory, rules=[* -> rmc_graph], consumer_kinds=[lib, bin, example, test, bench], summary=true, limit=200)
forbidden_dependency_check(directory, rules=[expected layering rules], consumer_kinds=[lib, bin, example, test, bench], summary=true, limit=200)
```

`rmc_graph` dependency metric:

```text
crate_name: rmc_graph
item_count: 585
efferent: 1
afferent: 11
instability: 0.08333333333333333
abstractness: 0.0
```

Outgoing edge inventory:

```text
rmc_graph -> rmc_engine
sample_symbol: rmc_engine::embeddings::backend::EmbeddingBackend
unique_symbols: 7
total_refs: 11
```

Incoming edge inventory:

```text
count_items -> rmc_graph
  sample_symbol: rmc_graph::graph::snapshot::BuildOptions
  unique_symbols: 6
  total_refs: 11

dead_pub_report -> rmc_graph
  sample_symbol: rmc_graph::graph::snapshot::BuildOptions
  unique_symbols: 8
  total_refs: 13

debug_burn_loader -> rmc_graph
  sample_symbol: rmc_graph::graph::loader::LoadedWorkspace
  unique_symbols: 2
  total_refs: 4

debug_burn_target -> rmc_graph
  sample_symbol: rmc_graph::graph::snapshot::OpenedSnapshot::read_txn
  unique_symbols: 11
  total_refs: 24

debug_itemscope -> rmc_graph
  sample_symbol: rmc_graph::graph::loader
  unique_symbols: 2
  total_refs: 2

graph_burn -> rmc_graph
  sample_symbol: rmc_graph::graph::snapshot::BuildOptions
  unique_symbols: 7
  total_refs: 12

probe_workspace -> rmc_graph
  sample_symbol: rmc_graph::graph::snapshot::BuildOptions
  unique_symbols: 10
  total_refs: 15

rebuild_burn_default -> rmc_graph
  sample_symbol: rmc_graph::graph::snapshot::BuildOptions
  unique_symbols: 2
  total_refs: 4

rmc_server -> rmc_graph
  sample_symbol: rmc_graph::graph::model::NodeKind
  unique_symbols: 108
  total_refs: 334

spike_usages -> rmc_graph
  sample_symbol: rmc_graph::graph::loader
  unique_symbols: 2
  total_refs: 2

timing_extract -> rmc_graph
  sample_symbol: rmc_graph::graph::ids::NodeId
  unique_symbols: 22
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

The crate-level dependency direction is healthy. `rmc_graph` depends only on
`rmc_engine`, and the sampled outgoing symbol is the embedding backend. That is
consistent with graph owning persisted graph/query semantics while delegating
embedding primitives to the lower engine crate.

No layering rule fired for graph/server/indexing. There is no graph-to-server
edge and no graph-to-indexing edge in the MCP edge inventory. This means the
main graph boundary risk is not crate direction; it is API breadth and consumer
coupling.

The incoming edge inventory makes that risk concrete. Most incoming consumers
are examples, probes, debug binaries, or reports, but `rmc_server` references
`108` unique graph symbols across `334` total refs. Phase 3 must determine
whether those server references go through stable snapshot/query DTOs or through
implementation modules such as `model`, `ids`, `storage`, `loader`, and
specific audit internals.

### Phase 2 Findings

- `rmc_graph` has one outgoing crate edge: `rmc_engine`.
- No `rmc_graph -> rmc_server` edge exists.
- No `rmc_graph -> rmc_indexing` edge exists.
- The expected cross-layer rules returned zero violations.
- `rmc_server` is the dominant production consumer of graph, with `108` unique
  symbols and `334` refs.
- Example/debug/report crates also use graph directly, often via snapshot,
  loader, ID, and model symbols.

### Open Questions For Later Phases

- Which `rmc_server` modules account for the 108 unique graph symbols?
- Is `rmc_server` using high-level query APIs, or is it coupled to graph's
  internal model/storage modules?
- Should debug and report binaries be allowed to keep deeper graph access while
  production crates are restricted to a smaller facade?

## Phase 3: Import And Usage Coupling

### Required VCS Check

Before Phase 3, `jj show --summary` reported:

```text
Commit ID: e047a81007cdc8061af4bbff6f14a830684c58a1
Change ID: yolyorrtvlppoosqrynmonlzuvptzqny
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
get_imports(directory, module="rmc_graph", summary=true, limit=300)
module_dependencies(directory, module="rmc_graph", summary=true, limit=300)
get_imports(directory, module="rmc_graph::graph", summary=true, limit=300)
module_dependencies(directory, module="rmc_graph::graph", summary=true, limit=300)
who_imports(directory, target=<key graph symbols>, summary=true, limit=200)
who_uses_summary(directory, target=<key graph symbols>, summary=true, limit=200)
module_dependencies(directory, module=<server graph modules>, summary=true, limit=200)
```

Crate root imports/dependencies:

```text
rmc_graph imports: 0
rmc_graph module dependencies: 0
```

`rmc_graph::graph` facade imports:

```text
total imports: 43
dependency modules: 8

rmc_graph::graph::query::model
  import_count: 22

rmc_graph::graph::model
  import_count: 9

rmc_graph::graph::snapshot
  import_count: 4

rmc_graph::graph::ids
  import_count: 2

rmc_graph::graph::loader
  import_count: 2

rmc_graph::graph::storage
  import_count: 2

rmc_graph::graph::embedding_cache
  import_count: 1

rmc_graph::graph::math
  import_count: 1
```

Key symbol import/usage rollups:

```text
BuildOptions
  who_imports total: 11
  who_uses_summary total modules: 11
  external/server consumers include:
    count_items, dead_pub_report, graph_burn, probe_workspace,
    rebuild_burn_default, rmc_server::tools::graph::core, timing_extract

OpenedSnapshot
  who_imports total: 43
  who_uses_summary total modules: 31
  server consumers:
    rmc_server::tools::graph::core
    rmc_server::tools::graph::response
    rmc_server::tools::graph::surface

NodeKind
  who_imports total: 38
  who_uses_summary total modules: 31
  server consumers:
    rmc_server::tools::graph::audits
    rmc_server::tools::graph::core
    rmc_server::tools::graph::response
    rmc_server::tools::graph::similarity
    rmc_server::tools::graph::surface

NodeId
  who_imports total: 62
  who_uses_summary total modules: 44
  server consumers:
    rmc_server::tools::graph::audits
    rmc_server::tools::graph::response
    rmc_server::tools::graph::similarity
    rmc_server::tools::graph::surface
    rmc_server::tools::graph::tests

GraphPaths
  who_imports total: 13
  who_uses_summary total modules: 13
  server consumers:
    rmc_server::tools::endpoints::cache
    rmc_server::tools::graph::response

load
  who_imports total: 3
  who_uses_summary total modules: 9
  server consumer:
    rmc_server::tools::graph::audits

ensure_embeddings_for
  who_imports total: 1
  who_uses_summary total modules: 2
  server consumer:
    rmc_server::tools::graph::similarity
```

Server module dependency rollups:

```text
rmc_server::tools::graph::core
  rmc_graph::graph::snapshot: imports 3, usages 35
  rmc_graph::graph::model: imports 4, usages 12
  rmc_graph::graph::query::model: imports 8, usages 10
  rmc_graph::graph::labels: imports 4, usages 3

rmc_server::tools::graph::response
  rmc_graph::graph::model: imports 4, usages 25
  rmc_graph::graph::snapshot: imports 2, usages 9
  rmc_graph::graph::ids: imports 1, usages 8
  rmc_graph::graph::storage: imports 2, usages 3
  rmc_graph::graph::query::model: imports 1, usages 4

rmc_server::tools::graph::surface
  rmc_graph::graph::snapshot: imports 1, usages 26
  rmc_graph::graph::model: imports 4, usages 19
  rmc_graph::graph::query::model: imports 9, usages 12
  rmc_graph::graph::ids: imports 1, usages 4
  rmc_graph::graph::derive_audit: imports 0, usages 3
  rmc_graph::graph::docs_audit: imports 0, usages 3

rmc_server::tools::graph::audits
  rmc_graph::graph::ids: imports 1, usages 8
  rmc_graph::graph::model: imports 1, usages 6
  rmc_graph::graph::snapshot: imports 0, usages 5
  rmc_graph::graph::fn_body_audit: imports 0, usages 4
  rmc_graph::graph::recursion_check: imports 0, usages 4
  rmc_graph::graph::loader: imports 0, usages 3
  rmc_graph::graph::channel_audit: imports 0, usages 3
  rmc_graph::graph::unsafe_audit: imports 0, usages 1

rmc_server::tools::graph::similarity
  rmc_graph::graph::ids: imports 1, usages 15
  rmc_graph::graph::model: imports 2, usages 6
  rmc_graph::graph::snapshot: imports 0, usages 2
  rmc_graph::graph::embedding_cache: imports 0, usages 1
  rmc_graph::graph::math: imports 0, usages 1

rmc_server::tools::endpoints::cache
  rmc_graph::graph::storage: imports 0, usages 3
```

### Phase 3 Interpretation

Consumers generally enter through the `rmc_graph::graph` public path, but that
path is not a thin API boundary. The facade reexports IDs, model structs/enums,
snapshot/storage types, loader functions, query DTOs, embedding-cache helpers,
and math helpers. As a result, server code can appear to use the facade while
still depending on graph internals by ownership and semantics.

The heaviest server coupling is in graph tool modules. `core`, `surface`, and
`response` use snapshot, model, query model, ID, storage, and label modules.
That is partly expected: server graph tools need to translate graph query data
into MCP responses. However, `audits` and `similarity` also reach into graph
implementation areas such as `loader`, audit modules, `embedding_cache`, and
`math`. Those are stronger boundary leaks than query DTO usage because they
couple MCP tool behavior to graph implementation helpers rather than a stable
query API.

`GraphPaths` also appears outside graph-specific server tooling in
`rmc_server::tools::endpoints::cache`. That suggests graph storage layout is
part of server cache behavior. This may be intentional, but it means graph
storage path construction is not fully encapsulated by graph.

### Phase 3 Findings

- The crate root has no coupling; all coupling is concentrated in
  `rmc_graph::graph`.
- `rmc_graph::graph` acts as a broad reexport surface over eight internal
  modules.
- Server graph tools depend heavily on `snapshot`, `model`, `query::model`,
  `ids`, `labels`, and `storage`.
- Server audit/similarity tools reach deeper into graph implementation modules:
  `loader`, `embedding_cache`, `math`, `channel_audit`, `fn_body_audit`,
  `recursion_check`, and `unsafe_audit`.
- `GraphPaths` is used by `rmc_server::tools::endpoints::cache`, so graph
  storage paths leak into a non-graph endpoint.
- `ensure_embeddings_for` is reexported publicly but has only two usage modules:
  graph codemap build and server similarity.
- `load` is public but appears mainly for graph internals, server audit tooling,
  and debug binaries.

### Open Questions For Later Phases

- Should `loader::load`, `embedding_cache::ensure_embeddings_for`, and
  `math::cosine` remain public exports, or become graph-internal helpers behind
  query/similarity APIs?
- Should audit-specific graph modules expose high-level query functions instead
  of letting server audit tools call multiple internal audit modules directly?
- Should `GraphPaths` remain visible to server cache code, or should graph own
  cache/snapshot path derivation behind a smaller storage facade?

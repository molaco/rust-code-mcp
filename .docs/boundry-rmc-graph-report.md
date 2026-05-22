# rmc-graph Boundary Report

## Status

- Crate: `rmc-graph`
- Graph qualified name: `rmc_graph`
- Analysis order: 2 of 4
- Current phase: Phase 5 complete
- Report state: complete
- Boundary score: 7/10

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | 7a9aa8f4 | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Complete | 2a193829 | Crate root is narrow, but `graph` exports a broad internal API surface. |
| Phase 2: Dependency boundary | Complete | ff821ccd | Outgoing edge is only to `rmc_engine`; expected layering rules have no violations. |
| Phase 3: Import and usage coupling | Complete | f483c864 | Server uses the graph facade path, but that facade exposes deep graph modules and DTOs. |
| Phase 4: Internal cohesion | Complete | f90b3c41 | Large cohesive graph/query crate with small duplication clusters around audits, storage helpers, labels, and test support. |
| Phase 5: Targeted source reads and recommendations | Complete | Pending commit | Dependency direction is healthy, but the graph facade exposes implementation modules used directly by server. |

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

## Phase 4: Internal Cohesion

### Required VCS Check

Before Phase 4, `jj show --summary` reported:

```text
Commit ID: e3b19d78ba34e8d2db4eaa882f38c7c8167a1916
Change ID: zrzmlxtvklqyyxrvxxnxkksqxnukzmsn
Description: (no description set)
```

### MCP Evidence

Commands used:

```text
functions_with_filter(directory, krate="rmc_graph", summary=true, limit=300)
functions_with_filter(directory, krate="rmc_graph", has_param_type="rmc_engine", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_graph", has_param_type="EmbeddingBackend", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_graph", has_param_type="OpenedSnapshot", summary=true, limit=200)
functions_with_filter(directory, krate="rmc_graph", has_param_type="NodeId", summary=true, limit=200)
functions_with_filter(directory, krate="rmc_graph", has_param_type="GraphPaths", summary=true, limit=100)
overlaps(directory, scope="local_no_vendor")
semantic_overlaps(directory, crate_name="rmc_graph", item_kind="Struct", summary=true, max_pairs=25)
semantic_overlaps(directory, crate_name="rmc_graph", item_kind="Enum", summary=true, max_pairs=25)
semantic_overlaps(directory, crate_name="rmc_graph", item_kind="Function", summary=true, max_pairs=25)
```

Function inventory:

```text
total indexed rmc_graph functions: 431
returned first page: 300
```

Boundary-signature filters:

```text
has_param_type="rmc_engine"
  total: 0

has_param_type="EmbeddingBackend"
  total: 1
  rmc_graph::graph::embedding_cache::ensure_embeddings_for

has_param_type="OpenedSnapshot"
  total: 20
  representative functions:
    audit_util::resolve_enclosing_function
    channel_audit::channel_capacity_audit
    codemap::build::build_codemap
    codemap::seeds::resolve_search_seeds
    derive_audit::derive_audit
    docs_audit::missing_docs_audit
    embedding_cache::ensure_embeddings_for
    fn_body_audit::fn_body_audit
    recursion_check::recursion_check
    unsafe_audit::unsafe_audit_impl

has_param_type="NodeId"
  total: 72
  representative areas:
    attributes
    bindings
    codemap
    embedding_cache
    extract
    impls
    query
    recursion_check
    signatures
    snapshot
    statics
    usages

has_param_type="GraphPaths"
  total: 3
  rmc_graph::graph::snapshot::open_current
  rmc_graph::graph::snapshot::open_specific
  rmc_graph::graph::snapshot::publish_current
```

Name-overlap context:

```text
cross_crate_type_collisions: 0
module_shadows: 0
common_fn_names: 0
within_crate_type_duplicates:
  rmc_graph::graph::test_support::SharedSnap
  rmc_graph::graph::usages::tests::SharedSnap
```

Semantic overlap, structs:

```text
seed_count: 73
total_pair_count: 10
total_cluster_count: 6

notable clusters:
  ReExportLink / ReExportChain
  ModuleDependencySymbol / ModuleDependency /
    ModuleDependencyAccumulator / ModuleDependencySymbolAccumulator
  CrateDeadPub / DeadPubFinding
  DocsAuditOpts / DeriveAuditOpts
  ForbiddenDependencyViolation / ForbiddenDependencyRule
  Codemap / CodemapStats
```

Semantic overlap, enums:

```text
seed_count: 11
total_pair_count: 0
total_cluster_count: 0
```

Semantic overlap, functions:

```text
seed_count: 153
total_pair_count: 12
total_cluster_count: 9

notable clusters:
  docs_audit::default_kind_filter / derive_audit::default_kind_filter
  fn_body_audit::match_unwrap / fn_body_audit::match_unwrap_unchecked
  storage::open_or_create_str_bytes /
    storage::open_or_create_bytes_bytes /
    storage::open_or_create_bytes_bincode
  labels::item_kind_id_label /
    labels::item_kind_display_label /
    labels::item_kind_short_label
  bindings::classify_value_provenance /
    bindings::classify_type_provenance
  storage::read_manifest / storage::read_manifest_compatible
  attributes::visit_assoc_item / impls::emit_assoc_item
  loader::target_kind_label / loader::canonical_target_kind
  codemap::test_support::shared_fixture /
    test_support::shared_snapshot
```

### Phase 4 Interpretation

`rmc_graph` is large but mostly coherent for its current role: extraction,
snapshot persistence, graph query APIs, codemap construction, and graph-backed
audits all orbit the persisted graph model. The high `NodeId` and
`OpenedSnapshot` signature counts are expected for this architecture. They show
the crate has a strong internal shared model rather than several disconnected
subsystems.

The cohesion issue is scale and surface shape, not dependency direction.
`OpenedSnapshot` has become the central query object for many functions and
tools, while `NodeId` is the common identity currency across extraction, query,
audit, codemap, and storage-facing code. That is understandable, but it means
the public boundary needs to be curated carefully; otherwise the same internal
model that makes the crate cohesive becomes an oversized public API.

The duplicate/semantic overlap findings are small and mostly intentional
pairs. The strongest cleanup candidates are repeated audit option/filter helpers,
storage open/read helper variants, label formatting functions, and duplicate
test-support snapshot fixtures. None of these imply a broken crate boundary by
themselves.

### Phase 4 Findings

- `rmc_graph` has 431 indexed functions, making it the largest analyzed crate
  so far.
- No function signature directly contains `rmc_engine`; only
  `ensure_embeddings_for` takes `EmbeddingBackend`.
- `OpenedSnapshot` appears in 20 function signatures and acts as the central
  graph query/snapshot context.
- `NodeId` appears in 72 function signatures and is the common identity type
  across most graph subsystems.
- `GraphPaths` is concentrated in snapshot open/publish functions, which is a
  good internal cohesion signal.
- No cross-crate type collisions or module shadows were reported.
- The only within-crate type duplicate is test-support `SharedSnap`.
- Semantic overlap found no enum duplication and only small struct/function
  clusters, mostly around paired DTOs, helper variants, and tests.

### Open Questions For Phase 5

- Which broad public modules should remain exposed because server tools need
  them, and which can be hidden behind `OpenedSnapshot` methods or query
  functions?
- Should repeated audit option/filter structs be unified, or are they clearer
  as separate audit-specific inputs?
- Should storage helper variants remain separate functions, or be normalized
  behind one typed helper before any storage API cleanup?

## Phase 5: Targeted Source Reads And Recommendations

### Required VCS Check

Before Phase 5, `jj show --summary` reported:

```text
Commit ID: 5058ac1b5bccf50c4242b51dde9e0f9ca4dc70a3
Change ID: xwylovkoqlyzyppnovyvoqnsssmxopsy
Description: (no description set)
```

### Source Reads

Source reads were limited to files identified by MCP phases 1-4:

```text
crates/rmc-graph/src/lib.rs
crates/rmc-graph/src/graph/mod.rs
crates/rmc-graph/src/graph/snapshot.rs
crates/rmc-graph/src/graph/storage.rs
crates/rmc-graph/src/graph/loader.rs
crates/rmc-graph/src/graph/embedding_cache.rs
crates/rmc-graph/src/graph/math.rs
crates/rmc-graph/src/graph/query/model.rs
crates/rmc-server/src/tools/graph/core.rs
crates/rmc-server/src/tools/graph/response.rs
crates/rmc-server/src/tools/graph/surface.rs
crates/rmc-server/src/tools/graph/audits.rs
crates/rmc-server/src/tools/graph/similarity.rs
crates/rmc-server/src/tools/endpoints/cache.rs
```

Key source evidence:

```text
crates/rmc-graph/src/lib.rs:
  pub mod graph;

crates/rmc-graph/src/graph/mod.rs:
  public modules include:
    ast_resolve, attributes, bindings, channel_audit, codemap,
    derive_audit, docs_audit, extract, fn_body_audit, hir_trim,
    ids, impls, labels, loader, model, recursion_check,
    signatures, snapshot, statics, storage, unsafe_audit, usages

  private modules reexport public helpers:
    mod embedding_cache;
    mod math;
    pub use embedding_cache::ensure_embeddings_for;
    pub use math::cosine;

  public loader and storage reexports:
    pub use loader::{LoadedWorkspace, load};
    pub use storage::{GraphEnvOptions, GraphPaths};

crates/rmc-graph/src/graph/snapshot.rs:
  BuildOptions is public.
  OpenedSnapshot is public.
  OpenedSnapshot exposes public fields:
    manifest, snapshot_dir, env, dbs
  OpenedSnapshot exposes public read/write transaction helpers.
  open_current is public.
  open_specific is pub(crate).

crates/rmc-graph/src/graph/storage.rs:
  GraphPaths owns workspace hash, root dir, current pointer path,
  and snapshots dir.
  default_data_dir is public through the public storage module.

crates/rmc-graph/src/graph/query/model.rs:
  Comment states that external callers reach query result types through the
  graph facade and that the explicit reexport list is the public contract.

crates/rmc-server/src/tools/graph/core.rs:
  Imports BuildOptions from rmc_graph::graph::snapshot and many DTOs from
  rmc_graph::graph.
  Runs build_and_persist in spawn_blocking.

crates/rmc-server/src/tools/graph/response.rs:
  Opens snapshots by constructing GraphPaths and calling open_current.
  Imports GraphEnvOptions, GraphPaths, Node, NodeId, OpenedSnapshot, and
  open_current from rmc_graph::graph.

crates/rmc-server/src/tools/graph/audits.rs:
  Calls rmc_graph::graph::loader::load directly for audits that need a full
  rust-analyzer workspace load.
  Calls graph audit modules directly from the server layer.

crates/rmc-server/src/tools/graph/similarity.rs:
  Calls rmc_graph::graph::ensure_embeddings_for.
  Calls rmc_graph::graph::cosine.

crates/rmc-server/src/tools/endpoints/cache.rs:
  Uses rmc_graph::graph::GraphPaths::for_workspace to clear hypergraph
  snapshot directories.
```

### Final Boundary Assessment

Boundary score: 7/10.

`rmc_graph` has the right dependency direction. It depends only on
`rmc_engine`, has no edge to server or indexing, and has no forbidden layering
violations. Internally, the crate is coherent: extraction, persistence, query,
codemap, audit, and semantic-overlap cache behavior all revolve around the
persisted hypergraph model.

The reason this is not an 8-10 boundary is public API shape. The crate root is
minimal, but `rmc_graph::graph` is both the facade and the implementation
namespace. It exposes many implementation modules directly and reexports
low-level helpers (`loader::load`, `ensure_embeddings_for`, `cosine`,
`GraphPaths`, `GraphEnvOptions`) alongside stable query DTOs and snapshot
operations. Server code then relies on that broad surface for MCP tool behavior.

The most important boundary leak is not that server imports graph types; that
is expected. The leak is that server owns orchestration around graph internals:
it opens graph storage paths, calls the graph loader for audit endpoints, calls
audit modules directly, and performs semantic-overlap embedding/cosine control
using helpers reexported from graph.

### Recommendations

1. Keep `rmc_graph -> rmc_engine` as the only graph dependency.
   The current crate direction is correct. Do not introduce dependencies from
   graph to server or indexing.

2. Treat `rmc_graph::graph` as a compatibility facade, then narrow new API.
   Existing callers can continue using it, but new server-facing work should
   prefer explicit facade groups such as snapshot/query/audit/similarity rather
   than exposing every implementation module.

3. Move server audit orchestration behind graph-owned functions.
   Server audit endpoints currently call `loader::load` and audit modules
   directly. A cleaner boundary would expose higher-level graph audit entry
   points that accept directory/options and own the load/snapshot/audit details.

4. Stop exporting `loader::load` unless debug binaries require it as API.
   MCP evidence shows production server audit tooling uses it. If that logic
   moves behind graph audit APIs, `load` can become `pub(crate)` or remain only
   for examples/debug binaries through a clearly marked dev surface.

5. Hide or reduce `ensure_embeddings_for` and `cosine` as public graph exports.
   These are implementation helpers for graph semantic-overlap behavior.
   Server similarity currently coordinates them directly. Prefer a graph-owned
   semantic-overlap operation or a small similarity service API.

6. Encapsulate graph storage cleanup.
   `rmc_server::tools::endpoints::cache` reaches into `GraphPaths` and graph
   storage layout. A graph-owned `clear_workspace_snapshot` or path-planning
   helper would keep storage layout decisions in graph while still supporting
   server cache tools.

7. Keep query DTOs public, but separate them from implementation modules.
   `query/model.rs` already documents the explicit reexport list as the public
   contract. That is good. The next cleanup should make this contract visually
   and structurally distinct from extraction/storage/loader/audit internals.

8. Leave the small semantic-overlap clusters alone unless churn continues.
   Audit option structs, storage open helpers, label helpers, and test fixtures
   show duplication, but they are local cleanup opportunities rather than
   current boundary failures.

### Final Findings

- Strong boundary: dependency direction is correct and layering rules are clean.
- Strong boundary: `GraphPaths` is concentrated in snapshot/storage code inside
  graph, although server currently reaches into it for cache cleanup.
- Strong boundary: internal model cohesion is high around `OpenedSnapshot` and
  `NodeId`.
- Weak boundary: `rmc_graph::graph` mixes facade API and implementation modules.
- Weak boundary: server audit/similarity tools call graph internals directly.
- Weak boundary: `OpenedSnapshot` exposes public storage/environment fields.
- Watch item: `loader::load`, `ensure_embeddings_for`, `cosine`, `GraphPaths`,
  and `GraphEnvOptions` should be treated as boundary-sensitive APIs.
- No immediate dependency-direction refactor is required before analyzing
  `rmc_indexing` and `rmc_server`.

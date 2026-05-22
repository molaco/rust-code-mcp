# rmc-graph Boundary Report

## Status

- Crate: `rmc-graph`
- Graph qualified name: `rmc_graph`
- Analysis order: 2 of 4
- Current phase: Phase 1 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | 7a9aa8f4 | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Complete | Pending commit | Crate root is narrow, but `graph` exports a broad internal API surface. |
| Phase 2: Dependency boundary | Pending | Not started |  |
| Phase 3: Import and usage coupling | Pending | Not started |  |
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

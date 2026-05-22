# Boundary Analysis Plan

## Objective

Analyze the crate boundaries for `rmc-server`, `rmc-graph`, `rmc-indexing`,
and `rmc-engine` separately, using the available rust-code-mcp MCP tools as the
primary evidence source.

The goal is to answer, for each crate:

- What responsibility does this crate appear to own?
- What is its public boundary?
- Which internals leak through exports, reexports, or downstream usage?
- Which dependencies point in or out of the crate?
- Does the crate depend on the right layer?
- What changes would make the boundary cleaner?

## Hard Rules

- Use MCP tool calls for the analysis.
- Do not write Python scripts.
- Do not use stdio harnesses.
- Do not run formatting commands.
- Do not use unscoped expensive semantic scans.
- Use local source reads only after MCP results point to specific files or
  symbols that need interpretation.

## Workspace

```text
directory = "/home/molaco/Documents/rust-code-mcp-refactor"
```

Cargo package names:

```text
rmc-server
rmc-graph
rmc-indexing
rmc-engine
```

Graph qualified names are expected to use Rust-style crate names:

```text
rmc_server
rmc_graph
rmc_indexing
rmc_engine
```

If any MCP query fails by qualified name, first confirm the canonical crate names
from `workspace_stats`, `crate_edges`, or `module_tree`, then rerun with the
resolved name.

## Expected Layering Hypothesis

This is the starting hypothesis to test, not something to assume as true:

```text
rmc-server   -> orchestration, MCP request/response boundary, tool routing
rmc-graph    -> persisted hypergraph, rust-analyzer extraction, graph queries
rmc-indexing -> codebase indexing, BM25/vector index coordination, cache sync
rmc-engine   -> low-level parser/chunker/embedding/search/vector primitives
```

Expected dependency direction, shown with Cargo package names:

```text
rmc-server   -> rmc-graph, rmc-indexing, rmc-engine, rmc-config
rmc-graph    -> rmc-engine
rmc-indexing -> rmc-engine, rmc-config
rmc-engine   -> no dependency on server, graph, indexing, or config
```

Potential smells:

- `rmc-engine` knowing about server, graph, indexing, workspace orchestration, or
  project policy.
- `rmc-graph` knowing about MCP response shaping or indexing implementation.
- `rmc-indexing` knowing about persisted hypergraph internals.
- `rmc-server` containing business logic that belongs in graph/indexing/engine.
- Downstream crates importing deep modules instead of crate-root facade exports.
- Public items that are only public to work around crate boundary placement.

## Phase 0: Snapshot Readiness

Use the refactor MCP server tools.

1. Check whether the graph snapshot is usable:

```text
build_hypergraph(directory, force_rebuild=false)
```

2. If the snapshot is missing or stale, rebuild once:

```text
build_hypergraph(directory, force_rebuild=true)
```

3. Capture a workspace baseline:

```text
workspace_stats(directory)
crate_edges(directory, summary=true, limit=200)
crate_dependency_metric(directory, sort_by="instability", limit=200)
```

Record:

- local crate list
- item counts by crate
- cross-crate dependency matrix
- afferent/efferent coupling
- instability and abstractness metrics

## Phase 1: Per-Crate Public Surface

Run this pass independently for:

```text
rmc_server
rmc_graph
rmc_indexing
rmc_engine
```

MCP calls:

```text
module_tree(directory, krate=<crate>, depth=null)
get_exports(directory, module=<crate>, consumer=<crate>, summary=true, limit=300)
get_declared_reexports(directory, module=<crate>, summary=false, limit=300)
pub_use_pub_type_audit(directory, crate_name=<crate>, summary=true, limit=300)
```

If the root module exports too much, repeat `get_exports` and
`get_declared_reexports` for important public submodules from `module_tree`.

Questions:

- Is there a clear crate-root facade?
- Are public modules grouped by responsibility?
- Are internals exposed directly from deep modules?
- Are reexports intentional and centralized?
- Are public types exposed through both canonical paths and facade paths?
- Are there public types that look like implementation details?

Evidence to record:

- top-level public modules
- root exports
- declared `pub use` declarations
- public type audit findings
- surprising public items

## Phase 2: Dependency Boundary

Use global crate dependency tools, then filter the results for the four target
crates.

MCP calls:

```text
crate_edges(directory, summary=false, limit=500)
crate_dependency_metric(directory, sort_by="efferent", limit=200)
crate_dependency_metric(directory, sort_by="afferent", limit=200)
```

Run an architecture-rule check against the expected layering:

```text
forbidden_dependency_check(
  directory,
  rules=[
    {
      consumer: "rmc_engine",
      producer: "rmc_*",
      severity: "error",
      message: "engine should remain the lowest-level primitive crate"
    },
    {
      consumer: "rmc_graph",
      producer: "rmc_server",
      severity: "error",
      message: "graph should not depend on the MCP server layer"
    },
    {
      consumer: "rmc_graph",
      producer: "rmc_indexing",
      severity: "warn",
      message: "graph and indexing should stay sibling layers unless a shared primitive belongs in engine"
    },
    {
      consumer: "rmc_indexing",
      producer: "rmc_server",
      severity: "error",
      message: "indexing should not depend on the MCP server layer"
    },
    {
      consumer: "rmc_indexing",
      producer: "rmc_graph",
      severity: "warn",
      message: "indexing should not depend on persisted hypergraph internals"
    }
  ],
  summary=false,
  limit=300
)
```

Questions:

- Which crates are depended on by many others?
- Which crates depend on many others?
- Are there upward dependencies?
- Is `rmc-server` mostly at the edge, or does it own reusable logic?
- Are graph/indexing dependencies truly sibling-independent?
- Should any shared logic move down into `rmc-engine` or another shared crate?

Evidence to record:

- incoming edges per crate
- outgoing edges per crate
- forbidden dependency violations
- coupling metrics
- symbols responsible for suspicious edges

## Phase 3: Import and Usage Coupling

Use this pass to see whether callers depend on stable facades or deep internals.

For each target crate root:

```text
get_imports(directory, module=<crate>, summary=true, limit=300)
module_dependencies(directory, module=<crate>, summary=true, limit=300)
```

For important public exports identified in Phase 1:

```text
who_imports(directory, target=<qualified_name>, summary=true, limit=200)
who_uses_summary(directory, target=<qualified_name>, summary=true, limit=200)
```

Pick targets from:

- crate-root reexports
- public structs/enums/traits used by sibling crates
- public functions that appear to coordinate another crate
- types flagged by `pub_use_pub_type_audit`

Questions:

- Do consumers import from the root facade or from deep modules?
- Are internal modules effectively part of the public API because callers import
  them directly?
- Are public items used only by one sibling crate?
- Are public items unused outside their own crate?
- Are there public adapters or DTOs that belong in a different layer?

Evidence to record:

- high-traffic public types/functions
- deep imports from sibling crates
- public but low-use items
- usage clusters that cross intended boundaries

## Phase 4: Internal Cohesion

Use crate-scoped tools only. Avoid workspace-wide semantic scans.

For each target crate:

```text
functions_with_filter(directory, krate=<crate>, summary=true, limit=300)
```

Then run focused parameter-type searches when checking boundary crossings:

```text
functions_with_filter(directory, krate="rmc_server", has_param_type="rmc_graph", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_server", has_param_type="rmc_indexing", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="rmc_engine", summary=true, limit=100)
functions_with_filter(directory, krate="rmc_graph", has_param_type="rmc_engine", summary=true, limit=100)
```

Use scoped semantic overlap only if duplication or mixed responsibility is
suspected from earlier phases:

```text
semantic_overlaps(directory, crate_name=<crate>, item_kind="Struct", summary=true, max_pairs=25)
semantic_overlaps(directory, crate_name=<crate>, item_kind="Enum", summary=true, max_pairs=25)
semantic_overlaps(directory, crate_name=<crate>, item_kind="Function", summary=true, max_pairs=25)
```

Use `overlaps` once for name/type collision context:

```text
overlaps(directory, scope="local_no_vendor")
```

Questions:

- Does each crate contain one coherent responsibility cluster?
- Are there repeated DTOs, config structs, error enums, or adapters?
- Are functions in one crate taking many types from another crate?
- Are there modules that look like extracted leftovers from the old monolith?
- Are naming collisions creating accidental boundary confusion?

Evidence to record:

- repeated type shapes
- suspicious function signatures
- same-name types across crates
- module clusters that do not match crate responsibility

## Phase 5: Targeted Source Reads

Only after MCP tools identify specific modules or symbols, read local source to
interpret the finding.

Allowed examples:

```text
sed -n '<range>p' crates/rmc-server/src/tools/router.rs
sed -n '<range>p' crates/rmc-graph/src/graph/snapshot.rs
```

Do not use source reads as the primary discovery mechanism. They are for
confirming and explaining MCP findings.

## Deliverable

Write the final analysis to:

```text
.docs/crate-boundary-analysis.md
```

Recommended structure:

```text
# Crate Boundary Analysis

## Summary
- current boundary score
- strongest boundaries
- weakest boundaries
- highest-priority changes

## rmc-server
- intended responsibility
- public surface
- incoming dependencies
- outgoing dependencies
- usage/import patterns
- boundary smells
- recommendations

## rmc-graph
...

## rmc-indexing
...

## rmc-engine
...

## Cross-Crate Findings
- dependency direction issues
- facade/reexport gaps
- deep-import problems
- duplicated abstractions
- candidate moves

## Proposed Follow-Up Work
- P0 changes
- P1 changes
- P2 cleanup
```

## Scoring Rubric

Use this rubric for each crate and for the whole four-crate boundary.

```text
10/10: Clear one-sentence responsibility, stable facade, no upward deps, no deep
       sibling imports, minimal public internals, coherent module tree.

8/10:  Direction is mostly right, but some facade gaps or public implementation
       types remain.

6/10:  Responsibilities are understandable, but callers rely on internals or
       sibling crates share unclear ownership.

4/10:  Mixed responsibilities, leaky APIs, or dependency direction problems.

2/10:  Crate boundary is mostly nominal; responsibilities and dependencies are
       tangled.
```

## Completion Criteria

The analysis is complete when the report can answer:

- What does each crate own?
- What should each crate not own?
- Which public APIs are intentional facades?
- Which public APIs are boundary leaks?
- Which dependencies violate or stress the intended layering?
- Which concrete moves or API changes would improve the boundary?

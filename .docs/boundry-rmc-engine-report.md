# rmc-engine Boundary Report

## Status

- Crate: `rmc-engine`
- Graph qualified name: `rmc_engine`
- Analysis order: 1 of 4
- Current phase: Phase 0 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | Pending commit | Graph snapshot reused; workspace and dependency baseline captured. |
| Phase 1: Public surface | Pending | Not started |  |
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


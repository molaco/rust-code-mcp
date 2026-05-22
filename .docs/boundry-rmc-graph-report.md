# rmc-graph Boundary Report

## Status

- Crate: `rmc-graph`
- Graph qualified name: `rmc_graph`
- Analysis order: 2 of 4
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


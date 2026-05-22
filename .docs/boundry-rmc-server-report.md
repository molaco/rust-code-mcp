# rmc-server Boundary Report

## Status

- Crate: `rmc-server`
- Graph qualified name: `rmc_server`
- Analysis order: 4 of 4
- Current phase: Phase 0 complete
- Report state: in progress

## Phase Log

| Phase | Status | Commit evidence | Notes |
| --- | --- | --- | --- |
| Phase 0: Snapshot readiness and baseline | Complete | Pending commit | Graph snapshot reused; workspace and server dependency baseline captured. |
| Phase 1: Public surface | Pending | Not started |  |
| Phase 2: Dependency boundary | Pending | Not started |  |
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

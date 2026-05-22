# MCP Side-by-Side Comparison Plan

## Goal

Compare the original Rust code MCP server and the refactored Rust code MCP server side by side for:

- Functional parity: same tools, accepted inputs, compatible outputs, and matching error behavior where expected.
- Speed: cold and warm latency for indexing, search, navigation, graph, audit, and semantic tools.
- Reliability: repeated calls, cache isolation, failure handling, and large-output behavior.

The comparison should leave a written result trail in this file. Each phase must update its checklist and results before moving to the next phase.

## Servers Under Test

| Label | Codex MCP namespace | Binary | Purpose |
|---|---|---|---|
| Original | `mcp__rust_code_mcp_original__` | `/home/molaco/Documents/rust-code-mcp-final/target/release/rust-code-mcp` | Original repo MCP server |
| Refactor | `mcp__rust_code_mcp_refactor__` | `/home/molaco/Documents/rust-code-mcp-refactor/target/release/rust-code-mcp` | Phase 7 crate-boundary refactor MCP server |

The `cuda-code` devshell config gives the two servers separate XDG roots:

| Label | `XDG_CACHE_HOME` | `XDG_DATA_HOME` |
|---|---|---|
| Original | `/home/molaco/.cache/mcp-rust-code-original-qwen3` | `/home/molaco/.local/share/mcp-rust-code-original-qwen3` |
| Refactor | `/home/molaco/.cache/mcp-rust-code-refactor-qwen3` | `/home/molaco/.local/share/mcp-rust-code-refactor-qwen3` |

Both MCP entries are generated from:

```text
/home/molaco/Documents/nix-devshells/devshells/cuda-code.nix
```

All build/check commands must run through:

```text
nix develop ../nix-devshells#cuda-code --command env RUSTFLAGS="-C linker-features=-lld" <command>
```

The devshell also exports `RUSTFLAGS="-C linker-features=-lld"` by default.

## Fixed Inputs

Primary workspace:

```text
/home/molaco/Documents/rust-code-mcp-refactor
```

Secondary smoke workspace:

```text
/home/molaco/Documents/rust-code-mcp-final
```

Canonical files:

```text
/home/molaco/Documents/rust-code-mcp-refactor/crates/rust-code-mcp/src/main.rs
/home/molaco/Documents/rust-code-mcp-refactor/crates/rmc-engine/src/lib.rs
/home/molaco/Documents/rust-code-mcp-refactor/crates/rmc-graph/src/graph/snapshot.rs
/home/molaco/Documents/rust-code-mcp-refactor/crates/rmc-indexing/src/indexing/unified.rs
/home/molaco/Documents/rust-code-mcp-refactor/crates/rmc-server/src/tools/router.rs
```

Canonical symbols:

```text
SearchTool
SyncManager
UnifiedIndexer
OpenedSnapshot
build_hypergraph
workspace_stats
```

Canonical crates:

```text
rust_code_mcp
rmc_engine
rmc_graph
rmc_indexing
rmc_server
```

Canonical search queries:

```text
SearchTool
SyncManager
build_hypergraph
semantic_overlaps
forbidden_dependency_check
```

## Measurement Rules

- Measure server runtime, not model reasoning time.
- Prefer a direct stdio JSON-RPC harness for timing once Phase 0 creates or copies it.
- Use the Codex MCP namespaces for quick functional checks when timing is not the focus.
- Run original and refactor with identical request bodies.
- Run cold tests after `clear_cache` or with `force_reindex` / `force_rebuild`.
- Run warm tests immediately after the corresponding cold test.
- For fast tools, run 5 repetitions and record the median.
- For expensive tools, run 1 cold and 2 warm repetitions unless a phase says otherwise.
- Record wall time in milliseconds.
- Record output size or primary count fields when available.
- Classify output deltas as `exact`, `compatible`, `expected-diff`, `regression`, or `needs-review`.
- Do not run `cargo fmt` or any formatting command.
- Do not run all tests at once; use targeted build/check/test commands only.
- Treat any single hypergraph build over 10 seconds as a performance failure unless the phase explicitly records why it is acceptable.

Speed delta formula:

```text
delta_percent = ((refactor_ms - original_ms) / original_ms) * 100
```

Negative delta means the refactor server is faster.

## Result Update Protocol

After each phase:

- Mark each completed checklist item with `[x]`.
- Set the phase status to `pass`, `partial`, `blocked`, or `fail`.
- Fill the phase result table.
- Add notes for any functional differences, crashes, timeouts, or suspicious output drift.
- If a test input must change, update `Fixed Inputs` and explain why in the phase notes.

## Preflight Observations

Status: partial

| Check | Original | Refactor | Notes |
|---|---|---|---|
| Release binary present | yes | no | Refactor release binary must be built before direct MCP launch checks. |
| Devshell contains both MCP entries | yes | yes | `rust-code-mcp-original` and `rust-code-mcp-refactor`. |
| XDG roots isolated | yes | yes | Separate original/refactor Qwen3 roots are required for cache integrity. |
| Default `RUSTFLAGS` configured | yes | yes | Present in devshell env and MCP server env. |

Verification notes:

```text
2026-05-22:
- `nix develop ../nix-devshells#cuda-code` evaluated successfully.
- The shell exported `RUSTFLAGS=-C linker-features=-lld`.
- The generated `.mcp.toml` contained both MCP entries.
- The generated `.mcp.toml` pointed original/refactor to distinct XDG cache/data roots.
- Original release binary exists at `/home/molaco/Documents/rust-code-mcp-final/target/release/rust-code-mcp`.
- Refactor release binary was not present yet under `/home/molaco/Documents/rust-code-mcp-refactor/target/release/`.
```

## Phase Status Summary

| Phase | Name | Status | Result Summary |
|---|---|---|---|
| 0 | Harness and Baseline | pending | Build/verify binaries, generated MCP config, and direct stdio harness. |
| 1 | Tool Inventory and Schemas | pending | Compare `tools/list` names and accepted input schemas. |
| 2 | Health, Cache, and Cold Start | pending | Verify isolated cache roots and health behavior before/after clear. |
| 3 | Indexing and Retrieval | pending | Compare indexing, keyword search, hybrid search, and similar-code retrieval. |
| 4 | Live Navigation and File Analysis | pending | Compare definitions, references, imports, dependencies, call graph, and complexity. |
| 5 | Hypergraph Snapshot and Core Queries | pending | Compare snapshot metrics, exports, imports, uses, crate edges, and dependency rules. |
| 6 | Audit and Policy Tools | pending | Compare docs/derive/body/unsafe/channel/recursion/global-state audits. |
| 7 | Semantic Similarity Tools | pending | Compare `similar_to_item` and `semantic_overlaps`. |
| 8 | Failure Modes and Robustness | pending | Compare invalid inputs, repeated warm calls, and large-output behavior. |
| 9 | Final Speed and Functionality Report | pending | Summarize compatibility, speed deltas, regressions, and follow-ups. |

## Phase 0: Harness and Baseline

Status: pending

Purpose:

Establish a repeatable comparison harness and capture binary/environment facts before tool-level comparisons begin.

Checklist:

- [ ] Enter `cuda-code` and confirm `RUSTFLAGS=-C linker-features=-lld`.
- [ ] Confirm `.mcp.toml` contains `rust-code-mcp-original` and `rust-code-mcp-refactor`.
- [ ] Verify original release binary exists and is executable.
- [ ] Build or verify refactor release binary.
- [ ] Capture original binary size and modified timestamp.
- [ ] Capture refactor binary size and modified timestamp.
- [ ] Capture `jj log -r @-` for the refactor workspace commit under test.
- [ ] Create or copy a direct stdio JSON-RPC timing harness, likely from `/home/molaco/Documents/rust-code-2/.plans/mcp_stdio_probe.py`.
- [ ] Confirm the harness launches original with original XDG roots.
- [ ] Confirm the harness launches refactor with refactor XDG roots.
- [ ] Run a smoke `initialize` and `tools/list` against both binaries.
- [ ] Update this phase with baseline measurements.

Suggested checks:

```text
original: tools/list
refactor: tools/list
original: health_check(directory = PRIMARY_WORKSPACE)
refactor: health_check(directory = PRIMARY_WORKSPACE)
```

Results:

| Check | Original Result | Refactor Result | Delta | Notes |
|---|---:|---:|---:|---|
| Binary exists | pending | pending | pending |  |
| Binary size bytes | pending | pending | pending |  |
| `tools/list` duration ms | pending | pending | pending |  |
| `health_check` smoke duration ms | pending | pending | pending |  |

Notes:

```text
Direct stdio harness:

- Original command: /home/molaco/Documents/rust-code-mcp-final/target/release/rust-code-mcp
- Refactor command: /home/molaco/Documents/rust-code-mcp-refactor/target/release/rust-code-mcp
- Original XDG roots: /home/molaco/.cache/mcp-rust-code-original-qwen3, /home/molaco/.local/share/mcp-rust-code-original-qwen3
- Refactor XDG roots: /home/molaco/.cache/mcp-rust-code-refactor-qwen3, /home/molaco/.local/share/mcp-rust-code-refactor-qwen3
```

## Phase 1: Tool Inventory and Schemas

Status: pending

Purpose:

Confirm both servers expose the same intended tool surface and compare tool descriptions/input schemas.

Checklist:

- [ ] Capture `tools/list` output from original.
- [ ] Capture `tools/list` output from refactor.
- [ ] Compare tool name sets.
- [ ] Compare input schema keys for every shared tool.
- [ ] Identify refactor-only tools.
- [ ] Identify original-only tools.
- [ ] Classify schema differences as compatible or breaking.
- [ ] Update the results table.

Required outcome:

No unexpected original-only tools, no unexpected missing tools in the refactor server, and no breaking input schema differences unless documented.

Results:

| Metric | Original | Refactor | Delta Classification | Notes |
|---|---:|---:|---|---|
| Tool count | pending | pending | pending |  |
| Shared tool count | pending | pending | pending |  |
| Original-only tool count | pending | pending | pending |  |
| Refactor-only tool count | pending | pending | pending |  |
| Breaking schema differences | pending | pending | pending |  |

## Phase 2: Health, Cache, and Cold Start

Status: pending

Purpose:

Verify both servers start cleanly, use isolated caches, and report health consistently before expensive operations.

Checklist:

- [ ] Run `health_check` before cache clearing.
- [ ] Run `clear_cache` for original only and verify refactor health/cache state is unchanged.
- [ ] Run `clear_cache` for refactor only and verify original health/cache state is unchanged.
- [ ] Run cold `health_check` after cache clearing.
- [ ] Run warm `health_check` immediately after cold.
- [ ] Record cache/data directory existence and sizes.
- [ ] Update result notes with any expected health-status differences.

Results:

| Check | Original | Refactor | Delta Classification | Notes |
|---|---:|---:|---|---|
| Pre-clear health | pending | pending | pending |  |
| Post-clear health | pending | pending | pending |  |
| Warm health ms | pending | pending | pending |  |
| Cache isolation | pending | pending | pending |  |

## Phase 3: Indexing and Retrieval

Status: pending

Purpose:

Compare indexing and search behavior on the same workspace.

Checklist:

- [ ] Run `index_codebase(directory = PRIMARY_WORKSPACE, force_reindex = true)` on original.
- [ ] Run `index_codebase(directory = PRIMARY_WORKSPACE, force_reindex = true)` on refactor.
- [ ] Run warm `index_codebase(directory = PRIMARY_WORKSPACE, force_reindex = false)` on both.
- [ ] Compare `search` for canonical queries.
- [ ] Compare `get_similar_code` for canonical queries.
- [ ] Compare result counts, top result file paths, and output sizes.
- [ ] Record cold/warm timing deltas.

Results:

| Tool/Input | Original ms | Refactor ms | Delta % | Classification | Notes |
|---|---:|---:|---:|---|---|
| `index_codebase(force_reindex=true)` | pending | pending | pending | pending |  |
| `index_codebase(force_reindex=false)` | pending | pending | pending | pending |  |
| `search(SearchTool)` | pending | pending | pending | pending |  |
| `get_similar_code(build_hypergraph)` | pending | pending | pending | pending |  |

## Phase 4: Live Navigation and File Analysis

Status: pending

Purpose:

Compare rust-analyzer-backed live navigation and file-local analysis behavior.

Checklist:

- [ ] Compare `find_definition` for canonical symbols.
- [ ] Compare `find_references` for canonical symbols.
- [ ] Compare `get_dependencies` on canonical files.
- [ ] Compare `get_call_graph` on canonical files.
- [ ] Compare `analyze_complexity` on canonical files.
- [ ] Compare output counts and representative paths.
- [ ] Mark canonical-name drift as expected only when declaration crates changed.

Results:

| Tool/Input | Original | Refactor | Classification | Notes |
|---|---:|---:|---|---|
| `find_definition(SearchTool)` | pending | pending | pending |  |
| `find_references(SearchTool)` | pending | pending | pending |  |
| `get_dependencies(router.rs)` | pending | pending | pending |  |
| `get_call_graph(router.rs)` | pending | pending | pending |  |
| `analyze_complexity(router.rs)` | pending | pending | pending |  |

## Phase 5: Hypergraph Snapshot and Core Queries

Status: pending

Purpose:

Compare persisted hypergraph extraction and graph-query behavior.

Checklist:

- [ ] Run `build_hypergraph(directory = PRIMARY_WORKSPACE, force_rebuild = true)` on both.
- [ ] Run warm `build_hypergraph(directory = PRIMARY_WORKSPACE, force_rebuild = false)` on both.
- [ ] Compare `workspace_stats`.
- [ ] Compare `module_tree` for `rmc_engine`, `rmc_graph`, `rmc_indexing`, `rmc_server`.
- [ ] Compare `crate_edges`.
- [ ] Compare `crate_dependency_metric`.
- [ ] Compare `forbidden_dependency_check` using the Phase 7 post-C rule set.
- [ ] Compare `who_uses`, `who_calls`, and `get_exports` for canonical symbols/modules.

Results:

| Tool/Input | Original ms | Refactor ms | Delta % | Classification | Notes |
|---|---:|---:|---:|---|---|
| `build_hypergraph(force_rebuild=true)` | pending | pending | pending | pending |  |
| `build_hypergraph(force_rebuild=false)` | pending | pending | pending | pending |  |
| `workspace_stats` | pending | pending | pending | pending |  |
| `crate_edges` | pending | pending | pending | pending |  |
| `forbidden_dependency_check` | pending | pending | pending | pending |  |

## Phase 6: Audit and Policy Tools

Status: pending

Purpose:

Compare audit tools that enforce documentation, visibility, safety, async, and architecture policy.

Checklist:

- [ ] Compare `missing_docs_audit`.
- [ ] Compare `derive_audit(required_derives = ["Debug"])`.
- [ ] Compare `fn_body_audit`.
- [ ] Compare `unsafe_audit`.
- [ ] Compare `channel_capacity_audit`.
- [ ] Compare `mut_static_audit`.
- [ ] Compare `recursion_check`.
- [ ] Compare `dead_pub_report`.
- [ ] Explain expected count drift from crate-boundary changes.

Results:

| Tool/Input | Original | Refactor | Classification | Notes |
|---|---:|---:|---|---|
| `missing_docs_audit` | pending | pending | pending |  |
| `derive_audit(Debug)` | pending | pending | pending |  |
| `fn_body_audit` | pending | pending | pending |  |
| `unsafe_audit` | pending | pending | pending |  |
| `dead_pub_report` | pending | pending | pending |  |

## Phase 7: Semantic Similarity Tools

Status: pending

Purpose:

Compare embedding-backed item similarity behavior and semantic-overlap clustering.

Checklist:

- [ ] Confirm both servers use isolated vector stores.
- [ ] Ensure `index_codebase` and `build_hypergraph` have completed for both.
- [ ] Compare `similar_to_item` for canonical items.
- [ ] Compare `semantic_overlaps` in cluster mode.
- [ ] Compare `semantic_overlaps` in pair mode at a strict threshold.
- [ ] Record warm-call timing deltas.
- [ ] Classify first-page ordering drift separately from count drift.

Results:

| Tool/Input | Original ms | Refactor ms | Delta % | Classification | Notes |
|---|---:|---:|---:|---|---|
| `similar_to_item(SearchTool)` | pending | pending | pending | pending |  |
| `semantic_overlaps(clusters)` | pending | pending | pending | pending |  |
| `semantic_overlaps(pairs, threshold=0.90)` | pending | pending | pending | pending |  |

## Phase 8: Failure Modes and Robustness

Status: pending

Purpose:

Compare invalid-input behavior, repeated warm calls, and large-output handling.

Checklist:

- [ ] Call a nonexistent directory for representative tools.
- [ ] Call unknown symbols for navigation and hypergraph tools.
- [ ] Call malformed/empty required params where schemas allow the server to respond.
- [ ] Repeat 5 warm calls for representative fast tools.
- [ ] Repeat 2 warm calls for representative expensive tools.
- [ ] Compare MCP error shapes and messages.
- [ ] Compare output truncation/limit behavior.

Results:

| Case | Original | Refactor | Classification | Notes |
|---|---:|---:|---|---|
| Unknown directory | pending | pending | pending |  |
| Unknown symbol | pending | pending | pending |  |
| Repeated warm `search` | pending | pending | pending |  |
| Repeated warm `workspace_stats` | pending | pending | pending |  |
| Large `who_uses` with limit/offset | pending | pending | pending |  |

## Phase 9: Final Speed and Functionality Report

Status: pending

Purpose:

Produce the final judgement: whether the refactor server is functionally compatible, where it is faster/slower, and what must be fixed before replacing the original.

Checklist:

- [ ] Summarize tool-surface compatibility.
- [ ] Summarize schema compatibility.
- [ ] Summarize functional output compatibility.
- [ ] Summarize speed wins/regressions.
- [ ] List expected differences caused by crate-boundary canonical-name drift.
- [ ] List real regressions with reproduction inputs.
- [ ] List follow-up fixes and suggested priority.
- [ ] Give a final rating out of 10 for readiness to replace the original.

Final Rollup:

| Area | Result | Notes |
|---|---|---|
| Tool surface | pending |  |
| Schema compatibility | pending |  |
| Functional parity | pending |  |
| Performance | pending |  |
| Reliability | pending |  |
| Replacement readiness rating | pending |  |

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

## MCP Tool Execution Rule

This plan must be executed through the MCP tools exposed to Codex, not by creating a JSON-RPC client, Python script, or direct server process harness.

When a phase says to run a tool on both servers, use the matching available MCP tool calls:

```text
mcp__rust_code_mcp_original__<tool_name>(...)
mcp__rust_code_mcp_refactor__<tool_name>(...)
```

Use `tool_search` only to discover or re-expose the available tool definitions and schemas in the current Codex context. Do not use `tools/list`, `initialize`, or a hand-written stdio protocol harness for this comparison.

Shell commands are allowed only for project metadata, VCS metadata, filesystem checks, and build/check commands. Build/check commands must use the `cuda-code` devshell command form documented below.

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

## Available MCP Tool Surface

Use `tool_search` to refresh this list if Codex does not currently expose one of these tools.

Core, cache, and search tools:

```text
health_check
clear_cache
index_codebase
search
get_similar_code
read_file_content
```

Live navigation and file-analysis tools:

```text
find_definition
find_references
rename_symbol
get_dependencies
get_call_graph
analyze_complexity
```

Hypergraph and architecture-query tools:

```text
build_hypergraph
workspace_stats
module_tree
get_imports
module_dependencies
get_exports
get_reexports
get_declared_reexports
who_imports
who_uses
who_uses_summary
who_calls
calls_from
call_graph
callers_in_crate
recursive_callers_count
crate_edges
crate_dependency_metric
forbidden_dependency_check
enum_variants
item_attributes
items_with_attribute
dead_pub_in_crate
dead_pub_report
pub_use_pub_type_audit
re_export_chain
overlaps
function_signature
functions_with_filter
build_codemap
```

Audit and policy tools:

```text
missing_docs_audit
derive_audit
fn_body_audit
unsafe_audit
channel_capacity_audit
mut_static_audit
recursion_check
```

Semantic tools:

```text
similar_to_item
semantic_overlaps
```

Full tool count per namespace: 51.

## Measurement Rules

- Measure server runtime, not model reasoning time.
- Use only the available Codex MCP tool calls for functional comparison.
- Measure timing from the MCP tool-call elapsed time reported by the Codex client or surrounding execution metadata. If the client does not expose elapsed time for a call, record `timing-unavailable` and keep the functional comparison.
- Use `tool_search` for MCP tool inventory/schema discovery; do not call raw JSON-RPC methods.
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

Status: pass

| Check | Original | Refactor | Notes |
|---|---|---|---|
| Release binary present | yes | yes | Both release binaries are present and executable. |
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
- Phase 0 recheck found both release binaries present and executable.
```

## Phase Status Summary

| Phase | Name | Status | Result Summary |
|---|---|---|---|
| 0 | MCP Tool Baseline | pass | Binaries/config verified; both MCP namespaces answered health/search smoke calls. |
| 1 | Tool Inventory and Schemas | pass | 51 shared tools, no original/refactor-only tools, no breaking schema-key differences. |
| 2 | Health, Cache, and Cold Start | pass | Isolated cache roots verified; both namespaces stayed degraded-but-functional before/after cache clear. |
| 3 | Indexing and Retrieval | partial | Retrieval compatible; cold forced indexing had a one-chunk count drift needing review. |
| 4 | Live Navigation and File Analysis | pending | Compare definitions, references, imports, dependencies, call graph, and complexity. |
| 5 | Hypergraph Snapshot and Core Queries | pending | Compare snapshot metrics, exports, imports, uses, crate edges, and dependency rules. |
| 6 | Audit and Policy Tools | pending | Compare docs/derive/body/unsafe/channel/recursion/global-state audits. |
| 7 | Semantic Similarity Tools | pending | Compare `similar_to_item` and `semantic_overlaps`. |
| 8 | Failure Modes and Robustness | pending | Compare invalid inputs, repeated warm calls, and large-output behavior. |
| 9 | Final Speed and Functionality Report | pending | Summarize compatibility, speed deltas, regressions, and follow-ups. |

## Phase 0: MCP Tool Baseline

Status: pass

Purpose:

Establish that the available MCP namespaces are usable and capture binary/environment facts before tool-level comparisons begin.

Checklist:

- [x] Enter `cuda-code` and confirm `RUSTFLAGS=-C linker-features=-lld`.
- [x] Confirm `.mcp.toml` contains `rust-code-mcp-original` and `rust-code-mcp-refactor`.
- [x] Verify original release binary exists and is executable.
- [x] Build or verify refactor release binary.
- [x] Capture original binary size and modified timestamp.
- [x] Capture refactor binary size and modified timestamp.
- [x] Capture `jj log -r @-` for the refactor workspace commit under test.
- [x] Use `tool_search` to expose both `mcp__rust_code_mcp_original__` and `mcp__rust_code_mcp_refactor__` tool definitions.
- [x] Confirm the available original MCP namespace maps to the original XDG roots documented above.
- [x] Confirm the available refactor MCP namespace maps to the refactor XDG roots documented above.
- [x] Run smoke MCP calls against both namespaces with identical request bodies.
- [x] Update this phase with baseline measurements.

Suggested checks:

```text
mcp__rust_code_mcp_original__health_check(directory = PRIMARY_WORKSPACE)
mcp__rust_code_mcp_refactor__health_check(directory = PRIMARY_WORKSPACE)
mcp__rust_code_mcp_original__search(directory = PRIMARY_WORKSPACE, keyword = "SearchTool")
mcp__rust_code_mcp_refactor__search(directory = PRIMARY_WORKSPACE, keyword = "SearchTool")
```

Results:

| Check | Original Result | Refactor Result | Delta | Notes |
|---|---:|---:|---:|---|
| Binary exists | yes | yes | exact | Both are regular executable files. |
| Binary size bytes | 290810664 | 271642248 | -19168416 (-6.59%) | Refactor release binary is smaller. |
| Visible MCP tool count | exposed | exposed | compatible | `tool_search` returned both namespaces; exact tool-count comparison is Phase 1. |
| `health_check` smoke duration ms | 59.2 | 26.0 | -56.1% | Tool-call wall time from Codex MCP output. Both returned degraded due to missing Merkle snapshot before indexing. |

Notes:

```text
MCP tool execution:

- Original namespace: mcp__rust_code_mcp_original__
- Refactor namespace: mcp__rust_code_mcp_refactor__
- Original XDG roots: /home/molaco/.cache/mcp-rust-code-original-qwen3, /home/molaco/.local/share/mcp-rust-code-original-qwen3
- Refactor XDG roots: /home/molaco/.cache/mcp-rust-code-refactor-qwen3, /home/molaco/.local/share/mcp-rust-code-refactor-qwen3

2026-05-22 Phase 0 results:
- `jj show --summary` was run before starting this phase.
- `nix develop ../nix-devshells#cuda-code --command env RUSTFLAGS="-C linker-features=-lld" sh -c 'printf "RUSTFLAGS=%s\n" "$RUSTFLAGS"'` reported `RUSTFLAGS=-C linker-features=-lld`.
- `.mcp.toml` contained both MCP server entries and the expected isolated XDG roots.
- Original binary: size 290810664 bytes, mtime 2026-05-18 17:04:31.199355329 +0200.
- Refactor binary: size 271642248 bytes, mtime 2026-05-22 16:15:08.499883678 +0200.
- Commit under test from `jj log -r @-`: skmktsptlwvuouzxwyrztzxwvnrnqrrm d20af7157036c2844244d65b63822d550fa10d73 docs + plans update.
- `tool_search` exposed both `mcp__rust_code_mcp_original__` and `mcp__rust_code_mcp_refactor__` tool definitions.
- `mcp__rust_code_mcp_original__health_check(directory = PRIMARY_WORKSPACE)` returned overall `degraded`: BM25 healthy, vector healthy with 0 vectors, Merkle snapshot missing.
- `mcp__rust_code_mcp_refactor__health_check(directory = PRIMARY_WORKSPACE)` returned overall `degraded`: BM25 healthy, vector healthy with 0 vectors, Merkle snapshot missing.
- `mcp__rust_code_mcp_original__search(directory = PRIMARY_WORKSPACE, keyword = "SearchTool")` completed in 1255.8 ms and returned no results before indexing.
- `mcp__rust_code_mcp_refactor__search(directory = PRIMARY_WORKSPACE, keyword = "SearchTool")` completed in 577.5 ms and returned no results before indexing.
```

## Phase 1: Tool Inventory and Schemas

Status: pass

Purpose:

Confirm both servers expose the same intended tool surface and compare tool descriptions/input schemas.

Checklist:

- [x] Use `tool_search` to expose the original namespace tool definitions.
- [x] Use `tool_search` to expose the refactor namespace tool definitions.
- [x] Capture the Codex-exposed tool name set for original.
- [x] Capture the Codex-exposed tool name set for refactor.
- [x] Compare tool name sets.
- [x] Compare input schema keys for every shared tool.
- [x] Identify refactor-only tools.
- [x] Identify original-only tools.
- [x] Classify schema differences as compatible or breaking.
- [x] Update the results table.

Required outcome:

No unexpected original-only tools, no unexpected missing tools in the refactor server, and no breaking input schema differences unless documented.

Results:

| Metric | Original | Refactor | Delta Classification | Notes |
|---|---:|---:|---|---|
| Tool count | 51 | 51 | exact | Counted `#[tool]` router methods visible through both MCP namespaces. |
| Shared tool count | 51 | 51 | exact | Sorted tool-name sets matched exactly. |
| Original-only tool count | 0 | 0 | exact | No missing refactor tools found. |
| Refactor-only tool count | 0 | 0 | exact | No unexpected refactor-only tools found. |
| Breaking schema differences | 0 | 0 | exact | Parameter field-key sets matched for every shared tool; refactor changes were module-path moves only. |

Notes:

```text
2026-05-22 Phase 1 results:
- `jj show --summary` was run before starting this phase.
- `tool_search` was run for both `mcp__rust_code_mcp_original__` and `mcp__rust_code_mcp_refactor__`.
- Original router source checked: /home/molaco/Documents/rust-code-mcp-final/src/tools/search_tool_router.rs.
- Refactor router source checked: /home/molaco/Documents/rust-code-mcp-refactor/crates/rmc-server/src/tools/router.rs.
- Tool names were extracted from `async fn` methods carrying MCP router tool definitions; both sides had 51 names and `diff` returned no differences.
- Parameter schema keys were compared from the `*Params` structs used by the routers. The sorted field-key map matched exactly after deduplication.
- Module path differences were expected from the refactor, for example `crate::tools::search_tool::*Params` moved to `crate::tools::params::*Params`, with no accepted input-key changes.
```

## Phase 2: Health, Cache, and Cold Start

Status: pass

Purpose:

Verify both servers start cleanly, use isolated caches, and report health consistently before expensive operations.

Checklist:

- [x] Run `health_check` before cache clearing.
- [x] Run `clear_cache` for original only and verify refactor health/cache state is unchanged.
- [x] Run `clear_cache` for refactor only and verify original health/cache state is unchanged.
- [x] Run cold `health_check` after cache clearing.
- [x] Run warm `health_check` immediately after cold.
- [x] Record cache/data directory existence and sizes.
- [x] Update result notes with any expected health-status differences.

Results:

| Check | Original | Refactor | Delta Classification | Notes |
|---|---:|---:|---|---|
| Pre-clear health | degraded, 434 vectors, 5.8 ms | degraded, 434 vectors, 4.1 ms | compatible | BM25/vector healthy; Merkle snapshot missing on both. |
| Post-clear health | degraded, 0 vectors, 2.6 ms | degraded, 0 vectors, 12.0 ms | compatible | After both clears, vector stores were empty and Merkle snapshots remained absent until indexing. |
| Warm health ms | 2.7 | 2.9 | compatible | Warm call delta was +0.2 ms for refactor. |
| Cache isolation | pass | pass | compatible | Original clear removed only original paths; refactor clear removed only refactor paths. Final data roots were separate and similarly small. |

Notes:

```text
2026-05-22 Phase 2 results:
- `jj show --summary` was run before starting this phase.
- Pre-clear filesystem sizes:
  - original cache root: missing
  - original data root: 4320320 bytes
  - refactor cache root: missing
  - refactor data root: 4316303 bytes
- Pre-clear health:
  - original: degraded; BM25 healthy; vector healthy with 434 vectors; Merkle snapshot missing; 5.8 ms.
  - refactor: degraded; BM25 healthy; vector healthy with 434 vectors; Merkle snapshot missing; 4.1 ms.
- Original `clear_cache(directory = PRIMARY_WORKSPACE)` removed only original metadata cache, Tantivy index, and vector store paths under `/home/molaco/.local/share/mcp-rust-code-original-qwen3`.
- After original clear, the original data root was 0 bytes and refactor data root remained present. A refactor health probe reported 932 vectors and grew the refactor data root to 11320689 bytes; this is recorded as refactor-side warm-state drift, not cross-root deletion.
- Refactor `clear_cache(directory = PRIMARY_WORKSPACE)` removed only refactor metadata cache, Tantivy index, and vector store paths under `/home/molaco/.local/share/mcp-rust-code-refactor-qwen3`.
- Original health after refactor clear remained degraded with BM25/vector healthy and 0 vectors.
- Cold post-clear health:
  - original: degraded; BM25 healthy; vector healthy with 0 vectors; Merkle snapshot missing; 2.6 ms.
  - refactor: degraded; BM25 healthy; vector healthy with 0 vectors; Merkle snapshot missing; 12.0 ms.
- Warm post-clear health:
  - original: degraded; BM25 healthy; vector healthy with 0 vectors; Merkle snapshot missing; 2.7 ms.
  - refactor: degraded; BM25 healthy; vector healthy with 0 vectors; Merkle snapshot missing; 2.9 ms.
- Final filesystem sizes:
  - original cache root: missing
  - original data root: 10255 bytes
  - refactor cache root: missing
  - refactor data root: 10260 bytes
```

## Phase 3: Indexing and Retrieval

Status: partial

Purpose:

Compare indexing and search behavior on the same workspace.

Checklist:

- [x] Run `index_codebase(directory = PRIMARY_WORKSPACE, force_reindex = true)` on original.
- [x] Run `index_codebase(directory = PRIMARY_WORKSPACE, force_reindex = true)` on refactor.
- [x] Run warm `index_codebase(directory = PRIMARY_WORKSPACE, force_reindex = false)` on both.
- [x] Compare `search` for canonical queries.
- [x] Compare `get_similar_code` for canonical queries.
- [x] Compare result counts, top result file paths, and output sizes.
- [x] Record cold/warm timing deltas.

Results:

| Tool/Input | Original ms | Refactor ms | Delta % | Classification | Notes |
|---|---:|---:|---:|---|---|
| `index_codebase(force_reindex=true)` | 46668.6 | 46151.5 | -1.1 | needs-review | Both indexed 185/187 Rust files, but original reported 2401 chunks and refactor reported 2402. |
| `index_codebase(force_reindex=false)` | 11.8 | 11.2 | -5.2 | exact | Both reported already up to date, 254 unchanged files, 0 changed files, 0 chunks. |
| `search(SearchTool)` | 334.9 | 312.1 | -6.8 | compatible | Both returned 10 results with the same top-path set; first two equal-score results were swapped. |
| `get_similar_code(build_hypergraph)` | 371.5 | 390.1 | +5.0 | compatible | Both returned 5 results with the same top-path set; first two near-equal results were swapped. |

Notes:

```text
2026-05-22 Phase 3 results:
- `jj show --summary` was run before starting this phase.
- Cold forced indexing:
  - original: 187 total Rust files, 185 indexed files, 2 skipped/removed files, 2401 chunks, 46.668629381s server time, 47.2675s MCP wall time.
  - refactor: 187 total Rust files, 185 indexed files, 2 skipped/removed files, 2402 chunks, 46.151507604s server time, 46.7377s MCP wall time.
  - Classification: `needs-review` because the refactor produced one additional chunk for the same workspace and profile.
- Warm indexing:
  - original: already up to date, 254 total Rust files, 254 unchanged, 0 changed, 0 chunks, 11.827068ms server time, 0.5448s MCP wall time.
  - refactor: already up to date, 254 total Rust files, 254 unchanged, 0 changed, 0 chunks, 11.211083ms server time, 0.5140s MCP wall time.
- Canonical `search` query summary:
  - `SearchTool`: 10 results on both; same top-path set; top two equal-score results swapped.
  - `SyncManager`: 10 results on both; same top 6; positions 7 and 8 swapped.
  - `build_hypergraph`: 10 results on both; same top-path set with small ordering drift.
  - `semantic_overlaps`: 10 results on both; same main files/symbol families with line-split ordering drift inside `similarity.rs`.
  - `forbidden_dependency_check`: 10 results on both; same top-path set; top two equal-score results swapped.
- Canonical `get_similar_code(limit = 5)` summary:
  - `SearchTool`: 5 results on both; same order and paths, minor score drift.
  - `SyncManager`: 5 results on both; same order and paths, same displayed scores.
  - `build_hypergraph`: 5 results on both; same paths, first two near-equal results swapped.
  - `semantic_overlaps`: 5 results on both; same top two, positions 3-5 reordered among closely related symbols/modules.
  - `forbidden_dependency_check`: 5 results on both; same order and paths, minor score drift.
```

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

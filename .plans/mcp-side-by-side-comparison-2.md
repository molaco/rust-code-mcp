# MCP Side-by-Side Comparison Plan Rerun

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

Phase 7 bounded semantic-overlap scope:

```text
crate_name = rmc_server
```

Use this scope when unscoped workspace-wide `semantic_overlaps` exceeds the MCP client timeout; record the unscoped timeout in the Phase 7 notes.

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
| Devshell contains both MCP entries | yes | yes | `.mcp.toml` and `cuda-code.nix` contain both `rust-code-mcp-original` and `rust-code-mcp-refactor`. |
| XDG roots isolated | yes | yes | Original and refactor MCP entries use separate cache/data roots. |
| Default `RUSTFLAGS` configured | yes | yes | Devshell exports `RUSTFLAGS=-C linker-features=-lld`. |

Verification notes:

```text
2026-05-25:
- `nix develop ../nix-devshells#cuda-code --command env RUSTFLAGS="-C linker-features=-lld" ...` evaluated successfully and reported `RUSTFLAGS=-C linker-features=-lld`.
- The nix devshell command emitted `warning: Git tree '/home/molaco/Documents/nix-devshells' is dirty`; this is external to the refactor workspace and did not block preflight.
- `.mcp.toml` and `/home/molaco/Documents/nix-devshells/devshells/cuda-code.nix` contain both MCP entries.
- Original XDG roots: `/home/molaco/.cache/mcp-rust-code-original-qwen3`, `/home/molaco/.local/share/mcp-rust-code-original-qwen3`.
- Refactor XDG roots: `/home/molaco/.cache/mcp-rust-code-refactor-qwen3`, `/home/molaco/.local/share/mcp-rust-code-refactor-qwen3`.
- Original release binary: size 290810664 bytes, mtime 2026-05-18 17:04:31.199355329 +0200.
- Refactor release binary: size 271857272 bytes, mtime 2026-05-25 08:38:48.955950502 +0200.
```

## Phase Status Summary

| Phase | Name | Status | Result Summary |
|---|---|---|---|
| 0 | MCP Tool Baseline | pass | Binaries/config verified; both MCP namespaces answered health/search smoke calls. |
| 1 | Tool Inventory and Schemas | pass | 51 shared tools, no original/refactor-only tools, and 0 parameter field-key differences across 52 shared parameter structs. |
| 2 | Health, Cache, and Cold Start | partial | Cache isolation passed and both health checks stayed healthy, but the original vector store began repopulating after clear, so the post-clear cold state was not stable. |
| 3 | Indexing and Retrieval | pass | Forced and warm indexing matched on file/chunk counts; search and similar-code probes were compatible with only small score/order drift in near-ties. |
| 4 | Live Navigation and File Analysis | pass | Definition, reference, dependency, call-graph, and complexity counts matched for all canonical symbols/files. |
| 5 | Hypergraph Snapshot and Core Queries | partial | Functional output exact; forced and warm hypergraph builds exceeded the 10s threshold on both servers. |
| 6 | Audit and Policy Tools | pass | Audit counts and first-page findings matched across documentation, derive, function-body, unsafe, channel, mutable-static, recursion, and dead-public reports. |
| 7 | Semantic Similarity Tools | pass | Semantic item similarity and bounded `rmc_server` overlap scans matched exactly on counts and first-page ordering; first overlap scan was cache-heavy on both servers, warm repeats were millisecond-scale. |
| 8 | Failure Modes and Robustness | partial | Error shapes, empty-result behavior, pagination, and repeated warm graph calls matched; repeated warm `search(SearchTool)` returned compatible results but the refactor median was slower in this run. |
| 9 | Final Speed and Functionality Report | pass | Final report completed: tool/schema/functionality parity is strong; replacement readiness is 8.3/10 with performance and cache-state caveats. |

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
| Binary size bytes | 290810664 | 271857272 | -18953392 (-6.52%) | Refactor release binary is smaller. |
| Visible MCP tool count | exposed | exposed | compatible | `tool_search` exposed both namespaces; exact tool-count comparison is Phase 1. |
| `health_check` smoke duration ms | 46.6 | 44.6 | -4.3% | Both returned healthy; original reported 2401 vectors, refactor reported 2401 vectors. |
| `search(SearchTool)` smoke duration ms | 1327.5 | 594.2 | -55.2% | Both returned 10 results with the same top result and same visible result order. |

Notes:

```text
MCP tool execution:

- Original namespace: mcp__rust_code_mcp_original__
- Refactor namespace: mcp__rust_code_mcp_refactor__
- Original XDG roots: /home/molaco/.cache/mcp-rust-code-original-qwen3, /home/molaco/.local/share/mcp-rust-code-original-qwen3
- Refactor XDG roots: /home/molaco/.cache/mcp-rust-code-refactor-qwen3, /home/molaco/.local/share/mcp-rust-code-refactor-qwen3

Rerun results:
- `jj log -r @-` commit under test: `ympowonkkvoyxultltoolnrkvvrxlnxs 094245e91bd07e8740ae563fb91bc17602640280 docs + plans update`.
- `tool_search` exposed `mcp__rust_code_mcp_original__health_check`, `mcp__rust_code_mcp_refactor__health_check`, `mcp__rust_code_mcp_original__search`, and `mcp__rust_code_mcp_refactor__search`.
- `mcp__rust_code_mcp_original__health_check(directory = PRIMARY_WORKSPACE)` returned overall `healthy`: BM25 healthy, vector healthy with 2401 vectors, Merkle snapshot present.
- `mcp__rust_code_mcp_refactor__health_check(directory = PRIMARY_WORKSPACE)` returned overall `healthy`: BM25 healthy, vector healthy with 2401 vectors, Merkle snapshot present.
- `mcp__rust_code_mcp_original__search(directory = PRIMARY_WORKSPACE, keyword = "SearchTool")` completed in 1327.5 ms and returned 10 results.
- `mcp__rust_code_mcp_refactor__search(directory = PRIMARY_WORKSPACE, keyword = "SearchTool")` completed in 594.2 ms and returned 10 results.
- Phase 0 began from a warm indexed state; Phase 2 will exercise cache clearing and cold health separately.
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
| Tool count | 51 | 51 | exact | Counted `async fn` methods in the `#[tool_router]` impls and cross-checked with `tool_search` exposure. |
| Shared tool count | 51 | 51 | exact | Sorted tool-name sets matched exactly. |
| Original-only tool count | 0 | 0 | exact | No missing refactor tools found. |
| Refactor-only tool count | 0 | 0 | exact | No unexpected refactor-only tools found. |
| Breaking schema differences | 0 | 0 | exact | 52 shared `*Params` structs had 0 field-key differences after expanding flattened pagination to `limit`, `offset`, and `summary`. |

Notes:

```text
2026-05-25 Phase 1 results:
- `tool_search` was run for both original and refactor MCP namespaces and exposed the active tool definitions needed for the comparison.
- Router source checked for original: `/home/molaco/Documents/rust-code-mcp-final/src/tools/search_tool_router.rs`.
- Router source checked for refactor: `/home/molaco/Documents/rust-code-mcp-refactor/crates/rmc-server/src/tools/router.rs`.
- Tool names were extracted from `async fn` methods in the `#[tool_router]` impls; both sides had 51 names.
- `comm` over the sorted tool-name sets returned no original-only or refactor-only tool names.
- Parameter structs were compared from original `search_tool.rs`, `health_tool.rs`, `clear_cache_tool.rs`, and `index_tool.rs` against refactor `params/*.rs` plus endpoint parameter structs.
- The comparison found 52 original parameter structs, 52 refactor parameter structs, 52 shared names, and 0 field-key differences.
- Flattened `ListPaginationParams` fields were expanded to `limit`, `offset`, and `summary` before comparison.
```

## Phase 2: Health, Cache, and Cold Start

Status: partial

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
| Pre-clear health | healthy, 2401 vectors, 3.4 ms | healthy, 2401 vectors, 3.6 ms | compatible | BM25, vector, and Merkle were healthy on both before clearing. |
| Post-clear health | healthy, 286 vectors, 2.5 ms | healthy, 0 vectors, 18.3 ms | needs-review | Original returned 0 vectors immediately after refactor clear, then 286 vectors on the next cold probe. Refactor stayed at 0 vectors. |
| Warm health ms | healthy, 395 vectors, 3.2 ms | healthy, 0 vectors, 3.4 ms | needs-review | Original vector count continued increasing after clear; likely background repopulation from the already-running server. |
| Cache isolation | pass | pass | compatible | Original clear affected original data root only; refactor clear affected refactor data root only. |

Notes:

```text
2026-05-25 Phase 2 results:
- Pre-clear filesystem sizes:
  - original cache root: missing
  - original data root: 59149328 bytes
  - refactor cache root: missing
  - refactor data root: 259600285 bytes
- Pre-clear health:
  - original: healthy; BM25 healthy; vector healthy with 2401 vectors; Merkle snapshot present; 3.4 ms MCP wall time.
  - refactor: healthy; BM25 healthy; vector healthy with 2401 vectors; Merkle snapshot present; 3.6 ms MCP wall time.
- Original `clear_cache(directory = PRIMARY_WORKSPACE)` removed only original metadata cache, Tantivy index, and vector store paths under `/home/molaco/.local/share/mcp-rust-code-original-qwen3/search/...`.
- After original clear:
  - original data root size dropped from 59149328 bytes to 25131936 bytes.
  - refactor data root remained 259600285 bytes.
  - refactor health remained healthy with 2401 vectors, confirming the original clear did not remove refactor cache state.
- Refactor `clear_cache(directory = PRIMARY_WORKSPACE)` removed only refactor metadata cache, Tantivy index, and vector store paths under `/home/molaco/.local/share/mcp-rust-code-refactor-qwen3/search/...`.
- After refactor clear:
  - original health remained healthy and initially reported 0 vectors.
  - refactor data root dropped from 259600285 bytes to 225573083 bytes.
- Cold post-clear health:
  - original: healthy; BM25 healthy; vector healthy with 286 vectors; Merkle snapshot present; 2.5 ms MCP wall time.
  - refactor: healthy; BM25 healthy; vector healthy with 0 vectors; Merkle snapshot present; 18.3 ms MCP wall time.
- Warm post-clear health:
  - original: healthy; BM25 healthy; vector healthy with 395 vectors; Merkle snapshot present; 3.2 ms MCP wall time.
  - refactor: healthy; BM25 healthy; vector healthy with 0 vectors; Merkle snapshot present; 3.4 ms MCP wall time.
- Final filesystem sizes:
  - original cache root: missing
  - original data root: 30372416 bytes
  - refactor cache root: missing
  - refactor data root: 225583338 bytes
- Phase classification is `partial` because cache isolation passed, but the original post-clear vector store did not remain at 0 vectors long enough to be a clean cold-state baseline. Phase 3 uses forced indexing on both servers, so this does not block the sequential rerun.
```

## Phase 3: Indexing and Retrieval

Status: pass

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
| `index_codebase(force_reindex=true)` | 44478.0 | 45111.4 | +1.4 | exact | Both indexed 189/190 Rust files, skipped 1 file, and produced 2513 chunks. |
| `index_codebase(force_reindex=false)` | 553.7 | 518.5 | -6.4 | exact | Both reported already up to date, 312 unchanged files, 0 changed files, 0 chunks. |
| `search(SearchTool)` | 333.8 | 321.2 | -3.8 | exact | Both returned 10 results with the same order, paths, symbols, and displayed scores. |
| `get_similar_code(build_hypergraph)` | 312.4 | 312.9 | +0.2 | compatible | Both returned 5 results with the same order and paths; a few displayed scores differed slightly. |

Notes:

```text
2026-05-25 Phase 3 results:
- Cold forced indexing:
  - original: 190 total Rust files, 189 indexed files, 1 skipped/removed file, 2513 chunks, 43.882072025s server time, 44.4780s MCP wall time.
  - refactor: 190 total Rust files, 189 indexed files, 1 skipped/removed file, 2513 chunks, 44.443669381s server time, 45.1114s MCP wall time.
  - Classification: `exact` for file and chunk counts; refactor was +1.4% slower by MCP wall time.
- Warm indexing:
  - original: already up to date, 312 total Rust files, 312 unchanged, 0 changed, 0 chunks, 25.176448ms server time, 553.7 ms MCP wall time.
  - refactor: already up to date, 312 total Rust files, 312 unchanged, 0 changed, 0 chunks, 24.918898ms server time, 518.5 ms MCP wall time.
- Canonical `search` query summary:
  - `SearchTool`: 10 results on both; same order, paths, symbols, and displayed scores.
  - `SyncManager`: 10 results on both; same order and paths; one displayed score differed by 0.0001.
  - `build_hypergraph`: 10 results on both; same top-path set and same first four results; minor score drift in lower results.
  - `semantic_overlaps`: 10 results on both; same top five; positions 6 and 7 swapped between `semantic_overlap_threshold` and `semantic_overlap_scoring_helper_matches_expected_output`.
  - `forbidden_dependency_check`: 10 results on both; same top-path set; top two near-equal results swapped.
- Canonical `get_similar_code(limit = 5)` summary:
  - `SearchTool`: 5 results on both; same order and paths; score drift only in the fourth decimal on one result.
  - `SyncManager`: 5 results on both; same order and paths; minor score drift on one result.
  - `build_hypergraph`: 5 results on both; same order and paths; small score drift.
  - `semantic_overlaps`: 5 results on both; same order and paths; small score drift.
  - `forbidden_dependency_check`: 5 results on both; same top set; top two near-equal results swapped.
```

## Phase 4: Live Navigation and File Analysis

Status: pass

Purpose:

Compare rust-analyzer-backed live navigation and file-local analysis behavior.

Checklist:

- [x] Compare `find_definition` for canonical symbols.
- [x] Compare `find_references` for canonical symbols.
- [x] Compare `get_dependencies` on canonical files.
- [x] Compare `get_call_graph` on canonical files.
- [x] Compare `analyze_complexity` on canonical files.
- [x] Compare output counts and representative paths.
- [x] Mark canonical-name drift as expected only when declaration crates changed.

Results:

| Tool/Input | Original | Refactor | Classification | Notes |
|---|---:|---:|---|---|
| `find_definition(SearchTool)` | 1 | 1 | exact | Same declaration path: `crates/rmc-server/src/tools/mod.rs:10:37`. |
| `find_references(SearchTool)` | 14 | 14 | exact | Same count and representative paths. |
| `get_dependencies(router.rs)` | 21 | 21 | exact | Same import list. |
| `get_call_graph(router.rs)` | 70 funcs / 72 rels | 70 funcs / 72 rels | compatible | Same function/callee sets; list ordering differed. |
| `analyze_complexity(router.rs)` | 721 lines / 56 funcs / 163 cyclo / 72 calls | 721 lines / 56 funcs / 163 cyclo / 72 calls | exact | Same metrics. |

Verification notes:

- Definition counts matched for all canonical symbols with `exact=true`: `SearchTool` 1, `SyncManager` 1, `UnifiedIndexer` 2, `OpenedSnapshot` 2, `build_hypergraph` 2, and `workspace_stats` 3.
- Reference counts matched for all canonical symbols with `exact=true`: `SearchTool` 14, `SyncManager` 19, `UnifiedIndexer` 15, `OpenedSnapshot` 72, `build_hypergraph` 3, and `workspace_stats` 4.
- Dependency counts matched for canonical files: `main.rs` 7, `rmc-engine/src/lib.rs` 0, `snapshot.rs` 38, `unified.rs` 19, and `router.rs` 21.
- Call graph counts matched for canonical files: `main.rs` 19 funcs / 18 rels, `rmc-engine/src/lib.rs` 0 / 0, `snapshot.rs` 86 / 152, `unified.rs` 76 / 111, and `router.rs` 70 / 72.
- Complexity metrics matched for canonical files: `main.rs` 55 lines / 1 func / 4 cyclo / 18 calls; `rmc-engine/src/lib.rs` 8 / 0 / 1 / 0; `snapshot.rs` 824 / 23 / 142 / 152; `unified.rs` 667 / 22 / 67 / 111; `router.rs` 721 / 56 / 163 / 72.

## Phase 5: Hypergraph Snapshot and Core Queries

Status: partial

Purpose:

Compare persisted hypergraph extraction and graph-query behavior.

Checklist:

- [x] Run `build_hypergraph(directory = PRIMARY_WORKSPACE, force_rebuild = true)` on both.
- [x] Run warm `build_hypergraph(directory = PRIMARY_WORKSPACE, force_rebuild = false)` on both.
- [x] Compare `workspace_stats`.
- [x] Compare `module_tree` for `rmc_engine`, `rmc_graph`, `rmc_indexing`, `rmc_server`.
- [x] Compare `crate_edges`.
- [x] Compare `crate_dependency_metric`.
- [x] Compare `forbidden_dependency_check` using the Phase 7 post-C rule set.
- [x] Compare `who_uses`, `who_calls`, and `get_exports` for canonical symbols/modules.

Results:

| Tool/Input | Original ms | Refactor ms | Delta % | Classification | Notes |
|---|---:|---:|---:|---|---|
| `build_hypergraph(force_rebuild=true)` | 21228.1 | 21114.8 | -0.5 | needs-review | Output exact; both exceed the 10s threshold. |
| `build_hypergraph(force_rebuild=false)` | 14612.0 | 14794.4 | +1.2 | needs-review | Median of two warm reuse measurements; output exact with `reused=true`; both exceed the 10s threshold. |
| `workspace_stats` | 16.8 | 27.9 | +66.1 | exact | Same node, item, binding, visibility, and `pub_crate_share` counts. |
| `crate_edges` | 16.4 | 43.1 | +162.8 | exact | `limit=0` count probe reported 48 total edges on both. |
| `forbidden_dependency_check` | 7.4 | 11.0 | +48.6 | exact | Five-rule boundary set returned 0 violations on both. |

Verification notes:

- Cold `build_hypergraph(force_rebuild=true)` returned the same graph id `b0810e8277b124a995405b624070885d`, fingerprint `a47f641b3ccda7e07935407e59975056c7e47c8caf2de6d1de1ddb9b8aaac6b7`, 3173 nodes, 5741 bindings, and 8328 usages on both servers.
- Snapshot paths stayed isolated under original/refactor XDG data roots.
- Warm `build_hypergraph(force_rebuild=false)` returned the same graph id/fingerprint/counts with `reused=true` on both servers. Warm measurements used for the table were original 14589.6 ms and 14634.3 ms, refactor 14179.9 ms and 15408.9 ms.
- Additional original warm reuse probes were run while refreshing the graph-query tool surface; they all returned the same reused graph and remained in the 14.2-15.4s range.
- `workspace_stats` matched exactly: 45 crates, 303 modules, 2573 items, 251 external symbols, 1935 declared bindings, 1688 glob imports, and 2118 named imports.
- `module_tree(depth=2)` matched exactly for `rmc_engine`, `rmc_graph`, `rmc_indexing`, and `rmc_server`. `rmc_engine` exposes top-level `chunker`, `embeddings`, `parser`, `schema`, `search`, and `vector_store`; `rmc_graph` exposes `graph`; `rmc_indexing` exposes `indexing`, `metadata_cache`, `metrics`, `monitoring`, and `security`; `rmc_server` exposes `mcp`, `semantic`, and `tools`.
- `crate_edges(summary=true, limit=0)` matched exactly with `total_match_count=48`.
- `crate_dependency_metric(summary=true, limit=100)` matched exactly with 45 crate metrics. Key local crate metrics: `rmc_engine` Ce 1 / Ca 14 / I 0.0667; `rmc_graph` 1 / 11 / 0.0833; `rmc_config` 1 / 2 / 0.3333; `rmc_indexing` 2 / 14 / 0.125; `rmc_server` 3 / 6 / 0.3333.
- `forbidden_dependency_check` used the current five-rule crate-layering set from `.plans/boundries-plan.md`: `rmc_engine -> rmc_*`, `rmc_graph -> rmc_server`, `rmc_graph -> rmc_indexing`, `rmc_indexing -> rmc_server`, and `rmc_indexing -> rmc_graph`. Both servers returned `rule_count=5` and `violation_count=0`.
- `who_uses(target=rmc_server::tools::SearchTool, summary=true, limit=20)` resolved through the re-export to `rmc_server::tools::router::SearchToolRouter` and returned 5 usages on both.
- `who_calls(target=rmc_server::tools::graph::core::build_hypergraph, summary=true, limit=20)` returned 3 call sites on both: `SearchToolRouter::build_hypergraph`, `ensure_default_snapshot`, and `mcp_round_trip_against_self`.
- `get_exports(module=rmc_server::tools, consumer=rust-code-mcp, summary=true, limit=20)` returned 4 exports on both: `IndexCodebaseParams`, `index_codebase`, `SearchTool`, and `SearchToolRouter`.

## Phase 6: Audit and Policy Tools

Status: pass

Purpose:

Compare audit tools that enforce documentation, visibility, safety, async, and architecture policy.

Checklist:

- [x] Compare `missing_docs_audit`.
- [x] Compare `derive_audit(required_derives = ["Debug"])`.
- [x] Compare `fn_body_audit`.
- [x] Compare `unsafe_audit`.
- [x] Compare `channel_capacity_audit`.
- [x] Compare `mut_static_audit`.
- [x] Compare `recursion_check`.
- [x] Compare `dead_pub_report`.
- [x] Explain expected count drift from crate-boundary changes.

Results:

| Tool/Input | Original | Refactor | Classification | Notes |
|---|---:|---:|---|---|
| `missing_docs_audit` | 135 findings, 5.8 ms | 135 findings, 8.5 ms | pass | Exact count and same first five findings. |
| `derive_audit(Debug)` | 49 findings, 4.5 ms | 49 findings, 3.7 ms | pass | Exact count and same first five findings. |
| `fn_body_audit` | 406 findings, 23.7200 s | 406 findings, 21.3611 s | pass | Exact count and same first five findings; both are RA-backed expensive calls. |
| `unsafe_audit` | 8 findings, 14.6247 s | 8 findings, 14.4526 s | pass | Exact count and same first five findings; both are RA-backed expensive calls. |
| `dead_pub_report` | 170 findings, 30.9 ms | 170 findings, 30.7-34.4 ms | pass | Exact count and same first-page crate/finding groups. |

Verification notes:

- `channel_capacity_audit(summary=true, limit=5)` returned 1 finding on both servers: unbounded `std_mpsc` in `crates/rust-code-mcp/tests/test_mcp_stdio_transport.rs`, enclosing `test_mcp_stdio_transport::test_index_codebase_force_reindex_stdout_is_json_only`.
- `mut_static_audit(summary=true, limit=5)` returned 5 findings on both servers: two `fastembed` `OnceLock` statics, `rmc_engine::embeddings::profile::BUILT_IN_PROFILES`, `rmc_server::semantic::SEMANTIC`, and `rmc_server::tools::graph::tests::DEFAULT_SNAPSHOT_BUILT`.
- `recursion_check(max_cycle_length=5, summary=true, limit=5)` returned 11 cycles on both servers, with the same first five direct-recursion findings.
- No count drift from crate-boundary refactoring was observed in Phase 6. The audit tools resolve against the same workspace and produced compatible policy evidence.

## Phase 7: Semantic Similarity Tools

Status: pass

Purpose:

Compare embedding-backed item similarity behavior and semantic-overlap clustering.

Checklist:

- [x] Confirm both servers use isolated vector stores.
- [x] Ensure `index_codebase` and `build_hypergraph` have completed for both.
- [x] Compare `similar_to_item` for canonical items.
- [x] Compare `semantic_overlaps` in cluster mode.
- [x] Compare `semantic_overlaps` in pair mode at a strict threshold.
- [x] Record warm-call timing deltas.
- [x] Classify first-page ordering drift separately from count drift.

Results:

| Tool/Input | Original ms | Refactor ms | Delta % | Classification | Notes |
|---|---:|---:|---:|---|---|
| `similar_to_item(SearchTool)` | 501.3 | 468.0 | -6.6% | pass | Exact seed, 5 matches, scores, and ordering. |
| `semantic_overlaps(clusters)` | 19849.8 first / 19.2 warm | 19591.6 first / 23.2 warm | -1.3% first / +20.8% warm | pass | Bounded to `crate_name=rmc_server`; exact 278 seeds, 126 pairs, 45 clusters, and first-page clusters. |
| `semantic_overlaps(pairs, threshold=0.90)` | 18.6 | 17.9 | -3.8% | pass | Bounded to `crate_name=rmc_server`; exact 40 pairs, 27 clusters, and first-page pair ordering. |

Verification notes:

- Isolated vector stores were established by the Phase 0 XDG-root check and preserved by Phase 2 cache-isolation checks.
- `index_codebase` completed in Phase 3 and `build_hypergraph` completed in Phase 5 for both servers before semantic-item probes.
- `similar_to_item(target="rmc_server::tools::SearchTool")` resolved to `rmc_server::tools::router::SearchToolRouter` on both servers and returned the same top five matches.
- No first-page ordering drift or count drift was observed in Phase 7.

## Phase 8: Failure Modes and Robustness

Status: partial

Purpose:

Compare invalid-input behavior, repeated warm calls, and large-output handling.

Checklist:

- [x] Call a nonexistent directory for representative tools.
- [x] Call unknown symbols for navigation and hypergraph tools.
- [x] Call malformed/empty required params where schemas allow the server to respond.
- [x] Repeat 5 warm calls for representative fast tools.
- [x] Repeat 2 warm calls for representative expensive tools.
- [x] Compare MCP error shapes and messages.
- [x] Compare output truncation/limit behavior.

Results:

| Case | Original | Refactor | Classification | Notes |
|---|---:|---:|---|---|
| Unknown directory | `-32602`, not a directory | `-32602`, not a directory | pass | `search` against `.missing-mcp-comparison` produced matching MCP invalid-params errors. |
| Unknown symbol | no definition; codemap unresolved seed | no definition; codemap unresolved seed | pass | `find_definition(exact=true)` returned the same empty message; `build_codemap` returned 0 nodes and matching diagnostics. |
| Malformed required params | `-32602`, invalid `sort_by` | `-32602`, invalid `sort_by` | pass | `crate_dependency_metric(sort_by="not_a_metric")` returned matching allowed-value errors. |
| Repeated warm `search` | 10 results; median 366.8 ms | 10 results; median 558.8 ms | needs-review | Same 10-result set; top two equal-score hits flip order on both servers. Refactor median was +52.3% in this run. |
| Repeated warm `workspace_stats` | exact output; median 3.3 ms | exact output; median 3.0 ms | pass | Five calls each; same workspace counters every time. |
| Repeated warm `semantic_overlaps` | exact output; 20.8 / 18.8 ms | exact output; 19.4 / 23.4 ms | pass | Two warm bounded pair-mode calls each; exact 278 seeds, 40 pairs, 27 clusters, and same first five pairs. |
| Large `who_uses` with limit/offset | total 5, returned 3 | total 5, returned 3 | pass | `target=rmc_server::tools::SearchTool`, `summary=true`, `limit=3`, `offset=2`; exact page and resolved target. |

Verification notes:

- Missing-directory and malformed-parameter failures used matching MCP `-32602` error shapes and message text.
- Unknown symbol handling is intentionally non-fatal for navigation/codemap: navigation returns no definition, while codemap returns an empty graph plus diagnostics.
- The only Phase 8 needs-review item is warm `search` latency. The functional result set remained compatible; the observed ordering flip is a shared equal-score tie behavior, not a refactor-only regression.

## Phase 9: Final Speed and Functionality Report

Status: pass

Purpose:

Produce the final judgement: whether the refactor server is functionally compatible, where it is faster/slower, and what must be fixed before replacing the original.

Checklist:

- [x] Summarize tool-surface compatibility.
- [x] Summarize schema compatibility.
- [x] Summarize functional output compatibility.
- [x] Summarize speed wins/regressions.
- [x] List expected differences caused by crate-boundary canonical-name drift.
- [x] List real regressions with reproduction inputs.
- [x] List follow-up fixes and suggested priority.
- [x] Give a final rating out of 10 for readiness to replace the original.

Final Rollup:

| Area | Result | Notes |
|---|---|---|
| Tool surface | pass | 51 shared tools, no original-only or refactor-only tools. |
| Schema compatibility | pass | 52 shared parameter structs and 0 field-key differences after flattening pagination fields. |
| Functional parity | pass | Indexing counts, navigation counts, hypergraph counts, audit counts, semantic similarity counts, pagination, and failure responses matched. |
| Performance | partial | Refactor was smaller and comparable on most paths, but warm `search(SearchTool)` was slower in Phase 8 and hypergraph reuse remained expensive on both servers. |
| Reliability | partial | Error handling and repeated calls were compatible; Phase 2 exposed unstable post-clear original vector-store repopulation, which makes cold-cache comparison noisy. |
| Replacement readiness rating | 8.3/10 | Functionally ready, but investigate warm search variance and shared hypergraph latency before treating the refactor as a strict performance replacement. |

Final judgement:

The refactored MCP server is functionally compatible with the original across the tested tool surface. Tool inventory, parameter schemas, indexing counts, navigation outputs, graph snapshots, audit results, semantic overlap outputs, error shapes, and pagination behavior all matched or stayed within documented equal-score ordering drift.

The performance result is mixed rather than uniformly better. The refactor binary is 6.52% smaller, forced indexing was within +1.4%, warm indexing was slightly faster, and many graph/audit/semantic calls were near-equal. The notable regression is Phase 8 warm `search(SearchTool)`: original median 366.8 ms versus refactor median 558.8 ms over five repeated calls. Hypergraph build/reuse latency also remains above the 10s threshold on both servers, so it is not a refactor-only defect but it is still a replacement-readiness concern.

Expected crate-boundary canonical-name drift was minimal in the final run. `rmc_server::tools::SearchTool` resolves to `rmc_server::tools::router::SearchToolRouter` on both servers. No count drift from crate-boundary refactoring was observed in the graph, audit, or semantic tools.

Readiness rating: 8.3/10. The refactor is suitable as a functional replacement candidate, but not yet proven as a strict speed replacement.

Needs-review items:

- P1: Investigate repeated warm `search(SearchTool)` latency on the refactor server. Reproduce with five consecutive `search(directory="/home/molaco/Documents/rust-code-mcp-refactor", keyword="SearchTool")` calls on both namespaces.
- P2: Investigate shared hypergraph reuse latency. Reproduce with repeated `build_hypergraph(force_rebuild=false)` calls after a successful forced build; both servers stayed around 14-15s warm.
- P3: Stabilize or document post-clear health/index behavior. Phase 2 showed the original vector store repopulating after `clear_cache`, which makes cold-cache comparisons noisy.
- P4: Treat equal-score search ordering as a known nondeterministic tie unless downstream clients require stable tie-breaking. The top two `SearchTool` results flipped on both servers.

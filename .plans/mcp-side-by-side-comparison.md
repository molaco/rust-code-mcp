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
| `find_definition(SearchTool)` | 1 | 1 | exact | Same declaration path: `crates/rmc-server/src/tools/mod.rs:12:37`. |
| `find_references(SearchTool)` | 14 | 14 | exact | Same count and representative paths. |
| `get_dependencies(router.rs)` | 21 | 21 | exact | Same import list. |
| `get_call_graph(router.rs)` | 70 funcs / 72 rels | 70 funcs / 72 rels | compatible | Same function/callee sets; list ordering differed. |
| `analyze_complexity(router.rs)` | 721 lines / 56 funcs / 163 cyclo / 72 calls | 721 lines / 56 funcs / 163 cyclo / 72 calls | exact | Same metrics. |

Verification notes:

- Definition counts matched for all canonical symbols: `SearchTool` 1, `SyncManager` 1, `UnifiedIndexer` 2, `OpenedSnapshot` 2, `build_hypergraph` 2, and `workspace_stats` 3.
- Reference counts matched for all canonical symbols: `SearchTool` 14, `SyncManager` 19, `UnifiedIndexer` 15, `OpenedSnapshot` 59, `build_hypergraph` 3, and `workspace_stats` 4.
- Dependency counts matched for canonical files: `main.rs` 7, `rmc-engine/src/lib.rs` 0, `snapshot.rs` 37, `unified.rs` 18, and `router.rs` 21.
- Call graph counts matched for canonical files: `main.rs` 19 funcs / 18 rels, `rmc-engine/src/lib.rs` 0 / 0, `snapshot.rs` 79 / 135, `unified.rs` 76 / 111, and `router.rs` 70 / 72.
- Complexity metrics matched for canonical files: `main.rs` 55 lines / 1 func / 4 cyclo / 18 calls; `rmc-engine/src/lib.rs` 8 / 0 / 1 / 0; `snapshot.rs` 667 / 16 / 129 / 135; `unified.rs` 666 / 22 / 67 / 111; `router.rs` 721 / 56 / 163 / 72.

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
| `build_hypergraph(force_rebuild=true)` | 21506.8 | 21125.9 | -1.8 | needs-review | Output exact; both exceed the 10s threshold. |
| `build_hypergraph(force_rebuild=false)` | 14476.3 | 13892.2 | -4.0 | needs-review | Output exact with `reused=true`; both exceed the 10s threshold. |
| `workspace_stats` | 10.8 | 3.2 | -70.4 | exact | Same node, item, binding, visibility, and `pub_crate_share` counts. |
| `crate_edges` | 16.9 | 9.8 | -42.0 | exact | 49 total edges on both. |
| `forbidden_dependency_check` | 6.3 | 6.8 | 7.9 | exact | Full post-C 17-rule boundary set returned 0 violations on both. |

Verification notes:

- Cold and warm `build_hypergraph` returned the same graph id `4fc200b6ab2a6d0ef4162f4fec31da5f`, fingerprint `a2800cb435de19d32f27bf58901fd5efb037e85565033279dd50611589501073`, 3040 nodes, 5371 bindings, and 7963 usages. Snapshot paths stayed isolated under original/refactor XDG data roots.
- `workspace_stats` matched exactly: 45 crates, 296 modules, 2448 items, 250 external symbols, 1830 declared bindings, 1583 glob imports, and 1958 named imports.
- `module_tree(depth=2)` matched exactly for `rmc_engine`, `rmc_graph`, `rmc_indexing`, and `rmc_server`. `rmc_engine` exposes top-level `chunker`, `embeddings`, `parser`, `schema`, `search`, and `vector_store`; `rmc_graph` exposes `graph`; `rmc_indexing` exposes `indexing`, `metadata_cache`, `metrics`, `monitoring`, and `security`; `rmc_server` exposes `mcp`, `semantic`, and `tools`.
- `crate_dependency_metric(summary=true)` matched exactly with 45 crate metrics. Key local crate metrics: `rmc_engine` Ce 1 / Ca 14 / I 0.0667; `rmc_graph` 1 / 11 / 0.0833; `rmc_config` 1 / 3 / 0.25; `rmc_indexing` 2 / 14 / 0.125; `rmc_server` 4 / 6 / 0.4.
- `who_uses(rmc_server::tools::SearchTool)` resolved through the re-export to `rmc_server::tools::router::SearchToolRouter` and returned 5 usages on both.
- `who_calls(rmc_server::tools::graph::core::build_hypergraph)` returned 10 call sites on both, including `SearchToolRouter::build_hypergraph` plus test callers.
- `get_exports(module=rmc_server::tools, consumer=rust-code-mcp)` returned 5 exports on both: `project_paths`, `IndexCodebaseParams`, `index_codebase`, `SearchTool`, and `SearchToolRouter`. The first attempt with consumer `rust_code_mcp` failed identically on both with `no node found for qualified name`.

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
| `missing_docs_audit` | 115 | 115 | exact | Same first page and total count. |
| `derive_audit(Debug)` | 50 | 50 | exact | Same first page and total count. |
| `fn_body_audit` | 423 | 423 | exact | Same first page and pattern set. |
| `unsafe_audit` | 8 | 8 | exact | Same unsafe blocks and safety-comment flags. |
| `dead_pub_report` | 131 | 131 | exact | Same aggregated workspace count and first page. |

Verification notes:

- `channel_capacity_audit(summary=true)` returned 1 finding on both: one unbounded `std_mpsc` call in `crates/rust-code-mcp/tests/test_mcp_stdio_transport.rs`.
- `mut_static_audit(summary=true)` returned 4 findings on both: two `OnceLock` findings in `fastembed`, `rmc_engine::embeddings::profile::BUILT_IN_PROFILES`, and `rmc_server::semantic::SEMANTIC`.
- `recursion_check(summary=true, max_cycle_length=5)` returned 11 cycles on both: 10 direct-recursion cycles and one 2-function cycle around `OpenedSnapshot::lookup_impl_module_item_alias` / `lookup_by_qualified_name_inner`.
- RA-backed audit timings were similar on both servers: `fn_body_audit` about 20.6s, `channel_capacity_audit` about 19.5s, and `unsafe_audit` about 14.2s.
- No server-to-server count drift was expected or observed. The counts reflect the current post-C crate-lift workspace and include local benchmark/test/vendor crates that the hypergraph sees as local crates.

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

# MCP Side-by-Side Comparison Report

Date: 2026-05-22

Plan: `.plans/mcp-side-by-side-comparison.md`

## Scope

This comparison tested the original and refactored Rust code MCP servers through the Codex-exposed MCP tool namespaces only:

- Original: `mcp__rust_code_mcp_original__`
- Refactor: `mcp__rust_code_mcp_refactor__`

No Python JSON-RPC client, stdio protocol harness, `tools/list`, or direct server process harness was used for the comparison. Shell usage was limited to project metadata, VCS metadata, filesystem checks, and documentation edits.

## Verdict

Replacement readiness: 8/10.

The refactor server is functionally compatible with the original across the sampled MCP tool surface. Tool inventory, accepted input field sets, live navigation, graph queries, audits, bounded semantic results, invalid-input behavior, repeated warm calls, and pagination all matched or were compatible.

The refactor should be acceptable as a replacement after the indexing chunk-count drift is explained or fixed and the expensive-query limits are either improved or documented.

## Phase Outcomes

| Phase | Result | Summary |
|---|---|---|
| 0 MCP Tool Baseline | pass | Both namespaces were usable; binaries/config and isolated XDG roots were verified. |
| 1 Tool Inventory and Schemas | pass | Both exposed 51 tools; no original-only or refactor-only tools; no breaking schema-key differences. |
| 2 Health, Cache, and Cold Start | pass | Cache roots were isolated and health behavior matched. |
| 3 Indexing and Retrieval | partial | Retrieval was compatible; forced cold indexing had a one-chunk count drift. |
| 4 Live Navigation and File Analysis | pass | Definitions, references, dependencies, call graphs, and complexity metrics matched. |
| 5 Hypergraph Snapshot and Core Queries | partial | Functional graph output was exact; cold and warm hypergraph builds exceeded 10s on both servers. |
| 6 Audit and Policy Tools | pass | Audit counts and representative findings matched. |
| 7 Semantic Similarity Tools | partial | Bounded semantic outputs matched exactly; unscoped workspace overlap timed out on both. |
| 8 Failure Modes and Robustness | pass | Invalid inputs, repeated calls, expensive repeats, and pagination matched. |
| 9 Final Report Rollup | pass | Final readiness, regressions, and follow-ups were documented. |

## Functional Compatibility

Tool surface and schemas:

- Original tool count: 51
- Refactor tool count: 51
- Shared tool count: 51
- Original-only tools: 0
- Refactor-only tools: 0
- Breaking schema differences: 0

Sampled behavior:

- `find_definition`, `find_references`, `get_dependencies`, `get_call_graph`, and `analyze_complexity` matched on canonical symbols and files.
- `workspace_stats`, `module_tree`, `crate_edges`, `crate_dependency_metric`, `forbidden_dependency_check`, `who_uses`, `who_calls`, and `get_exports` matched for sampled graph queries.
- `missing_docs_audit`, `derive_audit`, `fn_body_audit`, `unsafe_audit`, `channel_capacity_audit`, `mut_static_audit`, `recursion_check`, and `dead_pub_report` matched.
- `similar_to_item` and bounded `semantic_overlaps` matched exactly for the sampled `rmc_server` scope.
- Invalid-input errors and malformed-parameter errors matched in shape and message.

No confirmed refactor-specific functional regression was found.

## Review Items

Priority 1: cold indexing chunk-count drift.

Reproduction:

```text
index_codebase(directory = /home/molaco/Documents/rust-code-mcp-refactor, force_reindex = true)
```

Observed result:

- Original: 2401 chunks
- Refactor: 2402 chunks

The same run indexed 185 of 187 Rust files on both servers, so the extra refactor chunk needs explanation before calling the refactor fully equivalent.

Priority 2: expensive graph and semantic query latency.

Observed result:

- `build_hypergraph(force_rebuild=true)`: original 21506.8 ms, refactor 21125.9 ms
- `build_hypergraph(force_rebuild=false)`: original 14476.3 ms, refactor 13892.2 ms
- Unscoped workspace `semantic_overlaps(output_mode=clusters, summary=true, max_pairs=50)` timed out at the 120s MCP client boundary on both servers.

This is mostly a shared system limit, not a refactor-only regression.

Priority 3: refactor timeout cleanup.

After the unscoped refactor `semantic_overlaps` timeout, the first bounded retry failed with:

```text
environment already open in this program
```

The same bounded request succeeded after a 30 second wait and matched original output exactly. This looks like timeout cleanup lag.

## Performance Summary

| Operation | Original | Refactor | Delta | Result |
|---|---:|---:|---:|---|
| Binary size | 290810664 bytes | 271642248 bytes | -6.59% | refactor smaller |
| `health_check` smoke | 59.2 ms | 26.0 ms | -56.1% | refactor faster |
| `index_codebase(force_reindex=true)` | 46668.6 ms | 46151.5 ms | -1.1% | refactor faster, count drift |
| `index_codebase(force_reindex=false)` | 11.8 ms | 11.2 ms | -5.2% | exact |
| `search(SearchTool)` Phase 3 | 334.9 ms | 312.1 ms | -6.8% | compatible |
| Repeated warm `search(SearchTool)` median | 339.3 ms | 362.3 ms | +6.8% | compatible |
| `get_similar_code(build_hypergraph)` | 371.5 ms | 390.1 ms | +5.0% | compatible |
| `build_hypergraph(force_rebuild=true)` | 21506.8 ms | 21125.9 ms | -1.8% | exact, over threshold |
| `build_hypergraph(force_rebuild=false)` | 14476.3 ms | 13892.2 ms | -4.0% | exact, over threshold |
| `workspace_stats` Phase 5 | 10.8 ms | 3.2 ms | -70.4% | exact |
| Repeated warm `workspace_stats` median | 3.9 ms | 3.5 ms | -10.3% | exact |
| `crate_edges` | 16.9 ms | 9.8 ms | -42.0% | exact |
| `forbidden_dependency_check` | 6.3 ms | 6.8 ms | +7.9% | exact |
| `similar_to_item(SearchTool)` | 345.8 ms | 354.0 ms | +2.4% | exact |
| `semantic_overlaps` clusters, scoped | 23.8 ms | 21.2 ms | -10.9% | exact |
| `semantic_overlaps` pairs, scoped | 20.7 ms | 20.8 ms | +0.5% | exact |

Performance was mixed but acceptable for parity. The refactor was often faster on structural graph operations and indexing, while some warm search and semantic calls were slightly slower. The main performance problem is shared expensive-query behavior.

## Reliability

Error behavior matched:

- Missing workspace directory returned the same `-32602` not-a-directory error.
- Unknown `find_definition` symbol returned the same no-definition message.
- Unknown `who_uses` target returned the same `-32602` no-node error.
- `derive_audit(required_derives=[])` returned the same `-32602` validation error.

Repeated calls matched:

- Five warm `search(keyword=SearchTool)` calls returned 10 results every time on both servers. The top result set and scores matched; equal-score first results sometimes swapped order.
- Five warm `workspace_stats` calls returned identical counts every time.
- Two warm scoped `semantic_overlaps` pair-mode calls returned 297 seeds, 46 pairs, and 32 clusters on both servers.
- `who_uses(target=rmc_graph::graph::snapshot::OpenedSnapshot, offset=10, limit=10)` returned 52 total matches and the same 10-row page on both servers.

## Final Recommendation

Use the refactor server as the candidate replacement, but keep the replacement gated on:

1. Explaining or fixing the one-chunk indexing drift.
2. Deciding whether the shared `build_hypergraph` and unscoped `semantic_overlaps` latency is acceptable.
3. Checking timeout cleanup for refactor semantic overlap calls.

No MCP tool-surface or sampled functional blocker was found.

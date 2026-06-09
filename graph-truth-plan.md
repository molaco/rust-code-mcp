# Graph-Truth Plan

Purpose: make `rust-code-mcp` operate on Rust code at crate, module, file, and item level from the graph first. Embeddings and vectors are secondary: they help similarity search, but exact refactor operations must come from structural graph data.

## 1. Add A Canonical Scope Model

Feature goal: every tool can talk about the same crate/module/file/item scopes.

Code:

- Add `crates/rmc-graph/src/graph/query/scopes.rs`.
- Add DTOs:
  - `ScopeId`
  - `ScopeKind { Workspace, Crate, Module, File, Item }`
  - `ScopeSummary`
  - `ScopeChild`
- Build summaries from `OpenedSnapshot` using existing `NodeKind`, `ItemKind`, `file`, `span`, `parent_id`, `crate_id`.
- Expose via `crates/rmc-server/src/tools/graph/core.rs` as `graph_scopes`.

Leverage:

- This becomes the common input for crate/module/file operations.
- It stops tools from re-inventing "what is this module/file/crate?" differently.

Exit condition:

- Given a crate, module, or file, the tool returns contained modules, files, items, public surface count, private item count, and test item count.

## 2. Add Aggregated Boundary Edges

Feature goal: answer "what crosses this boundary?" exactly.

Code:

- Add `crates/rmc-graph/src/graph/query/boundaries.rs`.
- Aggregate existing bindings/usages/imports/calls into:
  - crate -> crate edges
  - module -> module edges
  - file -> file edges
  - item -> item edges
- Edge kinds:
  - `Import`
  - `ReExport`
  - `Use`
  - `Call`
  - `TypeRef`
  - `VisibilityExport`
- Add MCP tool `boundary_edges`.

Leverage:

- This directly supports `THEORY_3.md` Move, Split/Merge, and Lift/Lower decisions.
- It gives real evidence for "dense group", "bridge file", "hub module", and "leaf file".

Exit condition:

- For any crate/module/file, the tool returns inbound/outbound edge counts, top producers/consumers, public-surface edges, and internal-vs-external ratio.

## 3. Add Structural Inventory Reports

Feature goal: make Workflow A/A2-A5 possible from one graph-backed report.

Code:

- Add `crates/rmc-graph/src/graph/query/inventory.rs`.
- Add MCP tool `structure_inventory`.
- Output:
  - crate list
  - module tree
  - file list grouped by module
  - item counts by kind
  - hub files/modules
  - bridge files/modules
  - leaf files/modules
  - one-file module directories
  - large mixed files

Leverage:

- Agents can stop using filename vibes.
- This enables a concrete "current structural problem" report before edits.

Exit condition:

- One call can identify bad layout symptoms: too many files in one module, too many one-file modules, high cross-module edge count, and accidental public surface.

## 4. Add Operation Candidate Engine

Feature goal: suggest concrete Move, Split/Merge, Lift/Lower operations.

Code:

- Add `crates/rmc-graph/src/graph/query/operations.rs`.
- Add DTOs:
  - `PrimitiveOperation { Move, Split, Merge, Lift, Lower }`
  - `OperationCandidate`
  - `OperationEvidence`
- Add MCP tool `operation_candidates`.
- Candidate rules:
  - Move file/module: most edges point to another scope.
  - Split module/file: one scope has multiple dense internal groups and multiple public surfaces.
  - Merge modules/files: two scopes always change/reference together and have low independent public surface.
  - Lift module to crate: stable module, narrow public surface, low reverse dependency risk.
  - Lower crate to module: crate has no independent job or is only used by one parent.

Leverage:

- Converts `THEORY_3.md` into machine-checkable refactor suggestions.
- Highest feature value for crate/module/file operations.

Exit condition:

- Every candidate includes target scope, operation, evidence edges, affected files, expected public API impact, and verification tools.

## 5. Add Feature Target Finder

Feature goal: for a requested feature, find the smallest implementation scope.

Code:

- Add `crates/rmc-graph/src/graph/query/feature_target.rs`.
- Add MCP tool `feature_target`.
- Inputs:
  - free-text feature summary
  - optional seed files
  - optional qualified symbols
  - optional existing search hits
- Use exact graph first:
  - calls
  - imports
  - usages
  - signatures
  - public surface
- Use embeddings only as optional expansion/rerank.

Leverage:

- Implements Workflow B/B1 directly: "The feature belongs primarily in X."
- Reduces wide, unbounded edits.

Exit condition:

- Tool returns one preferred target scope, optional secondary scopes, reason, boundary risks, and "redesign first" if no good target exists.

## 6. Add Simulation For Structural Operations

Feature goal: preview crate/module/file changes before editing.

Code:

- Add `crates/rmc-server/src/tools/refactor/simulate.rs`.
- Add graph DTOs in `crates/rmc-graph/src/graph/query/operations.rs`.
- Add MCP tools:
  - `simulate_move_file`
  - `simulate_split_module`
  - `simulate_merge_modules`
  - `simulate_lift_module_to_crate`
  - `simulate_lower_crate_to_module`
- Simulation outputs:
  - files to move/create/delete
  - `mod` declarations to change
  - imports to rewrite
  - re-exports/adapters needed
  - public API impact
  - affected callers
  - blockers

Leverage:

- Makes mature Workflow C possible without big-bang edits.
- Turns operation candidates into concrete edit plans.

Exit condition:

- Simulations are dry-run only and produce a reviewable patch plan with no filesystem writes.

## 7. Add Check Gates For Each Operation

Feature goal: every operation has a graph-based exit condition.

Code:

- Add `crates/rmc-graph/src/graph/query/checks.rs`.
- Add MCP tool `operation_check`.
- Checks:
  - public surface did not widen unless requested
  - no new forbidden crate edge
  - module/file boundary external ratio improved or stayed justified
  - moved symbols still reachable through expected paths
  - old facade/re-export exists when compatibility mode is requested
  - dead public surface decreased or is explained

Leverage:

- Turns refactor safety from "compile passed" into boundary verification.
- Prevents the old failure mode where files moved but coupling stayed bad.

Exit condition:

- Each primitive operation has a before/after report with pass/fail status and concrete offending edges.

## 8. Add Graph Embedding Inputs For Similarity Only

Feature goal: semantic tools use graph identity without making vectors the source of truth.

Code:

- Add `crates/rmc-graph/src/graph/embedding_input.rs`.
- Add transient type:
  - `GraphEmbeddingInput`
  - `unit_id`
  - `node_id`
  - `unit_kind`
  - `input_text`
  - `input_hash`
  - `input_policy_version`
- Rendering policy:
  - functions/methods: signature + body within token budget
  - oversized functions: split/truncate with metadata
  - impls/modules: summary only, no full body
  - structs/enums/traits: declaration/signature/member summary

Leverage:

- Fixes semantic search and `semantic_overlaps` without polluting exact structural tools.
- All embeddings now point back to graph `NodeId`.

Exit condition:

- No graph similarity path embeds raw `Node.span` directly.

## 9. Replace Vector Rows With Graph Rows

Feature goal: vector search returns graph-native results.

Code:

- Add `GraphVectorRow` in `crates/rmc-engine/src/vector_store/mod.rs` or a new `rmc-engine::vector_store::graph_row` module.
- Extend `VectorStoreBackend`:
  - `upsert_graph_rows`
  - `search_graph_rows`
  - `delete_by_graph_id`
  - `delete_by_node_id`
- Update `crates/rmc-engine/src/vector_store/lancedb.rs` schema:
  - `unit_id`
  - `node_id`
  - `unit_kind`
  - `qualified_name`
  - `item_kind`
  - `file`
  - `byte_span_start`
  - `byte_span_end`
  - `line_start`
  - `line_end`
  - `input_hash`
  - `input_policy_version`
  - `graph_id`
  - `truncated`
  - `vector`
- Do not store full source documents in LanceDB.

Leverage:

- Removes file/line guessing from semantic search.
- Search can return exact graph targets.

Exit condition:

- Vector search result contains `node_id` and `unit_id`; `CodeChunk` is no longer required for graph-backed search.

## 10. Replace `index_codebase` Primary Path

Feature goal: build graph first, then vector rows from graph Items.

Code:

- In `crates/rmc-server/src/tools/endpoints/index.rs`, make graph-backed indexing the default:
  1. ensure current graph snapshot exists or build it
  2. enumerate graph Items
  3. create `GraphEmbeddingInput`
  4. batch embed inputs
  5. write `GraphVectorRow`
- Move parser-only `UnifiedIndexer` / `CodeChunk` path behind explicit fallback:
  - no graph snapshot
  - graph build failed and user requested fallback
  - non-Rust file mode, if kept

Leverage:

- This is where the old index actually stops being primary.
- All search results become graph-resolvable.

Exit condition:

- Default `index_codebase` creates graph-native vector rows and reports graph item count, vector row count, skipped oversized/truncated count, graph id, and embedder identity.

## 11. Route Semantic Tools Through Graph Rows

Feature goal: remove bridge code and make similarity graph-native.

Code:

- Update `crates/rmc-server/src/tools/graph/similarity.rs`.
- `similar_to_item`:
  - resolve target to `node_id`
  - build query from `GraphEmbeddingInput`
  - search graph vector rows
  - drop self by `node_id`, not file/line overlap
- `semantic_overlaps`:
  - use `GraphEmbeddingInput`
  - cache by `node_id + unit_id + input_hash + embedder_identity + input_policy_version`
  - aggregate split rows back to `node_id`
- `codemap`:
  - use `node_id` from search rows directly
  - keep line/span fallback only for old indexes

Leverage:

- Ends the current split between graph Items and search chunks.
- Makes semantic results usable by exact graph tools.

Exit condition:

- No production semantic path needs "smallest enclosing Item from file/line" except fallback compatibility.

## 12. Add Act Layer Only After Simulation Is Trusted

Feature goal: apply crate/module/file operations safely.

Code:

- Add `crates/rmc-server/src/tools/refactor/act.rs`.
- Start with the safest operations:
  1. move file within same crate
  2. update `mod` declarations
  3. update imports
  4. add compatibility re-export
- Defer high-risk operations:
  - split module
  - lift module to crate
  - lower crate to module
- Use rust-analyzer rename only for symbol rename, not filesystem/module moves.

Leverage:

- Makes the tool operational, not just diagnostic.
- Keeps write operations gated by prior graph simulation.

Exit condition:

- `act_*` tools require a simulation id or exact expected plan hash.

## 13. Migrate Existing Tools To The Five-Verb Flow

Feature goal: make tool behavior match how agents actually work.

Code:

- Tool families:
  - Observe: `structure_inventory`, `boundary_edges`, `graph_scopes`
  - Discover: `operation_candidates`, `feature_target`
  - Simulate: `simulate_*`
  - Act: `act_*`
  - Check: `operation_check`, existing audits
- Update router descriptions in `crates/rmc-server/src/tools/router.rs`.
- Update `TOOLS.md`.

Leverage:

- Agents get a clear workflow instead of a bag of unrelated tools.
- Maps directly to `THEORY_3.md` required output.

Exit condition:

- For a crate/module/file refactor, the expected path is visible from tool names alone: observe -> discover -> simulate -> act -> check.

## 14. Verification Milestones

Feature goal: prove the migration did not just add another parallel system.

Milestones:

1. `graph_scopes` and `boundary_edges` pass fixture tests.
2. `operation_candidates` finds known synthetic Move/Split/Merge cases.
3. `simulate_move_file` produces correct dry-run edits on a fixture crate.
4. `operation_check` detects an intentionally widened public API.
5. `index_codebase` writes graph vector rows by default.
6. `similar_to_item` returns `node_id` without file/line guessing.
7. `semantic_overlaps` uses graph embedding inputs, not raw spans.
8. Parser-only chunk indexing is fallback-only and explicitly reported as such.

## Non-Goals

- Do not use embeddings for imports, exports, usages, calls, signatures, or exact dependency checks.
- Do not store full source documents in the vector index.
- Do not make LanceDB the source of truth for code identity.
- Do not add write operations before simulation and check gates exist.
- Do not run formatting as part of this plan.

 Preparing actionable items

 I need to compile a list of actionable items based on previous pull requests (PRs) 1-7. The user likely wants follow-up items from the reviews. I’ll focus on keeping it concise, mentioning severity and prioritization, possibly marking
 items as P0, P1, P2. Key aspects could include noting tests failures that are actionable, using CursorPage, and file dependencies. I want to be careful not to overdo it, while ensuring everything is ready for PR8. Let’s craft this list
 effectively!

 Organizing actionable cleanup

 I need to clarify that "previous PRs" refers to PR 1-7, so we can focus on actionable items for cleanup before or alongside PR8. I’ll highlight regular issues like fixing library test failures linked to invalid project tests and
 deciding on structured JSON error conventions. For example, in PR1, I’ll mention moving inventory types, using CursorPage<T>, and adding validation for ItemId/SnapshotId. For PR2, I'll note changes to fingerprint implementations and
 avoiding global mutex blocks. Let’s ensure these are clear and actionable!

 Refining workspace management

 I need to focus on implementing per-workspace locks or consider storing an Arc session. It’s important to add a TTL for eviction as well. I'll clarify the semantics for stale_item_id, especially if the inventory isn't built, but the
 snapshot remains valid, while missing items are acceptable. I should also add tests for changed workspaces where cursor list_items return a snapshot that might be expired. Lastly, for PR3, I might include root modules in the inventory
 and clarify naming to indicate the root crate and Module kind.

 Here’s the actionable cleanup list for PRs 1–7, based on the review.

 Cross-cutting

 1. Fix full lib test failures
     - Failing tests:
           - tools::analysis_tools::tests::test_find_definition_invalid_project
           - tools::analysis_tools::tests::test_find_references_invalid_project
     - These are outside the semantic PR lane but currently make cargo test --lib fail.
 2. Standardize JSON/error envelopes
     - Some semantic errors are embedded as JSON strings inside MCP invalid_params.
     - Action: make sure clients can reliably inspect snapshot_expired / stale_item_id via structured data, not only string parsing.
 3. Avoid holding global semantic cache lock during heavy work
     - Current global Mutex<SemanticSessionCache> can serialize expensive dependency-surface work.
     - Action: move toward per-workspace locking or Arc-held cached sessions so one long query does not block all semantic tools.

 ────────────────────────────────────────────────────────────────────────────────

 PR 1: Structured Contracts

 1. Use CursorPage<T> in actual tool responses
     - CursorPage<T> exists but list_items uses a custom response shape.
     - Action: make list_items response embed or directly use CursorPage<InventoryItem>.
 2. Decide whether to keep contracts in src/semantic/contracts.rs
     - Plan expected src/semantic/inventory/types.rs.
     - Current central file is fine, but action is either:
           - document that contracts.rs is the canonical location, or
           - add module re-exports matching the planned path.
 3. Validate raw ItemId / SnapshotId strings on deserialization
     - They are transparent string wrappers.
     - Action: optionally add format validation like item_<sha256> / snapshot_<sha256>.
 4. Populate or clarify DependencySurfaceRow.item_id
     - Field exists but dependency-surface rows currently set item_id: None.
     - Action: either populate where possible or document that this is reserved for a later join pass.

 ────────────────────────────────────────────────────────────────────────────────

 PR 2: Semantic Session Cache

 1. Fix cursor pagination staleness check
     - list_items cursor calls currently pass verify_current=false.
     - This can serve a stale snapshot after workspace changes.
     - Action: always verify fingerprint before serving a cursor page, or explicitly document and test snapshot-pinned pagination semantics.
 2. Align fingerprinting with the plan
     - Plan said cheap fingerprint over Cargo files and Rust file mtimes.
     - Implementation hashes file contents.
     - Action: either switch to mtime/size metadata or document why content hashing was chosen.
 3. Add TTL / eviction
     - Cache is short-lived by intent, but no actual TTL or capacity policy is visible.
     - Action: add simple age-based eviction or max-session eviction.
 4. Reduce lock scope around RA loading
     - current_snapshot_id may load a full RA project while holding the global mutex.
     - Action: avoid loading under the global cache lock where possible.
 5. Add explicit test for list_items stale cursor behavior
     - Modify workspace after first page.
     - Second page with old snapshot_id + cursor should return snapshot_expired.

 ────────────────────────────────────────────────────────────────────────────────

 PR 3: Inventory Library + list_items

 1. Emit root modules explicitly
     - Inventory currently visits child modules but does not clearly emit crate root modules.
     - Action: add root module inventory rows, likely named by crate or canonical root module label.
 2. Use shared pagination contract
     - Action: replace custom { snapshot_id, items, next_cursor } with a shape using CursorPage<InventoryItem> plus snapshot_id.
 3. Review public_only semantics
     - Current behavior excludes only private; it keeps pub(self).
     - That was documented in the tool, but may surprise users.
     - Action: decide whether public_only should mean “not private” or “externally visible”.
 4. Expand coverage for module/crate filters
     - Add tests for:
           - workspace member crate filter
           - root module filter
           - crate::foo vs crate_name::foo normalization
           - empty result pagination
 5. Ensure stable ordering is documented
     - Sorting is file/range/kind/name.
     - Action: document ordering because cursors depend on it.

 ────────────────────────────────────────────────────────────────────────────────

 PR 4: Dependency Surface Refactor

 1. Move markdown rendering out of the MCP wrapper
     - Query logic is extracted, but dependency_surface_tool.rs still contains a large renderer.
     - Action: move renderer into a separate renderer module, e.g. src/tools/dependency_surface_render.rs.
 2. Add pagination or explicit bounded-result policy
     - Dependency-surface remains limit-based, not cursor-paginated.
     - Action: either add cursor pagination for workspace-scale responses or clearly state that dependency-surface is bounded by limit.
 3. Populate JSON rows with richer identity
     - JSON output works, but rows lack item_id.
     - Action: join rows to inventory where possible, especially symbol rollups.
 4. Add stronger JSON-vs-markdown equivalence tests
     - Current tests check JSON exists.
     - Action: add tests ensuring JSON row contents match markdown-rendered row content for representative import/export/reexport/usage cases.

 ────────────────────────────────────────────────────────────────────────────────

 PR 5: item_id-Based Composition

 1. Support more item_id selector positions
     - Currently item_id is only left-scope.
     - Action: consider right-scope item_id support for producer/consumer filtering.
 2. Clarify non-module item_id limitations
     - Non-module item IDs only work for usage.
     - Action: document this clearly in schema descriptions and docs.
 3. Normalize file+offset fallback against workspace root
     - File+offset currently resolves relative to requested directory.
     - Action: ensure behavior is clear when querying from a member crate directory inside a workspace.
 4. Add changed-workspace item selector test
     - Existing stale item test covers fake/missing ID.
     - Action: add test:
           - get real item_id
           - modify workspace without changing snapshot id in request
           - query by item
           - expect snapshot_expired, not stale_item_id.
 5. Populate dependency rows with selected item identity where applicable
     - When an item selector drives the query, rows could include the selected item_id.
     - Action: preserve item identity through query response where useful.

 ────────────────────────────────────────────────────────────────────────────────

 PR 6: Workflow Hardening

 1. Add explicit agent-workflow integration tests
     - Plan listed derivable workflows but tests do not fully prove them.
     - Add tests for:
           - crate edge matrix
           - per-edge symbol drilldown
           - workspace-unused-pub derivation
           - collision handling
           - list-items → dependency-surface multi-step cache reuse
 2. Add cache-reuse instrumentation test
     - Action: prove that normal list_items then query_dependency_surface does not reload rust-analyzer.
 3. Add docs for JSON-first and pagination expectations
     - Action: document:
           - list_items is JSON-only
           - cursors are opaque
           - snapshot_id must be supplied with cursors and item selectors
           - snapshot_expired / stale_item_id behavior
 4. Keep semantic tests serialized
     - Already recommended.
     - Action: add this requirement to contributor/test docs.

 ────────────────────────────────────────────────────────────────────────────────

 PR 7: Vector Schema V2

 1. Expose filter support through vector store API
     - LanceDB has promoted columns, but vector search API cannot filter on them yet.
     - Action: add filtered vector search accepting crate/symbol/visibility/path filters.
 2. Consider promoting start_byte / end_byte to LanceDB columns
     - Plan only required promoting symbol_name, visibility, and crate_name, but PR8 item-aware matching may benefit from byte ranges.
     - Action: optionally add scalar columns for start_byte and end_byte.
 3. BM25 schema lacks crate/visibility fields
     - PR8 hybrid filtering will need structural filters.
     - Action options:
           - post-filter BM25 results using chunk.context, or
           - extend Tantivy chunk schema to store/index crate_name, visibility, start_byte, end_byte.
 4. Add vector v2 migration/reindex user-facing note
     - Collection name changed to v2.
     - Action: document that old vector collections are untouched and v2 requires fresh reindex.
 5. Add test for old v1 collection untouched
     - Current tests verify v2 path/name.
     - Action: add explicit test that cleanup/reindex does not delete old v1 collection unexpectedly.

 ────────────────────────────────────────────────────────────────────────────────

 Highest-priority before PR8

 If we want PR8 to be smooth, I’d fix these first:

 1. list_items cursor calls should return snapshot_expired after workspace changes.
 2. Stop holding the global semantic cache lock during heavy dependency-surface work.
 3. Add filterable vector-search API or decide to do PR8 post-filtering.
 4. Decide JSON response shape for extended get_similar_code.
 5. Fix the two failing full-lib tests, or explicitly track them as unrelated known failures.

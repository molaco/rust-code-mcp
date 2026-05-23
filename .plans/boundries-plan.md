# Boundaries Cleanup Plan

## Objective

Tighten the crate-level and internal module/directory boundaries found by the
boundary analysis reports:

- `.docs/boundry-rmc-engine-report.md`
- `.docs/boundry-rmc-graph-report.md`
- `.docs/boundry-rmc-indexing-report.md`
- `.docs/boundry-rmc-server-report.md`
- `.docs/boundy-report-index.md`

The goal is not to reorganize everything at once. The goal is to add narrower
facades first, migrate server callers to those facades, then reduce public
implementation-module exposure where it is safe.

## Hard Rules

- Use `jj` first for VCS operations.
- Before each implementation phase, run `jj show --summary`.
- After each phase, update the progress notes and commit with `jj commit -m`.
- Do not run `cargo fmt` or any formatting command.
- Use MCP tools as the primary evidence source before changing a boundary.
- Do not write Python scripts.
- Do not use stdio MCP harnesses.
- Source-read only the files/symbols identified by MCP evidence or by compiler
  errors.
- Build/check commands must use:

```text
nix develop ../nix-devshells#cuda-code --command {command}
```

## Cross-Validated Decisions

These decisions were validated against the current hypergraph, targeted MCP
queries, and source reads before this plan was updated.

### Compatibility Policy

Use a compatibility-first migration.

1. Add narrow facades.
2. Migrate production server callers.
3. Keep old public exports while tests, benches, debug tools, and integration
   callers are still using them.
4. Tighten visibility only after MCP evidence shows production callers have
   moved and remaining callers are intentionally supported.

This is especially important for `rmc_graph::graph`,
`rmc_indexing::indexing`, and `IncrementalIndexer`, which have non-server
test/bench/tool consumers.

### Crate Layering

Keep the existing crate direction. The current forbidden-dependency rule check
has no violations, so this plan is about narrowing API boundaries rather than
fixing dependency cycles.

```text
rmc_server   -> rmc_graph, rmc_indexing, rmc_engine, rmc_config
rmc_graph    -> rmc_engine
rmc_indexing -> rmc_engine, rmc_config
rmc_engine   -> no rmc_* dependencies
```

### Indexing Decisions

- Add a small indexing-owned search facade so server query/codemap code no
  longer constructs `TantivyAdapter` directly.
- Keep `TantivyAdapter` public for compatibility during the migration.
- Do not make `IncrementalIndexer` private in the near term. It is currently
  used by server production code plus tests, benches, and standalone tools.
- Add a wrapper/service API for server sync/index flows so server stops owning
  incremental indexing construction details.
- After server migration, review implementation modules such as `identity`,
  `merkle`, `retry`, `consistency`, `indexer_core`, and `tantivy_adapter` for
  visibility tightening.

### Project Path And Identity Decisions

- Move indexing identity/path policy toward `rmc_indexing`.
- Keep server responsible for MCP-facing path orchestration and data-root
  discovery.
- Keep `rmc_server::mcp::project_paths::ProjectPaths` as a compatibility
  wrapper during migration.
- Consolidate duplicate `data_dir` and embedding-backend resolver helpers.

### Graph Decisions

- Keep `rmc_graph::graph` as a compatibility facade while adding narrower
  graph-owned APIs.
- Graph facades should return graph-owned DTOs/query results, not MCP
  `CallToolResult` or server-specific response types.
- Move graph audit loading/dispatch behind graph-owned audit entry points.
- Move graph semantic similarity mechanics behind a graph-owned similarity
  operation; server should not coordinate `ensure_embeddings_for` and `cosine`.
- Move graph storage cleanup behind a graph-owned cleanup/path API; server
  should not own `GraphPaths` layout policy.

### Server Decisions

- Keep `SearchToolRouter` / `SearchTool` as the primary public server facade.
- Keep router methods thin.
- Treat `rmc_server::semantic` privacy as a later low-risk cleanup. Its
  meaningful exports are already `pub(crate)`, but graph tests and symbol-level
  expectations mention the qualified path, so do not make it an early change.
- Remove `tools::project_paths` only after the remaining compatibility import
  is migrated to the canonical path.

### Engine Decisions

- Keep `EmbeddingProfile` in `rmc_engine` for now and document it as a
  deliberate engine-owned embedding config model.
- Treat `EmbeddingBackend` as a formal cross-crate boundary type.
- Tighten `search`, `vector_store`, and parser helper module visibility only
  after consumers are migrated to facade reexports.

## Planned Change Inventory By Phase

This section is a planning estimate, not a mechanical requirement to create
files. If source reads or compiler feedback show a change belongs in an
existing module, prefer the existing module and record the deviation in the
execution ledger.

LOC estimates are rough changed-line ranges. Replace them with actual `jj diff
--stat` numbers in `.docs/boundries-cleanup-progress.md` during execution.

### Phase 0 Inventory

- New dirs: none.
- New files: `.docs/boundries-cleanup-progress.md`.
- Modified files: `.docs/boundries-cleanup-progress.md`.
- Deleted files/modules: none.
- New modules/types/methods/functions: none.
- Modified modules/types/methods/functions: none.
- Deleted modules/types/methods/functions: none.
- Production LOC change: `0`.
- Test/docs LOC change: `+80..150`.

### Phase 1 Inventory

- New dirs: none expected.
- New files: optional architecture/boundary docs if no suitable existing
  location exists.
- Modified files: existing architecture/audit docs or focused architecture
  test location discovered by MCP evidence.
- Deleted files/modules: none.
- New modules/types/methods/functions: optional focused architecture test
  helper only if a local test pattern already exists.
- Modified modules/types/methods/functions: none in production code.
- Deleted modules/types/methods/functions: none.
- Production LOC change: `0`.
- Test/docs LOC change: `+40..120`.

### Phase 2 Inventory

- New dirs: none.
- New files/modules: prefer `crates/rmc-indexing/src/indexing/search.rs` for
  the search facade. If source reads show `unified.rs` is the better local
  home, add the facade there instead and do not create `search.rs`.
- Modified files/modules:
  - `crates/rmc-indexing/src/indexing/mod.rs`
  - `crates/rmc-indexing/src/indexing/unified.rs` or new `search.rs`
  - `crates/rmc-server/src/tools/endpoints/query.rs`
  - `crates/rmc-server/src/tools/graph/codemap.rs`
- Deleted files/modules: none.
- New types: optional small request/config wrapper only if it removes repeated
  parameters.
- New functions/methods:
  - `open_bm25_search(...)` or equivalent indexing-owned facade
  - optional `open_bm25_search_with_config(...)` if config construction cannot
    stay hidden
- Modified functions/methods:
  - `rmc_server::tools::endpoints::query::try_open_bm25`
  - codemap BM25/hybrid-search setup path
- Deleted functions/methods: no production deletion expected.
- Production LOC change: `+80..180`, with `-20..80` server simplification.
- Test/docs LOC change: `+40..120`.

### Phase 3 Inventory

- New dirs: none.
- New files/modules: prefer
  `crates/rmc-indexing/src/indexing/incremental_service.rs` for the
  server-facing incremental facade.
- Modified files/modules:
  - `crates/rmc-indexing/src/indexing/mod.rs`
  - `crates/rmc-indexing/src/indexing/incremental.rs`
  - `crates/rmc-server/src/tools/endpoints/index.rs`
  - `crates/rmc-server/src/mcp/sync.rs`
- Deleted files/modules: none.
- New types:
  - `IncrementalIndexRequest` or equivalent options struct
  - `IncrementalIndexOutcome` or equivalent result struct
- New functions/methods:
  - `index_project_incrementally(...)`
  - optional `clear_project_index_data(...)` if server clear behavior still
    constructs `IncrementalIndexer`
- Modified functions/methods:
  - `index_codebase`
  - `SyncManager::sync_directory_now`
  - `SyncManager::sync_now`
- Deleted functions/methods: none; keep `IncrementalIndexer` public.
- Production LOC change: `+150..320`, with `-40..120` server simplification.
- Test/docs LOC change: `+80..220`.

### Phase 4 Inventory

- New dirs: none.
- New files/modules: prefer
  `crates/rmc-indexing/src/indexing/project_paths.rs` or a smaller addition to
  `crates/rmc-indexing/src/indexing/identity.rs` if the implementation remains
  identity-only.
- Modified files/modules:
  - `crates/rmc-indexing/src/indexing/mod.rs`
  - `crates/rmc-indexing/src/indexing/identity.rs`
  - `crates/rmc-server/src/mcp/project_paths.rs`
  - `crates/rmc-server/src/tools/endpoints/query.rs`
  - `crates/rmc-server/src/tools/endpoints/index.rs`
  - `crates/rmc-server/src/tools/endpoints/health.rs`
  - `crates/rmc-server/src/tools/endpoints/indexing_support.rs`
  - `crates/rmc-server/src/tools/graph/similarity.rs`
- Deleted files/modules: none in this phase.
- New types:
  - `IndexingProjectPaths` or equivalent indexing-owned path bundle
  - `IndexedProfilePaths` moved or mirrored behind indexing ownership
- New functions/methods:
  - indexing-owned project/index path constructor
  - indexing-owned indexed-profile discovery
  - single backend-resolution helper or path-aware wrapper
- Modified functions/methods:
  - `ProjectPaths::from_directory`
  - `ProjectPaths::indexed_profiles`
  - server `data_dir` wrappers
  - backend resolver functions in index/query/similarity paths
- Deleted functions/methods: duplicate server helper functions only after all
  call sites move.
- Production LOC change: `+220..480`, with `-100..260` server simplification.
- Test/docs LOC change: `+120..280`.

### Phase 5 Inventory

- New dirs: none.
- New files/modules: prefer
  `crates/rmc-graph/src/graph/query/enrichment.rs` for graph-owned query/DTO
  enrichment.
- Modified files/modules:
  - `crates/rmc-graph/src/graph/query/mod.rs`
  - `crates/rmc-graph/src/graph/query/model.rs`
  - `crates/rmc-graph/src/graph/mod.rs`
  - `crates/rmc-server/src/tools/graph/response.rs`
  - `crates/rmc-server/src/tools/graph/core.rs`
  - `crates/rmc-server/src/tools/graph/surface.rs`
- Deleted files/modules: none.
- New types:
  - graph-owned item/node reference DTOs if existing query DTOs are too raw
  - graph-owned binding/usage/dead-pub enrichment DTOs as needed
- New functions/methods:
  - graph-owned node resolution helper
  - graph-owned item-ref conversion helper
  - graph-owned binding/usage enrichment helpers
- Modified functions/methods:
  - server helpers that currently take `OpenedSnapshot`
  - graph query methods if DTOs move closer to graph
- Deleted functions/methods: server-only graph enrichment helpers after
  migration, if no longer needed.
- Production LOC change: `+250..550`, with `-120..320` server simplification.
- Test/docs LOC change: `+120..320`.

### Phase 6 Inventory

- New dirs: none.
- New files/modules: prefer additions to
  `crates/rmc-graph/src/graph/query/audits.rs`; create
  `crates/rmc-graph/src/graph/audits.rs` only if the facade should sit outside
  query.
- Modified files/modules:
  - `crates/rmc-graph/src/graph/query/audits.rs`
  - graph audit modules used by the facade
  - `crates/rmc-graph/src/graph/mod.rs`
  - `crates/rmc-server/src/tools/graph/audits.rs`
- Deleted files/modules: none.
- New types:
  - graph-owned audit request/options wrappers where server params are too MCP
    specific
  - graph-owned audit result DTOs if current findings need enrichment
- New functions/methods:
  - `run_channel_capacity_audit(...)`
  - `run_fn_body_audit(...)`
  - `run_recursion_check(...)`
  - `run_unsafe_audit(...)`
  - `run_mut_static_audit(...)`
- Modified functions/methods:
  - server audit endpoints to call graph facade functions
  - graph audit helpers if option conversion moves down
- Deleted functions/methods: no graph audit deletion expected; remove only
  server loader/dispatch helper code.
- Production LOC change: `+300..650`, with `-150..360` server simplification.
- Test/docs LOC change: `+140..320`.

### Phase 7 Inventory

- New dirs: none.
- New files/modules: prefer
  `crates/rmc-graph/src/graph/query/similarity.rs`.
- Modified files/modules:
  - `crates/rmc-graph/src/graph/query/mod.rs`
  - `crates/rmc-graph/src/graph/query/model.rs`
  - `crates/rmc-graph/src/graph/embedding_cache.rs`
  - `crates/rmc-graph/src/graph/math.rs`
  - `crates/rmc-graph/src/graph/mod.rs`
  - `crates/rmc-server/src/tools/graph/similarity.rs`
- Deleted files/modules: none.
- New types:
  - `SimilarityRequest` or equivalent graph-owned options
  - `SimilarityItem`
  - `SimilarityPair`
  - `SimilarityCluster`
  - optional page/result wrapper
- New functions/methods:
  - graph-owned semantic overlap operation
  - graph-owned similar-to-item operation
- Modified functions/methods:
  - server `semantic_overlaps` graph tool implementation
  - server `similar_to_item` graph tool implementation
- Deleted functions/methods: server-local pairwise cosine/cache orchestration
  after migration.
- Production LOC change: `+250..520`, with `-140..340` server simplification.
- Test/docs LOC change: `+120..300`.

### Phase 8 Inventory

- New dirs/files: none expected.
- New modules: none expected.
- Modified files/modules:
  - `crates/rmc-graph/src/graph/storage.rs`
  - `crates/rmc-graph/src/graph/snapshot.rs` if cleanup needs snapshot
    semantics
  - `crates/rmc-graph/src/graph/mod.rs`
  - `crates/rmc-server/src/tools/endpoints/cache.rs`
- Deleted files/modules: none.
- New types: optional cleanup result DTO.
- New functions/methods:
  - `clear_workspace_snapshots(...)` or equivalent graph-owned cleanup API
  - optional `workspace_graph_cache_paths(...)` if dry-run/reporting needs path
    disclosure
- Modified functions/methods:
  - server cache endpoint cleanup branch
- Deleted functions/methods: server graph-path cleanup helper logic.
- Production LOC change: `+80..200`, with `-40..120` server simplification.
- Test/docs LOC change: `+60..160`.

### Phase 9 Inventory

- New dirs/files/modules: none expected.
- Modified files/modules:
  - `crates/rmc-server/src/lib.rs`
  - `crates/rmc-server/src/tools/mod.rs`
  - `crates/rmc-server/src/tools/project_paths.rs`
  - `crates/rmc-server/src/tools/endpoints/indexing_support.rs`
  - any server endpoint left with duplicate helper logic after phases 2-8
  - `crates/rust-code-mcp/tests/test_mcp_stdio_transport.rs` or other
    compatibility import callers, if present
- Deleted files/modules:
  - delete `crates/rmc-server/src/tools/project_paths.rs` only after all callers
    move to `rmc_server::mcp::project_paths`
  - do not delete or privatize `semantic` until symbol-level expectations move
- New types/methods/functions: none expected.
- Modified functions/methods:
  - duplicate `data_dir` wrappers
  - endpoint helpers left after facade migration
- Deleted functions/methods: duplicate server helpers only.
- Production LOC change: `-50..220` net.
- Test/docs LOC change: `-20..120`.

### Phase 10 Inventory

- New dirs/files/modules: none expected.
- Modified files/modules:
  - `crates/rmc-engine/src/search/mod.rs`
  - `crates/rmc-engine/src/vector_store/mod.rs`
  - `crates/rmc-engine/src/parser/mod.rs`
  - `crates/rmc-engine/src/embeddings/mod.rs`
  - production consumers that still use deep engine paths
  - engine README or module docs for boundary types
- Deleted files/modules: none; change visibility only after consumer migration.
- New types/methods/functions: none expected.
- Modified types/methods/functions:
  - module visibility/reexport declarations
  - docs for `EmbeddingBackend` and `EmbeddingProfile`
- Deleted types/methods/functions: none expected.
- Production LOC change: `-20..120` net.
- Test/docs LOC change: `+40..140`.

### Phase 11 Inventory

- New dirs/files/modules: none expected.
- Modified files/modules:
  - `crates/rmc-indexing/src/indexing/mod.rs`
  - implementation modules whose visibility changes:
    `tantivy_adapter`, `identity`, `merkle`, `retry`, `consistency`,
    `indexer_core`
  - tests/benches/tools still importing implementation modules
- Deleted files/modules: none expected.
- New types/methods/functions: none expected.
- Modified types/methods/functions:
  - visibility/reexport declarations
  - docs for official indexing facades
- Deleted types/methods/functions: none expected; do not delete
  `IncrementalIndexer`.
- Production LOC change: `-20..160` net.
- Test/docs LOC change: `+80..260`.

### Phase 12 Inventory

- New dirs/files/modules: none expected.
- Modified files/modules:
  - `crates/rmc-graph/src/graph/mod.rs`
  - `crates/rmc-graph/src/graph/query/mod.rs`
  - graph facade modules added in phases 5-8
  - tests/debug binaries/examples importing graph internals
- Deleted files/modules: none expected.
- New types/methods/functions: none expected.
- Modified types/methods/functions:
  - visibility/reexport declarations
  - compatibility docs on remaining broad exports
- Deleted types/methods/functions: no behavior deletion expected; remove
  reexports only after MCP evidence allows it.
- Production LOC change: `-40..180` net.
- Test/docs LOC change: `+120..320`.

### Phase 13 Inventory

- New dirs/files/modules: none expected.
- Modified files:
  - `.docs/boundries-cleanup-progress.md`
  - boundary reports if final verification needs a short appendix
- Deleted files/modules: none.
- New modules/types/methods/functions: none.
- Modified modules/types/methods/functions: none.
- Deleted modules/types/methods/functions: none.
- Production LOC change: `0`.
- Test/docs LOC change: `+120..300`.

## Execution Ledger

Create or update this file while executing the plan:

```text
.docs/boundries-cleanup-progress.md
```

For every phase, record:

- phase status
- `jj show --summary` output
- MCP tools used
- files changed
- verification command/result
- commit id
- remaining follow-up

## Phase 0: Baseline And Safety Checks

### Goal

Confirm the current dependency graph, public surfaces, and working-copy state
before making code changes.

### Steps

1. Run `jj show --summary`.
2. Run `jj status`.
3. Refresh or reuse the hypergraph:

```text
build_hypergraph(directory, force_rebuild=false)
workspace_stats(directory)
crate_edges(directory, summary=true, limit=300)
crate_dependency_metric(directory, sort_by="instability", limit=300)
```

4. Run the layering rule check:

```text
forbidden_dependency_check(
  directory,
  rules=[
    { consumer: "rmc_engine", producer: "rmc_*", severity: "error" },
    { consumer: "rmc_graph", producer: "rmc_server", severity: "error" },
    { consumer: "rmc_graph", producer: "rmc_indexing", severity: "warn" },
    { consumer: "rmc_indexing", producer: "rmc_server", severity: "error" },
    { consumer: "rmc_indexing", producer: "rmc_graph", severity: "warn" }
  ],
  summary=false,
  limit=300
)
```

5. Record baseline in `.docs/boundries-cleanup-progress.md`.
6. Commit:

```text
jj commit -m "docs: start boundaries cleanup ledger"
```

### Success Criteria

- Baseline is recorded.
- No implementation edits yet.
- Known dependency direction remains unchanged.

## Phase 1: Workspace Boundary Guardrails

### Goal

Make the intended crate layering explicit before larger refactors start.

### Steps

1. Run `jj show --summary`.
2. Source-read the existing architecture/audit test locations identified by:

```text
get_imports(directory, module="rmc_server", summary=true, limit=300)
module_dependencies(directory, module="rmc_server", summary=true, limit=300)
forbidden_dependency_check(...same rules as Phase 0...)
```

3. Add a documented boundary rule set in the most local existing place. Prefer
   an existing architecture/audit test if one exists. If not, add a small
   documentation-only rule section first rather than inventing a new framework.
4. Record the exact expected dependency direction:

```text
rmc_server   -> rmc_graph, rmc_indexing, rmc_engine, rmc_config
rmc_graph    -> rmc_engine
rmc_indexing -> rmc_engine, rmc_config
rmc_engine   -> no rmc_* dependencies
```

5. Verify with the MCP forbidden dependency check.
6. If a Rust test/check is added, run only the focused check through the nix
   dev shell.
7. Update the ledger and commit:

```text
jj commit -m "docs: document crate boundary guardrails"
```

### Success Criteria

- Intended layering is written down in-repo.
- The rule set can be checked with MCP tools.
- No broad code movement yet.

## Phase 2: `rmc-indexing` Search Facade

### Goal

Remove the need for server query/codemap code to open `TantivyAdapter`
directly.

Decision: add an indexing-owned search facade and keep `TantivyAdapter` public
for compatibility during the migration.

### Boundary Problem

`rmc_server::tools::endpoints::query` and
`rmc_server::tools::graph::codemap` import
`rmc_indexing::indexing::tantivy_adapter` directly. That makes a concrete
indexing adapter part of the server contract.

### MCP Evidence To Refresh

```text
who_imports(directory, item="rmc_indexing::indexing::tantivy_adapter", limit=200)
get_imports(directory, module="rmc_server::tools::endpoints::query", summary=false, limit=300)
get_imports(directory, module="rmc_server::tools::graph::codemap", summary=false, limit=300)
get_exports(directory, module="rmc_indexing::indexing", consumer="rmc_server", summary=false, limit=300)
```

### Steps

1. Run `jj show --summary`.
2. Add a narrow indexing API for BM25/search opening. Prefer a small function
   or method under `rmc_indexing::indexing::unified` or a new facade module
   under `rmc_indexing::indexing`.
3. Keep `TantivyAdapter` implementation ownership inside indexing.
4. Migrate server query code to the new indexing facade.
5. Migrate graph codemap server code to the same facade.
6. Leave `TantivyAdapter` public for compatibility in this phase.
7. Verify MCP imports no longer show server production modules importing
   `tantivy_adapter`.
8. Run focused checks through the nix dev shell if code changed.
9. Update the ledger and commit:

```text
jj commit -m "refactor: add indexing search facade"
```

### Success Criteria

- Server no longer opens `TantivyAdapter` directly in production query/codemap
  paths.
- Indexing owns concrete Tantivy adapter construction.
- Existing behavior remains unchanged.

## Phase 3: `rmc-indexing` Incremental Indexing Facade

### Goal

Add a server-facing indexing service API while keeping `IncrementalIndexer`
public for compatibility.

Decision: do not make `IncrementalIndexer` private in the near term. It is used
by server production code, tests, benches, and standalone tools. The cleanup is
to stop server from constructing it directly, not to remove it as a public
symbol immediately.

### Boundary Problem

`rmc_server::tools::endpoints::index` and `rmc_server::mcp::sync` construct
`IncrementalIndexer` directly.

### MCP Evidence To Refresh

```text
who_imports(directory, item="rmc_indexing::indexing::incremental::IncrementalIndexer", limit=200)
get_imports(directory, module="rmc_server::tools::endpoints::index", summary=false, limit=300)
get_imports(directory, module="rmc_server::mcp::sync", summary=false, limit=300)
functions_with_filter(directory, krate="rmc_indexing", has_param_type="IncrementalIndexer", summary=true, limit=100)
```

### Steps

1. Run `jj show --summary`.
2. Add a smaller indexing-owned service function for server sync/index flows.
3. Prefer a facade that accepts directory/backend/options and owns
   Merkle/change detection internally.
4. Migrate server index endpoint to the facade.
5. Migrate `SyncManager` to the facade.
6. Keep `IncrementalIndexer` and its current reexport for compatibility.
7. Verify direct production server imports of `incremental` are gone or
   intentionally documented.
8. Run focused checks through the nix dev shell.
9. Update the ledger and commit:

```text
jj commit -m "refactor: clarify incremental indexing boundary"
```

### Success Criteria

- Server does not need to know incremental indexing construction details.
- `IncrementalIndexer` remains public compatibility API while server uses a
  narrower indexing-owned service API.
- Indexing still owns Merkle/incremental state.

## Phase 4: Project Path And Identity Boundary

### Goal

Move or centralize project/index identity logic so server does not own mixed
engine/indexing path policy.

### Boundary Problem

`rmc_server::mcp::project_paths` combines server data directories, engine
embedding backend/profile identity, indexing identity, snapshot paths, vector
collection names, and indexed profile discovery.

### MCP Evidence To Refresh

```text
who_imports(directory, item="rmc_server::mcp::project_paths::ProjectPaths", limit=300)
functions_with_filter(directory, krate="rmc_server", has_param_type="ProjectPaths", summary=true, limit=100)
semantic_overlaps(directory, crate_name="rmc_server", item_kind="Function", summary=true, max_pairs=60)
get_imports(directory, module="rmc_server::mcp::project_paths", summary=false, limit=300)
get_imports(directory, module="rmc_indexing::indexing::identity", summary=false, limit=300)
```

### Steps

1. Run `jj show --summary`.
2. Split responsibilities on paper in the ledger before editing:
   - server data/root discovery
   - embedding backend/profile identity
   - indexing identity
   - snapshot path derivation
   - vector collection naming
3. Move indexing-owned identity/path helpers down into `rmc_indexing`.
   Escalate to a small shared support API only if implementation evidence shows
   indexing is not the right owner.
4. Keep `rmc_server::mcp::project_paths::ProjectPaths` as a compatibility
   wrapper initially.
5. Consolidate duplicate `data_dir` helpers.
6. Consolidate backend-resolution variants used by index, query, graph
   similarity, and project paths.
7. Verify with semantic overlap and import checks that duplicate helper
   clusters shrink.
8. Run focused checks through the nix dev shell.
9. Update the ledger and commit:

```text
jj commit -m "refactor: centralize project indexing identity"
```

### Success Criteria

- Indexing/path identity policy has one owner.
- Server keeps only MCP-facing path orchestration.
- Duplicate `data_dir` and backend resolver logic is reduced.

## Phase 5: `rmc-graph` Query And Response Facade

### Goal

Give server graph tools a narrower API so response code does not depend on raw
graph model/storage/snapshot internals.

Decision: graph should expose graph-owned DTOs/query results, not MCP
`CallToolResult` or server-specific response types. Server remains responsible
for wrapping graph DTOs into MCP responses.

### Boundary Problem

Server graph modules import graph snapshot, storage, model, query-model, ids,
labels, and response-enrichment internals directly.

### MCP Evidence To Refresh

```text
get_imports(directory, module="rmc_server::tools::graph::response", summary=false, limit=500)
get_imports(directory, module="rmc_server::tools::graph::core", summary=false, limit=500)
get_imports(directory, module="rmc_server::tools::graph::surface", summary=false, limit=500)
functions_with_filter(directory, krate="rmc_server", has_param_type="OpenedSnapshot", summary=true, limit=200)
get_exports(directory, module="rmc_graph::graph", consumer="rmc_server", summary=false, limit=500)
```

### Steps

1. Run `jj show --summary`.
2. Identify the server response helpers that only translate graph internals to
   MCP DTOs.
3. Add graph-owned query/DTO helpers for the most repeated response enrichment
   paths first.
4. Keep server DTO shapes stable unless an output change is deliberately
   planned.
5. Migrate server `graph::response`, `graph::core`, and `graph::surface` call
   sites incrementally.
6. Verify fewer server functions accept `OpenedSnapshot` directly.
7. Keep existing graph exports for compatibility in this phase.
8. Run focused checks through the nix dev shell.
9. Update the ledger and commit:

```text
jj commit -m "refactor: add graph server query facade"
```

### Success Criteria

- Server graph response code depends on graph-owned query/DTO APIs instead of
  raw model/storage internals where practical.
- `OpenedSnapshot` use in server is reduced.
- Graph remains independent of server-specific MCP types.

## Phase 6: `rmc-graph` Audit Facade

### Goal

Move graph audit orchestration behind graph-owned functions.

Decision: graph owns audit loading, snapshot access, audit dispatch, and graph
DTO construction. Server only parses MCP parameters and wraps the result.

### Boundary Problem

Server audit tools call `loader::load` and individual graph audit modules
directly.

### MCP Evidence To Refresh

```text
who_imports(directory, item="rmc_graph::graph::loader::load", limit=300)
get_imports(directory, module="rmc_server::tools::graph::audits", summary=false, limit=500)
get_exports(directory, module="rmc_graph::graph", consumer="rmc_server", summary=false, limit=500)
```

### Steps

1. Run `jj show --summary`.
2. Add graph-owned audit entry points that accept directory/options and own:
   - loading
   - snapshot access
   - audit module dispatch
   - graph DTO construction
3. Migrate server audit tools to call those entry points.
4. Keep server responsible only for MCP parameter parsing and result wrapping.
5. Verify production server imports of `loader::load` and individual audit
   internals are reduced or removed.
6. Run focused checks through the nix dev shell.
7. Update the ledger and commit:

```text
jj commit -m "refactor: add graph audit facade"
```

### Success Criteria

- Server does not manually orchestrate graph audit internals.
- Graph owns graph-specific audit loading and dispatch.
- No dependency from graph to server is introduced.

## Phase 7: `rmc-graph` Similarity Facade

### Goal

Hide graph semantic-overlap implementation helpers behind a graph-level
similarity API.

Decision: graph owns embedding-cache and cosine mechanics. Server asks graph
for similarity results rather than coordinating `ensure_embeddings_for` and
`cosine` directly.

### Boundary Problem

Server similarity code coordinates helpers such as embedding cache maintenance
and cosine math.

### MCP Evidence To Refresh

```text
get_imports(directory, module="rmc_server::tools::graph::similarity", summary=false, limit=500)
who_imports(directory, item="rmc_graph::graph::embedding_cache::ensure_embeddings_for", limit=300)
who_imports(directory, item="rmc_graph::graph::math::cosine", limit=300)
semantic_overlaps(directory, crate_name="rmc_graph", item_kind="Function", summary=true, max_pairs=40)
```

### Steps

1. Run `jj show --summary`.
2. Add a graph-owned similarity operation that accepts the necessary query
   options and returns server-usable DTOs or graph DTOs.
3. Keep embedding cache and cosine implementation details inside graph.
4. Migrate server similarity tools to the facade.
5. Verify server production imports no longer reach into graph
   `embedding_cache` or `math`.
6. Run focused checks through the nix dev shell.
7. Update the ledger and commit:

```text
jj commit -m "refactor: add graph similarity facade"
```

### Success Criteria

- Server asks graph for similarity results.
- Graph owns embedding-cache and scoring mechanics.
- Public low-level helper use is reduced.

## Phase 8: `rmc-graph` Storage Cleanup Facade

### Goal

Stop server cache endpoints from depending on graph storage layout.

Decision: graph owns graph storage layout and cache/snapshot cleanup. Server
should not construct or interpret `GraphPaths` except through compatibility
paths during migration.

### Boundary Problem

Server cache code uses graph storage path details through `GraphPaths`.

### MCP Evidence To Refresh

```text
who_imports(directory, item="rmc_graph::graph::GraphPaths", limit=300)
get_imports(directory, module="rmc_server::tools::endpoints::cache", summary=false, limit=300)
functions_with_filter(directory, krate="rmc_graph", has_param_type="GraphPaths", summary=true, limit=100)
```

### Steps

1. Run `jj show --summary`.
2. Add a graph-owned cache/snapshot cleanup API.
3. Migrate server cache endpoint to call the graph API instead of constructing
   or interpreting graph storage paths directly.
4. Verify `GraphPaths` use is concentrated in graph snapshot/storage modules.
5. Run focused checks through the nix dev shell.
6. Update the ledger and commit:

```text
jj commit -m "refactor: encapsulate graph storage cleanup"
```

### Success Criteria

- Graph storage layout decisions stay in graph.
- Server cache endpoint remains MCP-facing only.

## Phase 9: `rmc-server` Internal Boundary Cleanup

### Goal

Make server a thinner MCP adapter after graph/indexing facades exist.

### Boundary Problem

Server is correctly the top layer, but endpoint modules still own too much
lower-layer coordination and have duplicate helper logic.

### MCP Evidence To Refresh

```text
module_tree(directory, krate="rmc_server", depth=4)
functions_with_filter(directory, krate="rmc_server", returns_type_pattern="CallToolResult", summary=true, limit=300)
semantic_overlaps(directory, crate_name="rmc_server", item_kind="Function", summary=true, max_pairs=80)
get_exports(directory, module="rmc_server", consumer="rmc_server", summary=false, limit=500)
get_declared_reexports(directory, module="rmc_server", summary=false, limit=500)
```

### Steps

1. Run `jj show --summary`.
2. Keep `tools::router` thin. Do not move business logic into router methods.
3. Remove server-side helper duplication left after phases 2-8.
4. Keep `tools::params` as an internal `pub(crate)` parameter facade.
5. Treat `semantic` privacy as a late cleanup. Before changing it, verify graph
   tests and symbol-level expectations that mention `rmc_server::semantic`.
6. Migrate the remaining `tools::project_paths` compatibility import to the
   canonical path, then remove the compatibility reexport if no callers remain.
7. Verify server public exports still include the intended facade:
   - `SearchToolRouter`
   - `SearchTool`
   - `SyncManager`
   - `index_codebase`
   - `IndexCodebaseParams`
8. Run focused checks through the nix dev shell.
9. Update the ledger and commit:

```text
jj commit -m "refactor: tighten server internal boundaries"
```

### Success Criteria

- Router remains thin.
- Server public surface is intentional.
- Server helper duplication is reduced.
- `semantic` remains public unless all source/test expectations are migrated.
- `tools::project_paths` is removed only after compatibility callers move.
- No lower crate depends on server.

## Phase 10: `rmc-engine` Public Surface Tightening

### Goal

Reduce public implementation-module exposure in the foundation crate without
breaking active consumers.

### Boundary Problem

`rmc_engine` is clean directionally, but some implementation modules are public
alongside facade exports.

### MCP Evidence To Refresh

```text
get_exports(directory, module="rmc_engine::search", consumer="rmc_server", summary=false, limit=300)
get_exports(directory, module="rmc_engine::vector_store", consumer="rmc_server", summary=false, limit=300)
who_imports(directory, item="rmc_engine::search::bm25", limit=300)
who_imports(directory, item="rmc_engine::search::resilient", limit=300)
who_imports(directory, item="rmc_engine::vector_store::lancedb", limit=300)
who_imports(directory, item="rmc_engine::vector_store::traits", limit=300)
get_exports(directory, module="rmc_engine::embeddings", consumer="rmc_server", summary=false, limit=300)
```

### Steps

1. Run `jj show --summary`.
2. Confirm active consumers of:
   - `search::bm25`
   - `search::resilient`
   - `search::rrf_tuner`
   - `vector_store::lancedb`
   - `vector_store::traits`
   - parser helper modules
3. If production consumers can use facade reexports, migrate those imports.
4. Only after consumers are migrated, consider making implementation modules
   private or `pub(crate)`.
5. Do not move `EmbeddingProfile`; leave it in engine and document it as a
   deliberate engine-owned embedding config model.
6. Do not change embedding backend semantics in this phase.
7. Document `EmbeddingBackend` as a formal cross-crate boundary type.
8. Run focused checks through the nix dev shell.
9. Update the ledger and commit:

```text
jj commit -m "refactor: tighten engine public modules"
```

### Success Criteria

- Engine remains the lowest-level primitive crate.
- Consumers prefer one-level facades.
- Any remaining public implementation modules are intentionally public.

## Phase 11: `rmc-indexing` Visibility Tightening

### Goal

After server migration, reduce public indexing implementation modules.

### Boundary Problem

`rmc_indexing::indexing` exposes implementation modules that may no longer
need to be public once server uses the new facades.

### MCP Evidence To Refresh

```text
get_exports(directory, module="rmc_indexing::indexing", consumer="rmc_server", summary=false, limit=500)
who_imports(directory, item="rmc_indexing::indexing::tantivy_adapter", limit=300)
who_imports(directory, item="rmc_indexing::indexing::identity", limit=300)
who_imports(directory, item="rmc_indexing::indexing::merkle", limit=300)
who_imports(directory, item="rmc_indexing::indexing::retry", limit=300)
who_imports(directory, item="rmc_indexing::indexing::consistency", limit=300)
who_imports(directory, item="rmc_indexing::indexing::indexer_core", limit=300)
```

### Steps

1. Run `jj show --summary`.
2. Review each public implementation module:
   - `tantivy_adapter`
   - `identity`
   - `merkle`
   - `retry`
   - `consistency`
   - `indexer_core`
3. For modules with no production external consumers, make them `pub(crate)` or
   private.
4. Keep official facades public:
   - `UnifiedIndexer`
   - `IndexStats`
   - `IndexFileResult`
   - `IncrementalIndexer` compatibility reexport
   - server-facing incremental service facade added in Phase 3
5. Review `metadata_cache`, `metrics`, `monitoring`, and `security` for
   intentional public API status.
6. Do not make `IncrementalIndexer` private in this phase unless a separate
   evidence pass proves tests, benches, standalone tools, and server callers
   have all moved to replacement APIs.
7. Run focused checks through the nix dev shell.
8. Update the ledger and commit:

```text
jj commit -m "refactor: tighten indexing public modules"
```

### Success Criteria

- Public indexing API is facade-oriented.
- Implementation modules are not public only because server used to reach them.
- `IncrementalIndexer` remains compatible unless deliberately retired in a
  later plan.

## Phase 12: `rmc-graph` Visibility Tightening

### Goal

After server migration, reduce public graph implementation modules while
preserving compatibility where needed.

### Boundary Problem

`rmc_graph::graph` exposes facade types and implementation modules together.

### MCP Evidence To Refresh

```text
get_exports(directory, module="rmc_graph::graph", consumer="rmc_server", summary=false, limit=700)
get_declared_reexports(directory, module="rmc_graph::graph", summary=false, limit=700)
who_imports(directory, item="rmc_graph::graph::loader", limit=300)
who_imports(directory, item="rmc_graph::graph::storage", limit=300)
who_imports(directory, item="rmc_graph::graph::model", limit=300)
who_imports(directory, item="rmc_graph::graph::ids", limit=300)
who_imports(directory, item="rmc_graph::graph::bindings", limit=300)
who_imports(directory, item="rmc_graph::graph::usages", limit=300)
```

### Steps

1. Run `jj show --summary`.
2. Treat `rmc_graph::graph` as a compatibility facade.
3. Keep stable public groups visible:
   - snapshot open/build/publish APIs that are truly external
   - query DTOs
   - explicit audit/query/similarity facades added in earlier phases
4. Make implementation modules private or `pub(crate)` only when MCP evidence
   shows external production callers no longer depend on them.
5. Keep debug binaries/examples/tests working either through dev-only paths or
   the new facades.
6. Keep compatibility reexports where tests, debug binaries, standalone tools,
   or documented external workflows still rely on them.
7. Run focused checks through the nix dev shell.
8. Update the ledger and commit:

```text
jj commit -m "refactor: tighten graph public modules"
```

### Success Criteria

- `rmc_graph::graph` no longer exposes avoidable implementation modules to
  production consumers.
- Server uses graph-owned facades.
- Remaining broad graph exports are explicitly compatibility exports.
- Graph still has no dependency on server or indexing.

## Phase 13: Final Architecture Verification

### Goal

Confirm that boundary cleanup improved the architecture without changing the
intended dependency direction.

### Steps

1. Run `jj show --summary`.
2. Rebuild/reuse the hypergraph:

```text
build_hypergraph(directory, force_rebuild=true)
workspace_stats(directory)
crate_edges(directory, summary=false, limit=500)
crate_dependency_metric(directory, sort_by="instability", limit=300)
forbidden_dependency_check(...same rules as Phase 0...)
```

3. Refresh public-surface checks:

```text
get_exports(directory, module="rmc_engine", consumer="rmc_engine", summary=true, limit=300)
get_exports(directory, module="rmc_graph::graph", consumer="rmc_server", summary=true, limit=700)
get_exports(directory, module="rmc_indexing::indexing", consumer="rmc_server", summary=true, limit=500)
get_exports(directory, module="rmc_server", consumer="rmc_server", summary=true, limit=500)
```

4. Refresh server deep-import checks:

```text
get_imports(directory, module="rmc_server::tools::graph", summary=false, limit=700)
get_imports(directory, module="rmc_server::tools::endpoints::query", summary=false, limit=500)
get_imports(directory, module="rmc_server::mcp::sync", summary=false, limit=300)
```

5. Refresh semantic overlap checks:

```text
semantic_overlaps(directory, crate_name="rmc_server", item_kind="Function", summary=true, max_pairs=80)
semantic_overlaps(directory, crate_name="rmc_graph", item_kind="Function", summary=true, max_pairs=60)
semantic_overlaps(directory, crate_name="rmc_indexing", item_kind="Function", summary=true, max_pairs=60)
semantic_overlaps(directory, crate_name="rmc_engine", item_kind="Function", summary=true, max_pairs=60)
```

6. Run the focused test/check suite through the nix dev shell. Do not run
   formatting.
7. Update the ledger with final before/after results.
8. Commit:

```text
jj commit -m "docs: record final boundaries cleanup verification"
```

### Success Criteria

- Forbidden dependency check has no new violations.
- Server imports fewer graph/indexing internals.
- Public implementation modules are reduced or intentionally documented.
- Boundary reports/progress notes explain any remaining exceptions.

## Recommended Execution Order

Use this order unless evidence during a phase shows a safer local sequence:

1. Phase 0: Baseline and safety checks.
2. Phase 1: Workspace boundary guardrails.
3. Phase 2: Indexing search facade.
4. Phase 3: Incremental indexing facade.
5. Phase 4: Project path and identity boundary.
6. Phase 5: Graph query and response facade.
7. Phase 6: Graph audit facade.
8. Phase 7: Graph similarity facade.
9. Phase 8: Graph storage cleanup facade.
10. Phase 9: Server internal boundary cleanup.
11. Phase 10: Engine public surface tightening.
12. Phase 11: Indexing visibility tightening.
13. Phase 12: Graph visibility tightening.
14. Phase 13: Final architecture verification.

## Non-Goals

- Do not rename crates.
- Do not move `rmc_server` logic into lower crates if it is truly MCP
  request/response behavior.
- Do not make graph depend on server.
- Do not make indexing depend on graph unless a later explicit design decision
  changes the architecture.
- Do not combine `rmc_graph` and `rmc_indexing`.
- Do not rewrite all DTOs at once.
- Do not use formatting-only commits.

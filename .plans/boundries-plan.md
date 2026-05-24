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
- Before each phase commit, run `jj status` and confirm the working copy only
  contains intentional changes for that phase. If unrelated dirty work exists,
  split the work or stop and record the blocker; do not sweep unrelated changes
  into the phase commit.
- After each phase, update the progress notes and commit with `jj commit -m`.
- Do not run `cargo fmt` or any formatting command.
- Use MCP tools as the primary evidence source before changing a boundary.
- Use `module_dependencies` whenever fully qualified inline paths may carry a
  dependency. `get_imports` and `who_imports` are not sufficient by themselves
  for verification because they can miss inline references such as
  `rmc_graph::graph::cosine(...)`.
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
  - optional graph helper that scores already-resolved graph items without
    opening server/indexing search paths
- Modified functions/methods:
  - server `semantic_overlaps` graph tool implementation
  - server `similar_to_item` graph tool only if it can call a lower-level graph
    helper without moving server path/search policy into graph
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

## Execution Status

### Phase 0: Baseline And Safety Checks

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `bf2bb57e4a7066f9e2e70b68ac79ee6ac3d637bf` on change
  `uozpxtlmwxvypwqszkrprvsswspumypx`, with no description set.
- Step 2 `jj status`: completed. Pre-step `jj show --summary` reported
  working-copy commit `da52dacd54fb343c6d6a3aaa8ddeeddd7438f225` on change
  `qkxqzrmnqstknxpkpzuzqqprsntqtunx`. `jj status` reported no changes.
- Step 3 refresh/reuse hypergraph: completed. Pre-step `jj show --summary`
  reported working-copy commit `b86e39145d78da9ab0b35d5d0efea457a4acf92c`
  on change `lvsvwnwlkutqnuvmkponnkpprvvsomsn`. MCP
  `build_hypergraph(force_rebuild=false)` reused graph
  `4fc200b6ab2a6d0ef4162f4fec31da5f` with fingerprint
  `a2800cb435de19d32f27bf58901fd5efb037e85565033279dd50611589501073`,
  3040 nodes, 5371 bindings, and 7963 usages. `workspace_stats` reported
  45 crates, 296 modules, 2448 items, 250 external symbols, and
  `pub_crate_share=0.46781789638932497`. `crate_edges` returned 49 edges.
  `crate_dependency_metric(sort_by="instability")` returned 45 crate metrics;
  core crate instability values were `rmc_server=0.4`, `rmc_config=0.25`,
  `rmc_indexing=0.125`, `rmc_graph=0.08333333333333333`, and
  `rmc_engine=0.06666666666666667`.
- Step 4 layering rule check: completed. Pre-step `jj show --summary`
  reported working-copy commit `e7aa57387d7ef146bed8478a8837c866b92493e9`
  on change `rltttsuqztlllnsluvsyvzmnmswskwss`. MCP
  `forbidden_dependency_check` ran the five planned rules with
  `summary=false` and `limit=300`; result was `violation_count=0` and
  `total_match_count=0`.
- Step 5 record baseline in `.docs/boundries-cleanup-progress.md`: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `2abedcedcc47cde3f99bf21a6febde4f88373b7b` on change
  `tkswovouxtwnrkspxutmpumotmlkqmnz`. Created the progress ledger with the
  Phase 0 VCS, hypergraph, workspace stats, crate edge, dependency metric, and
  forbidden-dependency baseline.
- Step 6 phase ledger commit: completed. Pre-step `jj show --summary`
  reported working-copy commit `a9651239cb0298e4001d4d04e97f3df30b2f2c1f`
  on change `sxrtnnxswovmzktvvwoxzupyzlwuwuly`. The Phase 0 baseline ledger
  commit is `e4aeefdeac6b3e4dce3041158fdc681d564dc1ce`
  (`docs: record phase 0 baseline`).
- Phase completion report: completed. Pre-step `jj show --summary` reported
  working-copy commit `c7ce5feb04db0e8b05d824e61b6a078c3bdf6d7e` on change
  `nrvkrlonwkmkyqpqvqttwzmtpulyptqr`. Wrote
  `.docs/phase-0-boundrie-fix-report.md` and marked the Phase 0 progress
  ledger complete.

### Phase 1: Workspace Boundary Rules

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `ce5e84a39da64908d800cff4cf51aaa79fa7fb8c` on change
  `pskqvuyvmmnpltszoqrwtupkvkkowuwo`, with no description set.
- Step 2 source-read architecture/audit locations from MCP evidence:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `e9e69ce0e33c85c99debb1341b939344a8728455` on change
  `nnowvmrvvvmunoktxvowswutmqlruokx`. MCP `get_imports` and
  `module_dependencies` for `rmc_server` returned zero root-module matches;
  `forbidden_dependency_check` returned zero violations. Source reads covered
  `.docs/architectural-rules.md`, `crates/rmc-graph/src/graph/query/tests.rs`
  around the generic forbidden-dependency tests,
  `crates/rmc-graph/src/graph/query/model.rs` for rule shape, and
  `crates/rmc-graph/src/graph/query/crates.rs` for rule semantics. Existing
  docs are stale Phase B/Phase C wording; existing tests validate the generic
  check engine, not the current workspace rule set.
- Step 3 add repeatable or documented boundary rule set: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `de436c0ef016f208ae059a2e512b9ea34987bdcd` on change
  `xywskktvkkwvkywrnyxzsuwxwqnxnpqu`. Updated
  `.docs/architectural-rules.md` from stale Phase B/Phase C wording to the
  current five-rule crate boundary set, marked documentation-only until CI or a
  repo-local harness exists.
- Step 4 document MCP command/expected result if documentation-only:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `eb7c2f0aa4737f437cf7e94cfa27be831580755c` on change
  `nkyplvkotxlpnrszkkqnsumlukxmxqvy`. Updated
  `.docs/architectural-rules.md` with the exact `build_hypergraph` and
  `forbidden_dependency_check` calls and the expected zero-violation result.
- Step 5 record dependency direction: completed. Pre-step `jj show --summary`
  reported working-copy commit `a5f20835a668708b6d04b426b4806e55ded0cd97`
  on change `zmxqmmqluqpkxvroqtnymtrusxlpmxrt`. Replaced stale Phase C
  hierarchy text in `.docs/architectural-rules.md` with the current dependency
  direction:
  `rmc_server -> rmc_graph, rmc_indexing, rmc_engine, rmc_config`;
  `rmc_graph -> rmc_engine`; `rmc_indexing -> rmc_engine, rmc_config`;
  `rmc_config -> rmc_engine`; `rmc_engine -> no rmc_* dependencies`.
- Step 6 verify forbidden dependency check: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `f587295397281439ef951c0286b01a9d16033ff1` on change
  `qpquqlooskzunwwvvxukkkntlxvquvnr`. MCP
  `forbidden_dependency_check` ran the documented five-rule set with
  `summary=false` and `limit=300`; result was `rule_count=5`,
  `violation_count=0`, `total_match_count=0`, and `returned_match_count=0`.
- Step 7 focused nix check if a Rust test/check is added: completed as not
  required. Pre-step `jj show --summary` reported working-copy commit
  `5886379b36dec1c556d5b35f0331a153b60d3e96` on change
  `kulsqlwqynuwtnnrlypvklnrmnvoozpn`. Phase 1 added documentation-only
  boundary rules and did not add or change a Rust test/check, so no nix build or
  check command was required.
- Step 8 update ledger and commit: completed. Pre-step `jj show --summary`
  reported working-copy commit `cad14ff665f1b64cd1a18395970a6242365130ae`
  on change `lvwtntvqmtozyvpoolxyqzzuupnlqknl`. Updated
  `.docs/boundries-cleanup-progress.md` with the Phase 1 step evidence,
  verification, changed files, remaining follow-up, and commit ledger.
- Phase completion report: completed. Pre-step `jj show --summary` reported
  working-copy commit `1b1fb1a4068b5b2e9f400d9ff71cfaeb7bf850d8` on change
  `wqruqolzxulpkwsyvymqswztmrzovlop`. Wrote
  `.docs/phase-1-boundrie-fix-report.md` and marked the Phase 1 progress
  ledger complete.

### Phase 2: `rmc-indexing` Search Facade

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `fca77ee055ae15c0176a62da9d84654bbc0beb7b` on change
  `vpzltotxvvrvnosvqzsytlpnwoklzupw`, with no description set.
- Step 2 add narrow indexing API for BM25/search opening: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `7f3a08365114f8cddf7a3b8b01ee41b7fe057e25` on change
  `ysuwplquvvkqwyptnskkxlqmzymykvkw`. MCP evidence showed
  `rmc_server::tools::endpoints::query` and
  `rmc_server::tools::graph::codemap` both depend on
  `rmc_indexing::indexing::tantivy_adapter` through inline references.
  Added `rmc_indexing::indexing::search::open_bm25_search` and reexported it
  from `rmc_indexing::indexing`.
- Step 3 keep `TantivyAdapter` ownership inside indexing: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `826a427bd20ff885143d396195828ca36321d25e` on change
  `xvtmnykxqyvylnvynkktokluvtwmkqut`. Verified
  `TantivyAdapter::new` remains used inside `rmc_indexing::indexing::search`
  and `rmc_indexing::indexing::unified`, while `TantivyAdapter` remains a
  public compatibility reexport.
- Step 4 migrate server query code: completed. Pre-step `jj show --summary`
  reported working-copy commit `21f9c6e315ce37e8daf902f72316778732fb576e`
  on change `quoqqmqpumlzytqmpkwxsnluuutxltxl`. Updated
  `rmc_server::tools::endpoints::query::try_open_bm25` to call
  `rmc_indexing::indexing::open_bm25_search` instead of constructing
  `TantivyConfig` and `TantivyAdapter` directly.
- Step 5 migrate graph codemap server code: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `af97b059f19b35161ab72ff31e03f9ca2ea11bbd` on change
  `xqyxwtslpwrvqyykyumqswzzqmwsrwql`. Updated
  `rmc_server::tools::graph::codemap` to call
  `rmc_indexing::indexing::open_bm25_search` instead of constructing
  `TantivyConfig` and `TantivyAdapter` directly.
- Step 6 leave `TantivyAdapter` public for compatibility: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `71cbda4fd1d6e709c24b4702942295dc688a7dcb` on change
  `nnoruxukxwskzmzwotszmzronrklnkvl`. Verified
  `rmc_indexing::indexing::tantivy_adapter` remains `pub mod` and
  `TantivyAdapter` remains a public reexport.
- Step 7 verify server production modules no longer depend on
  `tantivy_adapter`: completed. Pre-step `jj show --summary` reported
  working-copy commit `5b923b94b5bf4227102442c81d7766111c23d9a9` on change
  `rpuomsqkxvovryzpmpslxlnwlkxomsss`. Rebuilt the hypergraph with
  `force_rebuild=true`, producing graph `06c80cff231427cb53c75e7c071397fd`.
  Refreshed `module_dependencies` and `get_imports` for server `query` and
  `codemap`; both now depend on `rmc_indexing::indexing::search` for
  `open_bm25_search`, and neither reports
  `rmc_indexing::indexing::tantivy_adapter`.
- Step 8 run focused nix checks if code changed: completed with external
  toolchain failure. Pre-step `jj show --summary` reported working-copy commit
  `a30f01c4c2f463ca12c0ef66f165c5fc8436538f` on change
  `ymxovsnvyuzuoolovssznxttulpkqkly`. Ran
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server`;
  it failed before checking the touched crates because `candle-kernels v0.10.2`
  hit a CUDA/GCC internal compiler error in `cc1plus` while compiling
  `src/moe/moe_wmma_gguf.cu`, then Cargo did not exit promptly and the cargo
  process was terminated. Retried with
  `nix develop ../nix-devshells#cuda-code --command env CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`;
  it exited with the same `candle-kernels` CUDA/GCC ICE.
- Step 9 update ledger and commit: completed. Pre-step `jj show --summary`
  reported working-copy commit `d2f9b7f18ace12b089248775d918b27097b86ac1`
  on change `nrpymunwwkmwmkzsxourumvqlnzrmoup`. Updated
  `.docs/boundries-cleanup-progress.md` with Phase 2 evidence, verification,
  check result, remaining follow-up, and commit ledger.
- Phase completion report: completed. Pre-step `jj show --summary` reported
  working-copy commit `85ef0c5adf1561983d1de656796d3e956adeb496` on change
  `zkkswxqloywvsptlrwzplxtkqxpvouxr`. Wrote
  `.docs/phase-2-boundrie-fix-report.md` and marked the Phase 2 progress
  ledger complete.
- Post-review correction: completed. Pre-step `jj show --summary` reported
  working-copy commit `b794b50a483091a2f1f0536196c4c04c0dabbad8` on change
  `qzznxxrwznurrnsmxwkzuroxwzrznrpw`. Updated
  `rmc_indexing::indexing::search::open_bm25_search` to open existing Tantivy
  indexes read-only instead of constructing `TantivyAdapter`; migrated the
  health probe to the same facade; added four focused facade tests. Verification
  passed with
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`
  and
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo test -p rmc-indexing open_bm25_search --jobs 1`.
  Commit: `2ae2e365` (`fix: open bm25 search read-only`).

### Phase 3: `rmc-indexing` Incremental Indexing Facade

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `1246ed40d952f65679ea505e67194973d857de67` on change
  `zqwzqttxromrslzpsupuympxunyyqvrq`, with no description set.
- Step 2 add a smaller indexing-owned service function for server sync/index
  flows: completed. Pre-step `jj show --summary` reported working-copy commit
  `cc2120bcf258f176c0a0699a87b8dc1d8ecf94d6` on change
  `nqxrrlqkuzrnvlspsoyxxrqsmsvomroq`. Rebuilt the hypergraph with
  `force_rebuild=true`, producing graph `73fff61394cb3013da54fdacb4324029`.
  MCP evidence confirmed direct production server dependencies on
  `rmc_indexing::indexing::incremental` from `rmc_server::tools::endpoints::index`
  and `rmc_server::mcp::sync`. Added
  `rmc_indexing::indexing::incremental_service` with
  `IncrementalIndexRequest`, `IncrementalIndexOutcome`, and
  `index_project_incrementally`, then reexported the facade from
  `rmc_indexing::indexing`. Verification passed with
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing --jobs 1`.
- Step 3 confirm facade shape owns incremental construction/change detection:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `0152c6a925058b57321b1492e746cf5aa24dbef5` on change
  `pklqnpxkpkrmlnokklzvpxyumkkltmvy`. The service facade accepts the
  codebase path, server-resolved index paths, backend, embedder identity,
  optional snapshot path, and force option, while indexing owns
  `IncrementalIndexer::with_backend`, force-clear execution, and
  `index_with_change_detection`.
- Step 4 migrate server index endpoint to the facade: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `479b445fcd47137579d8163c82a1c708da2e0d11` on change
  `vzrzzmwnoowxuqryywumzotpoqzyntxz`. Updated
  `rmc_server::tools::endpoints::index::index_codebase` to call
  `index_project_incrementally` through `IncrementalIndexRequest` instead of
  constructing `IncrementalIndexer` directly. The server still maps
  `VectorStoreError::VersionMismatch` to the existing actionable MCP error.
  Verification passed with
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-server --jobs 1`.
- Step 5 migrate `SyncManager` to the facade: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `dda24e869997d055f9695b08ca0d8e35ac39a2f4` on change
  `zuqzzsspslnonryqzuktruxpxtkqwxlm`. Updated
  `rmc_server::mcp::sync` to call `index_project_incrementally` for each
  indexed profile instead of constructing `IncrementalIndexer` directly. The
  stored embedder identity from `metadata.json` is still passed through for
  legacy index compatibility. Verification passed with
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-server --jobs 1`.
- Step 6 keep `IncrementalIndexer` public for compatibility: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `ae3f14da1e4348b5fb46115b610861532c401ad3` on change
  `pqltzkurvwyynrwnpxnkmnlrlmlmxuqn`. Verified
  `rmc_indexing::indexing::incremental` remains `pub mod`,
  `IncrementalIndexer` remains a public struct, and
  `rmc_indexing::indexing` still reexports `IncrementalIndexer`.
- Step 7 verify production server dependency on `incremental`: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `5d5ca5eb3e431d1b093b24d4b2c088ddc7dea252` on change
  `uyvppnvykoqwuvquzxtmtpllusxxrtnt`. Rebuilt the hypergraph with
  `force_rebuild=true`, producing graph `b2f982db0f3dcfb48cf162255b8d6696`.
  `module_dependencies` for `rmc_server::tools::endpoints::index` and
  `rmc_server::mcp::sync` now list
  `rmc_indexing::indexing::incremental_service`, not
  `rmc_indexing::indexing::incremental`. `who_imports` for
  `IncrementalIndexer` dropped from 14 to 11 bindings; remaining direct
  importers are compatibility consumers, tests, benches, tools, the public
  reexport, and the indexing-owned service.
- Step 8 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `15406891d014fe215287d928780808286fb44c87` on change
  `xvmqvywzlmwrwmurvnquzuvtolwxqpxy`. Verification passed with
  `nix develop ../nix-devshells#cuda-code --command env CUDAFORGE_THREADS=1 RAYON_NUM_THREADS=1 CARGO_BUILD_JOBS=1 cargo check -p rmc-indexing -p rmc-server --jobs 1`.
- Step 9 update ledger and commit: completed. Pre-step `jj show --summary`
  reported working-copy commit `93f05ca7ba0647954f8055ed3f2d1290a6abbc56`
  on change `ymyrwrtuxnrnoskvtwxmkuqntpmvknnl`. Updated
  `.docs/boundries-cleanup-progress.md` with Phase 3 evidence, verification,
  changed files, remaining follow-up, and commit ledger.
- Phase completion report: completed. Pre-step `jj show --summary` reported
  working-copy commit `494fc714c223d82d77d29f69388bdf814596252d` on change
  `umpvurtuuzqmzkozulwzkpkrwwxmqnxv`. Wrote
  `.docs/phase-3-boundrie-fix-report.md` and marked the Phase 3 progress
  ledger complete.

### Phase 4: Project Path And Identity Boundary

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `6310c735f1fa7e5662a932a85bb1b0bcfff08ac2` on change
  `quywnkvozwmxoypwwmorprwvnzkmxqvk`, with no description set.
- Step 2 split responsibilities on paper before editing: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `f8e17fd39098744e20a0b3d3a81d4e45a73db846` on change
  `vxylvrnxzysozqmnrxplqmllwlyuomll`. Reused graph
  `b2f982db0f3dcfb48cf162255b8d6696`. MCP evidence showed
  `ProjectPaths` has eight import bindings, six query helper functions take
  `ProjectPaths`, and `rmc_server::mcp::project_paths` depends on
  `rmc_indexing::indexing::identity`, `rmc_indexing::indexing::incremental`,
  engine embedding profile/backend APIs, `directories`, and `sha2`.
  `semantic_overlaps` found duplicate server helpers for `data_dir` and
  embedding-backend resolution. Recorded the Phase 4 ownership split:
  server keeps data-root discovery and MCP-facing orchestration; indexing owns
  indexing identity, chunking identity, snapshot derivation, artifact path
  bundles, vector collection naming, and indexed-profile discovery; engine
  keeps embedding profile/backend models.
- Step 3 move indexing-owned identity/path helpers down into `rmc_indexing`:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `8d26a4cd8a5bb97bba499da09a2efae75a54c6fa` on change
  `xluswuuvtvolvqxmwkwzlwquqpxlyuqs`. Added
  `rmc_indexing::indexing::project_paths::{IndexingProjectPaths,
  IndexedProfilePaths}` plus indexing-owned `dir_hash`, `collection_prefix`,
  and vector metadata identity reads. Updated
  `rmc_server::mcp::project_paths::ProjectPaths` to delegate path and
  identity derivation to the indexing facade while preserving the server
  compatibility type and server-owned `data_dir`. Removed the server crate's
  direct `sha2` dependency. After the code commit, rebuilt the hypergraph with
  `force_rebuild=true`, producing graph
  `ce626950ad825420375344f20d145a95`. Refreshed
  `module_dependencies` now shows `rmc_server::mcp::project_paths` depends on
  `rmc_indexing::indexing::project_paths`, not
  `rmc_indexing::indexing::identity`,
  `rmc_indexing::indexing::incremental`, or `sha2`. The regular focused test
  passed with two tests:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server project_paths`.
- Step 4 keep `rmc_server::mcp::project_paths::ProjectPaths` as a
  compatibility wrapper initially: completed. Pre-step `jj show --summary`
  reported working-copy commit `05c745e3e40b51d4440229c2d32aadea4226d6c0`
  on change `rwtpxuxtslyrnpsrqmxnpmvyvwyypoqq`. Source reads confirmed
  `ProjectPaths` and `IndexedProfilePaths` still expose the server-facing
  fields while converting from indexing-owned path DTOs, and
  `rmc_server::tools::project_paths` remains a compatibility reexport of
  `crate::mcp::project_paths::*`. MCP `who_imports(ProjectPaths)` still
  returns eight bindings across production endpoints, tests, and the
  compatibility module; `functions_with_filter(has_param_type="ProjectPaths")`
  still returns the six query helper users. No code change was required.
- Step 5 consolidate duplicate `data_dir` helpers: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `69e7b3d7daf2059e70c1e0bf4766dfa0a8afc309` on change
  `kkpottulqvowxsykxzsokrmwvsnyrmtt`. Removed the
  `rmc_server::tools::endpoints::indexing_support::data_dir` wrapper and had
  `open_or_create_index` / `open_cache` call the canonical
  `crate::mcp::project_paths::data_dir()` helper directly. Refreshed
  hypergraph graph `5f91461896d45246c51e9fa601cd5d90` shows only
  `rmc_server::mcp::project_paths::data_dir` among server PathBuf-returning
  data-root helpers, and `semantic_overlaps` no longer reports the old
  `data_dir` duplicate cluster. The focused server check passed with existing
  warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`.
- Step 6 consolidate backend-resolution variants used by index, query, graph
  similarity, and project paths: completed. Pre-step `jj show --summary`
  reported working-copy commit `54bcf32df81709850cc9f7941a72ecb57bf1bb7c`
  on change `mvqwqpkyuoxumrkotrmvqurkxuuqpqps`. Added one MCP-facing
  `resolve_embedding_backend_for_mcp` helper in
  `rmc_server::mcp::project_paths`, migrated query and graph similarity to it,
  and removed `resolve_requested_backend`, `resolve_graph_tool_backend`, and
  the now-redundant string-returning project-path resolver. The index endpoint
  keeps its small `resolve_backend` wrapper because it owns the legacy `model`
  parameter. Refreshed graph `2c6dfe88c8bad3b7db1838a94b00287b` shows
  server `EmbeddingBackend`-returning helpers reduced to the shared MCP helper
  plus the index legacy-model wrapper. `semantic_overlaps` shows the resolver
  cluster reduced from four endpoint-specific helpers to those two functions.
  The focused resolver tests passed with three tests:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server resolve_backend`.
- Step 7 verify with semantic overlap and import checks that duplicate helper
  clusters shrink: completed. Pre-step `jj show --summary` reported
  working-copy commit `3144906691a9fa8896a4f4af7d8bffe665ba5474` on change
  `qzmtzqumnzsrlrkxnnyumrprqpvkxwyv`. Reused graph
  `2c6dfe88c8bad3b7db1838a94b00287b`.
  `module_dependencies` for `rmc_server::mcp::project_paths` shows indexing
  path policy flows through `rmc_indexing::indexing::project_paths`, with
  server still owning `ProjectDirs` and MCP error mapping.
  `module_dependencies` for query and graph similarity both route profile
  resolution through `rmc_server::mcp::project_paths`.
  `semantic_overlaps` reports no `data_dir` cluster and only a two-function
  backend resolver cluster: the shared MCP helper plus the index legacy-model
  wrapper.
- Step 8 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `f39ef4930e029d287024ee687529aec909c539a8` on change
  `qmowvluwlrxounuyozuztlmtkykqklpx`. The focused check passed with existing
  warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing -p rmc-server`.
- Step 9 update the ledger and commit: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `ed4702930e69011dd382d14da4f25609465062f8` on change
  `qzyowuwkxmunktkrkonlmvwxqpkuzsxl`. Updated
  `.docs/boundries-cleanup-progress.md` with Phase 4 Step 9 status, commit
  ledger, verification summary, and remaining report follow-up.
- Phase completion report: completed. Pre-step `jj show --summary` reported
  working-copy commit `e200d879e10f59b95d28e632234c17b37cd81eb3` on change
  `rqltnpzptqsxmlovkmloswopywrmyopu`. Wrote
  `.docs/phase-4-boundrie-fix-report.md` and marked the Phase 4 progress
  ledger complete.

### Phase 5: `rmc-graph` Query And Response Facade

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `1244e9892186d5c681827698217f9393db4642aa` on change
  `vkxwsvmtrwvvuzvrsuuqznxrlwoyrurx`, with no description set.
- Step 2 identify server response helpers that only translate graph internals
  to MCP DTOs: completed. Pre-step `jj show --summary` reported working-copy
  commit `46cf2ba5e96637b7f6f24525b6adbb8079db2d16` on change
  `psuzmtoxpqzwynpxqtrosrnozstxmqpx`, with no description set.
  MCP evidence reused graph `2c6dfe88c8bad3b7db1838a94b00287b` with
  fingerprint
  `680958b42dd9eaa0c1d72a5958fc985c38673f053fd17072d09aeda0eaa58b6d`.
  The response/core/surface modules still depend on raw graph
  `snapshot`, `storage`, `model`, `ids`, `labels`, and `query::model`
  modules. `functions_with_filter(has_param_type="OpenedSnapshot")`
  reported seven server graph helpers that still accept snapshots directly:
  `core::enrich_bindings`, `core::enrich_usages`,
  `response::resolve_chunk_to_item`, `response::resolve_required_node`,
  `response::visibility_label`, `surface::enrich_crate_dead_pub`, and
  `surface::enrich_dead_pub`.
- Step 2 source-read result: the best first migration target is the repeated
  translation/enrichment path, not MCP result wrapping. Move or mirror
  `core::enrich_bindings`, `core::enrich_usages`, `surface::enrich_dead_pub`,
  and `surface::enrich_crate_dead_pub` behind graph-owned query/DTO helpers.
  Keep `open_workspace_snapshot` server-owned for now because it maps
  directory/storage failures into MCP tool errors. Leave `resolve_chunk_to_item`
  for a later pass because it currently has no production caller.
- Step 3 add graph-owned query/DTO helpers for repeated enrichment paths:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `7f998139160dc1b189254ff967624d9de7fc7784` on change
  `nxmnrtrpuvqmnowsxqzywykvwqvuzyno`, with no description set. Added
  `graph::query::enrichment` with `OpenedSnapshot` helpers for bindings,
  usages, dead-public findings, and per-crate dead-public reports. Added and
  re-exported graph-owned `EnrichedBinding`, `EnrichedUsage`,
  `EnrichedDeadPub`, and `EnrichedCrateDeadPub` DTOs. Verification:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`
  passed with existing warnings.
- Step 4 keep server DTO shapes stable: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `a1fefcb699275b61cf7645b3b00e205b112da2c9` on change
  `pzwkkmyoltqmtxossowuttulpnpzqqzs`, with no description set. Compared the
  graph-owned enrichment DTOs against the existing server response DTOs: JSON
  field names, `skip_serializing_if = "Option::is_none"`, and the
  `EnrichedCrateDeadPub` `crate` rename are preserved. Label fields moved from
  `&'static str` to `String`, which keeps serialized output unchanged. No
  output-shape change is planned for Phase 5.
- Step 5 migrate server graph call sites incrementally: completed for the
  repeated enrichment path. Pre-step `jj show --summary` reported working-copy
  commit `4921f7af669a97f6121d01dc59f2f65c3a5e5657` on change
  `mpturlnmpmxxypolrmpqkmuvyoorttso`, with no description set. Updated
  `graph::core` to call `snap.enrich_bindings(...)` and
  `snap.enrich_usages(...)`, and updated `graph::surface` to call
  `snap.enrich_dead_pub(...)` and `snap.enrich_crate_dead_pub(...)`.
  Removed the server-local `enrich_bindings`, `enrich_usages`,
  `enrich_dead_pub`, `enrich_crate_dead_pub`, and now-unused
  `response::visibility_label` helpers. Verification:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`
  passed with existing warnings.
- Step 6 verify fewer server functions accept `OpenedSnapshot` directly:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `de6bccdac20a01f7ad783bfbd2aebc13a465e680` on change
  `rsuvtstwwrvpurzwpsxzwysqvxtzuxnu`, with no description set.
  `build_hypergraph(force_rebuild=false)` built graph
  `085eaff90b1189f8e7a4dc3374610742`, fingerprint
  `349e4a62bdb66681623fdc7432c538e80f98e667ffd92cac4a9400383a022759`.
  `functions_with_filter(has_param_type="OpenedSnapshot")` now reports two
  server graph helpers instead of the Step 2 baseline of seven:
  `response::resolve_chunk_to_item` and `response::resolve_required_node`.
  `module_dependencies` also shows `core` no longer depends on
  `rmc_graph::graph::labels`; `surface` no longer imports
  `rmc_graph::graph::snapshot`; remaining raw graph dependencies are tied to
  still-server-owned response/opening/resolution logic and non-enrichment
  surface endpoints.
- Step 7 keep existing graph exports for compatibility: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `7e351bd2886522f405b4e8dae5c7a03398372960` on change
  `ywoymvywpomsortlzsrsysswkolussyz`, with no description set.
  `get_exports(module="rmc_graph::graph", consumer="rmc_server", summary=true,
  limit=120)` reported 68 visible exports. Existing compatibility exports
  such as `snapshot`, `storage`, `model`, `ids`, `OpenedSnapshot`,
  `GraphPaths`, `GraphEnvOptions`, `Node`, `NodeKind`, `Binding`, `Usage`,
  `DeadPubFinding`, and `CrateDeadPub` remain visible. New graph-owned
  enrichment DTOs `EnrichedBinding`, `EnrichedUsage`, `EnrichedDeadPub`, and
  `EnrichedCrateDeadPub` are also visible.
- Step 8 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `80ca87896b7e6251766396439f3e4f47d9c93d95` on change
  `ytzttuwsmnotnkospomqzxuyvnnrvznw`, with no description set.
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server` passed with existing warnings.
- Step 9 update the ledger and commit: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `70a9dd8ae962004c496c0c1d1f725b519bf11a26` on change
  `lswunxmsqyoykmzuyryssvvqrtupsqrt`, with no description set. Phase 5
  implementation work is complete; the separate phase report remains to be
  written and committed.
- Phase 5 report: completed after pre-report `jj show --summary` reported
  working-copy commit `d5fb2248b18b7f7a930a8e43e1586b6528647f01` on change
  `nuwpmwmowmwqsokroozulwlrpmmkokvv`, with no description set. Wrote
  `.docs/phase-5-boundrie-fix-report.md` and marked the Phase 5 progress
  ledger complete.

### Phase 6: `rmc-graph` Audit Facade

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `aba7ca27e917ab5b3dd8633befc7f65e6a1b3584` on change
  `ulzuvpoonzuyywyvqlrxrlrwsuvuwsuw`, with no description set.
- Step 2 add graph-owned audit entry points: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `c4258945e608ce3aea72681b42390440dccb7aeb` on change
  `rzwtmmytrvlpqpmoyznrtqolszssmsxv`, with no description set.
  MCP evidence reused graph `085eaff90b1189f8e7a4dc3374610742`, fingerprint
  `349e4a62bdb66681623fdc7432c538e80f98e667ffd92cac4a9400383a022759`.
  `module_dependencies(rmc_server::tools::graph::audits)` showed direct
  server usage of `loader::load`, `channel_audit`, `fn_body_audit`,
  `recursion_check`, and snapshot audit methods. Added graph-owned audit
  facade functions `run_unsafe_audit`, `run_mut_static_audit`,
  `run_recursion_check`, `run_channel_capacity_audit`, and
  `run_fn_body_audit`, plus graph-owned options/result DTOs. Verification:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph`
  passed with existing warnings.
- Step 3 migrate server audit tools to call graph entry points: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `87635fe52b0bd23abe2fdfe0fca66bc73faf9888` on change
  `ulwzstolouqvzutxpyurqulxlrltxzxy`, with no description set. Updated
  `rmc_server::tools::graph::audits` to call graph-owned
  `run_unsafe_audit`, `run_mut_static_audit`, `run_recursion_check`,
  `run_channel_capacity_audit`, and `run_fn_body_audit`. Server now keeps MCP
  response envelopes, pagination, summary location stripping, parameter
  defaults, error mapping, and `spawn_blocking` for RA-load-backed audits.
  Verification:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server`
  passed with existing warnings.
- Step 4 keep server responsible only for MCP parameter parsing/result
  wrapping: completed. Pre-step `jj show --summary` reported working-copy
  commit `54f0a84a61c9f2c8ad24c9bfab56568a61b435c2` on change
  `wzorvywrvosrvoptsrylxvtwylxtrutx`, with no description set. Source search
  in `rmc_server::tools::graph::audits` found no remaining direct references
  to graph `loader`, individual audit modules, `NodeId`, `NodeKind`, snapshot
  lookup, or `to_hex`. The server audit module now keeps MCP response
  envelopes, pagination, summary stripping, parameter defaults, error mapping,
  and async blocking orchestration.
- Step 5 verify production server dependencies on audit internals are reduced:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `a0754394bde35ae2c361e2740f99f87eedc72902` on change
  `ysmstvvnqltlrnsutlvzotpvpuruprvq`, with no description set.
  `build_hypergraph(force_rebuild=false)` built graph
  `350719e344857be9514c69be176c11a7`, fingerprint
  `59335f0aaf01780beb5032be2ff2022bbe20c2903f067ec4c6c8cd60e802adaf`.
  `module_dependencies(rmc_server::tools::graph::audits)` now reports
  dependencies on `rmc_graph::graph::query::audits` facade functions/options
  and `rmc_graph::graph::query::model` DTOs, with no production dependency on
  `loader`, `channel_audit`, `fn_body_audit`, `recursion_check`,
  `unsafe_audit`, or snapshot audit methods. `get_exports` reports 83 graph
  exports visible to server, including the new audit facade exports.
- Step 6 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `d2632d8bb7318e88322e234a0d6dededcb8eae53` on change
  `qszylstrynwtssmtkvukptpnmttornwp`, with no description set.
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server` passed with existing warnings.
- Step 7 update the ledger and commit: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `0285ccff1860dd0910983e9caa69aee9e75b8b58` on change
  `pqkwwxvqumuxlvovltqznxzlrmkpuomz`, with no description set. Phase 6
  implementation work is complete; the separate phase report remains to be
  written and committed.
- Phase 6 report: completed after pre-report `jj show --summary` reported
  working-copy commit `3aee1e3216e40021532f9eda618c691f31879fa7` on change
  `kkrkqztmlupqppvsuvunvrsmyyzokwkx`, with no description set. Wrote
  `.docs/phase-6-boundrie-fix-report.md` and marked the Phase 6 progress
  ledger complete.
- Post-Phase 6 remediation: completed after pre-step `jj show --summary`
  reported working-copy commit
  `e281eb189679deb5589ba1caabfc0f1cd6edfdde` on change
  `uyrmqyvukmwsqsqsyoknllwkvqkylvvx`, with no description set. User review
  found a Phase 5 server test compile regression after `EnrichedUsage` moved
  to `rmc_graph`; updated the graph test to import the graph-owned DTO and use
  the DTO's `String` category field. Verification:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server
  --no-run` passed with existing warnings.

### Issues 778 Remediation

- Phase 3 indexing facade tests: completed after pre-step `jj show --summary`
  reported working-copy commit
  `a5635c829a745ac1fb10a049bcd46ad4493aba45` on change
  `rusxvvkpytpmuwnlmqpvsknzkyllxumm`, with no description set. Added focused
  `index_project_incrementally` tests for force reindex snapshot deletion and
  clearing order, backend construction inputs, and factory/clear/index error
  propagation. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-indexing
  incremental_service::tests`.
- Phase 3 elapsed timing semantics: completed after pre-step
  `jj show --summary` reported working-copy commit
  `3a4785cb7c1c6851e35d3b494256e880e0d4e44f` on change
  `tvtwwmsqyrklpnupppuskkkuwksrqusn`, with no description set. Moved the
  elapsed timer to facade entry so `IncrementalIndexOutcome.elapsed` covers
  force-reindex cleanup as well as change detection, documented the field, and
  added a focused cleanup-delay test. Verification passed with existing
  warnings: `nix develop ../nix-devshells#cuda-code --command cargo test -p
  rmc-indexing incremental_service::tests`.
- Phase 3 version-mismatch error mapping: completed after pre-step
  `jj show --summary` reported working-copy commit
  `f11e1618cd9f7a6066027f02309340ce28717153` on change
  `mktwztyympzrvmlvmprsmuurwyuwtwvm`, with no description set. Added a server
  regression test proving an `anyhow`-wrapped `VectorStoreError::VersionMismatch`
  still maps to the actionable MCP `clear_cache` guidance with stored and
  configured embedder IDs. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server
  version_mismatch_error_keeps_clear_cache_guidance`.
- Phase 4 indexing-owned path policy coverage: completed after pre-step
  `jj show --summary` reported working-copy commit
  `2710be2151cd8ae99047cbf9be86f7d5d9506940` on change
  `srryzkzxzpmnzszyzxlnytkmyknrktqv`, with no description set. Added direct
  `IndexingProjectPaths` tests for data-root layout, identity-scoped
  collection names, existing collection path derivation, and indexed profile
  discovery. Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-indexing
  project_paths::tests`.
- Phase 4 injected vectors-root behavior: completed after pre-step
  `jj show --summary` reported working-copy commit
  `9d6b198ed3a009991ed314ef342c299d292181ab` on change
  `lqrxuumnmtqpqvwpzxluqpvyqqvnurlx`, with no description set. Existing
  collection path derivation now preserves the vectors root used during
  discovery, and the server helper has a regression test for returned
  `vector_path` values under the injected root. Verification passed with
  existing warnings: `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-indexing project_paths::tests` and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server
  mcp::project_paths::tests`.
- Phase 4 malformed metadata policy: completed after pre-step
  `jj show --summary` reported working-copy commit
  `91eb91c6ad3f98e115311946a2cb4a9ad2e4c328` on change
  `qprunovqqpnkrvxwvvlowppvktwuuxkk`, with no description set. Indexed
  profile discovery now skips malformed matching vector metadata or invalid
  embedder identities with a warning, while direct metadata reads remain
  strict. Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-indexing
  project_paths::tests`.

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

## Phase 1: Workspace Boundary Rules

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

3. Add a repeatable boundary rule set in the most local existing place. Prefer
   an existing architecture/audit test if one exists.
4. If no executable local test pattern exists, add a documentation-only rule
   section and include the exact MCP `forbidden_dependency_check` command plus
   expected zero-violation result. Label it as documentation-only, not CI
   enforcement.
5. Record the exact expected dependency direction:

```text
rmc_server   -> rmc_graph, rmc_indexing, rmc_engine, rmc_config
rmc_graph    -> rmc_engine
rmc_indexing -> rmc_engine, rmc_config
rmc_engine   -> no rmc_* dependencies
```

6. Verify with the MCP forbidden dependency check.
7. If a Rust test/check is added, run only the focused check through the nix
   dev shell.
8. Update the ledger and commit:

```text
jj commit -m "docs: document crate boundary rules"
```

### Success Criteria

- Intended layering is written down in-repo.
- The rule set can be checked repeatably with MCP tools, and any
  documentation-only rule is clearly labeled as not yet CI-enforced.
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
who_imports(directory, target="rmc_indexing::indexing::tantivy_adapter", limit=200)
get_imports(directory, module="rmc_server::tools::endpoints::query", summary=false, limit=300)
get_imports(directory, module="rmc_server::tools::graph::codemap", summary=false, limit=300)
module_dependencies(directory, module="rmc_server::tools::endpoints::query", summary=false, limit=300)
module_dependencies(directory, module="rmc_server::tools::graph::codemap", summary=false, limit=300)
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
7. Verify with `module_dependencies` that server production modules no longer
   depend on `tantivy_adapter`.
8. Run focused checks through the nix dev shell if code changed.
9. Update the ledger and commit:

```text
jj commit -m "refactor: add indexing search facade"
```

### Success Criteria

- Server no longer opens `TantivyAdapter` directly in production query/codemap
  paths.
- Indexing owns concrete Tantivy search opening.
- Server behavior remains compatible, with the post-review correction that
  search/health probes no longer create missing Tantivy indexes as a side
  effect.

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
who_imports(directory, target="rmc_indexing::indexing::incremental::IncrementalIndexer", limit=200)
get_imports(directory, module="rmc_server::tools::endpoints::index", summary=false, limit=300)
get_imports(directory, module="rmc_server::mcp::sync", summary=false, limit=300)
module_dependencies(directory, module="rmc_server::tools::endpoints::index", summary=false, limit=300)
module_dependencies(directory, module="rmc_server::mcp::sync", summary=false, limit=300)
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
7. Verify with `module_dependencies` that direct production server dependency
   on `incremental` is gone or intentionally documented.
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
who_imports(directory, target="rmc_server::mcp::project_paths::ProjectPaths", limit=300)
functions_with_filter(directory, krate="rmc_server", has_param_type="ProjectPaths", summary=true, limit=100)
semantic_overlaps(directory, crate_name="rmc_server", item_kind="Function", summary=true, max_pairs=60)
get_imports(directory, module="rmc_server::mcp::project_paths", summary=false, limit=300)
get_imports(directory, module="rmc_indexing::indexing::identity", summary=false, limit=300)
module_dependencies(directory, module="rmc_server::mcp::project_paths", summary=false, limit=300)
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
module_dependencies(directory, module="rmc_server::tools::graph::response", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::graph::core", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::graph::surface", summary=false, limit=500)
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
who_imports(directory, target="rmc_graph::graph::loader::load", limit=300)
get_imports(directory, module="rmc_server::tools::graph::audits", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::graph::audits", summary=false, limit=500)
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
5. Verify with `module_dependencies` that production server dependencies on
   `loader::load` and individual audit internals are reduced or removed.
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

Scope note: this phase is primarily for `semantic_overlaps`. Do not move the
whole `similar_to_item` tool into `rmc_graph` unless a clean lower-level search
facade already exists and the move does not introduce a graph dependency on
server or indexing. `similar_to_item` currently depends on server project-path
policy and server hybrid-search construction, so it remains server-owned by
default.

### Boundary Problem

Server similarity code coordinates helpers such as embedding cache maintenance
and cosine math.

### MCP Evidence To Refresh

```text
get_imports(directory, module="rmc_server::tools::graph::similarity", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::graph::similarity", summary=false, limit=500)
who_imports(directory, target="rmc_graph::graph::embedding_cache::ensure_embeddings_for", limit=300)
who_imports(directory, target="rmc_graph::graph::math::cosine", limit=300)
semantic_overlaps(directory, crate_name="rmc_graph", item_kind="Function", summary=true, max_pairs=40)
```

### Steps

1. Run `jj show --summary`.
2. Add a graph-owned semantic-overlap operation that accepts graph/query
   options and returns graph DTOs.
3. Keep embedding cache and cosine implementation details inside graph.
4. Migrate the server `semantic_overlaps` tool to the facade.
5. Keep `similar_to_item` server-owned unless a safe lower-level graph helper
   can be used without moving server/indexing path policy into graph.
6. Verify with `module_dependencies` that server production modules no longer
   reach into graph `embedding_cache` or `math` for semantic-overlap behavior.
7. Run focused checks through the nix dev shell.
8. Update the ledger and commit:

```text
jj commit -m "refactor: add graph similarity facade"
```

### Success Criteria

- Server asks graph for semantic-overlap results.
- Graph owns embedding-cache and scoring mechanics.
- Public low-level helper use is reduced.
- `similar_to_item` does not move into graph unless graph can stay independent
  of server and indexing.

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
who_imports(directory, target="rmc_graph::graph::GraphPaths", limit=300)
get_imports(directory, module="rmc_server::tools::endpoints::cache", summary=false, limit=300)
module_dependencies(directory, module="rmc_server::tools::endpoints::cache", summary=false, limit=300)
functions_with_filter(directory, krate="rmc_graph", has_param_type="GraphPaths", summary=true, limit=100)
```

### Steps

1. Run `jj show --summary`.
2. Add a graph-owned cache/snapshot cleanup API.
3. Migrate server cache endpoint to call the graph API instead of constructing
   or interpreting graph storage paths directly.
4. Verify with `module_dependencies` that server cache code no longer reaches
   into graph storage layout, and that `GraphPaths` use is concentrated in
   graph snapshot/storage modules.
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
who_imports(directory, target="rmc_engine::search::bm25", limit=300)
who_imports(directory, target="rmc_engine::search::resilient", limit=300)
who_imports(directory, target="rmc_engine::vector_store::lancedb", limit=300)
who_imports(directory, target="rmc_engine::vector_store::traits", limit=300)
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
who_imports(directory, target="rmc_indexing::indexing::tantivy_adapter", limit=300)
who_imports(directory, target="rmc_indexing::indexing::identity", limit=300)
who_imports(directory, target="rmc_indexing::indexing::merkle", limit=300)
who_imports(directory, target="rmc_indexing::indexing::retry", limit=300)
who_imports(directory, target="rmc_indexing::indexing::consistency", limit=300)
who_imports(directory, target="rmc_indexing::indexing::indexer_core", limit=300)
module_dependencies(directory, module="rmc_server::tools::endpoints::query", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::graph::codemap", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::endpoints::index", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::mcp::sync", summary=false, limit=300)
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
who_imports(directory, target="rmc_graph::graph::loader", limit=300)
who_imports(directory, target="rmc_graph::graph::storage", limit=300)
who_imports(directory, target="rmc_graph::graph::model", limit=300)
who_imports(directory, target="rmc_graph::graph::ids", limit=300)
who_imports(directory, target="rmc_graph::graph::bindings", limit=300)
who_imports(directory, target="rmc_graph::graph::usages", limit=300)
module_dependencies(directory, module="rmc_server::tools::graph::response", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::core", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::surface", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::audits", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::similarity", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::endpoints::cache", summary=false, limit=500)
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

4. Refresh server deep-dependency checks. Use both import and dependency tools:
   `get_imports` catches explicit `use` statements, while
   `module_dependencies` catches fully qualified inline paths.

```text
get_imports(directory, module="rmc_server::tools::graph", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::response", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::core", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::surface", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::audits", summary=false, limit=700)
module_dependencies(directory, module="rmc_server::tools::graph::similarity", summary=false, limit=700)
get_imports(directory, module="rmc_server::tools::endpoints::query", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::endpoints::query", summary=false, limit=500)
module_dependencies(directory, module="rmc_server::tools::endpoints::cache", summary=false, limit=500)
get_imports(directory, module="rmc_server::mcp::sync", summary=false, limit=300)
module_dependencies(directory, module="rmc_server::mcp::sync", summary=false, limit=300)
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
- Server has fewer graph/indexing internal dependencies, including fully
  qualified inline references.
- Public implementation modules are reduced or intentionally documented.
- Boundary reports/progress notes explain any remaining exceptions.

## Recommended Execution Order

Use this order unless evidence during a phase shows a safer local sequence:

1. Phase 0: Baseline and safety checks.
2. Phase 1: Workspace boundary rules.
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

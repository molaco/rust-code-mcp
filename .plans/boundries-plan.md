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
- Phase 5 static label DTOs: completed after pre-step `jj show --summary`
  reported working-copy commit
  `248569ba8316b640013ffb35aa19fd0833698184` on change
  `muwknkowzpxztnvrpyqprokvwozoosot`, with no description set. Graph
  enrichment DTO fields backed by closed label sets now use `&'static str`
  again (`namespace`, binding `kind`, usage `category`, and dead-public
  `item_kind`), while dynamic visibility and node-kind strings stay owned.
  Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server
  usage_summary_omits_navigation_fields`.
- Phase 5 enrichment error contract: completed after pre-step
  `jj show --summary` reported working-copy commit
  `9f19d735ab462bf976d048a01bf47a50e6e6f596` on change
  `osrvsrknsqtslyymnyouptvkzqkvkonn`, with no description set. Graph
  enrichment methods now return `Result` and propagate snapshot transaction
  failures, storage lookup errors, and missing referenced nodes instead of
  returning empty or partial data; server graph endpoints map those errors to
  MCP internal errors. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server`.
- Phase 5 graph-side enrichment tests: completed after pre-step
  `jj show --summary` reported working-copy commit
  `bfe76b5948a1190529cbf81a1bf982252c09e28f` on change
  `unpznyrtxrknvvznslqslmtywuwkqkyr`, with no description set. Added focused
  graph tests for enriched binding label/node resolution, usage summary shape,
  dead-public DTO shape, and missing referenced-node error propagation.
  Verification: the first `nix develop ../nix-devshells#cuda-code --command
  cargo test -p rmc-graph enrich_` run passed three new tests and exposed one
  over-specific category assertion; after loosening that assertion,
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  enrich_usages_applies_summary_shape_and_static_category` passed.
- Phase 6 typed audit error mapping: completed after pre-step
  `jj show --summary` reported working-copy commit
  `ac20436418fcb55e6734e31ba9b28406e1c6995f` on change
  `pwxznosuwplpolknyknuzmvlonrroqks`, with no description set. Added
  graph-owned `GraphAuditError` variants for invalid directories, missing
  snapshots, invalid crate filters, and unknown function-body patterns; server
  audit error mapping now classifies invalid parameters by `anyhow` downcast
  instead of substring matching. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server`.
- Phase 6 audit blocking behavior: completed after pre-step
  `jj show --summary` reported working-copy commit
  `13cdd4282c4e69a0735437c7c574a1569c29127e` on change
  `uqxnypnpuwuxturwmxuxsorpqwokpknv`, with no description set. Updated
  `mut_static_audit` and `recursion_check` to run synchronous graph audit
  facade calls inside `tokio::task::spawn_blocking`, matching the other audit
  endpoints. Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo check -p rmc-server`.
- Phase 6 audit facade tests: completed after pre-step
  `jj show --summary` reported working-copy commit
  `c4b8ce78069a6e48af4e3a899d52728393fa399b` on change
  `sknlnlrvsvtvmxwlxyuyqzmvypozxspu`, with no description set. Extracted
  graph audit DTO rendering helpers and added focused tests for unsafe,
  mutable-static, channel-capacity, and function-body audit DTO mapping. Added
  server tests proving typed `GraphAuditError` values map to
  `INVALID_PARAMS` while untyped failures map to `INTERNAL_ERROR`.
  Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-graph audit_dto`
  and `nix develop ../nix-devshells#cuda-code --command cargo test
  -p rmc-server graph_audit_error_maps`.
- Current-suite graph loader test: completed after pre-step `jj show
  --summary` reported working-copy commit
  `b4c460389d3c3beb463212d901e3757f6a018488` on change
  `wypssuzynqrqsmrlnxnkknwznllttuop`, with no description set. Updated
  `load_crate_target_kinds_finds_workspace_targets` to assert
  workspace-member target paths under the virtual workspace root instead of
  root-level `src/*.rs` paths. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  load_crate_target_kinds_finds_workspace_targets`.
- Current-suite indexing test reliability: completed after pre-step
  `jj show --summary` reported working-copy commit
  `f818b69c29570c19a60068ab22672c9319d0bfc1` on change
  `qyxqmnnulvqsztqzuwzxqkowpumtoqkl`, with no description set. Reworked
  `IndexerCore` unit tests so file-filtering and chunk-split checks do not
  construct the default embedding generator, and changed `MemoryMonitor::new`
  to initialize memory data only instead of all process/system data.
  Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-indexing
  indexer_core::tests`, `nix develop ../nix-devshells#cuda-code --command
  cargo test -p rmc-indexing test_calculate_safe_batch_size`, and
  `timeout 180s nix develop ../nix-devshells#cuda-code --command cargo test
  -p rmc-indexing --lib`.
- Current-suite server validation remediation: completed after pre-step
  `jj status` reported no changes, empty working-copy commit `9280c628`, and
  parent commit `59417009 test: fix remaining boundary validation issues`.
  Fixed the remaining server and indexing validation failures found after
  Phases 3 through 6: analysis endpoints now reject directories that are not
  Cargo projects; private/internal doctest examples are marked `rust,ignore`;
  server graph round-trip tests use the `rmc-server` crate root and current
  qualified names; the stale BM25 dependency assertion now checks the
  indexing-owned `open_bm25_search` facade; the expensive `dead_pub_report`
  aggregation was removed from the focused round-trip; and default-snapshot
  graph endpoint cases now run sequentially in one async test to avoid
  concurrent `heed`/LMDB opens against the same persisted graph snapshot.
  Verification passed with existing warnings: `nix develop
  ../nix-devshells#cuda-code --command timeout 1200s cargo test -p
  rmc-server` and `nix develop ../nix-devshells#cuda-code --command timeout
  600s cargo check -p rmc-indexing -p rmc-graph -p rmc-server`. Earlier in
  the same remediation pass, `nix develop ../nix-devshells#cuda-code
  --command timeout 900s cargo test -p rmc-indexing` passed.

### Phase 7: `rmc-graph` Similarity Facade

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `039b45e753bd7fb5203b19681768cd5997ad2aa6` on change
  `snlqzpzouynzrmunmsuomvuupqoovtvq`, with no description set.
- Step 2 add graph-owned semantic-overlap operation: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `6f802d4afdc3ed0b87731cef56667bad87ef4038` on change
  `qspkvyrummotnxnwqokkmuqsxrlqrzmy`, with no description set. Added
  `rmc_graph::graph::run_semantic_overlaps` with graph-owned options and
  similarity DTOs. The new graph query module owns snapshot opening, item
  enumeration, embedding-cache refresh, identical-source scoring, cosine
  pair scoring, cluster building, and response DTO construction. Verification
  passed with existing warnings: `nix develop ../nix-devshells#cuda-code
  --command cargo check -p rmc-graph`.
- Step 3 keep embedding cache and cosine implementation details inside graph:
  completed for the new facade. Pre-step `jj show --summary` reported
  working-copy commit `1fb5b0e52f108d220ef5d47affbd41f4f9a458e1` on change
  `wnusvywuxmpqkmxourpqzutosmryxvoq`, with no description set. Source search
  shows `rmc_graph::graph::query::similarity` reaches
  `embedding_cache::ensure_embeddings_for` and `math::cosine` through private
  graph module paths. The old public compatibility reexports remain only
  until the server `semantic_overlaps` migration in the next step; source
  search identified the remaining server calls at
  `crates/rmc-server/src/tools/graph/similarity.rs`.
- Step 4 migrate the server `semantic_overlaps` tool to the facade:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `d6e25d0d55f8190fce2e9e6c05eada5207aac4e3` on change
  `ozymokktwuqnqpzwqrnopwrprqznopqy`, with no description set. Server
  `semantic_overlaps` now resolves the embedding backend, passes graph-owned
  options to `rmc_graph::graph::run_semantic_overlaps`, maps typed graph
  similarity errors to MCP invalid params, and serializes graph-owned DTOs.
  Removed server-local similarity DTO/cluster helpers and moved their pure
  tests into graph. Removed public graph reexports of `ensure_embeddings_for`
  and `cosine`; remaining graph-internal codemap calls now use private graph
  module paths. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server` and `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph similarity_`.
- Step 5 keep `similar_to_item` server-owned: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `0de06380951010fdc893e2954a649b6246c661d7` on change
  `msuwkqmlltplmzuznwwuorqxpzwzmlyr`, with no description set. Source search
  confirms `similar_to_item` remains implemented in
  `rmc_server::tools::graph::similarity` and routed by the server. Its
  server-only dependencies on `resolve_embedding_backend_for_mcp`,
  `ProjectPaths::from_directory`, `create_hybrid_search`, and
  `vector_only_search` remain outside `rmc_graph`.
- Step 6 verify server production modules no longer reach graph
  `embedding_cache` or `math` for semantic-overlap behavior: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `7e840eff61b539599656d74b6f9f659983a5ebb8` on change
  `vovntxlqqmtptkmqntkvvowoxsqyyqzw`, with no description set.
  `build_hypergraph(force_rebuild=false)` rebuilt graph
  `56dbddbd49bf25977fef1d75a269d455`, fingerprint
  `53b0c34cc7a90b62bade00ab81ce4ae4baf13a37429fee9d4dd4c740b5364aae`.
  `module_dependencies(rmc_server::tools::graph::similarity)` now reports
  dependencies on the graph similarity facade
  `GraphSimilarityError`, `SemanticOverlapOptions`, and
  `run_semantic_overlaps`, with no server dependency on graph
  `embedding_cache` or `math`. `who_imports` for
  `embedding_cache::ensure_embeddings_for` and `math::cosine` reported only
  graph-internal query/test importers. MCP `semantic_overlaps` scoped to
  `crate_name="rmc_graph"`, `item_kind="Function"`, `summary=true`, and
  `max_pairs=40` returned 178 seeds, 18 pairs, and 15 clusters.
- Step 7 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `9a27b5751bc24252d875d8e87c761c0b7f097c5a` on change
  `ykpxzxowosoplyyukxtpwrrpqmquwwzu`, with no description set. Verification
  passed with existing warnings: `nix develop ../nix-devshells#cuda-code
  --command cargo check -p rmc-graph -p rmc-server` and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-graph similarity_`
  (6 tests passed).
- Step 8 update the ledger and commit: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `72cbf9b80d9d36ae0582bf93ebed808260226dda` on change
  `npyxysnzuxnrkzyswolsvsrttvqzltkq`, with no description set. Phase 7
  implementation work is complete; the separate phase report remains to be
  written and committed.
- Phase 7 report: completed after pre-report `jj show --summary` reported
  working-copy commit `32d3b1dd585f2eb4fa63471d5b56893d884f98de` on change
  `yqznxlswloqrkoptsswsskoztlxmoooo`, with no description set. Wrote
  `.docs/phase-7-boundrie-fix-report.md` and marked the Phase 7 progress
  ledger complete.

### Phase 8: `rmc-graph` Storage Cleanup Facade

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `b1c1d1efc726c59be81c2bab2173c5cc9901db53` on change
  `orrluuuuxommuvvnvqkuspowlszrkpoo`, with no description set.
- Step 2 add graph-owned cache/snapshot cleanup API: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `a142565cdcf8f1465c568be4234eff49b5e6fe2c` on change
  `omtlysvxxlpvylyzmkrtntnulupxynym`, with no description set. Added
  graph-owned `clear_workspace_snapshots` and
  `clear_all_workspace_snapshots`, plus cleanup options/report DTOs. The
  implementation owns graph snapshot path calculation, dry-run reporting,
  and removal/error collection. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-graph` and `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph clear_` (3 tests passed).
- Step 3 migrate server cache endpoint to call the graph API: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `98082c4c3d3d4cd0988387bcd31359ef4c51ff00` on change
  `txpklmnkupptwsnopypttlxnwktzzzqy`, with no description set. Server
  `clear_cache` now delegates per-workspace and all-workspace hypergraph
  cleanup to `clear_workspace_snapshots` and `clear_all_workspace_snapshots`.
  Source search found no remaining direct server references to
  `rmc_graph::graph::GraphPaths` or `rmc_graph::graph::storage`. Verification
  passed with existing warnings: `nix develop ../nix-devshells#cuda-code
  --command cargo check -p rmc-server` and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server cache` (7
  tests passed).
- Step 4 storage dependency verification: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `163531c3b4b477154d3f85a6f1b867785003e94d` on change
  `yypzuvnosopmvpvmwnkskzonvqntrplt`, with no description set. Moved the
  public cleanup facade to the graph snapshot layer and added
  `open_current_for_workspace` so server graph response code also stops
  constructing `GraphPaths` directly. Refreshed the hypergraph:
  graph `6a0f0a501756b0c9b36c694e073a60fc`, fingerprint
  `d291e5830be17d570abd3d5892e8c467a858c35d3bfcce3f5617e62be37f118d`.
  `module_dependencies` for `rmc_server::tools::endpoints::cache` now shows
  only graph snapshot cleanup dependencies:
  `GraphSnapshotCleanupOptions`, `GraphSnapshotCleanupReport`,
  `clear_workspace_snapshots`, and `clear_all_workspace_snapshots`.
  `get_imports` for the cache endpoint imports only the two snapshot cleanup
  DTOs from graph. `who_imports` for `GraphPaths` returned 16 bindings, all in
  graph modules/tests, debug binaries, compatibility reexport, or
  `probe_workspace`; no server module imports `GraphPaths`. The
  `functions_with_filter` check found only graph snapshot functions with
  `GraphPaths` parameters: `open_current`, `open_specific`, and
  `publish_current`. Supporting verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server`, `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph clear_` (3 tests passed), and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server cache` (7
  tests passed).
- Step 5 focused nix checks: completed. Pre-step `jj show --summary`
  reported working-copy commit
  `1ca43a0b76b242c5e4804561f0e9fefe6fc17772` on change
  `mwvzzoypltvwrsuvusomvyvplyouxzpl`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server`, `nix develop ../nix-devshells#cuda-code --command cargo
  test -p rmc-graph clear_` (3 tests passed), and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rmc-server cache` (7
  tests passed).
- Step 6 ledger update: completed. Pre-step `jj show --summary` reported
  working-copy commit `7c408edae0a83fd3b20a5b30bd00004fdb3287a9` on change
  `mxqwsnznktqlvmsnomxrrupkxonkvmvz`, with no description set. Phase 8 code
  and verification steps were complete; the separate Phase 8 report was still
  pending at this step.
- Phase 8 report: completed after pre-report `jj show --summary` reported
  working-copy commit `fa9e7c158816499cbb23a3aa5578d840a4463b60` on change
  `tymoomxzuunzzpxyyuvrtunyzppoutsy`, with no description set. Wrote
  `.docs/phase-8-boundrie-fix-report.md` and marked the Phase 8 progress
  ledger complete.

### Phase 9: `rmc-server` Internal Boundary Cleanup

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `418aceba11919b3c1a46448f3bc5885e1044b4ea` on change
  `mrozmyslwyvyymmnwlmwmqnylrooxqtu`, with no description set.
- Step 2 keep `tools::router` thin: completed as verification-only. Pre-step
  `jj show --summary` reported working-copy commit
  `d0bd6a43da799a6069e09c4f775f642225020e06` on change
  `lwwmnsznmwowomykvtwmuvsrwwkwmuyq`, with no description set. Source review
  confirmed `crates/rmc-server/src/tools/router.rs` declares MCP tool methods
  and delegates to `tools::endpoints::*` or `tools::graph::*`; it does not own
  lower-layer graph/indexing business logic. The larger `build_codemap`
  method only destructures parameters before delegating to
  `tools::graph::codemap::handle_build_codemap`.
- Step 3 remove server-side helper duplication: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `5f834598d6cdc20a22e216c90c65c7568e867f3f` on change
  `powrovouytsxlwomwklzwuvrplmuvmov`, with no description set. Removed the
  unused `tools::endpoints::indexing_support` module, which only wrapped
  low-level Tantivy index/cache opening after the indexing facades existed,
  and removed stale router/module documentation references to it. Source
  search found no remaining `indexing_support` references. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-server`.
- Step 4 keep `tools::params` internal: completed as verification-only.
  Pre-step `jj show --summary` reported working-copy commit
  `f4da094f0a1f7c729a74b93373fb3d2abb1d1615` on change
  `rnvkvtnqvwkyutvmlllprxzvmnxmtrzl`, with no description set. Source review
  confirmed `tools::params` is a private module, its family modules are
  private, its flat reexports are `pub(crate)`, and all MCP parameter structs
  in `tools::params` remain `pub(crate)` rather than public API.
- Step 5 treat `semantic` privacy as late cleanup: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `36e3e7cd2696bbaded99e62b9cc9fcb11bd7b340` on change
  `punsvpsnkrslukpusrworzvopyzxyzll`, with no description set. Source search
  found `crates/rmc-graph/src/graph/statics.rs` tests that assert the
  qualified name `rmc_server::semantic::SEMANTIC`, so Phase 9 leaves
  `semantic` public until those symbol-level expectations move. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  semantic` (1 test passed, 200 filtered out, finished in 108.38s).
- Step 6 migrate `tools::project_paths` compatibility caller: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `c71c3b4f61a2e1d3feeacc64a3db2c471222b1d4` on change
  `orykxsrltvovutxuxqsvnvmmyvsyltky`, with no description set. Updated the
  stdio regression test to import `ProjectPaths` from
  `rmc_server::mcp::project_paths`, removed the `tools::project_paths`
  compatibility module, and removed the `tools` module declaration for it.
  Source search found no remaining `rmc_server::tools::project_paths`,
  `crate::tools::project_paths`, or `tools::project_paths` callers.
  Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-server -p rust-code-mcp` and `nix develop ../nix-devshells#cuda-code
  --command cargo test -p rust-code-mcp --test test_mcp_stdio_transport
  --no-run`.
- Step 7 verify server public exports: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `7ea2b94aa4faf206c22435ac3f2792c703ba7f0a` on change
  `kknqyswrtonszvszpttxolopwmmoystr`, with no description set. Refreshed the
  MCP hypergraph: graph `e669ac6eeba2bb252aa05150b435baa2`, fingerprint
  `cff3f2f33f298d34766a50b6578f4212466eadbfdf76f6399bf5b36567eddb29`.
  `get_exports(module="rmc_server::tools", consumer="rmc_server")` returned
  the intended tools facade: `SearchToolRouter`, `SearchTool`,
  `index_codebase`, and `IndexCodebaseParams`. `get_exports` for
  `rmc_server::mcp` returned `SyncManager` plus the public `sync` and
  `project_paths` modules. Root `rmc_server` exports remain `tools`, `mcp`,
  and `semantic`. `get_declared_reexports` for `rmc_server::tools` returned
  only the four intended tools reexports, and `rmc_server::mcp` returned the
  `SyncManager` glob reexport.
- Step 8 focused nix checks: completed. Pre-step `jj show --summary`
  reported working-copy commit
  `656d72cdc727c084cd25e212ea8599beb4b0324c` on change
  `xvwksozxvkqultozmvrtwmsozlrmrmpx`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-server -p rust-code-mcp`, `nix develop ../nix-devshells#cuda-code
  --command cargo test -p rmc-server --no-run`, and `nix develop
  ../nix-devshells#cuda-code --command cargo test -p rust-code-mcp --test
  test_mcp_stdio_transport --no-run`.
- Step 9 ledger update: completed. Pre-step `jj show --summary` reported
  working-copy commit `1af4c10e99da408c7ac5071ee5aa8b58b53a4fc7` on change
  `zpqmmtnpxssynwlokxqoplwvwwxzsuqr`, with no description set. Phase 9 code,
  export verification, and focused checks were complete; the separate Phase 9
  report was still pending at this step.
- Phase 9 report: completed after pre-report `jj show --summary` reported
  working-copy commit `03d824b9b855fb22d08bcc52ff5f32818257eb45` on change
  `xsqrqployrzwyptkmoqurswywpmxlukn`, with no description set. Wrote
  `.docs/phase-9-boundrie-fix-report.md` and marked the Phase 9 progress
  ledger complete.

### Phase 10: `rmc-engine` Public Surface Tightening

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `93da984cc37b76983fbf24e64a16df3136ddd97b` on change
  `rpymxmznvyyowomornmtylzpsuuqonsy`, with no description set.
- Step 2 confirm active engine implementation-module consumers: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `e6de81386071f5530aeeea7ea33e7e14452ff4ee` on change
  `xvpssnupvstnqqynwwspkxsszrzzumrm`, with no description set. MCP
  `who_imports` found only engine test-module glob imports for
  `search::bm25`, `search::resilient`, `search::rrf_tuner`,
  `vector_store::lancedb`, and `vector_store::traits`; no cross-crate import
  edges depend on those implementation modules. Source search found inline
  production references to `rmc_engine::search::bm25::Bm25Search` in
  indexing/server code, which can use the existing
  `rmc_engine::search::Bm25Search` facade reexport. Parser helper-module
  search found no external importers for `imports` or `call_graph`, and
  `type_references` is used inside `rmc_engine::parser` itself. Export
  checks confirmed `rmc_engine::search` and `rmc_engine::vector_store` still
  expose both implementation modules and facade reexports, while
  `rmc_engine::embeddings` exposes `EmbeddingBackend`, `EmbeddingProfile`,
  `EmbeddingGenerator`, and related embedding boundary types.
- Step 3 migrate production consumers to facade reexports: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `61f941d7e1f85ff3994f81e05ad0ab3473ed01ac` on change
  `pssllkpopytptyqvltpyzkpwtvxnxypu`, with no description set. Updated
  indexing and server production code to use `rmc_engine::search::Bm25Search`
  instead of inline `rmc_engine::search::bm25::Bm25Search` paths. Source
  search found no remaining production inline `search::bm25::Bm25Search`
  references. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-indexing -p rmc-server`.
- Step 4 tighten implementation-module visibility: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `7f68455c8ce3445f05842d283d6c380de6b7ac8f` on change
  `yvurtoqvqqvokrpylxpqrmyoxmzwmuky`, with no description set. Made the
  `rmc_engine::search`, `rmc_engine::vector_store`, and `rmc_engine::parser`
  implementation modules private while preserving their existing facade
  reexports. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-engine -p rmc-indexing -p rmc-server -p rust-code-mcp`.
- Step 5 document `EmbeddingProfile` ownership: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `50ca0238532f22e83a9f4e9615d99b95590c49ed` on change
  `lllzosuzmrooqkposktyxzyzzlqllzmr`, with no description set. Added rustdoc
  that keeps `EmbeddingProfile` as the engine-owned embedding configuration
  model and clarifies that higher crates select, persist, and pass profiles
  without owning the schema.
- Step 6 confirm embedding backend semantics are unchanged: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `ccbebd6c078cbca95db5ca5465f5a99af04577c1` on change
  `stwryqtyryzsvruqsrklqzuyzlxpqmou`, with no description set. No embedding
  runtime, identity, profile parsing, or backend construction semantics were
  changed; the previous step was ownership rustdoc only. The
  `jj diff --from @- --stat` check reported no working-copy file changes at
  the start of this step.
- Step 7 document `EmbeddingBackend` as the formal cross-crate boundary:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `e6985ba7d4ac63019d3fbc6c1e8a13062e1bf20d` on change
  `txoyuqwnvqzttkymnvyowsmmvxtrpuwu`, with no description set. Added rustdoc
  identifying `EmbeddingBackend` as the shared runtime, cache identity, and
  dimension contract used by indexing, graph, and server crates.
- Step 8 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `87d64b368ae7ba68431ae04fbea6c2b49de960c0` on change
  `mmttxlnqxspokwwqrmyznzqllovvonsz`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-engine -p rmc-indexing -p rmc-server -p rust-code-mcp`.
- Step 9 update ledger and commit: completed. Pre-step `jj show --summary`
  reported working-copy commit `a6239134dec747b3c3c1714df2625bbf306bd9df`
  on change `uvvonrmnuryxwnntprmwqqvukyvwnruz`, with no description set.
  Updated the Phase 10 ledger with changed files, verification, commit
  history, and remaining follow-up.
- Phase completion report: completed. Pre-report `jj show --summary`
  reported working-copy commit `3e2d9a6ba3e31f35f831495ffb91fbf80ddaa8e1`
  on change `kvpnuvmpokupsuyvtnuvtotylmxsypzs`, with no description set.
  Wrote `.docs/phase-10-boundrie-fix-report.md` and marked the Phase 10
  progress ledger complete.

### Phase 11: `rmc-indexing` Visibility Tightening

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `548b442d84e685843e1dc80bd27d32197e6d7de9` on change
  `rmtkyypxkmopxowpostpqvuzryoslttm`, with no description set.
- Step 2 review public indexing implementation modules: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `a548aba593df1259e5f87a0b28d37abfbebace15` on change
  `mqpnultyottokqxmtomstwlwzrxomyxw`, with no description set. Refreshed the
  MCP hypergraph with `force_rebuild=true`, producing graph
  `f1cee8ca9468963703d096ec6dc25950`. `get_exports` showed
  `rmc_indexing::indexing` still exposes implementation modules including
  `consistency`, `identity`, `indexer_core`, `merkle`, `retry`,
  `tantivy_adapter`, and `unified`. `who_imports` returned zero import
  bindings for the target implementation modules, but source search found
  external test/example deep paths for `merkle`, `tantivy_adapter`, and
  `unified`. MCP `module_dependencies` showed server query still depends on
  `rmc_indexing::indexing::unified`, while server codemap, index, and sync use
  the newer search/incremental service facades.
- Step 3 migrate deep indexing-path consumers to facade reexports: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `43db92827a8c6802d780c3c46401d7f3dd719993` on change
  `twrwporsryquyryomowoyxkpzlxynrtr`, with no description set. Added facade
  reexports for `ChangeSet` and `FileSystemMerkle`, migrated server query to
  `IndexStats` and `UnifiedIndexer` from the indexing facade, and migrated
  rust-code-mcp tests/examples off deep `unified`, `merkle`,
  `tantivy_adapter`, and `incremental` paths. Source search found no remaining
  external deep indexing paths for those modules. Verification passed with
  existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-indexing -p rmc-server -p rust-code-mcp`; touched test target
  compilation also passed with
  `nix develop ../nix-devshells#cuda-code --command cargo test -p
  rust-code-mcp --test test_merkle_standalone --test test_hybrid_search
  --test test_mcp_stdio_transport --no-run` and
  `nix develop ../nix-devshells#cuda-code --command cargo test -p
  rust-code-mcp --test test_merkle_standalone --no-run`.
- Step 4 tighten indexing implementation-module visibility while keeping
  facade exports public: completed. Pre-step `jj show --summary` reported
  working-copy commit `958a3eed30a039b39ca12c7577471f694f53ca70` on change
  `zwnzwxxkvoouwuwokkuzuolozovxsrtw`, with no description set. Made
  `consistency`, `identity`, `incremental`, `indexer_core`, `merkle`,
  `retry`, `tantivy_adapter`, and `unified` private modules, kept their
  supported facade reexports public, and moved the remaining internal
  monitoring Merkle caller to the facade reexport. Refreshed the MCP
  hypergraph with `force_rebuild=true`, producing graph
  `8d2fad2e10bdcfc9de811ac36e699ca3`; `get_exports` no longer lists those
  implementation modules as declared exports, while facade exports such as
  `UnifiedIndexer`, `IndexStats`, `IndexFileResult`, `IncrementalIndexer`,
  `get_snapshot_path`, `TantivyAdapter`, `FileSystemMerkle`, and `ChangeSet`
  remain visible. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-indexing -p rmc-server -p rust-code-mcp` and
  `nix develop ../nix-devshells#cuda-code --command cargo test -p
  rust-code-mcp --test test_merkle_standalone --test test_hybrid_search
  --test test_mcp_stdio_transport --no-run`.
- Step 5 review `metadata_cache`, `metrics`, `monitoring`, and `security`:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `94c3bc794881f945ec5e519123c164efd29ca867` on change
  `vsllwmsowrxzkxusmwxupylvknonxruv`, with no description set. Made
  `metadata_cache`, `security`, and `monitoring::backup` private
  implementation modules, exposed `metrics::MemoryMonitor` and
  `monitoring::{ComponentHealth, HealthMonitor, HealthStatus, Status}`
  through support-module facades, and migrated server/test/example consumers
  away from deep support-module paths. Verification passed with existing
  warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-indexing -p rmc-server -p rust-code-mcp`,
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rust-code-mcp --example benchmark_phases`, and
  `nix develop ../nix-devshells#cuda-code --command cargo test -p
  rust-code-mcp --test test_gpu_index_jsonrpc --no-run`.
- Step 6 keep `IncrementalIndexer` public for compatibility: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `49463879742b3d67ba3a6bf6e23c3bc0ee4f0d6d` on change
  `xwlwttpuwlmntlvwlwpyxtouoxlmqmql`, with no description set. Refreshed the
  MCP hypergraph with `force_rebuild=true`, producing graph
  `da64f03ea621c18612caf4468a58b64f`. MCP `who_imports` and source search
  confirmed `IncrementalIndexer` remains used by rust-code-mcp tests, benches,
  examples, a standalone index tool, and internal indexing tests/facades. MCP
  `module_dependencies` confirmed server `index` and `sync` production paths
  use `incremental_service` rather than constructing `IncrementalIndexer`
  directly. No code change was made and no build command was required.
- Step 7 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `9f5b2e8e31c0b0c39811428f31200dfacbc318f4` on change
  `lrxoslltluxowkwzpxozzxtqwpwsrykz`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-indexing -p rmc-server -p rust-code-mcp`,
  `nix develop ../nix-devshells#cuda-code --command cargo test -p
  rust-code-mcp --test test_merkle_standalone --test test_hybrid_search
  --test test_mcp_stdio_transport --test test_gpu_index_jsonrpc --no-run`,
  and `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rust-code-mcp --example benchmark_phases`.
- Step 8 update the ledger and commit: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `8111febfc5435e4db68027cdd7ab9c06fd707a8d` on change
  `vzkkvxrltnpkpqspzrmxytwwvxukqszv`, with no description set. Updated the
  Phase 11 ledger with changed files, MCP evidence, source-read results,
  verification, commit history, and remaining follow-up. The separate Phase 11
  completion report remains to be written and committed.
- Phase completion report: completed. Pre-step `jj show --summary` reported
  working-copy commit `38acda816bcd4f0d9467d80625b6b87b5c71bf21` on change
  `nvnylqzostnuoykrrynxmuwwwnmztwpk`, with no description set. Wrote
  `.docs/phase-11-boundrie-fix-report.md` and marked Phase 11 complete in the
  progress ledger.

### Phase 12: `rmc-graph` Visibility Tightening

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `bd54c6908c8042ae3e370500674a33794ebc1098` on change
  `kuxlmlrylsxmlvxksxnsmyxwsvlsrszo`, with no description set.
- Step 2 treat `rmc_graph::graph` as a compatibility facade: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `d98edfa467d0b00c8f4e2e93f905a2df04583d6e` on change
  `kupxotvnkszypusruwmvqylrvwlrpwsp`, with no description set. Refreshed the
  MCP hypergraph with `force_rebuild=true`, producing graph
  `da64f03ea621c18612caf4468a58b64f`. `get_exports` reported 96
  server-visible graph bindings and `get_reexports` reported 74 explicit
  facade reexports, confirming that `rmc_graph::graph` remains the
  compatibility facade while implementation modules are tightened underneath.
- Step 3 keep stable public graph groups visible: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `395df4c382a3267153f110dc6f0e57ed22fbb523` on change
  `vnqozyqnxywrlwqvykyzspypyrvymxwq`, with no description set. The stable
  public groups for this phase are snapshot build/open/cleanup APIs,
  `OpenedSnapshot`, root-reexported graph ID/model types, query DTOs, graph
  audit/similarity facades, and the codemap module used by server codemap
  tools as a graph-owned facade.
- Step 4 make implementation modules private when MCP evidence allows:
  completed. Pre-step `jj show --summary` reported working-copy commit
  `f3f8ab455af60ff50a0d89c90e46b122465e4e5d` on change
  `pzuxrlpwxqtpynklntypmsswxlxsosmo`, with no description set. Added graph
  wrappers for missing-docs and derive audits, migrated server/debug callers
  to root facade exports, and made avoidable graph implementation modules
  private. Kept `codemap`, `ids`, `model`, and `snapshot` public as stable or
  compatibility modules. Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rmc-graph -p rmc-server -p rust-code-mcp` and
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rust-code-mcp --example debug_itemscope --example spike_usages --example
  timing_extract`. Refreshed MCP evidence showed graph exports narrowed from
  96 to 88 server-visible bindings.
- Step 5 keep debug binaries/examples/tests working: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `856a5492b9096cdcf05e214c783e94ce9f236d01` on change
  `qpqnkzrxyqytxlnustulzmxorykvoozu`, with no description set. Touched debug
  examples now use root graph facade exports instead of private modules.
  Verification passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  --no-run`; touched debug examples had already checked successfully in Step 4.
- Step 6 keep compatibility reexports where still needed: completed.
  Pre-step `jj show --summary` reported working-copy commit
  `d68c06ba3e00640c4f65daf2339c23710f58b5dd` on change
  `kvywxwuslyrslwullmtzzxsqtnvvqkqp`, with no description set. The remaining
  public graph modules are `codemap`, `ids`, `model`, and `snapshot`.
  `codemap` remains public for active server codemap tools; `ids`, `model`,
  and `snapshot` remain compatibility/stable graph surface modules while root
  reexports remain the preferred public path.
- Step 7 run focused checks through the nix dev shell: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `c5a3067633e6cc466fc8167f711a9b2eb05bc70c` on change
  `snzmuzvlxwyrxptlxyqqmtwypstmvqmn`, with no description set. Verification
  passed with existing warnings:
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph
  --no-run`,
  `nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server
  --no-run`,
  `nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph
  -p rmc-server -p rust-code-mcp`, and
  `nix develop ../nix-devshells#cuda-code --command cargo check -p
  rust-code-mcp --example debug_itemscope --example spike_usages --example
  timing_extract`.
- Step 8 update the ledger and commit: completed. Pre-step
  `jj show --summary` reported working-copy commit
  `dfeec2a6e346e754e97e0da43d9b69d8548a4892` on change
  `kvpoywpkxnoynymspvyysuxytokuumww`, with no description set. Updated the
  Phase 12 ledger with changed files, verification, MCP evidence, commit
  history, compatibility exceptions, and remaining follow-up.
- Phase 12 completion report: completed. Pre-report `jj show --summary`
  reported working-copy commit
  `9a2c75de241ff9926a987cb8126338cce8aef311` on change
  `mymsyztxknxnokmtysyzyxrpqmpxzppx`, with no description set. Wrote
  `.docs/phase-12-boundrie-fix-report.md` and marked Phase 12 complete.

### Phase 13: Final Architecture Verification

- Step 1 `jj show --summary`: completed. Current working-copy commit was
  `730414f56507ec606ee1bb10cc6e88d5baa774f6` on change
  `nxmxwtquxzyxxxkqvttqqknvkpwsuqkk`, with no description set. `jj status`
  reported a clean working copy before Phase 13 verification started.
- Step 2 rebuild/reuse the hypergraph and refresh crate-level architecture
  checks: completed. Pre-step `jj show --summary` reported working-copy
  commit `21ad48c1b40488e02156761b125696a9d4650051` on change
  `vyvoptxyzvurwwvqoyrosylkqxlwqqzm`, with no description set.
  `build_hypergraph(force_rebuild=true)` produced graph
  `b9e01b5aeda04ae51a1c584f0512f8dc`, fingerprint
  `3d561d6c149beda49ab51ac0da17115f6de0d4ebeb8771c5e034de7968a57d10`.
  `crate_edges` returned 48 cross-crate edges, the core crate instability
  values remained low (`rmc_server=0.3333333333333333`,
  `rmc_indexing=0.125`, `rmc_graph=0.08333333333333333`,
  `rmc_engine=0.06666666666666667`), and the five-rule
  `forbidden_dependency_check` returned `violation_count=0`.

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

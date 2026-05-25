# Fixes 1717 Plan

## Purpose

This plan covers the follow-up fixes discovered during the side-by-side MCP comparison work:

- Phase 2 cache clear and health-check noise.
- Phase 5 warm hypergraph reuse latency.
- Phase 8 search latency variance.

The plan intentionally excludes Phase 8 result-ordering changes. Phase 8 work here is limited to latency investigation, sampling, and runtime object reuse.

## Repository Rules

- Use jujutsu first for VCS operations.
- Do not run `cargo fmt` or any formatting command.
- Run build and test commands through:

```sh
nix develop ../nix-devshells#cuda-code --command {command}
```

## Success Criteria

- Health probes do not create missing vector-store or BM25 state.
- Targeted cache clears do not immediately get rebuilt by background sync.
- Clear/index/search/sync operations do not race each other for the same workspace.
- Warm hypergraph reuse returns before rust-analyzer workspace loading when the persisted graph is reusable.
- Repeated warm search has lower setup overhead and less latency variance.
- All behavior changes are covered by focused tests.
- Public MCP tool behavior remains compatible unless the plan explicitly says otherwise.

## Non-Goals

- Do not remove public compatibility APIs such as existing constructors unless a later phase explicitly deprecates them.
- Do not change MCP tool schemas.
- Do not change search result ordering behavior in this plan.
- Do not introduce broad refactors unrelated to these fixes.

## Phase 0: Baseline And Guardrails

### Execution Status

- [x] Step 1: checked working copy with `jj status`. Current expected dirty files before the step commit were this new plan file and the pre-existing `.plans/mcp-side-by-side-comparison-2.md` change. The pre-existing side-by-side comparison plan change is unrelated and must stay out of fixes-1717 commits.
- [x] Step 2: recorded current relevant code paths.
- [x] Step 3: established targeted test commands.
- [ ] Step 4: record baseline MCP observations.

### Current Code Path Notes

- `crates/rmc-server/src/tools/endpoints/health.rs`: BM25 already uses read-only `open_bm25_search`, but vector health still calls `VectorStore::new_embedded`, which can create LanceDB state during a probe.
- `crates/rmc-server/src/tools/endpoints/cache.rs`: `clear_cache` directly deletes cache/index/vector directories and graph snapshots, but it is not passed `SyncManager`, so it cannot untrack a targeted workspace before deletion.
- `crates/rmc-server/src/tools/endpoints/query.rs`: search tracks successful indexed workspaces in `SyncManager`; missing/corrupt indexes are cleaned and rebuilt through `ensure_indexed`; `create_hybrid_search` constructs a fresh `EmbeddingGenerator` and `VectorStore` per call.
- `crates/rmc-server/src/mcp/sync.rs`: `SyncManager` already supports `track_directory`, `untrack_directory`, and listing tracked directories, but it has no global untrack API and no shared per-workspace operation lock.
- `crates/rmc-engine/src/vector_store/lancedb.rs`: `LanceDbBackend::new` creates the database directory, ensures the table exists, creates indexes, and writes metadata when absent. A separate read-only opener is needed for health.
- `crates/rmc-engine/src/search/mod.rs`: hybrid search owns `EmbeddingGenerator` and `VectorStore`; repeated query setup happens above this layer in server query construction. Result ordering changes are intentionally out of scope for this plan.
- `crates/rmc-engine/src/embeddings/mod.rs`: `EmbeddingGenerator::with_backend` constructs the runtime embedder; repeated server-side construction is a plausible contributor to warm-search latency variance.
- `crates/rmc-graph/src/graph/snapshot.rs`: `build_and_persist` calls `loader::load` before computing fingerprint and checking for an existing manifest, so warm reuse still pays the RA workspace load cost.
- `crates/rmc-graph/src/graph/loader.rs`: `loader::load` canonicalizes the workspace and loads rust-analyzer with dependencies, all targets, all features, tests, and prefilled caches. This is the expensive work Phase 4 should bypass on warm reuse.

### Targeted Test Commands

Use these as the focused command set while implementing the phases. Adjust filters to new test names as they are added.

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-engine lancedb
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server health
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server clear_cache
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server sync
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server query
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph snapshot
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server build_hypergraph
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-engine -p rmc-indexing -p rmc-graph -p rmc-server
```

### Goal

Capture the current behavior before changing production code, so regressions can be separated from known existing behavior.

### Steps

1. Check the working copy.
   - Run `jj status`.
   - Confirm only expected files are dirty.
   - Do not overwrite existing user changes.

2. Record the current relevant code paths.
   - `crates/rmc-server/src/tools/endpoints/health.rs`
   - `crates/rmc-server/src/tools/endpoints/cache.rs`
   - `crates/rmc-server/src/tools/endpoints/query.rs`
   - `crates/rmc-server/src/mcp/sync.rs`
   - `crates/rmc-engine/src/vector_store/lancedb.rs`
   - `crates/rmc-engine/src/search/mod.rs`
   - `crates/rmc-engine/src/embeddings/mod.rs`
   - `crates/rmc-graph/src/graph/snapshot.rs`
   - `crates/rmc-graph/src/graph/loader.rs`

3. Establish targeted test commands.
   - Prefer narrow package tests first.
   - Use nix shell for every cargo command.
   - Do not run formatting.

4. Record baseline MCP observations.
   - Health after missing cache/index/vector paths.
   - Targeted clear followed by delayed health check.
   - Warm `build_hypergraph(force_rebuild=false)`.
   - Repeated warm search timings.

### Expected Output

- A short note in the final implementation report with baseline observations and commands used.

### Estimated Change

- Production LOC: 0
- Test LOC: 0
- Documentation LOC: 10-30, if a report is created

## Phase 1: Read-Only Vector Store Opener

### Goal

Make health checks read existing vector-store state without creating missing LanceDB directories, tables, indexes, or metadata.

### Current Problem

`health_check` currently uses `VectorStore::new_embedded`, and that constructor can create vector-store state while the server is only probing health.

### Design

Add a read-only opener in the engine vector-store layer.

Proposed API:

```rust
impl VectorStore {
    pub async fn open_existing_embedded(db_path: &Path, table_name: &str) -> Result<Self> {
        // Validate required state exists.
        // Open existing LanceDB state only.
        // Do not create directories, tables, indexes, or metadata.
    }
}
```

The exact signature should follow existing `VectorStore` conventions if names or arguments differ.

### Steps

1. Inspect `VectorStore::new_embedded`.
   - Identify every write side effect.
   - Separate required validation from creation behavior.

2. Add the read-only opener.
   - Return `Err` when the database path is missing.
   - Return `Err` when the table is missing.
   - Return `Err` when required metadata is missing.
   - Do not call `create_dir_all`.
   - Do not create a table.
   - Do not create indexes.
   - Do not write metadata.

3. Refactor shared read-only pieces if useful.
   - Keep the compatibility constructor intact.
   - Avoid a broad vector-store cleanup in this phase.

4. Update server health.
   - Replace the vector health probe in `crates/rmc-server/src/tools/endpoints/health.rs`.
   - Use the read-only opener.
   - Preserve the existing health response shape.

5. Add focused tests.
   - Missing vector path returns unhealthy or absent state without creating files.
   - Missing table does not create a table.
   - Existing valid vector store opens successfully.
   - Health check does not create a missing vector path.

### Files

Modified:

- `crates/rmc-engine/src/vector_store/lancedb.rs`
- `crates/rmc-server/src/tools/endpoints/health.rs`

Likely modified tests:

- Existing vector-store tests under `crates/rmc-engine`
- Existing health endpoint tests under `crates/rmc-server`, if present

### Verification

Run focused tests through nix, for example:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-engine vector_store
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server health
```

Adjust test filters to match actual test names.

### Estimated Change

- Production LOC: 40-90
- Test LOC: 80-160
- Documentation LOC: 10-20

## Phase 2: Clear Cache And Sync Coordination

### Goal

Prevent targeted clears from being immediately rebuilt by background sync.

### Current Problem

Search and indexing paths can track workspaces for background sync. A targeted `clear_cache` can delete state while the same workspace is still tracked, allowing background sync to rebuild cleared state soon after the clear.

### Design

Pass the router's `SyncManager` into cache clearing code and untrack targeted directories before deleting cache/index/vector state.

### Steps

1. Trace router wiring.
   - Locate where search/index endpoints receive `SyncManager`.
   - Locate where `clear_cache` is registered and invoked.

2. Update cache endpoint dependencies.
   - Pass `SyncManager` into the cache endpoint handler.
   - Keep the public MCP tool schema unchanged.

3. Canonicalize targeted directory inputs.
   - Use the same canonicalization style as search/index paths.
   - Handle missing target directories consistently with current error behavior.

4. Untrack before deletion.
   - For targeted clears, call `untrack_directory` before clearing files.
   - Ensure untracking uses the same canonical path form that sync tracking uses.

5. Preserve global clear semantics.
   - Decide whether global clear should untrack all known directories or only clear storage.
   - Prefer explicit all-directory untracking if `SyncManager` exposes a safe API.
   - If no safe API exists, document the limitation and keep the behavior unchanged until Phase 3.

6. Add tests.
   - Targeted clear calls untrack before deletion.
   - Targeted clear does not re-add a directory to sync tracking.
   - Missing or invalid directory behavior remains compatible.

### Files

Modified:

- `crates/rmc-server/src/tools/endpoints/cache.rs`
- Router or tool registration module that wires endpoint dependencies
- `crates/rmc-server/src/mcp/sync.rs`, only if a small API addition is needed

Likely modified tests:

- Cache endpoint tests under `crates/rmc-server`
- Sync manager tests under `crates/rmc-server`

### Verification

Run focused tests through nix, for example:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server clear_cache
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server sync
```

Adjust test filters to match actual test names.

### Estimated Change

- Production LOC: 40-100
- Test LOC: 80-180
- Documentation LOC: 10-25

## Phase 3: Per-Workspace Operation Lock

### Goal

Coordinate operations that can read, write, or delete workspace-derived index state.

### Current Problem

Multiple MCP calls can overlap. Search, indexing, background sync, and clear operations can operate on the same workspace at the same time. The robust fix is a shared per-workspace lock.

### Design

Add a small server-side lock registry keyed by canonical workspace path.

Lock behavior:

- `clear_cache` takes an exclusive lock for the targeted workspace while deleting derived state.
- `index_codebase` takes an exclusive lock while writing derived state.
- Background sync takes an exclusive lock while writing derived state.
- Search takes a read lock for normal reads, and upgrades through the indexing path only when it needs to write.

If the existing async runtime and lock types make read/write lock ownership awkward, use a conservative mutex first and revisit read/write concurrency later.

### Steps

1. Locate existing shared server state.
   - Identify where `SyncManager`, tool state, and endpoint dependencies are owned.
   - Choose the narrowest place to store a lock registry.

2. Add lock registry type.
   - Key by canonical workspace path.
   - Use async-compatible locks.
   - Avoid holding a global map lock while awaiting long operations.

3. Wire lock registry into endpoints.
   - `query.rs`
   - indexing endpoint
   - `cache.rs`
   - `sync.rs`

4. Apply locking in write paths.
   - Protect forced indexing.
   - Protect sync indexing.
   - Protect cache deletion.

5. Apply locking around search paths that may trigger indexing.
   - Keep pure read paths lightweight where possible.
   - Ensure any fallback indexing path is coordinated.

6. Add concurrency tests.
   - Clear waits for indexing or indexing waits for clear.
   - Sync does not rebuild while targeted clear is active.
   - Search-triggered indexing does not overlap with clear.

### Files

New:

- A small lock-registry module under `crates/rmc-server/src`, exact location to follow existing server state layout

Modified:

- `crates/rmc-server/src/tools/endpoints/cache.rs`
- `crates/rmc-server/src/tools/endpoints/query.rs`
- Indexing endpoint module
- `crates/rmc-server/src/mcp/sync.rs`
- Router or state wiring module

### Verification

Run focused tests through nix, for example:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server cache
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server sync
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server query
```

Adjust test filters to match actual test names.

### Estimated Change

- Production LOC: 100-220
- Test LOC: 140-260
- Documentation LOC: 15-30

## Phase 4: Fast Hypergraph Reuse Preflight

### Goal

Make warm `build_hypergraph(force_rebuild=false)` reuse persisted graph state without loading the rust-analyzer workspace first.

### Current Problem

`build_and_persist` loads the rust-analyzer workspace before it checks whether a compatible persisted graph can be reused. That makes warm reuse take roughly the same expensive front-loaded cost as a rebuild.

### Design

Split graph snapshot creation into a fast reuse preflight and a slow rebuild path.

Fast preflight:

1. Canonicalize workspace root.
2. Compute workspace hash.
3. Compute fingerprint from relevant files and graph build options.
4. Derive graph id.
5. Check manifest compatibility.
6. Return a reused result immediately when the manifest is compatible and `force_rebuild=false`.

Slow rebuild path:

1. Load rust-analyzer workspace.
2. Extract graph.
3. Persist graph.
4. Write manifest.
5. Return non-reused result.

### Steps

1. Inspect current snapshot identity logic.
   - Find workspace hash calculation.
   - Find fingerprint calculation.
   - Find graph id derivation.
   - Find manifest compatibility checks.

2. Extract pure preflight helpers.
   - Avoid duplicating identity logic.
   - Keep helper ownership inside `rmc_graph`.
   - Do not make server responsible for graph identity internals.

3. Add fast reuse function.
   - Return `Option` or a typed preflight result.
   - Preserve existing error semantics for invalid roots and unreadable manifests.
   - Do not hide real corruption as successful reuse.

4. Reorder `build_and_persist`.
   - If `force_rebuild=false`, run preflight first.
   - If reusable, return before `loader::load`.
   - Otherwise continue to existing load/rebuild path.

5. Ensure manifest writes keep the same fields needed by preflight.
   - Add missing manifest fields only if required.
   - Keep backward compatibility if older manifests are still expected.

6. Add tests.
   - Warm reuse does not call loader.
   - `force_rebuild=true` calls loader.
   - Fingerprint change calls loader.
   - Missing manifest calls loader.
   - Incompatible schema calls loader or returns the existing expected error.

### Files

Modified:

- `crates/rmc-graph/src/graph/snapshot.rs`
- `crates/rmc-graph/src/graph/loader.rs`, only if a test seam is needed
- Graph snapshot tests under `crates/rmc-graph`

Possible new:

- `crates/rmc-graph/src/graph/snapshot/preflight.rs`, only if `snapshot.rs` is already too large for the new helper code

### Verification

Run focused tests through nix, for example:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph snapshot
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server build_hypergraph
```

Then run a small side-by-side MCP sample:

- First `build_hypergraph(force_rebuild=true)`.
- Then `build_hypergraph(force_rebuild=false)`.
- Confirm the second run returns `reused=true`.
- Confirm the second run no longer pays rust-analyzer load cost.

### Estimated Change

- Production LOC: 100-240
- Test LOC: 120-260
- Documentation LOC: 15-30

## Phase 5: Phase 8 Search Latency Sampling

### Goal

Separate stable latency regressions from runtime variance before adding lifecycle-sensitive caching.

### Current Problem

Fresh samples did not reproduce the earlier refactor slowdown. Search still performs expensive setup per request, so variance is expected even without a confirmed regression.

### Steps

1. Define a repeatable warm-search sample.
   - Same workspace.
   - Same query.
   - Same result limit.
   - Same existing indexes.
   - At least 10 runs per server when doing side-by-side measurement.

2. Record latency distribution.
   - Median.
   - Minimum.
   - Maximum.
   - p90 if sample size is sufficient.

3. Record setup actions per call.
   - Embedding generator construction.
   - Vector-store open.
   - BM25 open.
   - Hybrid search call.

4. Identify cacheable objects.
   - Confirm which objects are safe to share.
   - Confirm which objects must be recreated after clear or reindex.
   - Confirm whether underlying handles are `Send` and `Sync`.

5. Decide whether to proceed with Phase 6.
   - Proceed if per-call setup is a measurable part of warm search latency.
   - Defer if caching risk is higher than observed benefit.

### Files

Modified:

- None required for sampling-only work

Optional documentation:

- `.docs/fixes-1717-search-latency-notes.md`

### Verification

Run MCP side-by-side samples or focused local benchmarks through the normal server flow. Do not use microbenchmarks as the only evidence.

### Estimated Change

- Production LOC: 0
- Test LOC: 0
- Documentation LOC: 20-60

## Phase 6: Search Runtime Object Cache

### Goal

Reduce repeated warm-search setup overhead by caching expensive runtime objects with explicit invalidation.

### Current Problem

Each search constructs fresh runtime objects, including embedding generation and vector-store handles. This adds latency and variance to otherwise warm searches.

### Design

Add a small server-side cache keyed by search profile.

Cache key fields:

- Canonical workspace path.
- Embedding identity.
- Vector path.
- Tantivy path.

Cached objects:

- `EmbeddingGenerator`, if safe to share.
- `VectorStore`, if safe to share.
- `Bm25Search`, if safe to share and if read-only reopening remains the main cost.

Invalidation:

- Targeted `clear_cache`.
- Global `clear_cache`.
- `index_codebase(force_reindex=true)`.
- Profile mismatch.
- Any path where vector or Tantivy state is recreated.

### Steps

1. Confirm object sharing constraints.
   - Check trait bounds and runtime ownership.
   - Avoid caching objects that are not safe across async calls.

2. Add cache type.
   - Keep it server-side.
   - Do not move MCP-specific lifecycle into engine or indexing crates.
   - Use bounded size or explicit invalidation to avoid unbounded growth.

3. Wire cache into query path.
   - Build cache key from canonical paths and profile identity.
   - Reuse existing object when valid.
   - Fall back to current construction on miss.

4. Add invalidation hooks.
   - Invalidate targeted workspace on targeted clear.
   - Invalidate all relevant entries on global clear.
   - Invalidate on force reindex.
   - Invalidate on any path that recreates vector or BM25 state.

5. Add tests.
   - Repeated search reuses cached runtime objects.
   - Targeted clear invalidates only the targeted workspace.
   - Global clear invalidates all entries.
   - Force reindex invalidates the affected workspace.
   - Profile mismatch creates a separate cache entry.

6. Re-run latency sample.
   - Compare against Phase 5 baseline.
   - Report median and variance changes.

### Files

New:

- A search runtime cache module under `crates/rmc-server/src`, exact location to follow existing server state layout

Modified:

- `crates/rmc-server/src/tools/endpoints/query.rs`
- `crates/rmc-server/src/tools/endpoints/cache.rs`
- Indexing endpoint module
- Router or state wiring module

Likely modified tests:

- Query endpoint tests
- Cache endpoint tests
- Indexing endpoint tests

### Verification

Run focused tests through nix, for example:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server query
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server cache
```

Then run repeated warm search through MCP and compare against Phase 5 samples.

### Estimated Change

- Production LOC: 120-280
- Test LOC: 140-300
- Documentation LOC: 20-40

## Phase 7: Integration Verification

### Goal

Prove that the fixes work together and do not change the public MCP surface.

### Steps

1. Run targeted unit and integration tests.
   - Engine vector-store tests.
   - Server health/cache/query tests.
   - Graph snapshot tests.
   - Sync tests.

2. Run package checks if the environment permits.

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-engine -p rmc-indexing -p rmc-graph -p rmc-server
```

3. Run MCP behavior checks.
   - Health before and after missing state probes.
   - Targeted clear followed by delayed health.
   - Forced hypergraph build followed by warm reuse.
   - Repeated warm search sample.

4. Run MCP tool-surface check.
   - Confirm tool count unchanged.
   - Confirm parameter schemas unchanged.
   - Confirm changed behavior is limited to cache/readiness/performance semantics.

5. Inspect dependency boundaries.
   - Server may own runtime coordination and caches.
   - Engine may own vector-store opening behavior.
   - Graph may own graph snapshot preflight and reuse.
   - Indexing should not gain server dependencies.

6. Record results.
   - Add a short implementation report under `.docs/` if useful.
   - Include commands, pass/fail status, and any environment caveats.

### Expected Output

- Passing focused tests.
- No public MCP schema drift.
- No formatting-only churn.
- Clear report of any build/test limitations.

### Estimated Change

- Production LOC: 0-20
- Test LOC: 0-40
- Documentation LOC: 40-100

## Implementation Order

1. Phase 4: Fast hypergraph reuse preflight.
   - Biggest concrete performance win.
   - Mostly contained inside `rmc_graph`.

2. Phase 1: Read-only vector store opener.
   - Fixes health probe write side effects.
   - Enables cleaner Phase 2 behavior.

3. Phase 2: Clear cache and sync coordination.
   - Prevents immediate rebuild after targeted clear.

4. Phase 3: Per-workspace operation lock.
   - Robust coordination for overlapping MCP calls.
   - More invasive than Phase 2, so do after simple untracking.

5. Phase 5: Search latency sampling.
   - Confirms whether caching is worth the lifecycle complexity.

6. Phase 6: Search runtime object cache.
   - Useful only after sampling confirms the cost.

7. Phase 7: Integration verification.
   - Final pass over correctness, performance, and MCP compatibility.

## Commit Plan

Use separate commits when possible:

1. `graph: reuse warm hypergraph before ra load`
2. `engine: add read-only vector store opener`
3. `server: untrack sync dirs before cache clear`
4. `server: coordinate workspace index operations`
5. `server: document warm search latency samples`
6. `server: cache warm search runtime objects`
7. `docs: record fixes 1717 verification`

Before each commit:

- Run `jj status`.
- Confirm unrelated dirty files are not included accidentally.
- Use `jj diff` to review the intended change.

## Rollback Strategy

- If the graph preflight path causes correctness risk, keep helper extraction and disable the early return behind a narrow internal condition.
- If read-only vector opening is blocked by LanceDB API limitations, keep health from creating parent directories and document any unavoidable open side effect.
- If sync coordination is blocked by missing `SyncManager` APIs, add only the targeted untrack API first.
- If the operation lock creates deadlock risk, replace read/write locking with a conservative per-workspace mutex.
- If search runtime caching is unsafe, keep Phase 5 sampling documentation and defer Phase 6.

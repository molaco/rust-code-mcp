# Codexing Plan — Workspace Split, Compatibility First

This file is the authoritative migration plan for this repository.

It is intentionally self-contained. It is based on the codebase as it exists
today, and it resolves the contradictions in the earlier workspace plans:

- storage and hash compatibility come before cleanup
- `clear_cache` behavior is explicit
- file-scoped analysis tools stay usable without forcing persisted-graph startup
- storage migration and hash unification are deferred to a dedicated phase

No formatting work is part of this plan.

---

## 1. Purpose

Split the current single-crate `file-search-mcp` into a Rust workspace with
enforced boundaries, long-lived services, and explicit ownership, without
breaking existing on-disk data or casually changing tool behavior.

The migration is structural first, behavioral second, storage last.

---

## 2. Facts About The Current Repo

These are load-bearing facts from the current code and must shape the plan.

1. Search/index/cache/vector path derivation is duplicated and raw-input-based.
   Current sites include:
   - `src/tools/project_paths.rs`
   - `src/tools/clear_cache_tool.rs`
   - `src/tools/health_tool.rs`
   - `src/indexing/incremental.rs`

2. Graph snapshot storage is separate and uses canonicalized workspace input
   before hashing.
   Current sites include:
   - `src/graph/storage.rs`
   - `src/graph/ids.rs`
   - `src/tools/graph_tools.rs`

3. The repo currently has at least one real runtime singleton:
   - `src/semantic/mod.rs` defines `static SEMANTIC: LazyLock<Mutex<SemanticService>>`

4. Search and health tools still open expensive backends on request paths.
   Current sites include:
   - `src/tools/query_tools.rs`
   - `src/tools/health_tool.rs`

5. Graph tools open snapshots on the request path today.
   Current site:
   - `src/tools/graph_tools.rs`

6. File-scoped analysis tools are split across two implementations today:
   - `find_definition` / `find_references` use the semantic service
   - `get_dependencies` / `get_call_graph` / `analyze_complexity` use the syntax parser
   Current site:
   - `src/tools/analysis_tools.rs`

7. Rust 1.95.0 is already viable on this machine for the current crate:
   `cargo check --bin file-search-mcp` succeeds.
   The repo is not warning-clean yet, so strict workspace linting needs an
   explicit legacy carve-out during early phases.

These facts mean the migration must preserve multiple current path recipes
first, then unify them only in a storage-migration phase.

---

## 3. Goals

1. Create a workspace with capability boundaries that the compiler can enforce.
2. Remove runtime global state and construct services once in the server.
3. Centralize all storage-path logic in one leaf crate without breaking current
   data locations.
4. Share one embedder instance across search and graph-facing features.
5. Keep existing MCP tool semantics stable through the adapter phases.
6. Reduce duplicated parser responsibilities only after ownership is clear.
7. Defer storage layout changes and hash unification to a dedicated migration.

---

## 4. Non-Goals

This plan does not include:

- a new transport
- a new embedding backend
- replacing Tantivy, LanceDB, heed, sled, or rust-analyzer
- multi-tenancy
- automatic graph rebuild on every graph cache miss
- early storage/hash cleanup before compatibility is locked down
- formatting work

---

## 5. Target Workspace

### Crate Inventory

| Crate | Type | Purpose |
|---|---|---|
| `rcm-paths` | infra leaf | Own every storage-path and workspace-hash derivation. Phase 1 preserves current v1 behavior exactly, even when the current repo uses different recipes for different stores. |
| `rcm-ra-syntax` | infra leaf | Narrow `ra_ap_syntax` re-exports for syntax-only extraction. |
| `rcm-ra-host` | infra leaf | `RootDatabase` + `Vfs` lifecycle wrapper for IDE and graph code. |
| `rcm-embedding` | infra leaf | Sealed embedder trait, production embedder, deterministic test fake. |
| `rcm-search` | capability | Indexing, chunking, Tantivy, LanceDB, metadata cache, Merkle change detection, hybrid search. |
| `rcm-graph` | capability | Persisted hypergraph build/open/query/audits. Owns snapshot-backed tools only. |
| `rcm-ide` | capability | Live rust-analyzer navigation and file-scoped analysis tools: `find_definition`, `find_references`, `get_dependencies`, `get_call_graph`, `analyze_complexity`. |
| `rcm-server` | bin + thin lib | rmcp router, params, config, sync shell, composition root, server-only composition tools. |
| `xtask` | tooling | Workspace automation, smoke checks, policy checks, optional storage migration. |

### Dependency Rules

```text
rcm-server  -> { rcm-search, rcm-graph, rcm-ide, rcm-paths, rcm-embedding }
rcm-search  -> { rcm-ra-syntax, rcm-embedding, rcm-paths }
rcm-graph   -> { rcm-ra-host, rcm-embedding, rcm-paths }
rcm-ide     -> { rcm-ra-host, rcm-ra-syntax, rcm-paths }
rcm-ra-host -> { rcm-ra-syntax }

Forbidden:
- search -> graph
- graph -> search
- search -> ide
- ide -> search
- graph -> ide
- ide -> graph
```

`xtask` is excluded from runtime dependency policy checks.

---

## 6. Non-Negotiable Decisions

### 6.1 Compatibility-First Path Policy

`rcm-paths` becomes the only owner of storage derivation, but Phase 1 does
not invent a cleaned-up recipe.

It preserves current behavior exactly:

- raw-input-based search/index/cache/vector derivation
- raw-input-based Merkle snapshot naming
- canonical-workspace-based graph snapshot derivation

If those behaviors are ugly, they still remain the contract until the explicit
storage-migration phase.

There is no early canonicalization cleanup.

### 6.2 No Hash Unification Before Storage Migration

The current repo does not have one storage recipe. It has several.

Phase 1 centralizes them.
Phase 7 is the first phase allowed to replace them with one canonical recipe.

### 6.3 `clear_cache` Behavior Is Explicit

Before storage migration:

1. Delete on-disk artifacts for the targeted workspace.
2. Invalidate in-memory service handles.
3. `search` lazily rebuilds on the next request.
4. Graph queries still require an explicit `build_hypergraph` after graph data
   was cleared.

That asymmetry is intentional and preserved until a separate product decision.

### 6.4 One Embedder Per Process

The server constructs one embedder and shares it by `Arc`.

No request path or background job constructs its own production embedder.

### 6.5 No Runtime Global Mutex State

No `LazyLock<Mutex<...>>`, `OnceLock<Mutex<...>>`, or equivalent runtime
singletons survive past the singleton-removal phase.

Per-service-instance locking is allowed when required by upstream types.

### 6.6 File-Scoped Analysis Does Not Depend On Persisted Graph State

`find_definition`, `find_references`, `get_dependencies`, `get_call_graph`,
and `analyze_complexity` remain usable without requiring the user to build a
hypergraph first.

Therefore these tools belong to `rcm-ide`, not `rcm-graph`.

If the product later wants graph-backed replacements, that is a separate change
and must explicitly decide between:

- auto-building snapshots for those tools
- requiring `build_hypergraph`

This migration does not smuggle in that behavior change.

### 6.7 API Boundaries

- Strict-tier crates do not leak `tantivy`, `lancedb`, `heed`, `sled`,
  `ra_ap_*`, `fastembed`, `ort`, or `rmcp` types in public APIs.
- Only `rcm-server` serializes responses.
- Capability DTOs are not `Serialize` by default.

### 6.8 Async Boundary

- Leaves are sync by default.
- Capability methods may be async.
- `rcm-server` owns the runtime.
- Blocking rust-analyzer and embedding work is wrapped at the boundary.

### 6.9 Error Policy

- operation-scoped `thiserror` enums in leaves and capabilities
- `anyhow` only in `rcm-server` and tooling
- no god-enum

### 6.10 No Storage Layout Change Before The Storage Phase

No early path renames, no root-layout cleanup, no v2 workspace partition, and
no backfilled manifest files before the explicit storage-migration phase.

---

## 7. Phase Order

```text
Phase 0 -> Phase 1 -> Phase 2 -> Phase 3 -> Phase 4 -> Phase 5 -> Phase 6 -> Phase 8

Phase 7 is optional and lands only after Phase 4 or later.
If Phase 7 is skipped, Phase 8 still removes legacy while keeping v1 storage.
```

---

## 8. Phase Plan

### Phase 0 — Baseline, Fixtures, And Guardrails

**Goal**

Freeze current behavior so the migration can be judged against something real.

**Implementation**

1. Add a small fixture workspace under `fixtures/sample-workspace/`.
2. Add a smoke harness in `xtask` or equivalent automation that exercises:
   - `index_codebase`
   - `search`
   - `find_definition`
   - `find_references`
   - `get_dependencies`
   - `get_call_graph`
   - `analyze_complexity`
   - `build_hypergraph`
   - representative graph queries
   - `clear_cache` followed by `search`
3. Add compatibility tests that freeze current v1 storage derivation behavior.
   These tests must cover:
   - raw input directory hash
   - graph workspace hash
   - Merkle snapshot naming
   - vector collection naming
4. Record the current warning baseline and treat it as legacy debt, not hidden
   debt.
5. Verify the current crate still builds on Rust 1.95.0.

**Checklist**

- [ ] `fixtures/sample-workspace/` exists and is small enough for fast smoke runs.
- [ ] A smoke harness exists and runs against the current binary.
- [ ] Compatibility tests pin current storage derivation behavior.
- [ ] The current warning set is explicitly tracked.
- [ ] `cargo check --bin file-search-mcp` passes on Rust 1.95.0.

**Rollback**

This phase is additive. Revert the fixture and tooling commits.

---

### Phase 1 — Workspace Shell And `rcm-paths`

**Goal**

Wrap the repo in a workspace and create one compatibility owner for every
current storage-path rule.

**Implementation**

1. Move the current crate to `crates/file-search-mcp-legacy/`.
2. Add a virtual root `Cargo.toml`, `xtask`, and placeholder crates.
3. Implement `rcm-paths` with explicit v1-compatible path families rather than
   a fake unified model.
4. Replace local hash/path logic in all current runtime sites with calls into
   `rcm-paths`.
5. Pin `rust-toolchain.toml` and `flake.nix` to Rust 1.95.0 after verification.
6. Keep a temporary legacy lint carve-out. Use `--lib --bins` CI gates until
   stale examples and benches are retired.

**Mandatory call sites to centralize**

- `src/tools/project_paths.rs`
- `src/tools/clear_cache_tool.rs`
- `src/tools/health_tool.rs`
- `src/indexing/incremental.rs`
- `src/graph/storage.rs`
- any remaining runtime hash sites found by grep

**Checklist**

- [ ] The repo builds as a workspace.
- [ ] Runtime code no longer implements ad-hoc workspace hashing outside `rcm-paths`.
- [ ] Existing search/vector/cache/Merkle/graph data is still found at the same locations.
- [ ] The smoke harness passes against the legacy binary inside the workspace.
- [ ] Rust 1.95.0 is the pinned toolchain.

**Rollback**

Revert the workspace wrap and `rcm-paths` introduction together.

---

### Phase 2 — Facade Crates And `rcm-server`

**Goal**

Create the real crate boundaries while keeping legacy code as the execution
engine behind adapters.

**Implementation**

1. Implement the public APIs for:
   - `rcm-ra-syntax`
   - `rcm-ra-host`
   - `rcm-embedding`
   - `rcm-search`
   - `rcm-graph`
   - `rcm-ide`
   - `rcm-server`
2. Keep `file-search-mcp-legacy` as a private dependency only.
3. Use per-crate `legacy_adapter` modules for conversions.
4. Route MCP tools through `rcm-server`.
5. Make crate ownership explicit:
   - `rcm-search`: indexing + retrieval
   - `rcm-graph`: persisted snapshot queries/audits
   - `rcm-ide`: live navigation + file-scoped analysis tools
6. Retire or quarantine stale examples and benches before widening target
   coverage in CI.

**Checklist**

- [ ] `cargo build -p rcm-server` succeeds.
- [ ] The smoke harness passes through the new binary.
- [ ] Capability public APIs do not leak backend types.
- [ ] `file-search-mcp-legacy` is not exposed in any public API.
- [ ] Dependency policy is enforced by automation.

**Rollback**

Revert the `rcm-server` flip and point the binary back at legacy directly.

---

### Phase 3 — Remove Hidden Singletons And Centralize Composition

**Goal**

Construct services once in the server and remove runtime global state.

**Implementation**

1. Replace `static SEMANTIC` with an `IdeService` instance owned by
   `rcm-server`.
2. Centralize service construction in `main`.
3. Ensure `SyncManager` is constructed once and shared explicitly.
4. Stop constructing production embedders from arbitrary request paths.
5. Add grep-based regression checks for runtime singleton patterns.

**Checklist**

- [ ] `src/semantic/mod.rs` no longer defines a runtime singleton.
- [ ] No runtime `LazyLock<Mutex<...>>` or equivalent remains in runtime crates.
- [ ] Services are constructed once in `rcm-server::main`.
- [ ] Concurrent IDE queries no longer serialize through one global lock.
- [ ] The smoke harness passes.

**Rollback**

Revert the service wiring and temporarily restore the singleton only if needed.

---

### Phase 4 — Errors, Long-Lived Resources, And `clear_cache`

**Goal**

Replace ad-hoc per-request opens with long-lived service resources and make
error and invalidation behavior explicit.

**Implementation**

1. Introduce operation-scoped errors:
   - `PathError`
   - `RaError`
   - `EmbedError`
   - `SearchError`
   - `IndexError`
   - `QueryError`
   - `AuditError`
   - `IdeError`
2. Remove `anyhow` from leaf and capability public APIs.
3. Move search resources to long-lived service state:
   - Tantivy reader
   - Tantivy writer
   - LanceDB connection
   - metadata cache handle
4. Move graph snapshot state behind a long-lived graph service.
5. Move IDE project-host caching behind `IdeService`.
6. Rework `clear_cache` to:
   - delete all targeted on-disk artifacts
   - invalidate in-memory handles
   - preserve the current asymmetry:
     - `search` lazily rebuilds
     - graph tools require `build_hypergraph` after graph data was cleared
7. Add graceful shutdown with a cancellation token and bounded drain.

**Checklist**

- [ ] Search hot paths no longer construct `Bm25Search` and `VectorStore` per request.
- [ ] Graph tool handlers no longer own snapshot-open logic directly.
- [ ] `clear_cache` deletes every relevant store family for a workspace:
      keyword/search, vector, metadata, Merkle, and graph.
- [ ] `clear_cache` followed by `search` works without a restart.
- [ ] `clear_cache` followed by a graph query returns an explicit
      `build_hypergraph`-first error.
- [ ] Shutdown on SIGINT or stdin EOF completes cleanly.
- [ ] The smoke harness passes.

**Rollback**

Revert service-lifetime changes per capability if needed. Keep the new error
types if they are already public and stable.

---

### Phase 5 — Sealed Embedder And Feature Gating

**Goal**

Make embedding lifecycle explicit and testable.

**Implementation**

1. Introduce a sealed embedder trait in `rcm-embedding`.
2. Add:
   - production embedder
   - deterministic fake embedder for tests
3. Make capability crates depend on the trait, not the concrete production
   implementation.
4. Keep embedding-dependent graph features optional where appropriate.
5. Convert unit tests away from real ONNX startup.

**Checklist**

- [ ] `rcm-search` does not pull `fastembed` or `ort` directly.
- [ ] Unit tests can use a deterministic embedder without real model startup.
- [ ] Full binary behavior for embedding-backed tools still works.
- [ ] The smoke harness passes.

**Rollback**

Inline the production embedder again if necessary, but keep the single
construction site in the server.

---

### Phase 6 — Move File-Scoped Analysis Into `rcm-ide` And Shrink Parser Scope

**Goal**

Keep file-scoped tools usable without a hypergraph build, while removing broad
parser ownership from the legacy crate.

**Implementation**

1. Route these tools through `rcm-ide`:
   - `find_definition`
   - `find_references`
   - `get_dependencies`
   - `get_call_graph`
   - `analyze_complexity`
2. Move any surviving syntax-only file-analysis helpers into `rcm-ide`.
3. Keep `rcm-search` responsible only for chunking-context extraction needed by
   indexing and retrieval.
4. Keep `rcm-graph` responsible only for persisted snapshot build/open/query.
5. Delete legacy parser modules only after equivalent replacements exist in the
   new owners.
6. Do not introduce a snapshot prerequisite for file-scoped tools in this
   phase.
7. Do not change Tantivy schema or storage layout in this phase.

**Checklist**

- [ ] All five file-scoped analysis tools route through `rcm-ide`.
- [ ] Those tools still work on a workspace that has never built a hypergraph.
- [ ] `rcm-search` no longer depends on the broad legacy parser surface.
- [ ] No storage layout or Tantivy schema change ships in this phase.
- [ ] The smoke harness passes.

**Rollback**

Restore legacy parser-backed call sites if needed. This phase must remain
reversible without a data migration.

---

### Phase 7 — Optional Storage V2 Migration And Hash Unification

**Goal**

Only now unify storage derivation and move to a workspace-partitioned layout.

**Implementation**

1. Introduce a v2 layout such as:
   - `<data>/rust-code-mcp/workspaces/<workspace_hash>/keyword`
   - `<data>/rust-code-mcp/workspaces/<workspace_hash>/vector`
   - `<data>/rust-code-mcp/workspaces/<workspace_hash>/metadata`
   - `<data>/rust-code-mcp/workspaces/<workspace_hash>/merkle`
   - `<data>/rust-code-mcp/workspaces/<workspace_hash>/graph`
2. Add `xtask migrate-storage` with:
   - `--dry-run`
   - `--resume`
   - explicit version arguments if needed
3. Make this the first phase allowed to collapse the current multiple path
   recipes into one canonical workspace-based recipe.
4. Expand `clear_cache` to support scoped deletion if desired.
5. Keep migration idempotent and resumable.

**Checklist**

- [ ] `xtask migrate-storage --dry-run` prints a complete plan.
- [ ] The real migration is resumable after interruption.
- [ ] Existing v1 data remains readable during the compatibility window.
- [ ] After migration, the smoke harness passes on migrated workspaces.
- [ ] Only after this phase may `rcm-paths` remove v1 compatibility helpers.

**Rollback**

Only via the explicit migration tooling or manual cleanup. This is the first
phase with real operational risk.

---

### Phase 8 — Remove Legacy

**Goal**

Delete `file-search-mcp-legacy` only after behavior lives in real crates.

**Implementation**

1. Remove the legacy crate from the workspace.
2. Delete all `legacy_adapter` modules.
3. Tighten dependency checks to forbid any new legacy references.
4. Keep only intentional compatibility code:
   - v1 path compatibility, if Phase 7 was skipped
   - v2 migration compatibility, if Phase 7 shipped

**Checklist**

- [ ] `cargo tree --workspace` contains no legacy crate.
- [ ] No adapter modules remain.
- [ ] The smoke harness passes.
- [ ] Workspace build and policy checks are green.

**Rollback**

Restore legacy from version control if absolutely necessary.

---

## 9. Smoke Contract

Run the smoke harness after every phase.

Minimum tool set:

- `index_codebase`
- `search`
- `find_definition`
- `find_references`
- `get_dependencies`
- `get_call_graph`
- `analyze_complexity`
- `build_hypergraph`
- representative graph queries
- `clear_cache` followed by `search`

Expected graph-cache behavior before Phase 7 or a separate graph-auto-rebuild
decision:

- after clearing graph data, graph tools should ask for `build_hypergraph`
  explicitly

That is not a failure. That is the current preserved contract.

---

## 10. Exit Criteria

The migration is done only when all of the following are true:

- the workspace is the normal build shape
- no runtime singleton remains
- all storage derivation lives in `rcm-paths`
- request hot paths do not reopen core backends every time
- embedding lifecycle is explicit and testable
- file-scoped analysis has a clear owner in `rcm-ide`
- persisted graph has a clear owner in `rcm-graph`
- `file-search-mcp-legacy` is gone

If Phase 7 is skipped, that is acceptable. Storage v1 compatibility simply
remains part of the product until a later migration.

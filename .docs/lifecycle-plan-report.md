# Lifecycle Plan Implementation Report

Date: 2026-05-26

Plan: `.plans/lifecycle-plan.md`

Repository: `/home/molaco/Documents/rust-code-mcp-refactor`

## Executive Summary

The lifecycle/CUDA isolation plan was implemented across six committed phases.
The work addresses the observed stuck `D`-state process class by reducing
accidental CUDA linkage and initialization in graph/test paths, replacing heavy
rust-analyzer fixtures in small tests, adding explicit MCP runtime ownership,
documenting safe stuck-process diagnostics, and making MCP automatic/background
defaults conservative.

The final committed behavior is intentionally layered:

1. Graph-only builds no longer link CUDA-capable embedding/vector dependencies
   by default.
2. Skeleton unit tests avoid loading the real workspace through rust-analyzer.
3. Incremental indexing avoids constructing embedding/vector resources when the
   Merkle no-change fast path can return first.
4. The MCP server owns background sync, search cache, semantic service, and
   shutdown lifecycle through `ServerRuntime`.
5. Stuck-process diagnostics are documented with commands that avoid unsafe
   `/proc/$pid/cmdline` reads for `D`-state tasks.
6. MCP startup now disables background sync by default and uses
   `local-cpu-small` for automatic/default MCP embedding selection; local CUDA
   remains explicit foreground opt-in.

No formatting command was run.

## Commit Summary

| Phase | Commit | Description |
| --- | --- | --- |
| 1 | `7b1258a7` | Isolate CUDA from graph-only builds |
| 2 | `a83f77bb` | Use synthetic fixture for skeleton tests |
| 3 | `287a67eb` | Lazy initialize incremental indexing resources |
| 4 | `e7b95ce8` | Add explicit MCP runtime lifecycle |
| 5 | `3799a8c4` | Document safe stuck-process diagnostics |
| 6 | `0726579f` | Conserve MCP operational defaults |

## Phase Details

### Phase 1: CUDA Isolation For Graph Builds

`rmc-engine` embedding, CUDA, vector-store, and hybrid-search dependencies were
split behind explicit features. `rmc-graph` no longer pulls the heavy
embedding/CUDA path in default graph-only builds; semantic embedding paths are
behind `semantic-embeddings`.

Key outcome: the focused `rmc-graph` skeleton test binary built without CUDA
dynamic dependencies. The `readelf` check for `libcuda`, `libcublas`,
`libcudart`, and `libnvrtc` returned no matches for the selected test binary.

### Phase 2: Synthetic Skeleton Fixtures

Skeleton collect/render unit tests now use a small synthetic temp Cargo package
instead of loading the real `rmc-graph` workspace. The fixture covers the
source shapes needed by skeleton behavior: functions, impl methods, trait
associated items, const/static items, docs/attrs, nested modules, and test-item
pruning.

Key outcome: the skeleton test filter completed without the full real-workspace
rust-analyzer worker set that had appeared in the stuck-process incidents.

### Phase 3: Lazy Incremental Indexing Resources

The incremental indexing path now checks the Merkle no-change fast path before
constructing `UnifiedIndexer`, `VectorStore`, or embedding/model resources.
Tests verify the no-change path can return before the underlying indexer is
initialized.

Key outcome: unchanged automatic or background indexing work avoids needless
embedding/vector startup, reducing the chance of touching CUDA/NVIDIA driver
state during no-op work.

### Phase 4: Explicit MCP Runtime Lifecycle

`ServerRuntime` was introduced as the owner for long-lived MCP resources:
background sync task state, shutdown signaling, workspace locks, search runtime
cache, and semantic service state. The stdio server now starts through the
runtime and calls graceful shutdown after service exit.

Runtime management tools were added:

- `runtime_status`
- `clear_runtime`

Supporting cleanup/status APIs were added for search cache, semantic service,
and sync tracking. Cache-producing read endpoints participate in workspace
locks so `clear_runtime` serializes with operations that can create cached
runtime resources.

Review gate: 8.8/10, pass. Residual noted in the plan: an already-active
background sync cycle may finish after a clear if it copied the tracked
directory list before the clear, but background task cancellation is still
owned by `ServerRuntime::shutdown_gracefully`.

During this phase, a review-fix validation path reproduced a stuck
rust-analyzer-backed graph static fixture process in `D` state at `do_exit`.
That fixture was replaced with a manual persisted graph fixture.

### Phase 5: Stuck-Process Diagnostics Runbook

Added `.docs/stuck-process-diagnostics.md` with safe incident collection and
triage guidance. The runbook emphasizes `/proc/$pid/stat`, `/proc/$pid/status`,
`/proc/$pid/wchan`, `/proc/$pid/stack`, `/proc/$pid/maps`, and `/proc/$pid/fd`
collection while avoiding `cmdline` reads for already stuck `D`-state tasks.

It also documents exact-PID and typed-confirmation process-group termination,
hibernation blocker triage, and NVIDIA-specific `wchan` interpretation.

Review gate: 9.5/10, pass, after adding a process-group member preview and
typed process-group confirmation guard.

### Phase 6: Conservative MCP Operational Defaults

MCP startup and tool defaults were changed to reduce accidental background CUDA
interaction:

- `RMC_BACKGROUND_SYNC` is disabled by default and enabled only for
  `1`, `true`, `yes`, or `on` after trimming and case-folding.
- Startup logs the background-sync state, raw env var value, accepted enabled
  values, automatic/background embedding profile, and whether CUDA-capable
  features are compiled.
- Omitted MCP embedding profile/model now resolves to `local-cpu-small` at the
  MCP boundary without changing `EmbeddingBackend::default()` globally.
- Router calls pass a background sync manager only when background sync is
  enabled.
- `SyncManager::track_directory_for_backend` centralizes the rule that
  background sync can track CPU or remote embedding profiles, but not local
  CUDA profiles.
- `index_codebase` and query/index paths use backend-aware background
  tracking, so explicit local CUDA foreground commands are not registered for
  background sync.
- Background sync also skips local CUDA-backed existing profiles if they are
  discovered under a tracked workspace.
- `build_codemap` gained an optional `embedding_profile` for prompt-driven
  HybridSearch seed lookup. Default lookup uses `local-cpu-small`; explicit
  local CUDA profiles remain available for foreground use.
- MCP schema/user-facing descriptions were updated to stop implying an
  implicit local-GPU default.

Review gate:

- First review: 8.1/10, fail. Findings were background tracking of local CUDA
  requests and missing `build_codemap` explicit profile support.
- Final review after fixes: 9.2/10, pass.

## Validation

Validation commands were run through the required dev shell:

```bash
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib --features semantic-embeddings
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton --no-run
readelf -d target/debug/deps/rmc_graph-c878c3b05c79b3a8 | rg 'libcuda|libcublas|libcudart|libnvrtc'
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo tree -p rmc-graph --no-default-features --depth 1
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-indexing --lib no_change_skips_unified_indexer_initialization
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib get_similar_code_waits_for_workspace_lock
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib similar_to_item_waits_for_workspace_lock
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib build_codemap_waits_for_workspace_lock_before_search_cache_use
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib runtime
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib background_sync
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib automatic
nix develop ../nix-devshells#cuda-code --command cargo check -p rust-code-mcp
```

The final Phase 6 validation rerun passed:

- `cargo test -p rmc-server --lib background_sync`: 5 passed.
- `cargo test -p rmc-server --lib automatic`: 3 passed.
- `cargo check -p rmc-server --lib`: passed.
- `cargo check -p rust-code-mcp`: passed.

Cargo emitted existing dead-code warnings in lower crates and reported the
adjacent `../nix-devshells` git tree as dirty.

## Residual Risks

This plan reduces the most likely accidental CUDA/NVIDIA-driver interactions,
but it cannot make a process already stuck in kernel `D` state recover. A task
blocked in `do_exit` or inside a driver wait can ignore kill signals until the
kernel wait resolves.

Local CUDA support remains available. Users can still explicitly select
`local-gpu-small` or another local Qwen3 profile for foreground indexing,
querying, or codemap seed lookup. Those explicit foreground paths can still
exercise CUDA and should be handled with the stuck-process runbook if the
driver/kernel interaction repeats.

Background sync intentionally skips local CUDA profiles. Existing local CUDA
indexes will not be refreshed automatically by background sync; they require an
explicit foreground indexing command with the CUDA profile.

The Phase 4 residual remains: a sync cycle already in progress may finish after
runtime clear if it copied the tracked-directory list before the clear. Process
shutdown still goes through `ServerRuntime::shutdown_gracefully`, which requests
shutdown and joins/aborts owned runtime tasks.

## Operational Notes

For normal MCP startup, background sync is off:

```bash
rust-code-mcp
```

To enable CPU/remote-profile background sync explicitly:

```bash
RMC_BACKGROUND_SYNC=1 rust-code-mcp
```

To opt into local CUDA for foreground indexing:

```text
embedding_profile = "local-gpu-small"
```

Do not use broad command-line process scans or `pkill -f` as the first response
to a suspected stuck process. Follow `.docs/stuck-process-diagnostics.md`.

# Plan: resource lifecycle and CUDA isolation for MCP/test stability

Status: completed. Written against `/home/molaco/Documents/rust-code-mcp-refactor`
on 2026-05-26.

Progress:

- Phase 1 completed on 2026-05-26. CUDA-capable embedding/vector dependencies
  are now feature-gated out of default `rmc-graph` builds, and the focused
  skeleton test binary built with default graph features has no CUDA dynamic
  dependencies.
- Phase 2 completed on 2026-05-26. Skeleton collect/render unit tests now use a
  synthetic temp Cargo package instead of loading the real `rmc-graph`
  workspace through rust-analyzer.
- Phase 3 completed on 2026-05-26. Incremental no-change detection now runs the
  Merkle fast path before constructing `UnifiedIndexer`, `VectorStore`, or
  embedding/model resources.
- Phase 4 completed on 2026-05-26. The MCP server now has an explicit
  runtime owner for background sync, search cache, semantic state, shutdown,
  and runtime status/cleanup tools. The graph static metadata tests now use a
  manual persisted graph fixture instead of the removed semantic singleton or a
  rust-analyzer workspace load.
- Phase 5 completed on 2026-05-26. Added a stuck-process diagnostics runbook
  under `.docs` with safe `/proc` collection commands, candidate discovery that
  avoids command-line reads, PID/process-group kill guidance, hibernation
  blocker triage, and NVIDIA-specific `wchan` notes.
- Phase 6 completed on 2026-05-26. MCP startup is conservative: background
  sync is disabled unless `RMC_BACKGROUND_SYNC` is explicitly set to an
  enabled value, automatic/default MCP embedding selection uses
  `local-cpu-small`, background tracking skips local CUDA embedding profiles,
  and prompt-driven `build_codemap` can explicitly opt into a non-default
  profile. Review gate passed at 9.2/10.

This plan addresses the stuck D-state process class observed while agents run
MCP tools and focused Rust tests. The latest live incident was:

- `rmc_graph-8dd91`, PID 205235, stuck in `D` state at `do_exit`.
- Executable:
  `/home/molaco/Documents/rust-code-mcp-refactor/target/debug/deps/rmc_graph-8dd91074aaab35b8`.
- CWD:
  `/home/molaco/Documents/rust-code-mcp-refactor/crates/rmc-graph`.
- Threads included rust-analyzer workers such as `VfsLoader`,
  `PrimeCaches#*`, `ParseNodeDroppe`, and `SpanMapDropper`.
- The binary was linked against CUDA libraries through the workspace dependency
  graph, even though the failing skeleton tests did not need embeddings.

The earlier incident involved a `rustc` task stuck in D-state during exit /
coredump handling. Both incidents point at the same broad hazard: very large
Rust/rust-analyzer/CUDA-linked processes being killed or torn down while the
kernel and NVIDIA driver have active memory mappings.

## Core conclusion

Add lifecycle management, but do not make it the only fix.

Lifecycle management helps the long-running MCP server shut down gracefully,
cancel background work, and drop caches. It does not run after SIGKILL, and it
cannot fix a test process already stuck in kernel `do_exit`.

The primary fix is therefore dependency and execution isolation:

1. Do not link CUDA into graph-only test binaries.
2. Do not initialize CUDA before cheap no-op checks.
3. Do not use full real-workspace rust-analyzer loads in small unit tests.
4. Add explicit MCP runtime lifecycle and status/cleanup tools for the
   long-lived server process.

## Phase 1: feature-split CUDA out of graph-only code

Goal: `cargo test -p rmc-graph --lib skeleton` should not produce a test binary
with `NEEDED libcuda.so.1`.

Status: completed on 2026-05-26.

Completed implementation:

- Made workspace `fastembed` feature-neutral by default.
- Added `rmc-engine` feature gates for `embeddings`, `embeddings-cuda`,
  `vector-store`, and `hybrid-search`.
- Made `rmc-engine` embedding/vector dependencies optional.
- Kept default `rmc-graph` independent of `rmc-engine`; graph semantic
  embedding paths are behind the `semantic-embeddings` feature.
- Gated graph embedding cache, semantic overlap exports, and codemap
  embedding rerank/cache access behind `semantic-embeddings`.
- Updated `rmc-config`, `rmc-indexing`, and `rmc-server` to request the
  explicit engine/graph features they need.

Validation performed:

```bash
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib --features semantic-embeddings
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton --no-run
readelf -d target/debug/deps/rmc_graph-c878c3b05c79b3a8 | rg 'libcuda|libcublas|libcudart|libnvrtc'
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo tree -p rmc-graph --no-default-features --depth 1
```

The CUDA `readelf` check returned no matches. The `NEEDED` entries for the
default skeleton test binary were limited to standard system libraries:
`libgcc_s`, `libm`, `libc`, and `ld-linux`.

Original problem addressed by this phase:

- Workspace `fastembed` was globally configured with the `cuda` feature.
- `rmc-engine` unconditionally depended on `fastembed` and `candle-core`.
- `rmc-graph` depended on all of `rmc-engine`, mostly for embedding-adjacent
  types and optional graph similarity paths.
- As a result, graph tests that only exercised skeleton rendering were still
  CUDA-linked.

Implemented shape:

1. Convert workspace `fastembed` to a feature-neutral dependency.

   ```toml
   fastembed = { version = "5.13.4", default-features = false }
   ```

2. Added `rmc-engine` features.

   ```toml
   [features]
   default = ["core"]
   core = []
   embeddings = [
     "dep:reqwest",
     "dep:hf-hub",
     "dep:fastembed",
     "dep:tokenizers",
     "dep:futures",
     "dep:tokio",
     "dep:toml",
     "fastembed/hf-hub-native-tls",
     "fastembed/ort-load-dynamic",
   ]
   embeddings-cuda = [
     "embeddings",
     "dep:candle-core",
     "fastembed/qwen3",
     "fastembed/cuda",
   ]
   vector-store = [
     "embeddings",
     "dep:lancedb",
     "dep:arrow-array",
     "dep:arrow-schema",
     "dep:async-trait",
     "dep:directories",
     "dep:futures",
     "dep:tokio",
   ]
   hybrid-search = [
     "embeddings",
     "vector-store",
     "dep:anyhow",
     "dep:tokio",
   ]
   ```

3. Marked embedding/vector dependencies optional in `rmc-engine`.

4. Gated modules in `rmc-engine`:

   - Always available: parser, chunker, schema, IDs/basic data types.
   - `embeddings` feature: embedding backend/generator/profile/token counter.
   - `vector-store` feature: LanceDB-backed vector store.
   - Search code should either be split into BM25-only and hybrid/vector
     pieces, or gated behind the right feature combination.

5. Changed `rmc-graph` to avoid a default dependency on CUDA-capable
   embedding code.

   Graph-only modules should build without `rmc-engine/embeddings-cuda`.
   Gate these paths behind a graph feature such as `semantic-embeddings`:

   - `graph/embedding_cache.rs`
   - `graph/query/similarity.rs`
   - codemap embedding rerank / compute-missing paths

6. Enabled the heavy features only where they are really needed.

   `rmc-server` should depend on:

   ```toml
   rmc-engine = { path = "../rmc-engine", features = ["embeddings-cuda", "vector-store", "hybrid-search"] }
   rmc-graph = { path = "../rmc-graph", features = ["semantic-embeddings"] }
   ```

Acceptance checks:

```bash
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
readelf -d target/debug/deps/rmc_graph-* | rg 'libcuda|libcublas|libcudart|libnvrtc'
```

The second command should have no matches for the skeleton-only graph test
binary. Use a precise test binary path if multiple old binaries exist.

## Phase 2: replace real-workspace skeleton fixtures

Goal: skeleton unit tests should not load the whole repository through
rust-analyzer.

Status: completed on 2026-05-26.

Completed implementation:

- Added `graph::skeleton::test_support`, a skeleton-specific synthetic Cargo
  package fixture stored in temp directories for both workspace source and
  graph data.
- The fixture covers functions with bodies, inherent impl methods, trait
  associated items, constants/statics, attributes/docs, nested modules, and
  `#[cfg(test)]` / `#[test]` cases.
- Updated skeleton collect/render tests to use the synthetic fixture instead of
  `graph::test_support::shared_snapshot()`.
- Retargeted missing-source fallback tests to function/static items in the
  synthetic fixture.

Validation performed:

```bash
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
```

Result: `22 passed; 0 failed; 202 filtered out`, finishing in about 14 seconds.

Current problem:

`crates/rmc-graph/src/graph/test_support.rs` builds a shared snapshot using:

```rust
build_and_persist(Path::new(env!("CARGO_MANIFEST_DIR")), opts)
```

That invokes `loader::load`, which uses:

- `no_deps: false`
- `all_targets: true`
- `set_test: true`
- `prefill_caches: true`
- `num_worker_threads: num_cpus::get_physical()`

This is useful for integration coverage, but it is too heavy for small skeleton
unit tests. It creates the exact RA worker set seen in the stuck process.

Implementation shape:

1. Add a synthetic skeleton fixture builder under `graph/skeleton/test_support`
   or reuse the existing graph test fixture pattern with a temp Cargo
   workspace.

2. Include only the source cases skeleton needs:

   - functions with bodies
   - impl methods
   - trait associated items
   - const/static initializers
   - test modules and `#[cfg(test)]`
   - attributes/docs
   - nested modules where needed

3. Point skeleton tests at the synthetic fixture instead of the real
   `rmc-graph` crate snapshot.

4. Keep one real-workspace smoke test only if it provides distinct value.
   Mark it `#[ignore]` or place it behind a feature such as
   `real-workspace-tests`.

Acceptance checks:

```bash
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
```

The focused skeleton test should finish without long-running full RA workspace
load. It should not spawn dozens of `VfsLoader` / `PrimeCaches` threads for the
entire repository.

## Phase 3: lazy initialize embedding resources after no-op checks

Goal: background sync and incremental indexing should not touch CUDA when there
are no changed files.

Status: completed on 2026-05-26.

Completed implementation:

- `IndexerCore` now stores embedding backend/configuration and lazily constructs
  `EmbeddingBatcher` only when embedding, memory, or cloned-generator access is
  requested.
- `IndexerCore::process_file_sync` no longer requires tokenizer/model
  initialization for chunk splitting; it uses the cheap estimate path until
  embedding work is actually needed.
- `IncrementalIndexer::new` and `IncrementalIndexer::with_backend` now store a
  preflight configuration instead of immediately constructing `UnifiedIndexer`.
- `IncrementalIndexer::index_with_change_detection` loads/builds/compares
  Merkle snapshots before calling the lazy `ensure_indexer` path.
- Added a no-change regression test that pre-saves a matching Merkle snapshot
  and passes invalid index/vector paths that would fail if `UnifiedIndexer` or
  LanceDB were initialized.
- Updated examples and integration-style tests that explicitly access the
  underlying indexer/generator to use the now-fallible lazy accessors.

Validation performed:

```bash
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-indexing --lib no_changes_detection
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-indexing --lib
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo check -p rust-code-mcp --example quick_bench --example benchmark_phases --example index_codebase --test benchmark_gpu_performance --test test_hybrid_search --test evaluation
```

The no-change test passed without initializing the underlying `UnifiedIndexer`.
The manual `nvidia-smi` runtime check remains an operational check for a real
sync run; it was not needed for the unit-level gate.

Current problem:

`IncrementalIndexer::with_backend` constructs `UnifiedIndexer`, which constructs
`IndexerCore`, which constructs `EmbeddingGenerator` immediately. That happens
before `index_with_change_detection` checks the Merkle fast path.

This means a background sync can load Qwen3/CUDA even when Merkle roots match
and no file needs indexing.

Implementation shape:

1. Change `IndexerCore` to store embedding configuration instead of an already
   constructed `EmbeddingBatcher`.

   ```rust
   pub(crate) struct IndexerCore {
       file_processor: FileProcessor,
       chunker: Chunker,
       chunk_split_config: ChunkSplitConfig,
       embedding_backend: EmbeddingBackend,
       embedding_config: EmbeddingRuntimeConfig,
       embedding_batcher: OnceLock<EmbeddingBatcher>,
   }
   ```

   The exact config type can reuse `IndexerCoreConfig` fields instead of
   introducing a new public type.

2. Create the `EmbeddingGenerator` only inside methods that actually need
   embeddings:

   - `generate_embeddings_batched`
   - `count_chunk_raw_tokens`, if exact tokenizer counting remains tied to the
     embedding backend

3. Avoid exact token counting during parse/chunk if it forces model/tokenizer
   initialization. Prefer a cheap character/token estimate until embedding is
   actually required.

4. Ensure `IncrementalIndexer::index_with_change_detection` can execute:

   - load old Merkle
   - build new Merkle
   - compare roots
   - return unchanged stats

   without constructing `EmbeddingGenerator`, `VectorStore`, or LanceDB if
   possible.

5. If `VectorStore` initialization is also expensive, split the indexing
   object into:

   - `IndexingPreflight`: metadata paths, backend identity, Merkle check
   - `UnifiedIndexer`: heavy object built only when there are changes

Acceptance checks:

```bash
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-indexing --lib no_changes_detection
```

Manual runtime check:

```bash
nvidia-smi --query-compute-apps=pid,process_name,used_gpu_memory --format=csv
```

Run a no-change incremental sync. It should not add a new CUDA compute client.

## Phase 4: explicit MCP server lifecycle

Goal: the long-running MCP server should own all background tasks and heavy
caches explicitly, and it should be able to shut them down or clear them
gracefully.

Status: completed on 2026-05-26.

Completed implementation:

- Added `mcp::ServerRuntime` and `mcp::RuntimeState` to own sync manager,
  workspace locks, search runtime cache, semantic service state, shutdown
  signaling, and background task handles.
- Added `SyncManager::run_until_shutdown` using a `tokio::sync::watch`
  shutdown signal. Existing `run` remains as a compatibility wrapper.
- Updated `rust-code-mcp` main to create `ServerRuntime`, start background sync
  through it, serve MCP with runtime-owned state, and on service exit request
  shutdown, wait up to 10 seconds for tasks, abort timed-out tasks, and clear
  runtime caches/tracking.
- Removed the global semantic singleton from router-owned paths. Analysis and
  rename endpoints now receive the runtime-owned
  `Arc<Mutex<SemanticService>>`.
- Added search-cache status/key reporting and count-returning invalidation.
- Added semantic status plus `clear_all` / `clear_project`.
- Added sync tracking status/count and sorted tracked directory reporting.
- Added `runtime_status` and `clear_runtime` MCP tools.
- Made cache-producing read tools (`get_similar_code`, `similar_to_item`, and
  prompt-seeded `build_codemap`) participate in workspace lifecycle locks
  before constructing or inserting runtime search-cache entries.
- Retargeted graph static metadata regression tests from the removed
  `rmc_server::semantic::SEMANTIC` singleton to a manually persisted graph
  fixture so the static metadata tests continue to cover persistence/query
  behavior without loading rust-analyzer.
- Added a test-only `persist_test_model` helper for graph query tests that need
  LMDB round-trip coverage without starting a full rust-analyzer workspace
  load.

Validation performed:

```bash
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib --tests
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib statics::tests -- --test-threads=1
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib static_metadata_round_trip_for_static_mut_fixture -- --test-threads=1
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib audit_detects_known_static_mut -- --test-threads=1
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib get_similar_code_waits_for_workspace_lock
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib similar_to_item_waits_for_workspace_lock
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib build_codemap_waits_for_workspace_lock_before_search_cache_use
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib runtime
nix develop ../nix-devshells#cuda-code --command cargo check -p rust-code-mcp
```

Result: all listed checks passed. The runtime test filter ran 13 tests covering
status, clear behavior, sync cancellation, semantic cleanup, cache status, RSS
parsing, and whole-runtime clear serialization with workspace operations.
Focused lock-participation tests also cover the cache-producing read endpoints
that can construct `HybridSearch` runtime entries.
During review-fix validation, a temporary rust-analyzer-backed static fixture
reproduced the D-state failure in `rmc_graph-c878c` at `do_exit`; that fixture
was replaced by the manual persisted graph fixture above, whose focused tests
complete immediately.

Review gate:

- Reviewer score: 8.8/10 on 2026-05-26.
- Gate result: pass (`> 8.5`).
- Residual note: an already-active background sync cycle may have copied the
  tracked-directory list before `clear_runtime` untracks directories. That can
  allow one in-flight cycle to finish work after a clear while the server keeps
  serving, but it does not reintroduce the runtime search-cache race fixed in
  this phase. Full task cancellation remains owned by
  `ServerRuntime::shutdown_gracefully`.

Original problem addressed by this phase:

- `main.rs` spawns sync and never keeps the task handle.
- `SyncManager::run` loops forever.
- semantic RA state is a global `LazyLock<Mutex<SemanticService>>`.
- search runtime cache stores `EmbeddingGenerator` and `VectorStore` entries
  that can keep GPU/vector resources alive.

Implemented shape:

1. `ServerRuntime` owns the long-lived server resources:

   ```rust
   pub struct ServerRuntime {
       state: RuntimeState,
       shutdown_tx: watch::Sender<bool>,
       tasks: Mutex<Vec<JoinHandle<()>>>,
   }
   ```

   `RuntimeState` contains the shared sync manager, workspace lock registry,
   search runtime cache, semantic service, and background-sync status flags.

2. `SyncManager::run_until_shutdown` accepts a `watch::Receiver<bool>` and
   exits when shutdown is requested:

   ```rust
   pub async fn run_until_shutdown(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) {
       loop {
           tokio::select! {
               changed = shutdown.changed() => { ... }
               _ = interval.tick() => self.handle_sync_all().await,
           }
       }
   }
   ```

3. `main.rs` now creates `ServerRuntime`, starts background sync through it,
   serves MCP with `SearchToolRouter::with_server_runtime`, and calls
   `shutdown_gracefully(Duration::from_secs(10))` after the MCP service exits.
   Graceful shutdown requests cancellation, waits for task completion, aborts
   timed-out tasks, and clears runtime-owned caches/tracking.

4. Semantic service ownership moved out of the global singleton. Router methods
   receive the runtime-owned `Arc<Mutex<SemanticService>>` through
   `RuntimeState`:

   ```rust
   SearchToolRouter {
       runtime: RuntimeState,
   }
   ```

5. Cleanup/status APIs were added for the owned resources:

   - `SearchRuntimeCache::invalidate_all`
   - `SearchRuntimeCache::invalidate_workspace`
   - `SearchRuntimeCache::status`
   - `SemanticService::clear_all`
   - `SemanticService::clear_project`
   - `SemanticService::status`
   - `SyncManager::untrack_all_directories`
   - `SyncManager::status`

6. MCP lifecycle tools were added:

   - `runtime_status`
   - `clear_runtime`

   `runtime_status` reports:

   - tracked sync directories
   - search cache entry count and keys
   - semantic project count, paths, and load kind
   - whether background sync is enabled
   - current process PID and RSS when available

   `clear_runtime` supports in-memory cache and sync-tracking cleanup only. It
   does not stop the background sync task while the server is still serving MCP
   requests. Background task cancellation and joining is handled by
   `ServerRuntime::shutdown_gracefully` during process shutdown. Whole-runtime
   clears take the global workspace lock, and cache-producing read endpoints
   take workspace locks before runtime search-cache use, so clears serialize
   with in-flight workspace operations such as `index_codebase`,
   `get_similar_code`, `similar_to_item`, and prompt-seeded `build_codemap`.

   `clear_runtime` supports:

   - all caches
   - one workspace
   - semantic-only
   - search-cache-only
   - sync-tracking-only

Acceptance checks:

```bash
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib runtime
```

Do not run `cargo fmt`.

## Phase 5: safer process diagnostics

Goal: avoid diagnostic commands that themselves get stuck once a target process
is wedged.

Status: completed on 2026-05-26.

Completed implementation:

- Added `.docs/stuck-process-diagnostics.md`.
- Documented safe-ish known-PID collection using only:

  ```bash
  cat /proc/$pid/status
  cat /proc/$pid/stat
  cat /proc/$pid/wchan
  readlink /proc/$pid/exe
  readlink /proc/$pid/cwd
  ```

- Added `/proc` loops for finding candidate PIDs by `Name`/`State` from
  `/proc/$pid/status`, without `ps ... cmd`, `pgrep -af`, `pkill -f`, or raw
  `/proc/$pid/cmdline` reads.
- Added a `stuck_proc_snapshot PID` shell function that records
  `status`, `stat`, parsed state/parent/process-group fields, `wchan`, `exe`,
  and `cwd`.
- Added a decision tree for `S`, `D`, and `Z` states, including hibernation and
  suspend blocker handling.
- Added NVIDIA-specific notes for `D` state in `do_exit`,
  `do_mprotect_pkey`, `__vma_start_write`, and `__access_remote_vm`, including
  what those waits suggest and what diagnostics to avoid.
- Documented exact-PID and process-group termination using the process group
  parsed from `/proc/$pid/stat`, including a `/proc/*/stat` group-member
  preview and typed process-group confirmation before sending a group signal.
  The runbook warns that `D`-state processes may keep signals pending until
  the kernel wait resolves.

Validation performed:

```bash
sed -n '1,260p' .docs/stuck-process-diagnostics.md
rg -n 'cmdline|pgrep -af|pkill -f|do_exit|do_mprotect_pkey|__vma_start_write|__access_remote_vm|hibernate|hibernation' .docs/stuck-process-diagnostics.md .plans/lifecycle-plan.md
rg -n 'process group members|Type the process group number' .docs/stuck-process-diagnostics.md .plans/lifecycle-plan.md
jj diff --stat
```

No cargo tests were run; Phase 5 is documentation/runbook-only. Operational
review remains pending for the next incident where the runbook can be followed
without probing a live `D`-state process beyond the safe reads.

Review gate:

- Reviewer score: 9.5/10 on 2026-05-26.
- Gate result: pass (`> 8.5`).
- Reviewer findings: none after adding the process-group member preview and
  typed confirmation guard.

Observed collateral:

Commands such as `ps ... cmd`, `pgrep -af`, `pkill -f`, and raw
`/proc/$pid/cmdline` reads got stuck in `__access_remote_vm` after the target
process wedged.

Implementation shape:

1. Add a small script or documented runbook under `.docs` for stuck-process
   diagnosis.

2. Prefer these safe-ish reads:

   ```bash
   cat /proc/$pid/status
   cat /proc/$pid/stat
   cat /proc/$pid/wchan
   readlink /proc/$pid/exe
   readlink /proc/$pid/cwd
   ```

3. Avoid these once D-state is suspected:

   ```bash
   ps ... cmd
   pgrep -af ...
   pkill -f ...
   tr '\0' ' ' < /proc/$pid/cmdline
   ```

4. If killing is required, kill by exact PID or process group from `/proc/$pid/stat`,
   not by `pkill -f`.

## Phase 6: operational defaults

Goal: make the default MCP server conservative.

Status: completed on 2026-05-26.

Completed implementation:

- Added MCP operational defaults for startup/background work:
  `RMC_BACKGROUND_SYNC` is disabled when unset and enabled only for
  `1`, `true`, `yes`, or `on` after trimming and case-folding.
- Updated `rust-code-mcp` startup to log one summary line with background sync
  enabled/disabled state, the raw `RMC_BACKGROUND_SYNC` value, accepted enabled
  values, the automatic/background embedding profile default, and whether
  CUDA-capable embedding features are compiled in.
- Changed MCP-level automatic/default embedding resolution to
  `local-cpu-small` without changing `EmbeddingBackend::default()` globally.
- Kept explicit GPU selection working through `embedding_profile =
  "local-gpu-small"` / other local Qwen3 profiles and the legacy explicit
  `model` parameter on `index_codebase`.
- Made tool routing pass a sync manager to indexing/search only when the
  background sync task is enabled, so disabled startup does not advertise or
  accumulate background tracking for normal tool calls.
- Made background sync skip local CUDA-backed profiles and continue syncing
  CPU or remote profiles.
- Added backend-aware background registration through
  `SyncManager::track_directory_for_backend`, so foreground requests using
  explicit local CUDA profiles are not added to the background sync tracking
  set.
- Added an optional `embedding_profile` to `build_codemap` for prompt-driven
  HybridSearch seed lookup. The default remains `local-cpu-small`, while
  explicit local CUDA profiles remain available for foreground use.
- Updated MCP tool schema descriptions that referred to the old local-GPU
  default.

Validation performed:

```bash
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib background_sync
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib automatic
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo check -p rust-code-mcp
```

The checks passed after the review fixes. The background-sync filter ran
5 tests; the automatic-default filter ran 3 tests. Cargo emitted existing
dead-code warnings in lower crates and reported the adjacent
`../nix-devshells` git tree as dirty.

Review gate:

- Pre-phase `jj show --summary` was run before implementation from working
  copy change `mvumvlsvtmlxlltlnqwmstsvknqlwpny` with parent commit
  `3799a8c4`.
- First reviewer score: 8.1/10 on 2026-05-26. Gate result: fail. The reviewer
  found that local CUDA profiles could still be registered for background
  sync, and that prompt-driven `build_codemap` had no explicit embedding
  profile path.
- Final reviewer score after fixes: 9.2/10 on 2026-05-26.
- Gate result: pass (`> 8.5`).

Implementation shape:

1. Disable background sync by default, or require an env var:

   ```text
   RMC_BACKGROUND_SYNC=1
   ```

2. Prefer CPU or remote embedding profiles for automatic/background work.

3. Keep local CUDA embeddings opt-in per user request:

   - explicit `embedding_profile = "local-gpu-small"`
   - explicit index/build command

4. Add a startup log line summarizing:

   - background sync enabled/disabled
   - default embedding profile
   - whether CUDA-capable features are compiled in

## Implementation order

1. Feature-split CUDA out of graph-only code.
2. Replace skeleton real-workspace fixtures with synthetic fixtures.
3. Lazy-initialize embedding resources after Merkle preflight.
4. Add server lifecycle ownership and cleanup/status tools.
5. Add the stuck-process diagnostic runbook.
6. Adjust operational defaults for background sync and CUDA opt-in.

This order fixes the test-binary D-state hazard first, then improves graceful
server behavior.

## Non-goals

- Do not paper over the issue by only adding `Drop` implementations.
- Do not depend on destructors for SIGKILL safety.
- Do not add formatting-only churn.
- Do not remove CUDA support; make it explicit and isolated.

## Validation rules

- Use the project dev shell:

  ```bash
  nix develop ../nix-devshells#cuda-code --command {command}
  ```

- Use `jj` first for VCS inspection.
- Do not run `cargo fmt` or any formatting command.
- For CUDA isolation, verify binaries with `readelf -d` rather than assuming
  from Cargo features alone.

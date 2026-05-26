# Plan: resource lifecycle and CUDA isolation for MCP/test stability

Status: in progress. Written against `/home/molaco/Documents/rust-code-mcp-refactor`
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

Current problem:

- `main.rs` spawns sync and never keeps the task handle.
- `SyncManager::run` loops forever.
- semantic RA state is a global `LazyLock<Mutex<SemanticService>>`.
- search runtime cache stores `EmbeddingGenerator` and `VectorStore` entries
  that can keep GPU/vector resources alive.

Implementation shape:

1. Introduce a runtime owner:

   ```rust
   pub struct ServerRuntime {
       shutdown: CancellationToken,
       sync_manager: Arc<SyncManager>,
       search_cache: SearchRuntimeCache,
       semantic: Arc<Mutex<SemanticService>>,
       tasks: Mutex<JoinSet<()>>,
   }
   ```

   `tokio-util` can provide `CancellationToken`, or a `watch` channel can be
   used to avoid a new dependency.

2. Change `SyncManager::run` to accept cancellation:

   ```rust
   pub async fn run_until_shutdown(self: Arc<Self>, shutdown: CancellationToken) {
       loop {
           tokio::select! {
               _ = shutdown.cancelled() => break,
               _ = interval.tick() => self.handle_sync_all().await,
           }
       }
   }
   ```

3. Change `main.rs` to:

   - create `ServerRuntime`
   - start background tasks through it
   - serve MCP
   - request shutdown after `service.waiting().await`
   - await task completion with a timeout
   - clear caches before process exit

4. Move semantic service ownership out of the global:

   Current:

   ```rust
   static SEMANTIC: LazyLock<Mutex<SemanticService>>
   ```

   Proposed:

   ```rust
   SearchToolRouter {
       semantic: Arc<Mutex<SemanticService>>,
       search_cache: SearchRuntimeCache,
       sync_manager: Option<Arc<SyncManager>>,
       ...
   }
   ```

5. Add explicit cleanup APIs:

   - `SearchRuntimeCache::invalidate_all`
   - `SearchRuntimeCache::invalidate_workspace`
   - `SemanticService::clear_all`
   - `SemanticService::clear_project`
   - `SyncManager::untrack_all_directories`

6. Add MCP tools:

   - `runtime_status`
   - `clear_runtime`

   `runtime_status` should report:

   - tracked sync directories
   - search cache entry count and keys
   - semantic project count, paths, and load kind
   - whether background sync is enabled
   - optionally, current process PID and memory RSS

   `clear_runtime` should support:

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

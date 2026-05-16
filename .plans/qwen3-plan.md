# Plan: Replace embedding backend with fastembed Qwen3 (Candle, CUDA)

## Goal

Replace the hardcoded `AllMiniLML6V2` ONNX embedding path with the Qwen3
embedding family (`Qwen3-Embedding-0.6B`, `4B`, `8B`) via fastembed-rs's
Candle backend on CUDA. The ONNX path goes away entirely.

We stay on `fastembed-rs`. We do **not** swap to `EmbedAnything` or
`embedrs`. The indexing pipeline (file walker, chunker, batcher,
incremental index, vector store) is untouched — only the encoder layer
is rewritten.

## Why this path

- `fastembed-rs` 5.13.4 ships `Qwen3-Embedding-{0.6B, 4B, 8B}` and
  `Qwen3-VL-Embedding-2B` behind a `qwen3` Cargo feature (Candle
  backend, CUDA via `fastembed/cuda`).
- Our `Cargo.toml:65` is unpinned to git main; the resolved checkout is
  5.2.0 (no Qwen3). We are ~11 minor versions behind.
- Our `EmbeddingGenerator` (`src/embeddings/mod.rs`) is ~560 LOC,
  well-isolated, and already the only thing that talks to fastembed.
  No other crate depends on `ort` directly.
- All other indexing logic is code-aware (AST-driven chunker, Merkle
  change detection, RAM-aware batcher, secret scanning) and is not
  replaceable by any off-the-shelf library.
- We have a CUDA-configured environment. Keeping a CPU-capable ONNX path
  alongside Candle would mean maintaining two backends, two CUDA
  initialization paths, and two memory-management strategies. The
  simpler design is one path: Candle on CUDA.

## Constraints

- Use `nix develop ../nix-devshells#cuda-code --command cargo check --lib`
  for every compile checkpoint. Do not run `cargo test` unless asked
  (snapshot build ~115s).
- Do not run `cargo fmt`.
- Small, compile-checkable steps. Land in this order — each step
  compiles on its own.
- Existing on-disk indexes built with `AllMiniLML6V2` (384-dim) are
  **invalidated** by this change. Step 6 wires up the refuse-and-tell
  behavior; users will need to clear their cache once.

## Background — current state

- Single hardcoded model at `src/embeddings/mod.rs:92`:
  `InitOptions::new(EmbeddingModel::AllMiniLML6V2)`.
- Backend: fastembed-rs (ONNX via `ort` 2.0.0-rc.10) with hand-rolled
  CUDA/CPU dispatch at `mod.rs:42-88`. **All of this gets deleted.**
- Public surface (`src/embeddings/mod.rs`): `EmbeddingGenerator::{new,
  embed, embed_async, embed_batch, embed_batch_async, embed_chunks,
  dimensions}`, `EmbeddingPipeline`, the alias `Embedding = Vec<f32>`,
  and the constant `EMBEDDING_DIM: usize = 384`.
- `EMBEDDING_DIM` is imported by:
  - `src/tools/query_tools.rs:14`
  - `src/mcp/sync.rs:9`
  - `src/tools/index_tool.rs:6`
- Dim flows into LanceDB via the `vector_size` parameter at
  `src/vector_store/mod.rs:50`, `src/indexing/unified.rs:611,634`,
  `src/indexing/incremental.rs:280,313,353`,
  `src/vector_store/lancedb.rs:459`. Production default in
  `vector_store/mod.rs:50` is literally `384`; the test literals at the
  other sites are also `384`.

## Target design

### Config

Add an `EmbeddingBackend` struct in `src/embeddings/mod.rs`. There is no
backend-family enum because there is only one backend — the variants
live inside Qwen3.

```rust
#[derive(Debug, Clone, Copy)]
pub struct EmbeddingBackend {
    pub variant: Qwen3Variant,
    pub max_len: usize,
    /// Off by default. Set only for CI/benchmark runs. Enabling this
    /// emits a warn! on every construction.
    pub force_cpu: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Qwen3Variant {
    Embedding0_6B,   // 1024-dim
    Embedding4B,     // 2560-dim
    Embedding8B,     // 4096-dim
}

impl Default for EmbeddingBackend {
    fn default() -> Self {
        Self {
            variant: Qwen3Variant::Embedding0_6B,
            max_len: 2048,
            force_cpu: false,
        }
    }
}
```

The backend reports two things:

- `fn dim(&self) -> usize` — output vector dimension, decided by
  `variant`.
- `fn identity(&self) -> String` — stable string used in cache paths
  and the `EMBEDDER_VERSION` field. Example:
  `"fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max2048:v1"`.

The `EMBEDDING_DIM` constant goes away.

### Max sequence length

Qwen3's `Qwen3TextEmbedding::from_hf(model_id, device, dtype, max_len)`
takes a max-length argument in tokens (not output dim — the upstream
README example uses `512`). Code chunks from our AST chunker can exceed
512 tokens for large functions, so we pick a deliberate default:

- Default `max_len = 2048` for all Qwen3 variants. Large enough to
  swallow almost any function-sized chunk after the contextual-retrieval
  header is prepended; small enough to keep VRAM bounded on 0.6B.
- It's a field on `EmbeddingBackend` and part of `identity()` so
  changing it invalidates indexes.
- Document, but don't expose, the option of pushing it to 8192 for
  recall experiments later. Don't ship a CLI knob until there is a real
  reason.

### Instruction-aware embedding (query vs document)

Qwen3 embeddings are instruction-tuned: queries take a task instruction
prefix, documents take raw text. Skipping this hurts recall measurably
on retrieval benchmarks. The public surface splits the two roles:

```rust
impl EmbeddingGenerator {
    pub async fn embed_documents(&self, texts: Vec<String>) -> Result<Vec<Embedding>, EmbeddingError>;
    pub async fn embed_queries(&self, texts: Vec<String>) -> Result<Vec<Embedding>, EmbeddingError>;
    pub fn dimensions(&self) -> usize;
}
```

- `embed_documents` calls `Qwen3TextEmbedding::embed` with the raw text.
- `embed_queries` prepends the Qwen3 instruction template before the
  same call:
  `"Instruct: Given a code search query, retrieve relevant code\nQuery: {text}"`.
  Confirm the exact wording against the upstream Qwen3-Embedding model
  card on first integration — it may have evolved. Keep the literal in
  one place so changing it is a one-line edit.

Existing call sites are migrated as part of Step 4:

- Index-building callers (`indexer_core.rs`, `unified.rs`, the graph
  embedding cache in `graph_tools.rs`) → `embed_documents`.
- Search-time callers (`src/search/mod.rs`, `src/tools/query_tools.rs`)
  → `embed_queries`.

No deprecation shim. The old `embed`/`embed_batch`/`embed_chunks` names
go away in the same commit that introduces the split; there's no
ambiguity about which one a call site wants once you look at it.

### Cargo

`fastembed` 5.13.4 is on crates.io; use the version pin, not the git
source. The previous git line, the direct `ort` dep, and any ORT-only
fastembed features are removed.

```toml
[dependencies]
# Was: fastembed = { git = "...", ... }
fastembed = { version = "5.13.4", default-features = false, features = [
    "hf-hub-native-tls",
    "qwen3",
    "cuda",
] }

# Direct dep because src/embeddings/qwen3.rs imports Device/DType to
# build the Qwen3 embedder. Keep in lockstep with fastembed's pin
# (0.10.2 at 5.13.4).
candle-core = { version = "0.10.2" }

# Removed:
# - ort = "=2.0.0-rc.10"
# - fastembed feature "ort-download-binaries-native-tls"
# - fastembed feature "image-models"

[features]
default = []

# Apple dev override. Swaps cuda for metal. Mutually exclusive with the
# default CUDA build; check at compile time.
metal = ["fastembed/metal"]
```

Notes:

- `fastembed/cuda` already implies `fastembed/qwen3` per fastembed's
  feature graph, but we list both explicitly so the intent is obvious.
- No `qwen3-cpu` workspace feature. The runtime `force_cpu` field on
  `EmbeddingBackend` is the single override; the binary always builds
  with Candle + CUDA.
- `metal` is a compile-time alternative to CUDA. A real cross-platform
  build matrix is out of scope; macOS dev boxes pass `--features metal
  --no-default-features`.

### Device policy

GPU is the only intended runtime path:

- Build via `Device::new_cuda(0)?` (or `Device::new_metal(0)?` under
  the `metal` feature). Construction failure returns
  `EmbeddingError::GpuRequired` with a diagnostic block dumping
  `CUDA_HOME`, `CUDA_PATH`, and the first few entries of
  `LD_LIBRARY_PATH`. Do **not** use `Device::cuda_if_available` —
  that masks configuration problems.
- The only path to `Device::Cpu` is `EmbeddingBackend::force_cpu =
  true`; that path emits a `tracing::warn!` on every construction and
  is not surfaced in any "happy path" example.

## Step-by-step

Each step ends green: `cargo check --lib` passes inside the nix
devshell.

### Step 1 — strip ORT, swap fastembed dep, bump lancedb, no Qwen3 yet — **DONE 2026-05-16**

**Outcome:** `cargo check --lib` green in the new `cuda-code` devshell.

**Work that landed:**
- `Cargo.toml`: replaced git-sourced `fastembed` with `fastembed = { version = "5.13.4", default-features = false, features = ["hf-hub-native-tls", "qwen3", "cuda"] }`; deleted `ort = "=2.0.0-rc.10"`; added `candle-core = "0.10.2"`; bumped `lancedb` `0.15` → `0.29.0` and `arrow-array` / `arrow-schema` `53` → `58`.
- `Cargo.lock`: `tempfile` bumped `3.19.1` → `3.23.0` (required by `lance-index 6.0.0`'s use of `TempDir::keep`).
- `src/embeddings/mod.rs`: deleted `use ort::execution_providers::{...}`, the CUDA-init block (was `mod.rs:42-88`), and `.with_execution_providers(...)`. `EmbeddingGenerator::new()` is now a thin `TextEmbedding::try_new(InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true))`.
- `src/vector_store/lancedb.rs`: two `Box::new(RecordBatchIterator{..})` sites coerced to `Box<dyn RecordBatchReader + Send>` (lancedb 0.29 swapped its `IntoArrow` blanket impl for `Scannable`, which only matches the explicit trait-object form).
- **New devshell** `/home/molaco/Documents/nix-devshells/devshells/cuda-code.nix` provides `nvcc` (cudatoolkit), `cuda_cudart`, `libcublas`, `cudnn`, and the right `LD_LIBRARY_PATH` (including `/run/opengl-driver/lib`). All cargo invocations from this point on must use `nix develop ../nix-devshells#cuda-code --command ...`. The legacy `code` devshell stays around for the old MiniLM workflow.
- **Pre-existing build break fixed inline:** `src/tools/search_tool_router.rs:701` had a stray `-` diff marker that made the parent commit fail to compile. Removed the marker. Not Step 1's responsibility on paper, but unblocked verification.



This step is the demolition pass. After it, the crate still embeds with
MiniLM but the dependency surface has shifted under the wrapper.

**Dep-graph leak discovered during execution.** `lancedb 0.15` pins
`half = "=2.4.1"`, but `candle-core 0.10.2` (pulled by fastembed
5.13.4's Qwen3 feature) needs `half ^2.5.0`. Cargo's single-version
rule makes this unresolvable. The fix is to bump lancedb to `0.29.0`
(latest), which uses `half ^2.7.1` and `arrow ^58.0.0`. arrow-array and
arrow-schema bump in lockstep.

- Replace the git-sourced `fastembed` line at `Cargo.toml:65` with
  `version = "5.13.4"` and the features set from the Cargo section
  above. **Include `qwen3` and `cuda`** from the start — we are not
  staging the Candle pull-in.
- **Delete** the direct `ort` dependency from `Cargo.toml`. fastembed
  still uses it internally for non-Candle models; we no longer touch it
  from our code.
- **Bump lancedb** from `0.15` to `0.29.0`. Bump `arrow-array` and
  `arrow-schema` from `53` to `58`. These are dictated by the lancedb
  0.29 transitive constraints.
- Migrate `src/vector_store/lancedb.rs` for any API drift across the
  lancedb 0.15 → 0.29 span. The API surface we touch is small: `connect`,
  `BTreeIndexBuilder`, `Index`, `ExecutableQuery`, `QueryBase`,
  `DistanceType`, `Connection`, `Table`. Most are likely unchanged; a
  few may have moved modules or grown new parameters. Read the
  upstream changelog and adapt minimally — do not rewrite the file.
- The arrow types we use (`RecordBatch`, `RecordBatchIterator`,
  `StringArray`, `Float32Array`, `FixedSizeListArray`, `Array`,
  `Schema`, `Field`, `DataType`) are stable across arrow 53→58; only
  expect compile-time fix-ups, not real migration work.
- In `src/embeddings/mod.rs`, delete:
  - The `use ort::execution_providers::{...}` line.
  - The CUDA-init block (`mod.rs:42-88`).
  - The `with_execution_providers(...)` call.
- Keep `EmbeddingGenerator::new()` calling MiniLM via fastembed's
  default path so callers still compile. The wrapper degrades to a
  thin `TextEmbedding::try_new(InitOptions::new(model))` for one step.
- Add `candle-core = "0.10.2"` to `[dependencies]`.
- Verify with `cargo check --lib` in the devshell. Allowed scope of
  edits this step: `Cargo.toml`, `src/embeddings/mod.rs`,
  `src/vector_store/lancedb.rs`. Anything beyond that is a fresh leak —
  stop and audit.

### Step 2 — introduce the Qwen3 backend struct — **DONE 2026-05-16**

**Outcome:** `cargo check --lib` green in `cuda-code`. Types-only change, no behavior delta.

**Work that landed:**
- New module `src/embeddings/backend.rs` with `EmbeddingBackend` (struct: `variant`, `max_len`, `force_cpu`), `Qwen3Variant` (enum: `Embedding0_6B`, `Embedding4B`, `Embedding8B`; `Hash`-derived for Step 6's cache keys), `Default` impl (Qwen3-0.6B / max_len=2048 / force_cpu=false), and `dim()` + `identity()` + `hf_model_id()` methods. Four compile-only `#[cfg(test)]` sanity asserts at the bottom.
- `src/embeddings/mod.rs` got a two-line wire-up: `mod backend; pub use backend::{EmbeddingBackend, Qwen3Variant};`. Nothing else touched.



### Step 3 — write the Qwen3 embedder — **DONE 2026-05-16**

**Outcome:** `cargo check --lib` green in `cuda-code`. New `Qwen3Embedder` exists but isn't called yet; Step 4 wires it in.

**Work that landed:**
- New `src/embeddings/qwen3.rs` with `Qwen3Embedder { inner: Mutex<Qwen3TextEmbedding>, dim: usize }`. `new(&EmbeddingBackend)` builds the Candle device (`Device::new_cuda(0)` or `Device::Cpu` only if `force_cpu`), calls `Qwen3TextEmbedding::from_hf(model_id, &device, DType::F32, max_len)`, and logs a Candle env probe (`CUDA_HOME`, `CUDA_PATH`, first `LD_LIBRARY_PATH` entry).
- `embed_documents(&[&str])` calls fastembed `embed` directly; `embed_queries(&[&str])` prepends the instruction template `"Instruct: Given a code search query, retrieve relevant code\nQuery: "` before delegating to `embed_documents`. The literal lives in one `const QUERY_INSTRUCTION` so it's a one-line edit.
- `src/embeddings/error.rs`: added `GpuRequired(String)` variant + `gpu_required(impl Into<String>)` helper. Error message includes the actionable hint to use `cuda-code` devshell.
- `src/embeddings/mod.rs`: one-line `mod qwen3;` addition. No `pub use` — the embedder is `pub(super)` and consumed only by Step 4.
- `#[allow(dead_code)]` annotations on the new methods so the unused-warning gate stays green until Step 4.

**Verified fastembed API (5.13.4):**
- `Qwen3TextEmbedding::from_hf(repo_id: &str, device: &Device, dtype: DType, max_length: usize) -> candle_core::Result<Self>`.
- `Qwen3TextEmbedding::embed<S: AsRef<str>>(&self, texts: &[S]) -> candle_core::Result<Vec<Vec<f32>>>` — **`&self`**, not `&mut self`; output is L2-normalized by the model.
- `Mutex` kept anyway as defensive thread-safety guard since fastembed doesn't document `Send + Sync` guarantees.



- Create `src/embeddings/qwen3.rs` containing
  `Qwen3Embedder { inner: Qwen3TextEmbedding, dim: usize, instruction: &'static str }`.
- Construction maps to fastembed's
  `Qwen3TextEmbedding::from_hf(model_id, &device, dtype, max_len)`:
  - **Model IDs**:
    - `Qwen3Variant::Embedding0_6B` → `"Qwen/Qwen3-Embedding-0.6B"`
    - `Qwen3Variant::Embedding4B`   → `"Qwen/Qwen3-Embedding-4B"`
    - `Qwen3Variant::Embedding8B`   → `"Qwen/Qwen3-Embedding-8B"`
  - **Device**: `Device::new_cuda(0)?` (or `Device::new_metal(0)?`
    under `--features metal`). Failure returns
    `EmbeddingError::GpuRequired` with the CUDA-env diagnostic dump
    (port the spirit of `mod.rs:42-61`, not the contents — those audit
    ORT, this audits Candle). `force_cpu = true` is the only path to
    `Device::Cpu`.
  - **dtype**: `DType::F32` to start. Revisit F16 once 0.6B works; the
    upstream commit `b39d84b` ("Fix Qwen3 F16 dtype mismatches in
    attention and l2_normalize") suggests F16 has had quirks recently.
  - **max_len**: from `EmbeddingBackend.max_len`.
- Methods: `fn embed_documents(&self, texts: &[&str]) -> Result<Vec<Embedding>, EmbeddingError>`
  calls `inner.embed(texts)` directly. `fn embed_queries(...)`
  prepends the instruction template, then the same call. Store the
  template as a `&'static str` field so editing it is one line.
- Tracing lines: `tracing::info!("=== Qwen3 INITIALIZATION ===")`,
  log device + dtype + variant + max_len. No ORT-style env-var checks.

### Step 4 — rewrite `EmbeddingGenerator` on top of Qwen3 — **DONE 2026-05-16**

**Outcome:** `cargo check --lib` green in `cuda-code`. 18 warnings, all pre-existing (zero new). MiniLM is gone from the codebase; the binary is now Qwen3-only.

**Work that landed:**
- `src/embeddings/mod.rs` rewritten (337 → 179 lines). `EmbeddingGenerator { inner: Arc<Qwen3Embedder>, backend: EmbeddingBackend }` with `new`, `with_backend`, `dimensions`, `backend`, `embed_documents`, `embed_queries`, `embed_chunks`. All async via `tokio::task::spawn_blocking`. Synchronous `embed*` methods deleted. MiniLM-dim test block deleted.
- `EmbeddingPipeline::process_chunks` is now `async`. Default `batch_size` 128 → 32 (Qwen3-0.6B starting point; smoke test calibrates).
- `EMBEDDING_DIM` left in place; Step 5 removes it.
- Call sites migrated:
  - **`embed_documents` (index/cache/batcher path):** `indexing/embedding_batcher.rs`, `indexing/indexer_core.rs`, `indexing/unified.rs`, `tools/graph_tools.rs`.
  - **`embed_queries` (search path):** `search/mod.rs`, `search/resilient.rs`, `graph/codemap.rs` (prompt reranking).
- Newly-async signatures: `EmbeddingBatcher::generate_embeddings_batched`, `IndexerCore::generate_embeddings_batched`, `EmbeddingPipeline::process_chunks`.
- No `block_in_place` adaptations were needed — every call site already lived on an async stack.



- `EmbeddingGenerator` now holds `Qwen3Embedder` and a cached
  `dim: usize`. No `Arc<dyn Trait>`, no internal trait — there is one
  implementation.
- Replace the existing methods with:
  - `pub fn new() -> Result<Self, EmbeddingError>` → constructs with
    `EmbeddingBackend::default()`.
  - `pub fn with_backend(backend: EmbeddingBackend) -> Result<Self, EmbeddingError>`.
  - `pub async fn embed_documents(...)`, `pub async fn embed_queries(...)`.
  - `pub fn embed_chunks(...)` — uses `embed_documents` internally.
  - `pub fn dimensions(&self) -> usize`.
- The synchronous `embed`/`embed_batch` methods go away; all callers
  are async-capable. The async methods run the synchronous Candle call
  on a `spawn_blocking` task (mirror the existing pattern at
  `mod.rs:128-138`).
- Migrate call sites in the same commit:
  - Index builders (`indexer_core.rs:78`, `unified.rs:9`,
    `graph/codemap.rs`, the graph cache in `graph_tools.rs`) →
    `embed_documents`.
  - Search callers (`search/mod.rs:16`, `tools/query_tools.rs`) →
    `embed_queries`.
- Update `EmbeddingPipeline` (currently at `mod.rs:197`) to delegate to
  `embed_documents`. Its `batch_size = 128` default was tuned for
  ONNX-on-CUDA; for Qwen3-0.6B drop it to `32` and add a comment that
  this is a starting point to be measured during the smoke test.

### Step 5 — remove `EMBEDDING_DIM`

- Delete `pub const EMBEDDING_DIM: usize = 384;` from
  `src/embeddings/mod.rs`.
- Update the three importers (`mcp/sync.rs:9`,
  `tools/query_tools.rs:14`, `tools/index_tool.rs:6`) to call
  `generator.dimensions()` at the right point. If a call site does not
  hold a generator, plumb one in via constructor args — do not add a
  global.
- The literal `384`s in tests (`incremental.rs:280,313,353`,
  `unified.rs:611,634`, `lancedb.rs:459`) get replaced with `1024`
  (the new default's dim) or, better, with a constant pulled from
  `EmbeddingBackend::default().dim()`. Either is fine — pick whichever
  produces less test churn when you get there.
- `vector_store/mod.rs:50` default `vector_size: 384` becomes
  `EmbeddingBackend::default().dim()`.

### Step 6 — cache & path identity by model

Switching variants silently is a footgun: LanceDB will reject the dim
mismatch, but the graph embedding cache and the on-disk vector path
would otherwise happily mix vectors from two different models. Existing
MiniLM indexes on disk will also be invalidated by this whole change —
the same machinery handles them.

1. **`EMBEDDER_VERSION` is currently a `const`** at
   `src/tools/graph_tools.rs:990`:
   `"fastembed:all-MiniLM-L6-v2:dim384:v1"`. Replace the constant with
   a function `embedder_version(&EmbeddingBackend) -> String` that
   returns the backend's `identity()` value. Audit every reader and
   feed them the active backend.
2. **Vector store path** in `src/tools/project_paths.rs:30` is
   `format!("code_chunks_{}", &dir_hash[..8])` — keyed only by project
   directory. Extend it:
   `format!("code_chunks_{}_{}", &dir_hash[..8], &model_fp[..8])` where
   `model_fp = sha256(backend.identity())`. Two indexes for the same
   project under different variants live in separate LanceDB
   directories instead of fighting over one.
3. **Health check / clear_cache** (MCP handlers under `src/tools/`)
   must surface the active model in their output and, on startup,
   refuse to attach an existing index whose recorded `EMBEDDER_VERSION`
   does not match the configured backend. The refusal message must
   tell the user the exact `clear_cache` invocation. Do **not**
   auto-wipe — first run after this change should be a clear,
   user-driven action.
4. Write the active `EMBEDDER_VERSION` into a small `metadata.json`
   alongside the LanceDB directory at first index, verify on reopen.
   This is what makes (3) implementable.

### Step 7 — let the indexer pick a variant

- Add a `model: EmbeddingBackend` field to whatever struct configures
  the indexer (read `src/indexing/unified.rs` and `indexer_core.rs`
  during this step to see where to slot it).
- Default to `EmbeddingBackend::default()` (Qwen3-0.6B).
- Expose `Qwen3Variant` choice on the MCP tool surface. Wrong choice
  here is what causes the most user pain (wasted download, wrong dim),
  so the tool must echo the resolved backend's `identity()` in its
  response and reject mismatches against an existing index.

### Step 8 — smoke test

- Build with default features in the devshell. CUDA is implied; if the
  build fails because Candle's CUDA crate cannot find the sysroot, that
  is a devshell issue (blocker, not code).
- Run the binary against a small fixture directory with the default
  backend (Qwen3-0.6B, CUDA, `Device::new_cuda(0)`).
- Confirm GPU is actually in use: `nvidia-smi` shows the process
  holding VRAM while the indexer runs. If construction logs
  `EmbeddingError::GpuRequired` instead, fix the environment — do not
  set `force_cpu` to make the smoke test pass.
- Confirm the LanceDB table is created with `vector_size = 1024` by
  inspecting the table schema.
- Confirm `embed_queries` and `embed_documents` produce **different**
  vectors for the same input string — that proves the instruction
  prefix is being applied.
- Measure batch-size sensitivity for 0.6B on the dev GPU; adjust the
  `EmbeddingPipeline` default if 32 is too low or VRAM-fatal.
- Do **not** attempt 4B/8B on first run — verify 0.6B works end-to-end,
  then size up.

## Risks and open questions

1. **Candle CUDA build in the devshell.** `fastembed/cuda` pulls in
   `candle-core/cuda` + `candle-nn/cuda`. The current devshell is
   configured for ORT's CUDA needs, not Candle's. Verify the build at
   the start of Step 3; if pieces are missing, that is a devshell
   change. Don't paper over it from code.
2. **fastembed Qwen3 batching ergonomics.** `Qwen3TextEmbedding::embed`
   takes `&[&str]` and is synchronous on Candle. Confirm it supports
   real batches efficiently. The README example uses two strings, which
   doesn't prove much — Step 8's batch-size measurement will tell us
   whether 32 / 64 / 128 is the right starting point.
3. **VRAM headroom.** Qwen3-0.6B is small enough for 8 GB cards;
   Qwen3-4B and -8B will not fit. The old 5.5 GB ORT memory cap is
   deleted with the ONNX path, so there's no longer a competing
   reservation, but enabling 4B/8B will need a separate VRAM-vs-batch
   measurement. Out of scope for this plan beyond the 0.6B happy path.
4. **Instruction template drift.** The exact Qwen3 instruction string
   for code retrieval may change between model card revisions; we are
   hard-coding what works today. Confirm against the model card on
   first integration and centralize the literal in one place.
5. **Cache invalidation surface (Step 6).** Every existing on-disk
   index built with MiniLM is invalidated. `clear_cache` must work
   cleanly against pre-migration directory layouts (the directory hash
   was 8 chars, no model fingerprint suffix). Walk the old layout in
   addition to the new one when listing things to clear.
6. **HF token / network.** Qwen3 download is gated behind a network
   round-trip to HF Hub on first use; fastembed handles this. Confirm
   the nix runtime sandbox does not block HF downloads (build-time
   sandbox is a separate concern).
7. **F32 vs F16 memory and quality.** Starting at F32 trades VRAM for
   stability given the recent upstream F16 fixes. Revisit after 0.6B
   is green; F16 will be worth it for 4B/8B specifically.

## Out of scope for this plan

- Adding remote API providers (OpenAI / Cohere / etc.). Tracked
  separately. Local Qwen3 will be enough to evaluate quality first.
- Replacing the chunker, parser, batcher, incremental indexer, or
  vector store. None of them need to change for this.
- Replacing fastembed-rs entirely with `EmbedAnything` or `embedrs`.
  Re-evaluate only if fastembed's Qwen3 path proves unreliable or
  abandoned.
- Keeping MiniLM or any ONNX path as a fallback. The ONNX dependency
  surface (`ort` direct dep, CUDA execution providers, memory cap) is
  deleted in Step 1 and not restored.
- Multi-GPU. `Device::new_cuda(0)` hardcodes the first device; making
  that configurable is a follow-up if a real machine ever has more
  than one.

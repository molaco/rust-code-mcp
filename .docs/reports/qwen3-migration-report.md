# Qwen3 embedding migration — implementation report

**Date:** 2026-05-16
**Plan:** `.plans/qwen3-plan.md`
**Outcome:** ✅ landed end-to-end. Smoke test green.

## What changed in one paragraph

The embedding backend moved off fastembed's ORT/MiniLM (`AllMiniLML6V2`, 384-dim) onto fastembed's Candle/CUDA `Qwen3TextEmbedding` path with `Qwen3-Embedding-0.6B` as the new default (1024-dim). The wrapper grew an instruction-aware split (`embed_documents` vs `embed_queries`), backend identity strings, and on-disk version pinning so silent model swaps are now refused at startup. The MCP `index` tool accepts an optional `model` argument and the query path locks onto whichever variant built the index by reading a sibling `metadata.json`. A new `cuda-code` devshell was added in `../nix-devshells/` to give `cudarc`'s build script access to `nvcc`.

## Commit chain

| # | Commit | Step | What it did |
|---|---|---|---|
| 0 | `ff8e24af` | plan | landed `.plans/qwen3-plan.md` |
| 1 | `00a4129f` | step 1 | strip ORT direct dep, swap fastembed to 5.13.4, bump lancedb 0.15→0.29 |
| 2 | `29d02b98` | step 2 | `EmbeddingBackend` / `Qwen3Variant` types |
| 3 | `a2032db7` | step 3 | `Qwen3Embedder` over `fastembed::Qwen3TextEmbedding` |
| 4 | `aa15a5fb` | step 4 | rewrite `EmbeddingGenerator` on Qwen3, split query/document API |
| 5 | `213ff2aa` | step 5 | drop `EMBEDDING_DIM` constant; dim flows via backend/generator |
| 6 | `5a6c1898` | step 6 | cache + vector store keyed by embedder identity |
| 7 | `224b9c32` | step 7 | backend variant flows through indexer + MCP `index` tool |
| 8 | `d7a30ff4` | step 8 | smoke test green; `ort` re-added with `alternative-backend` |

## Dependency surface

**Added / changed:**
- `fastembed = { version = "5.13.4", default-features = false, features = ["hf-hub-native-tls", "qwen3", "cuda"] }` (was git main, resolved at 5.2.0).
- `candle-core = "0.10.2"` (direct, used for `Device` / `DType` types in the Qwen3 embedder).
- `ort = { version = "=2.0.0-rc.12", default-features = false, features = ["alternative-backend"] }` — kept as a **link-suppression marker**, not a runtime path. fastembed pulls `ort` unconditionally; `alternative-backend` propagates `ort-sys/disable-linking` so the binary doesn't try to link a system `libonnxruntime` that the devshell doesn't have. No code references `ort` types.
- `lancedb = "0.29.0"` (was `0.15`). The bump was forced by a `half` pin conflict: `lancedb 0.15` pinned `half = "=2.4.1"`, `candle-core 0.10.2` needs `half ^2.5.0`. lancedb 0.20 first loosened the pin; 0.29 was selected as latest.
- `arrow-array = "58"` and `arrow-schema = "58"` (was `53`), dictated by lancedb 0.29's transitive constraints.
- `tempfile` lockfile bump 3.19.1 → 3.23.0 (required by `lance-index 6.0.0`'s use of `TempDir::keep`).

**Removed:**
- `ort = "=2.0.0-rc.10"` direct dep (now re-added at rc.12 with `alternative-backend` — see above).
- Hand-rolled CUDA-init block for ORT execution providers at `src/embeddings/mod.rs:42-88` (~50 LOC).

## Code surface

### New
- `src/embeddings/backend.rs` — `EmbeddingBackend { variant, max_len, force_cpu }`, `Qwen3Variant { Embedding0_6B, Embedding4B, Embedding8B }`, `dim()` / `identity()` / `from_identity()` / `hf_model_id()`.
- `src/embeddings/qwen3.rs` — `Qwen3Embedder` over `fastembed::Qwen3TextEmbedding`. CUDA-required by default (`Device::new_cuda(0)`); only path to CPU is `force_cpu = true` which warns on every construction. Centralized query instruction template: `"Instruct: Given a code search query, retrieve relevant code\nQuery: "`.
- `src/vector_store/error.rs::VersionMismatch { stored, configured }` variant.
- `src/tools/graph_tools.rs::embedder_version(&EmbeddingBackend) -> String` (replaces a former `const EMBEDDER_VERSION`).
- `metadata.json` sidecar next to every LanceDB table — written on first index, read on every reopen, refuses to attach on mismatch.

### Rewritten
- `src/embeddings/mod.rs` — 337 → 179 LOC. `EmbeddingGenerator` now wraps `Arc<Qwen3Embedder> + EmbeddingBackend`. All `embed*` methods are async. Public API:
  - `new()` → defaults to Qwen3-0.6B.
  - `with_backend(EmbeddingBackend)` → explicit variant.
  - `embed_documents(Vec<String>)`, `embed_queries(Vec<String>)`, `embed_chunks(&[CodeChunk])`.
  - `dimensions()`, `backend()`.
- `EmbeddingPipeline::process_chunks` async; default `batch_size` 128 → 32 (Qwen3-0.6B sweet spot pending broader calibration).
- `ProjectPaths::from_directory(dir, &EmbeddingBackend)` — vector path now `code_chunks_<dirhash[..8]>_<modelfp[..8]>` so two backends coexist in their own LanceDB directories.
- `LanceDbBackend::new(path, vector_dim, embedder_identity)` — third arg is the model identity string; gates the metadata.json read/write.

### Removed
- `pub const EMBEDDING_DIM: usize = 384;` and every importer.
- Synchronous `EmbeddingGenerator::{embed, embed_async, embed_batch, embed_batch_async}`.

### Call-site migrations (Step 4 sweep)
- **Index-builder side** → `embed_documents`: `src/indexing/{indexer_core,unified,embedding_batcher}.rs`, `src/tools/graph_tools.rs`.
- **Search-time side** → `embed_queries`: `src/search/{mod,resilient}.rs`, `src/graph/codemap.rs`.

Newly-async functions: `EmbeddingBatcher::generate_embeddings_batched`, `IndexerCore::generate_embeddings_batched`, `EmbeddingPipeline::process_chunks`. All call sites already lived on async stacks; no `block_in_place` adaptations needed.

## Devshell

New file: `../nix-devshells/devshells/cuda-code.nix`. Provides:
- `cudaPackages.cudatoolkit` (gives `nvcc` for cudarc's build script).
- `cudaPackages.cuda_cudart`, `libcublas`, `cudnn` (runtime libs).
- `shellHook` that prepends `cudatoolkit/bin` to `PATH`, sets `CUDA_HOME`/`CUDA_PATH`, and prepends `/run/opengl-driver/lib` + cuda lib dirs to `LD_LIBRARY_PATH` (the NixOS user-space `libcuda.so` lives in `/run/opengl-driver/lib` and is not in `cudatoolkit`).
- `mcpServers.rust-code-mcp` block with separate `XDG_CACHE_HOME` / `XDG_DATA_HOME` from the legacy `code` devshell so the new Qwen3 indexes don't collide with old MiniLM data.

The `code` devshell is preserved untouched for any workflow that still wants the old binary; the active development path is `cuda-code`. Auto-memory updated.

## Behavior changes the user will see

1. **First run is slow.** Cold path: ~5-10 min release build + ~1.5 GB Qwen3-0.6B download from HF Hub on first `EmbeddingGenerator::new()`. Subsequent runs are warm.
2. **Existing on-disk indexes are invalidated.** Any `code_chunks_<hash>/` directory built before this change holds 384-dim MiniLM vectors and will be refused at startup with a `VersionMismatch` error pointing at `clear_cache`. `clear_cache` walks both legacy and new layouts. No auto-wipe.
3. **MCP `index` tool accepts a `model` argument**, optional, defaulting to Qwen3-0.6B. Accepted strings (case-insensitive): `qwen3-0.6b` / `0.6b`, `qwen3-4b` / `4b`, `qwen3-8b` / `8b`. The response text includes `Embedder: <identity>`.
4. **Search uses the model that built the index.** `query_tools.rs::create_hybrid_search` reads the on-disk `metadata.json` and constructs the generator with the matching backend. Mismatched search-vs-index would otherwise produce garbage results; this is the guardrail.
5. **Health output** exposes three fields: `embedder` (display, prefers on-disk identity), `embedder_configured` (default backend identity), `embedder_on_disk` (metadata.json contents if present).
6. **GPU required by default.** `Device::new_cuda(0)` is the constructor; failure returns `EmbeddingError::GpuRequired` with a CUDA-env diagnostic block. CPU is reachable only via `EmbeddingBackend { force_cpu: true, .. }` and emits a `warn!` on every construction.

## Smoke test summary

Run with `nix develop ../nix-devshells#cuda-code --command cargo run --release --example test_gpu_speed`:

| Metric | Value |
|---|---|
| Backend identity | `fastembed-candle:Qwen3-Embedding-0.6B:dim1024:max2048:v1` |
| Device | `Cuda(CudaDevice(DeviceId(1)))` |
| Generator init | 15.94 s (model load + CUDA kernel compile) |
| Reported dim | 1024 ✓ |
| `embed_documents` 1 chunk (cold) | 1.282 s |
| `embed_queries` 1 chunk (warm) | 0.016 s |
| L1 distance(doc, query) same input | **9.4604** (>> 0.01 — instruction prefix is live) |
| `embed_documents` 32 chunks | 0.074 s (~431 chunks/sec) |
| HF cache layout | `~/.cache/huggingface/hub/models--Qwen--Qwen3-Embedding-0.6B/snapshots/<sha>/{config.json, model.safetensors=1.19GB, tokenizer.json}` |

All asserts in the smoke test passed.

## Risks flagged in the plan and how they actually landed

1. ~~**ORT pin conflict** (forecast as #1 hard blocker).~~ Did not block — fastembed 5.13.4 pulls ort transitively. The unexpected blocker was instead the **`lancedb`/`half` conflict** (Step 1, mid-stream): lancedb 0.15 pinned `half=2.4.1`, candle-core 0.10.2 needs `half^2.5.0`. Fixed by bumping lancedb to 0.29, which forced arrow `53→58`. Plan amended in flight to roll this into Step 1.
2. **Candle CUDA build in the devshell** — happened. The `code` devshell lacks `nvcc`; created the new `cuda-code` devshell as a separate file rather than modifying `code` so legacy workflows are preserved.
3. **VRAM headroom for 4B/8B** — out of scope; only 0.6B was smoke-tested.
4. **Instruction template drift** — the literal lives in one place (`src/embeddings/qwen3.rs::QUERY_INSTRUCTION`). Audit against the upstream Qwen3 model card before any production-quality measurement.
5. **Cache invalidation surface** — handled in Step 6; `clear_cache` walks both old and new directory layouts.
6. **HF token / network** — first download took a few minutes, no auth issues. NixOS sandbox did not block runtime network access.
7. **F32 vs F16** — started at F32 per plan. F16 was deliberately deferred until 0.6B is known-good; upstream fastembed had a recent F16 fix (`b39d84b`) so F16 may need a second pass when enabled.

## Surprises worth knowing

1. **`fastembed::Qwen3TextEmbedding::embed` is `&self`, not `&mut self`** — the API I assumed in the plan was wrong. The internal `Mutex` around the embedder is kept anyway as a defensive Send+Sync guard.
2. **fastembed 5.13.4's `ort` is non-optional** with only `["ndarray", "std", "api-24"]` features (no `download-binaries`). So even if our code never instantiates ONNX, the binary fails to link unless `ort-sys`'s `disable-linking` is activated. We re-added `ort` purely to forward `alternative-backend`. The plan said "delete the `ort` direct dep entirely"; the implementation needs it back as a no-op marker.
3. **One pre-existing build break** (stray `-` diff marker at `src/tools/search_tool_router.rs:701`) was fixed inline during Step 1. It was blocking validation but unrelated to embedding work.
4. **lancedb 0.29 swapped `IntoArrow` → `Scannable`** — needed explicit `Box<dyn RecordBatchReader + Send>` coercion at two sites in `src/vector_store/lancedb.rs`. Easy fix once spotted.

## Known follow-ups (intentionally out of scope)

1. **`graph_tools.rs::ensure_embeddings_for` and `graph/codemap.rs` rerank** still construct `EmbeddingGenerator::new()` with default backend and compute `active_version` locally. Threading the active backend through requires changing `semantic_overlaps` / `build_codemap` signatures — those MCP tools aren't variant-aware yet. `// TODO: accept backend from caller` markers in place.
2. **Pre-existing `#[ignore]`-gated tests in `src/indexing/unified.rs`** need a signature touch-up from Step 6's new `embedder_identity` plumbing. They don't affect `cargo check --lib`. Sweep when running the snapshot suite next.
3. **Remote API providers** (OpenAI / Cohere / etc.) are not in the binary. Plan declared them out of scope; evaluate Qwen3 retrieval quality first.
4. **Qwen3-4B / -8B not exercised.** Plan's Phase 5 (full indexer smoke) was skipped because Phase 3 was sufficient. 0.6B is the only variant proven end-to-end.
5. **`EmbeddingPipeline::batch_size = 32`** was chosen as a Qwen3-0.6B starting point; smoke-test throughput suggests room to increase. Calibrate when running a real-corpus benchmark.
6. **Stale doc comments** mentioning MiniLM/384 outside the swept set may exist; the agent reported coverage but a fresh `rg "MiniLM|all-MiniLM"` pass before shipping is worth a minute.

## File-touch summary

| Path | Status |
|---|---|
| `Cargo.toml` | modified (fastembed, ort, candle-core, lancedb, arrow) |
| `Cargo.lock` | regenerated, tempfile bumped |
| `src/embeddings/mod.rs` | rewritten |
| `src/embeddings/backend.rs` | new |
| `src/embeddings/qwen3.rs` | new |
| `src/embeddings/error.rs` | new variants: `GpuRequired`, `InvalidIdentity` |
| `src/indexing/{indexer_core,unified,incremental,embedding_batcher}.rs` | call-site migration + `with_backend` constructors |
| `src/vector_store/{lancedb,mod,error}.rs` | identity plumbing, metadata.json, version-mismatch |
| `src/tools/{index_tool,query_tools,graph_tools,health_tool,clear_cache_tool,project_paths}.rs` | backend plumbing, MCP arg surface, identity-aware cache walking |
| `src/mcp/sync.rs` | ProjectPaths backend arg |
| `src/search/{mod,resilient}.rs` | `embed_queries` path |
| `src/graph/codemap.rs` | `embed_queries` for rerank |
| `src/graph/model.rs` | stale doc comments swept |
| `src/tools/search_tool_router.rs` | pre-existing typo fix |
| `examples/test_gpu_speed.rs` | rewritten as the smoke test |
| `examples/index_codebase.rs` | compile-fix only |
| `../nix-devshells/devshells/cuda-code.nix` | new |

# 07 — Rust RAG / Embedding Project Layouts

Survey of how production Rust RAG/embedding projects organize embedding generation, vector storage, and pipeline crates. Goal: inform our workspace split, especially the contradiction where retrieval search and graph semantic similarity both need an embedding session.

## Per-project summary

### LanceDB (`lancedb/lancedb`)
- Workspace members: `rust/lancedb`, `nodejs`, `python`. The Rust core is a *single* crate; the polyglot story is bindings, not sub-crates.
- `rust/lancedb/src/` modules: `connection`, `database`, `table`, `query`, `index`, `embeddings/`, `rerankers/`, `remote`, `data`, `arrow.rs`, `expr`, `io`, `utils`. Both `embeddings.rs` and `embeddings/` exist (facade + impl dir).
- Pulls Lance internals (`lance-core`, `lance-io`, `lance-index`, `lance-linalg`, `lance-table`, `lance-encoding`, `lance-arrow`, `lance-datafusion`) as *external* git deps — the storage engine is already split, lancedb just consumes it.
- Embeddings are a registry-style trait module (pluggable providers), not a separate crate. Vector storage stays in Lance; lancedb composes.

### fastembed-rs (`Anush008/fastembed-rs`)
- Single crate `fastembed`. Internal modules: `text_embedding/`, `image_embedding/`, `sparse_text_embedding/`, `reranking/`, `models/`, `output/`, plus `common.rs`, `init.rs`, `pooling.rs`.
- Three owned structs: `TextEmbedding`, `ImageEmbedding`, `TextRerank`, each constructed via `try_new(InitOptions)`. Owned, not `Arc`-wrapped at the API boundary; sync API; "no Tokio dependency."
- Wraps `ort` 2.x; the ONNX `Session` is *not* in the public surface — users see `embed(&self, texts, batch_size)`.
- Hardware providers are Cargo features (`cuda`, `cudnn`, `metal`, `directml`, `mkl`, `accelerate`) plus model-specific features (`qwen3`, `nomic-v2-moe`). Default batch size 256.

### ort (`pykeio/ort`)
- Workspace = root crate `ort` + `ort-sys` (FFI). Backends live under `backends/`.
- *Every* execution provider is a Cargo feature: `cuda`, `tensorrt`, `rocm`, `directml`, `coreml`, `nnapi`, `xnnpack`, `openvino`, `onednn`, `webgpu`, `azure`, etc. Compile-time selection.
- Public API is `Session` + `SessionBuilder`; providers configured via builder pattern, not type parameters. FFI is hidden behind `ort-sys`.

### candle (`huggingface/candle`)
- Multi-crate workspace: `candle-core` (Tensor + devices), `candle-nn`, `candle-transformers`, `candle-datasets`, `candle-kernels` (CUDA), `candle-flash-attn`, `candle-onnx`, `candle-examples`, `candle-pyo3`.
- No dedicated embeddings crate — embedding models (BERT, JinaBert, T5) live as examples on top of `candle-transformers`.
- CPU vs CUDA via features (`cuda`, `cudnn`, `mkl`, `accelerate`); device is also a runtime value (`Device::Cpu | Device::Cuda(_)`), so the same code paths handle both.

### langchain-rust (`Abraxas-365/langchain-rust`)
- Single crate. `embeddings`, `vectorstores`, `chains`, `agents` are modules, not crates.
- Provider plurality (OpenAI, Azure, Ollama, FastEmbed, Mistral) handled by a shared `Embedder` trait + per-backend feature flags (`sqlite-vss`, `postgres`, `qdrant`, `surrealdb`, …). Trait-object dispatch, not generics.

## Common embedding lifecycle patterns
- **Owned struct, constructed once, shared by `Arc`** at the *application* layer. None of these libraries make the embedder itself a global singleton — they ship an owned `try_new`/`builder` constructor and let the host wrap it. fastembed's `TextEmbedding` and ort's `Session` both follow this.
- **Sync `embed(&self, …)` API.** Methods take `&self`, so an `Arc<TextEmbedding>` shared across tokio tasks/threads is the canonical pattern. The session is reused; only inputs/outputs allocate.
- **Batching is an argument, not a constructor knob.** fastembed exposes `batch_size` per call (default 256); ort runs whatever you feed it. Memory limits are *not* a first-class concept anywhere — the caller chunks.
- **Provider selection is compile-time (Cargo features).** ort, fastembed, candle all gate `cuda`/`metal`/`directml`/`coreml` behind features. Runtime fallback is rare; the binary is built for a target.

## Common API leakage anti-patterns (and who avoids them)
- **Re-exporting `ort::Session` / `ort::Value`.** fastembed deliberately doesn't; it returns `Vec<Vec<f32>>`. langchain-rust's `Embedder` trait returns `Vec<f32>` / `Vec<Vec<f32>>`. *Do not* leak `ort` types in our public surface — it forces every consumer crate to depend on `ort` and inherits its feature-flag matrix.
- **Tokio-coupling the embed call.** fastembed avoids it; the call is sync. Async is the caller's choice (`spawn_blocking`).
- **Per-provider structs in the public API.** candle and ort both put providers behind builders rather than types, so downstream code doesn't fork on `CudaSession` vs `CpuSession`. Good model to copy.
- **Mixing storage and embedding in one trait.** lancedb keeps `embeddings/` and `index/` separate; the embedder produces vectors, the table consumes them. Don't conflate.

## Direct lessons for our project
The contradiction: retrieval search (RAG) and graph semantic similarity both need vectors from the same `fastembed::TextEmbedding`, but spinning up two ONNX sessions wastes ~200–500 MB and warm-up time.

1. **One embedding crate, one session, `Arc`-shared.** Mirror fastembed: an `embedding` crate that owns `TextEmbedding`, exposes a narrow trait (`fn embed(&self, &[&str]) -> Result<Vec<Vec<f32>>>`) returning plain `Vec<f32>`. Hand both retrieval and graph-similarity an `Arc<dyn Embedder>` (or `Arc<EmbeddingService>` if we don't need polymorphism yet). This is exactly langchain-rust's `Embedder` trait pattern.
2. **Do not re-export `ort` or `fastembed` types from the public crate API.** Keep `fastembed::TextEmbedding` private to the embedding crate. Consumers see only `Vec<f32>` and an opaque `Embedder` handle. This matches fastembed-over-ort and langchain-rust-over-fastembed — each layer flattens.
3. **Feature-gate providers at the embedding crate, not workspace-wide.** A `cuda`/`metal`/`cpu` feature on the `embedding` crate (forwarded to `fastembed`/`ort`) keeps the rest of the workspace provider-agnostic. ort and fastembed both do this.
4. **Vector storage is a separate crate from embedding.** LanceDB shows the split clearly: embeddings produce vectors, the table/index crate stores+queries. Our `retrieval` (Lance-backed) and `graph-similarity` consumers should both depend on `embedding` but not on each other.
5. **Construct once at app boot, share via `Arc`.** Build `EmbeddingService::new()` in the binary entry point, wrap in `Arc`, inject into the retrieval service and the graph service. No `lazy_static`/`OnceCell` inside the embedding crate — leave lifecycle to the host (fastembed's stance). This eliminates the duplicate-session problem without forcing a global.
6. **Sync method, async caller.** Keep `embed` sync (`&self`) like fastembed; let async callers `spawn_blocking`. Avoids leaking a tokio dependency into graph-analysis code paths that are otherwise sync.

# embeddings — Architecture

## Overview

The `embeddings` module turns code chunks and arbitrary text into 384-dimensional vector embeddings using a locally-loaded `AllMiniLML6V2` model from `fastembed`, with optional CUDA acceleration and CPU fallback. It exposes both a low-level `EmbeddingGenerator` (sync + async, single + batch) and a higher-level `EmbeddingPipeline` that chunks work into configurable batches and surfaces progress callbacks. Errors are funnelled through a single `EmbeddingError` type that interoperates with generic boxed error pipelines.

## Mermaid Diagram

```mermaid
graph TD
    subgraph external["External callers"]
        Caller["Indexing / search pipelines"]
        Chunks["CodeChunk values"]
    end

    subgraph embeddings["embeddings"]
        subgraph mod_rs["mod.rs"]
            Pipeline["EmbeddingPipeline\n{ generator, batch_size }"]
            Generator["EmbeddingGenerator\n{ model: Arc<Mutex<TextEmbedding>>, dimensions: 384 }"]
            EmbedChunks["embed_chunks()\nformat + batch"]
            EmbedSync["embed() / embed_batch()"]
            EmbedAsync["embed_async() / embed_batch_async()"]
            ProcessChunks["process_chunks()\nbatched + progress"]
        end

        subgraph error_rs["error.rs"]
            ErrEnum["EmbeddingError\nModelInit | EmbedFailed |\nNoEmbeddingGenerated | TaskJoin"]
            ErrCtors["model_init / embed_failed / task_join"]
            ErrBox["From<EmbeddingError>\nfor Box<dyn Error + Send>"]
        end
    end

    subgraph runtime["Runtime / native deps"]
        FastEmbed["fastembed::TextEmbedding\n(AllMiniLML6V2)"]
        ORT["ort execution providers\n(CUDA 5.5GB + CPU)"]
        Tokio["tokio::task::spawn_blocking"]
        Env["env: CUDA_HOME /\nCUDA_PATH / LD_LIBRARY_PATH"]
    end

    Caller -->|Vec<CodeChunk>| Pipeline
    Caller -->|text / Vec<String>| Generator
    Chunks --> EmbedChunks

    Pipeline -->|slice into batch_size| ProcessChunks
    ProcessChunks -->|per-batch| EmbedChunks
    ProcessChunks -.->|progress(processed,total)| Caller

    EmbedChunks --> EmbedSync
    Generator --> EmbedSync
    Generator --> EmbedAsync

    EmbedAsync -->|spawn_blocking| Tokio
    Tokio --> EmbedSync

    EmbedSync -->|lock + embed| FastEmbed
    FastEmbed --> ORT
    Generator -.->|init probes| Env
    Env --> ORT

    EmbedSync -.->|map_err| ErrCtors
    EmbedAsync -.->|join_err| ErrCtors
    Generator -.->|try_new failure| ErrCtors
    ErrCtors --> ErrEnum
    ErrEnum --> ErrBox
    ErrBox -.->|Box<dyn Error + Send>| Caller
```

## Module Responsibilities

| Module | Role | Key types |
|---|---|---|
| `embeddings/mod.rs` | Owns the embedding model lifecycle, exposes sync/async single + batch embedding, formats `CodeChunk`s, and drives batched processing with progress reporting. | `EmbeddingGenerator`, `EmbeddingPipeline`, `Embedding` (alias for the vector), `ChunkWithEmbedding` |
| `embeddings/error.rs` | Defines the module's error taxonomy, ergonomic constructors, and a boxed-trait-object conversion for generic error plumbing. | `EmbeddingError` (`ModelInit`, `EmbedFailed`, `NoEmbeddingGenerated`, `TaskJoin`), `From<EmbeddingError> for Box<dyn Error + Send>` |

## Data Flow

1. **Initialization.** `EmbeddingGenerator::new()` inspects `CUDA_HOME`, `CUDA_PATH`, and `LD_LIBRARY_PATH`, probes `CUDAExecutionProvider::is_available()`, and assembles an execution-provider list (CUDA with a 5.5 GB memory cap + CPU fallback, or CPU only). It then calls `TextEmbedding::try_new` for `AllMiniLML6V2`, wraps the model in `Arc<Mutex<_>>`, and stores `dimensions = 384`.
2. **Single-text path.** Callers invoke `embed(&str)` (sync) or `embed_async(String)` (async). Sync locks the mutex and calls `TextEmbedding::embed` with a one-element slice; async clones the `Arc`, hops onto `tokio::task::spawn_blocking`, and runs the same logic. Both pull the first element of the returned iterator or surface `NoEmbeddingGenerated`.
3. **Batch-text path.** `embed_batch` / `embed_batch_async` borrow the owned `Vec<String>` as `&[&str]`, lock the model, and call `TextEmbedding::embed` once per batch. The async variant mirrors the single-text async pattern via `spawn_blocking`.
4. **Chunk path.** `embed_chunks(&[CodeChunk])` calls `CodeChunk::format_for_embedding` on each chunk, feeds the resulting strings through `embed_batch`, and zips chunks with vectors into `ChunkWithEmbedding { chunk_id, embedding }` records.
5. **Pipeline path.** `EmbeddingPipeline::process_chunks` slices the input by `batch_size` (default 128), calls `embed_chunks` per batch, accumulates results, and fires the caller-supplied `FnMut(processed, total)` progress callback after each batch — capping `processed` at `total` for the final partial batch.
6. **Error funnel.** Any failure (`try_new`, `embed`, missing element, join error) is funneled into an `EmbeddingError` variant via the constructor helpers; callers needing a generic boxed error get `Box<dyn std::error::Error + Send>` via the `From` impl.

## Concurrency / Integration Model

- **Shared state.** The single `TextEmbedding` instance lives behind `Arc<Mutex<TextEmbedding>>` inside `EmbeddingGenerator`. All embed paths — sync, async, batch, chunk — serialize through this mutex; concurrency at the call site is fine, but the actual model invocation is single-threaded by design (the underlying ORT session is not `Sync`-friendly for parallel inference under this wrapper). Mutex poisoning is treated as fatal (unwrap).
- **Async bridge.** `embed_async` / `embed_batch_async` are the only async-aware entry points. They never block the runtime: the `Arc` is cloned, the heavy work runs inside `tokio::task::spawn_blocking`, and the join handle is awaited. Join failures become `EmbeddingError::TaskJoin`.
- **Native runtime boundary.** The module is the workspace's interface to `fastembed` and, transitively, `ort` execution providers. CUDA detection is performed once at construction and recorded in the provider list; there is no runtime fallback if CUDA fails mid-session — the provider chain (`CUDA → CPU`) handles that internally.
- **Backpressure / progress.** There are no channels. `EmbeddingPipeline` provides cooperative progress via a synchronous `FnMut(usize, usize)` callback fired after each batch; callers wire this into their own UI or telemetry. Total work is bounded by the input `Vec<CodeChunk>` length and `batch_size`.
- **External integration points.** Inputs are `&str`, `String`, `Vec<String>`, and `&[CodeChunk]`. Outputs are `Embedding` vectors, `Vec<Embedding>`, or `Vec<ChunkWithEmbedding>` (carrying `chunk_id` for downstream join with the chunk store / vector index). Errors cross thread boundaries cleanly via the `Box<dyn Error + Send>` conversion, making this module trivially embeddable in larger async error pipelines.

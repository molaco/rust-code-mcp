# embeddings — Abstract Logic

## Module: embeddings/mod.rs
**Purpose:** Provides text-to-vector embedding generation for code chunks using a local AllMiniLML6V2 model with optional CUDA acceleration and batch processing.

1. **Initialize the embedding model with CUDA detection and CPU fallback** -> `EmbeddingGenerator::new()`
2. **Expose the embedding vector dimensionality** -> `EmbeddingGenerator::dimensions()`, `EmbeddingPipeline::dimensions()`
3. **Embed a single text synchronously under a model lock** -> `EmbeddingGenerator::embed()`
4. **Embed a single text from an async context via a blocking task** -> `EmbeddingGenerator::embed_async()`
5. **Embed multiple texts as one batch synchronously or asynchronously** -> `EmbeddingGenerator::embed_batch()`, `EmbeddingGenerator::embed_batch_async()`
6. **Format and embed code chunks, pairing each chunk id with its vector** -> `EmbeddingGenerator::embed_chunks()`
7. **Construct a higher-level pipeline wrapping a generator with a configurable batch size** -> `EmbeddingPipeline::new()`, `EmbeddingPipeline::with_batch_size()`
8. **Process a chunk collection in batches with progress callbacks and aggregated results** -> `EmbeddingPipeline::process_chunks()`

## Module: embeddings/error.rs
**Purpose:** Defines the error type for embedding operations with constructor helpers and conversions for cross-thread interop.

1. **Construct typed error variants from string-like messages** -> `EmbeddingError::model_init()`, `EmbeddingError::embed_failed()`, `EmbeddingError::task_join()`
2. **Convert the error into a boxed sendable trait object for generic error pipelines** -> `impl From<EmbeddingError> for Box<dyn std::error::Error + Send>`
3. **Auto-derive Display and Error implementations via thiserror templates** -> `#[derive(Error, Debug)] EmbeddingError`

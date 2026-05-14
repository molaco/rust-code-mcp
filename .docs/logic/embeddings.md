# embeddings — Detailed Logic

## Module: embeddings/mod.rs

### `EmbeddingGenerator::new() -> Result<Self, EmbeddingError>`
**Call graph:** new -> tracing::info!, std::env::var, std::path::Path::new, std::fs::read_dir, CUDAExecutionProvider::default, CUDAExecutionProvider::is_available, CUDAExecutionProvider::with_memory_limit, CUDAExecutionProvider::build, CPUExecutionProvider::default, CPUExecutionProvider::build, TextEmbedding::try_new, InitOptions::new, InitOptions::with_show_download_progress, InitOptions::with_execution_providers, EmbeddingError::model_init, Arc::new, Mutex::new
**Steps:**
1. Log a debug header and the values of `CUDA_HOME`, `CUDA_PATH`, and `LD_LIBRARY_PATH` environment variables.
2. If `LD_LIBRARY_PATH` is set, iterate the first five colon-separated entries and log whether each path exists and contains CUDA libraries (`libcudart.so`, `libcublas.so`, or any filename containing "cuda").
3. Call `CUDAExecutionProvider::default().is_available()` and log its raw result.
4. Unwrap the availability result, falling back to `false` and emitting a warning log if the check itself errored.
5. If CUDA is available, build an execution provider list containing a CUDA provider configured with a 5.5GB memory limit followed by a CPU fallback provider; otherwise build a list containing only the CPU provider and log a warning.
6. Call `TextEmbedding::try_new` with `InitOptions` for `AllMiniLML6V2`, enabling download progress and the configured execution providers, mapping any error to `EmbeddingError::model_init`.
7. Log successful initialization including the CUDA flag and dimension count.
8. Wrap the model in `Arc<Mutex<_>>`, hardcode `dimensions = 384`, and return the constructed `EmbeddingGenerator`.

### `EmbeddingGenerator::dimensions(&self) -> usize`
**Call graph:** (none)
**Steps:**
1. Return the stored `dimensions` field directly.

### `EmbeddingGenerator::embed(&self, text: &str) -> Result<Embedding, EmbeddingError>`
**Call graph:** embed -> Mutex::lock, TextEmbedding::embed, EmbeddingError::embed_failed, Iterator::next, EmbeddingError::NoEmbeddingGenerated
**Steps:**
1. Lock the model mutex, panicking on poison.
2. Call `model.embed` with a single-element vector containing `text` and no batch override, mapping any error to `EmbeddingError::embed_failed`.
3. Take the first embedding from the returned iterator, returning `EmbeddingError::NoEmbeddingGenerated` if none was produced.

### `EmbeddingGenerator::embed_async(&self, text: String) -> Result<Embedding, EmbeddingError>`
**Call graph:** embed_async -> Arc::clone, tokio::task::spawn_blocking, Mutex::lock, TextEmbedding::embed, EmbeddingError::embed_failed, Iterator::next, EmbeddingError::NoEmbeddingGenerated, EmbeddingError::task_join
**Steps:**
1. Clone the `Arc<Mutex<TextEmbedding>>` so it can be moved into the blocking task.
2. Spawn a blocking task that locks the model, calls `model.embed` with the owned text, and returns the first embedding or `NoEmbeddingGenerated`.
3. Await the join handle, mapping any join error to `EmbeddingError::task_join` and flattening the inner result.

### `EmbeddingGenerator::embed_batch(&self, texts: Vec<String>) -> Result<Vec<Embedding>, EmbeddingError>`
**Call graph:** embed_batch -> Iterator::map, Iterator::collect, Mutex::lock, TextEmbedding::embed, EmbeddingError::embed_failed
**Steps:**
1. Convert the owned `Vec<String>` to a `Vec<&str>` of borrowed slices.
2. Lock the model mutex.
3. Call `model.embed` with the slice references and no batch override, mapping errors to `EmbeddingError::embed_failed`.

### `EmbeddingGenerator::embed_batch_async(&self, texts: Vec<String>) -> Result<Vec<Embedding>, EmbeddingError>`
**Call graph:** embed_batch_async -> Arc::clone, tokio::task::spawn_blocking, Iterator::map, Iterator::collect, Mutex::lock, TextEmbedding::embed, EmbeddingError::embed_failed, EmbeddingError::task_join
**Steps:**
1. Clone the model `Arc` for transfer into the blocking task.
2. Spawn a blocking task that converts texts to `&str` slices, locks the model, and calls `model.embed`, mapping errors to `embed_failed`.
3. Await the join handle and map join errors to `EmbeddingError::task_join`, flattening the inner result.

### `EmbeddingGenerator::embed_chunks(&self, chunks: &[CodeChunk]) -> Result<Vec<ChunkWithEmbedding>, EmbeddingError>`
**Call graph:** embed_chunks -> Iterator::map, CodeChunk::format_for_embedding, Iterator::collect, EmbeddingGenerator::embed_batch, Iterator::zip
**Steps:**
1. Format every chunk via `chunk.format_for_embedding()` into a `Vec<String>`.
2. Run the formatted strings through `self.embed_batch` to produce a `Vec<Embedding>`.
3. Zip the chunks with their embeddings to build `ChunkWithEmbedding { chunk_id, embedding }` records.
4. Return the collected results.

### `EmbeddingPipeline::new(generator: EmbeddingGenerator) -> Self`
**Call graph:** (none)
**Steps:**
1. Construct the pipeline with the supplied generator and a default `batch_size` of 128.

### `EmbeddingPipeline::with_batch_size(generator: EmbeddingGenerator, batch_size: usize) -> Self`
**Call graph:** (none)
**Steps:**
1. Construct the pipeline storing the supplied generator and explicit batch size.

### `EmbeddingPipeline::process_chunks<F>(&self, chunks: Vec<CodeChunk>, mut progress: F) -> Result<Vec<ChunkWithEmbedding>, EmbeddingError>`
**Call graph:** process_chunks -> slice::chunks, Iterator::enumerate, EmbeddingGenerator::embed_chunks, Vec::extend, usize::min, progress (caller-supplied FnMut)
**Steps:**
1. Record the total chunk count and create an empty results vector.
2. Iterate over `chunks.chunks(self.batch_size)` with their indices.
3. For each batch, call `self.generator.embed_chunks(batch)` and append the results to the accumulator, propagating any error.
4. Compute the processed count `(batch_idx + 1) * batch_size` capped at `total` and invoke the `progress` callback with `(processed, total)`.
5. After all batches finish, return the aggregated `Vec<ChunkWithEmbedding>`.

### `EmbeddingPipeline::dimensions(&self) -> usize`
**Call graph:** dimensions -> EmbeddingGenerator::dimensions
**Steps:**
1. Delegate to the underlying generator's `dimensions()` method.

## Module: embeddings/error.rs

### `EmbeddingError::model_init(msg: impl Into<String>) -> Self`
**Call graph:** model_init -> Into::into
**Steps:**
1. Convert the input into a `String` and wrap it in the `ModelInit` variant.

### `EmbeddingError::embed_failed(msg: impl Into<String>) -> Self`
**Call graph:** embed_failed -> Into::into
**Steps:**
1. Convert the input into a `String` and wrap it in the `EmbedFailed` variant.

### `EmbeddingError::task_join(msg: impl Into<String>) -> Self`
**Call graph:** task_join -> Into::into
**Steps:**
1. Convert the input into a `String` and wrap it in the `TaskJoin` variant.

### `impl From<EmbeddingError> for Box<dyn std::error::Error + Send>`
**Call graph:** from -> Box::new
**Steps:**
1. Box the `EmbeddingError` value as a trait object so it can interoperate with code expecting `Box<dyn Error + Send>`.

### `#[derive(Error, Debug)] impl Display/Error for EmbeddingError`
**Call graph:** (auto-generated by `thiserror`)
**Steps:**
1. `thiserror` generates `Display` using each variant's `#[error("…")]` template (`"Model initialization failed: {0}"`, `"Embedding generation failed: {0}"`, `"No embedding generated"`, `"Async task failed: {0}"`).
2. `thiserror` generates `std::error::Error` with no custom `source`, since no variant declares `#[source]` or `#[from]`.

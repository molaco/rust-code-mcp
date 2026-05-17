//! Embedding generation using fastembed's Candle backend (Qwen3).
//!
//! `EmbeddingGenerator` wraps a `Qwen3Embedder`. The synchronous
//! ONNX path is gone: every public method is `async` and runs the
//! underlying blocking Candle call on the tokio blocking pool.
//!
//! The public surface splits document- and query-side embedding so
//! Qwen3's instruction tuning is applied correctly:
//! - `embed_documents` — raw text, no instruction prefix. Used by the
//!   indexer / cache / batcher.
//! - `embed_queries` — instruction prefix applied. Used by search.

mod error;
pub use error::EmbeddingError;

mod backend;
pub use backend::{
    EmbeddingBackend, EmbeddingModelSpec, EmbeddingProfile, EmbeddingRuntime,
    Qwen3Variant, QueryFormatting,
};

mod fastembed_cpu;
mod openrouter;
pub use openrouter::{
    openrouter_runtime_config, OpenRouterEncodingFormat, OpenRouterProviderPreferences,
    OpenRouterProviderSort, OpenRouterRuntimeConfig,
};
mod qwen3;

mod token_lengths;
pub use token_lengths::{EmbeddingTextLen, EmbeddingTokenCounter};

use crate::chunker::{ChunkId, CodeChunk};
use std::sync::Arc;

/// An embedding vector. Dimension depends on the active backend
/// (1024 for Qwen3-0.6B by default).
pub type Embedding = Vec<f32>;

/// A chunk paired with its generated embedding.
#[derive(Debug, Clone)]
pub struct ChunkWithEmbedding {
    pub chunk_id: ChunkId,
    pub embedding: Embedding,
}

/// Embedding generator backed by Qwen3 over fastembed's Candle path.
#[derive(Clone)]
pub struct EmbeddingGenerator {
    inner: EmbeddingGeneratorInner,
    backend: EmbeddingBackend,
}

#[derive(Clone)]
enum EmbeddingGeneratorInner {
    Qwen3(Arc<qwen3::Qwen3Embedder>),
    FastembedCpu(Arc<fastembed_cpu::FastembedCpuEmbedder>),
    OpenRouter(Arc<openrouter::OpenRouterEmbedder>),
}

impl EmbeddingGenerator {
    /// Construct with the default backend (Qwen3-Embedding-0.6B,
    /// max_len=1024, GPU).
    pub fn new() -> Result<Self, EmbeddingError> {
        Self::with_backend(EmbeddingBackend::default())
    }

    /// Construct with an explicit backend configuration.
    pub fn with_backend(backend: EmbeddingBackend) -> Result<Self, EmbeddingError> {
        let inner = match backend.runtime {
            EmbeddingRuntime::LocalQwen3CandleCuda => {
                EmbeddingGeneratorInner::Qwen3(Arc::new(qwen3::Qwen3Embedder::new(&backend)?))
            }
            EmbeddingRuntime::OpenRouter => EmbeddingGeneratorInner::OpenRouter(Arc::new(
                openrouter::OpenRouterEmbedder::new(&backend)?,
            )),
            EmbeddingRuntime::LocalFastembedOnnxCpu => {
                EmbeddingGeneratorInner::FastembedCpu(Arc::new(
                    fastembed_cpu::FastembedCpuEmbedder::new(&backend)?,
                ))
            }
        };
        Ok(Self { inner, backend })
    }

    /// Output vector dimension for the active backend.
    pub fn dimensions(&self) -> usize {
        match &self.inner {
            EmbeddingGeneratorInner::Qwen3(inner) => inner.dim(),
            EmbeddingGeneratorInner::FastembedCpu(inner) => inner.dim(),
            EmbeddingGeneratorInner::OpenRouter(inner) => inner.dim(),
        }
    }

    /// Borrow the active backend configuration.
    pub fn backend(&self) -> &EmbeddingBackend {
        &self.backend
    }

    /// Document-side embedding (raw text, no instruction prefix).
    /// Used by indexer / cache / batcher.
    pub async fn embed_documents(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        match &self.inner {
            EmbeddingGeneratorInner::Qwen3(inner) => {
                let inner = inner.clone();
                tokio::task::spawn_blocking(move || {
                    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    inner.embed_documents(&refs)
                })
                .await
                .map_err(|e| EmbeddingError::task_join(e.to_string()))?
            }
            EmbeddingGeneratorInner::FastembedCpu(inner) => {
                let inner = inner.clone();
                tokio::task::spawn_blocking(move || {
                    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    inner.embed_documents(&refs)
                })
                .await
                .map_err(|e| EmbeddingError::task_join(e.to_string()))?
            }
            EmbeddingGeneratorInner::OpenRouter(inner) => {
                inner.embed_documents(texts).await
            }
        }
    }

    /// Query-side embedding (Qwen3 instruction prefix applied).
    /// Used by search.
    pub async fn embed_queries(
        &self,
        texts: Vec<String>,
    ) -> Result<Vec<Embedding>, EmbeddingError> {
        match &self.inner {
            EmbeddingGeneratorInner::Qwen3(inner) => {
                let inner = inner.clone();
                tokio::task::spawn_blocking(move || {
                    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    inner.embed_queries(&refs)
                })
                .await
                .map_err(|e| EmbeddingError::task_join(e.to_string()))?
            }
            EmbeddingGeneratorInner::FastembedCpu(inner) => {
                let inner = inner.clone();
                tokio::task::spawn_blocking(move || {
                    let refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();
                    inner.embed_queries(&refs)
                })
                .await
                .map_err(|e| EmbeddingError::task_join(e.to_string()))?
            }
            EmbeddingGeneratorInner::OpenRouter(inner) => inner.embed_queries(texts).await,
        }
    }

    /// Embed a slice of code chunks for the index.
    ///
    /// Wraps `embed_documents` over each chunk's
    /// `format_for_embedding()` output.
    pub async fn embed_chunks(
        &self,
        chunks: &[CodeChunk],
    ) -> Result<Vec<ChunkWithEmbedding>, EmbeddingError> {
        let formatted: Vec<String> =
            chunks.iter().map(|c| c.format_for_embedding()).collect();
        let embeddings = self.embed_documents(formatted).await?;
        let results: Vec<ChunkWithEmbedding> = chunks
            .iter()
            .zip(embeddings.into_iter())
            .map(|(chunk, embedding)| ChunkWithEmbedding {
                chunk_id: chunk.id,
                embedding,
            })
            .collect();
        Ok(results)
    }
}

/// Embedding pipeline with batch processing and progress reporting.
pub struct EmbeddingPipeline {
    generator: EmbeddingGenerator,
    batch_size: usize,
}

impl EmbeddingPipeline {
    /// Create a new embedding pipeline.
    pub fn new(generator: EmbeddingGenerator) -> Self {
        Self {
            generator,
            // Starting point for Qwen3-0.6B; calibrate during smoke test.
            batch_size: 32,
        }
    }

    /// Create with a custom batch size.
    pub fn with_batch_size(generator: EmbeddingGenerator, batch_size: usize) -> Self {
        Self {
            generator,
            batch_size,
        }
    }

    /// Process chunks with a progress callback.
    ///
    /// The callback receives `(current, total)` after each batch.
    pub async fn process_chunks<F>(
        &self,
        chunks: Vec<CodeChunk>,
        mut progress: F,
    ) -> Result<Vec<ChunkWithEmbedding>, EmbeddingError>
    where
        F: FnMut(usize, usize),
    {
        let total = chunks.len();
        let mut results = Vec::new();

        for (batch_idx, batch) in chunks.chunks(self.batch_size).enumerate() {
            let batch_results = self.generator.embed_chunks(batch).await?;
            results.extend(batch_results);

            let processed = (batch_idx + 1) * self.batch_size;
            progress(processed.min(total), total);
        }

        Ok(results)
    }

    /// Output vector dimension for the active backend.
    pub fn dimensions(&self) -> usize {
        self.generator.dimensions()
    }
}

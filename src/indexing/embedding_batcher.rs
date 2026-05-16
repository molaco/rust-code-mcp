//! Batch embedding generation with memory-aware sizing
//!
//! Extracted from `IndexerCore` to encapsulate embedding pipeline concerns:
//! GPU-optimized batch embedding generation and memory-aware batch sizing.

use crate::chunker::CodeChunk;
use crate::embeddings::{Embedding, EmbeddingGenerator};
use crate::indexing::IndexingError;
use crate::metrics::memory::MemoryMonitor;
use std::sync::{Arc, Mutex};

/// Handles batch embedding generation and memory-aware batch sizing.
pub(crate) struct EmbeddingBatcher {
    /// Embedding model
    embedding_generator: EmbeddingGenerator,
    /// Memory monitor for safe batch sizing
    memory_monitor: Arc<Mutex<MemoryMonitor>>,
    /// GPU batch size for embedding generation
    gpu_batch_size: usize,
}

impl EmbeddingBatcher {
    /// Create a new EmbeddingBatcher with a pre-constructed generator
    pub(crate) fn new(
        embedding_generator: EmbeddingGenerator,
        gpu_batch_size: usize,
    ) -> Self {
        let memory_monitor = MemoryMonitor::new();
        tracing::info!(
            "EmbeddingBatcher configured with GPU embedding batch size: {}",
            gpu_batch_size
        );
        Self {
            embedding_generator,
            memory_monitor: Arc::new(Mutex::new(memory_monitor)),
            gpu_batch_size,
        }
    }

    /// Generate embeddings for chunks in batches.
    ///
    /// Uses GPU-optimized batch size to avoid OOM on GPU memory.
    /// Chunks are formatted with their context and embedded via
    /// `EmbeddingGenerator::embed_documents` (no instruction prefix).
    pub(crate) async fn generate_embeddings_batched(
        &self,
        chunks: &[CodeChunk],
    ) -> Result<Vec<Embedding>, IndexingError> {
        let chunk_texts: Vec<String> = chunks
            .iter()
            .map(|c| c.format_for_embedding())
            .collect();

        let mut all_embeddings = Vec::new();

        for (batch_idx, chunk_batch) in chunk_texts.chunks(self.gpu_batch_size).enumerate() {
            tracing::debug!(
                "Embedding GPU sub-batch {}/{} ({} chunks, configured max {})",
                batch_idx + 1,
                (chunk_texts.len() + self.gpu_batch_size - 1) / self.gpu_batch_size,
                chunk_batch.len(),
                self.gpu_batch_size
            );
            let batch_embeddings = self
                .embedding_generator
                .embed_documents(chunk_batch.to_vec())
                .await?;
            all_embeddings.extend(batch_embeddings);
        }

        Ok(all_embeddings)
    }

    /// Calculate safe batch size for parallel processing based on available memory
    pub(crate) fn calculate_safe_batch_size(&self) -> usize {
        let available_mb = self.memory_monitor.lock().unwrap().available_bytes() / 1_000_000;

        // Assume 15 MB per file in memory (content + AST + chunks)
        let safe_concurrent = (available_mb / 15).max(1) as usize;

        // Cap at CPU cores to avoid thrashing
        let max_concurrent = num_cpus::get();

        let batch_size = safe_concurrent.min(max_concurrent).min(100);

        tracing::debug!(
            "Memory-based batch size: {} (available: {} MB, max concurrent: {})",
            batch_size,
            available_mb,
            max_concurrent
        );

        batch_size
    }

    /// Get current memory usage percentage
    pub(crate) fn memory_usage_percent(&self) -> f64 {
        self.memory_monitor.lock().unwrap().usage_percent()
    }

    /// Refresh memory monitor
    pub(crate) fn refresh_memory_monitor(&self) {
        self.memory_monitor.lock().unwrap().refresh();
    }

    /// Get current memory used in bytes
    pub(crate) fn memory_used_bytes(&self) -> u64 {
        self.memory_monitor.lock().unwrap().used_bytes()
    }

    /// Get reference to embedding generator
    pub(crate) fn embedding_generator(&self) -> &EmbeddingGenerator {
        &self.embedding_generator
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: tests that need EmbeddingGenerator require the model to be loaded,
    // so we only test memory-related functionality here.

    #[test]
    fn test_calculate_safe_batch_size() {
        // We can't easily construct an EmbeddingBatcher without a real EmbeddingGenerator,
        // but we can test the memory monitor independently.
        let monitor = MemoryMonitor::new();
        let available_mb = monitor.available_bytes() / 1_000_000;
        let safe_concurrent = (available_mb / 15).max(1) as usize;
        let max_concurrent = num_cpus::get();
        let batch_size = safe_concurrent.min(max_concurrent).min(100);

        assert!(batch_size > 0);
        assert!(batch_size <= 100);
    }

    #[test]
    fn test_memory_monitor_usage() {
        let monitor = MemoryMonitor::new();
        let usage = monitor.usage_percent();
        assert!(usage >= 0.0);
        assert!(usage <= 100.0);
    }
}

//! Batch embedding generation with memory-aware sizing
//!
//! Extracted from `IndexerCore` to encapsulate embedding pipeline concerns:
//! GPU-optimized batch embedding generation and memory-aware batch sizing.

use crate::indexing::IndexingError;
use crate::metrics::MemoryMonitor;
use rmc_engine::chunker::CodeChunk;
use rmc_engine::embeddings::batching::{
    BatchPlan as EmbeddingBatchPlan, BatchingPolicy, FixedInputShape,
};
use rmc_engine::embeddings::{
    Embedding, EmbeddingGenerator, EmbeddingRuntime, EmbeddingTextLen, EmbeddingTokenCounter,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Handles batch embedding generation and memory-aware batch sizing.
pub(crate) struct EmbeddingBatcher {
    /// Embedding model
    embedding_generator: EmbeddingGenerator,
    /// Memory monitor for safe batch sizing
    memory_monitor: Arc<Mutex<MemoryMonitor>>,
    /// Who dictates the batch shape: the model or the indexer settings.
    ///
    /// Decided ONCE when the batcher is built, not on every run: it determines
    /// the cut, the input sorting, and what padding even means in
    /// the metrics below.
    batching_policy: BatchingPolicy,
    /// Fixed input shape, if the runtime requires one.
    ///
    /// Stored whole (rather than as a single length) because an honest padding count
    /// needs BOTH axes: a run costs `rows × seq_len` regardless of the input
    /// lengths and of how many rows in the batch are actually used.
    fixed_shape: Option<FixedInputShape>,
    /// Token counter for Qwen3 model-input metrics.
    token_counter: Option<EmbeddingTokenCounter>,
}

#[derive(Debug, Clone, Copy)]
struct TokenLengthSummary {
    raw_tokens_total: usize,
    capped_tokens_total: usize,
    padded_tokens_total: usize,
    min_tokens: usize,
    max_tokens: usize,
}

impl EmbeddingBatcher {
    /// Create a new EmbeddingBatcher with a pre-constructed generator
    pub(crate) fn new(
        embedding_generator: EmbeddingGenerator,
        gpu_batch_size: usize,
        max_tokens_per_batch: usize,
    ) -> Self {
        let memory_monitor = MemoryMonitor::new();
        let gpu_batch_size = if gpu_batch_size == 0 {
            tracing::warn!("Embedding GPU batch size was 0; clamping to 1");
            1
        } else {
            gpu_batch_size
        };
        let max_tokens_per_batch = if max_tokens_per_batch == 0 {
            tracing::warn!("Embedding token budget was 0; clamping to 1");
            1
        } else {
            max_tokens_per_batch
        };

        // The shape is asked from the MODEL. If it requires one, the indexer settings
        // do not affect the cut, and that must be said out loud: a silently
        // ignored setting reads as 'I set it and it didn't work'.
        let fixed_shape = embedding_generator.backend().fixed_input_shape();
        let batching_policy = BatchingPolicy::new(
            fixed_shape.map(|shape| shape.rows),
            gpu_batch_size,
            max_tokens_per_batch,
        );
        if let Some(shape) = fixed_shape {
            if shape.rows.0 != gpu_batch_size {
                tracing::warn!(
                    configured_gpu_batch_size = gpu_batch_size,
                    model_batch_rows = shape.rows.0,
                    "Embedding batch size is dictated by the model's fixed input shape; \
                     the configured value is NOT applied"
                );
            }
            tracing::info!(
                rows = shape.rows.0,
                seq_len = shape.seq_len,
                "Embedding batch shape is fixed by the model; \
                 token budget and length sorting are not applied"
            );
        }
        let token_counter = match EmbeddingTokenCounter::from_backend(embedding_generator.backend())
        {
            Ok(counter) => {
                tracing::info!(
                    max_len = counter.max_len(),
                    "Embedding token counter initialized"
                );
                Some(counter)
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "Embedding token metrics unavailable; falling back to character-only batch logs"
                );
                None
            }
        };
        tracing::info!(
            policy = %batching_policy,
            "EmbeddingBatcher configured"
        );
        Self {
            embedding_generator,
            memory_monitor: Arc::new(Mutex::new(memory_monitor)),
            batching_policy,
            fixed_shape,
            token_counter,
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
        let chunk_texts: Vec<String> = chunks.iter().map(|c| c.format_for_embedding()).collect();

        let token_lengths = self.count_token_lengths(&chunk_texts);

        if self.embedding_generator.backend().runtime == EmbeddingRuntime::OpenRouter {
            return self
                .generate_openrouter_embeddings(chunk_texts, token_lengths)
                .await;
        }

        // Without a fixed shape a batch is padded to the longest row IN IT,
        // so inputs of similar length are kept together (the order is restored
        // below via `original_idx`). With a fixed shape this is wasted work:
        // every row is padded to `seq_len` anyway.
        let mut ordered_texts: Vec<(usize, String, Option<EmbeddingTextLen>)> = chunk_texts
            .into_iter()
            .enumerate()
            .map(|(idx, text)| {
                let token_len = token_lengths.as_ref().map(|lengths| lengths[idx]);
                (idx, text, token_len)
            })
            .collect();
        if self.batching_policy.sorts_by_length() {
            sort_embedding_inputs(&mut ordered_texts);
        }

        let batch_plan = plan_embedding_batches(&ordered_texts, self.batching_policy);
        let total_batches = batch_plan.len();
        let min_chars = ordered_texts
            .iter()
            .map(|(_, text, _)| text.len())
            .min()
            .unwrap_or(0);
        let max_chars = ordered_texts
            .iter()
            .map(|(_, text, _)| text.len())
            .max()
            .unwrap_or(0);
        let token_summary = summarize_token_lengths(&ordered_texts, &batch_plan, self.fixed_shape);

        if let Some(summary) = token_summary {
            tracing::info!(
                chunks = ordered_texts.len(),
                sub_batches = total_batches,
                policy = %self.batching_policy,
                min_chars,
                max_chars,
                raw_tokens_total = summary.raw_tokens_total,
                capped_tokens_total = summary.capped_tokens_total,
                padded_tokens_total = summary.padded_tokens_total,
                padding_waste_tokens = summary
                    .padded_tokens_total
                    .saturating_sub(summary.capped_tokens_total),
                min_tokens = summary.min_tokens,
                max_tokens = summary.max_tokens,
                token_metrics_available = true,
                "Embedding batch plan"
            );
        } else {
            tracing::info!(
                chunks = ordered_texts.len(),
                sub_batches = total_batches,
                policy = %self.batching_policy,
                min_chars,
                max_chars,
                token_metrics_available = false,
                "Embedding batch plan"
            );
        }

        let embed_start = Instant::now();
        let mut all_embeddings: Vec<Option<Embedding>> =
            (0..ordered_texts.len()).map(|_| None).collect();

        for (batch_idx, plan) in batch_plan.iter().enumerate() {
            let chunk_batch = &ordered_texts[plan.start..plan.end];
            let min_chars = chunk_batch
                .iter()
                .map(|(_, text, _)| text.len())
                .min()
                .unwrap_or(0);
            let max_chars = chunk_batch
                .iter()
                .map(|(_, text, _)| text.len())
                .max()
                .unwrap_or(0);
            tracing::debug!(
                "Embedding GPU sub-batch {}/{} ({} chunks, policy {}, chars {}..{})",
                batch_idx + 1,
                total_batches,
                chunk_batch.len(),
                self.batching_policy,
                min_chars,
                max_chars
            );
            let batch_texts: Vec<String> = chunk_batch
                .iter()
                .map(|(_, text, _)| text.clone())
                .collect();
            let batch_embeddings = self
                .embedding_generator
                .embed_documents(batch_texts)
                .await?;

            for ((original_idx, _, _), embedding) in chunk_batch.iter().zip(batch_embeddings) {
                all_embeddings[*original_idx] = Some(embedding);
            }
        }

        let embeddings = all_embeddings
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| IndexingError::Parser("Embedding result ordering failed".into()))?;

        let embed_duration = embed_start.elapsed();
        let chunks_per_sec = if embed_duration.is_zero() {
            0.0
        } else {
            embeddings.len() as f64 / embed_duration.as_secs_f64()
        };
        if let Some(summary) = token_summary {
            tracing::info!(
                chunks = embeddings.len(),
                sub_batches = total_batches,
                policy = %self.batching_policy,
                elapsed_secs = embed_duration.as_secs_f64(),
                chunks_per_sec,
                min_chars,
                max_chars,
                raw_tokens_total = summary.raw_tokens_total,
                capped_tokens_total = summary.capped_tokens_total,
                padded_tokens_total = summary.padded_tokens_total,
                padded_tokens_per_sec = if embed_duration.is_zero() {
                    0.0
                } else {
                    summary.padded_tokens_total as f64 / embed_duration.as_secs_f64()
                },
                token_metrics_available = true,
                "Embedding batcher completed document embeddings"
            );
        } else {
            tracing::info!(
                chunks = embeddings.len(),
                sub_batches = total_batches,
                policy = %self.batching_policy,
                elapsed_secs = embed_duration.as_secs_f64(),
                chunks_per_sec,
                min_chars,
                max_chars,
                token_metrics_available = false,
                "Embedding batcher completed document embeddings"
            );
        }

        Ok(embeddings)
    }

    async fn generate_openrouter_embeddings(
        &self,
        chunk_texts: Vec<String>,
        token_lengths: Option<Vec<EmbeddingTextLen>>,
    ) -> Result<Vec<Embedding>, IndexingError> {
        let total_chunks = chunk_texts.len();
        let min_chars = chunk_texts.iter().map(|text| text.len()).min().unwrap_or(0);
        let max_chars = chunk_texts.iter().map(|text| text.len()).max().unwrap_or(0);
        let token_summary = summarize_unsorted_token_lengths(token_lengths.as_deref());

        if let Some(summary) = token_summary {
            tracing::info!(
                chunks = total_chunks,
                min_chars,
                max_chars,
                raw_tokens_total = summary.raw_tokens_total,
                capped_tokens_total = summary.capped_tokens_total,
                min_tokens = summary.min_tokens,
                max_tokens = summary.max_tokens,
                token_metrics_available = true,
                "Embedding OpenRouter remote batch plan"
            );
        } else {
            tracing::info!(
                chunks = total_chunks,
                min_chars,
                max_chars,
                token_metrics_available = false,
                "Embedding OpenRouter remote batch plan"
            );
        }

        let embed_start = Instant::now();
        let embeddings = self
            .embedding_generator
            .embed_documents(chunk_texts)
            .await?;

        if embeddings.len() != total_chunks {
            return Err(IndexingError::Parser(format!(
                "OpenRouter returned {} embeddings for {} chunks",
                embeddings.len(),
                total_chunks
            )));
        }

        let embed_duration = embed_start.elapsed();
        let chunks_per_sec = if embed_duration.is_zero() {
            0.0
        } else {
            embeddings.len() as f64 / embed_duration.as_secs_f64()
        };

        if let Some(summary) = token_summary {
            tracing::info!(
                chunks = embeddings.len(),
                elapsed_secs = embed_duration.as_secs_f64(),
                chunks_per_sec,
                min_chars,
                max_chars,
                raw_tokens_total = summary.raw_tokens_total,
                capped_tokens_total = summary.capped_tokens_total,
                min_tokens = summary.min_tokens,
                max_tokens = summary.max_tokens,
                token_metrics_available = true,
                "Embedding batcher completed OpenRouter document embeddings"
            );
        } else {
            tracing::info!(
                chunks = embeddings.len(),
                elapsed_secs = embed_duration.as_secs_f64(),
                chunks_per_sec,
                min_chars,
                max_chars,
                token_metrics_available = false,
                "Embedding batcher completed OpenRouter document embeddings"
            );
        }

        Ok(embeddings)
    }

    fn count_token_lengths(&self, texts: &[String]) -> Option<Vec<EmbeddingTextLen>> {
        let counter = self.token_counter.as_ref()?;
        match counter.count_batch(texts) {
            Ok(lengths) => Some(lengths),
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "Embedding token metrics unavailable for this batch"
                );
                None
            }
        }
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

/// Summary of input lengths + how many tokens ACTUALLY went through the model.
///
/// `fixed_shape`: the fixed input shape, if the runtime requires one.
/// It changes the meaning of `padded_tokens_total`: with a floating shape a batch costs
/// rows × the longest row IN IT; with a fixed one, shape rows ×
/// seq_len, counting shape rows rather than actual ones (the tail is padded with copies).
/// They cannot be counted the same way: on the GPU profile, counting by actual lengths would
/// understate the cost of a run several times over, and padding_waste_tokens would show almost zero exactly
/// where padding is at its maximum.
fn summarize_token_lengths(
    ordered_texts: &[(usize, String, Option<EmbeddingTextLen>)],
    batch_plan: &[EmbeddingBatchPlan],
    fixed_shape: Option<FixedInputShape>,
) -> Option<TokenLengthSummary> {
    if ordered_texts.is_empty() {
        return Some(TokenLengthSummary {
            raw_tokens_total: 0,
            capped_tokens_total: 0,
            padded_tokens_total: 0,
            min_tokens: 0,
            max_tokens: 0,
        });
    }

    let mut raw_tokens_total = 0usize;
    let mut capped_tokens_total = 0usize;
    let mut min_tokens = usize::MAX;
    let mut max_tokens = 0usize;

    for (_, _, token_len) in ordered_texts {
        let token_len = token_len.as_ref()?;
        raw_tokens_total += token_len.raw_tokens;
        capped_tokens_total += token_len.capped_tokens;
        min_tokens = min_tokens.min(token_len.capped_tokens);
        max_tokens = max_tokens.max(token_len.capped_tokens);
    }

    let mut padded_tokens_total = 0usize;
    for plan in batch_plan {
        let chunk_batch = &ordered_texts[plan.start..plan.end];
        padded_tokens_total += match fixed_shape {
            // The shape is fixed: the batch cost depends neither on lengths nor on
            // how many rows in it are actually used.
            Some(shape) => shape.rows.0 * shape.seq_len,
            None => {
                let batch_max = chunk_batch
                    .iter()
                    .filter_map(|(_, _, token_len)| token_len.map(|len| len.capped_tokens))
                    .max()
                    .unwrap_or(0);
                batch_max * chunk_batch.len()
            }
        };
    }

    Some(TokenLengthSummary {
        raw_tokens_total,
        capped_tokens_total,
        padded_tokens_total,
        min_tokens,
        max_tokens,
    })
}

fn summarize_unsorted_token_lengths(
    token_lengths: Option<&[EmbeddingTextLen]>,
) -> Option<TokenLengthSummary> {
    let token_lengths = token_lengths?;
    if token_lengths.is_empty() {
        return Some(TokenLengthSummary {
            raw_tokens_total: 0,
            capped_tokens_total: 0,
            padded_tokens_total: 0,
            min_tokens: 0,
            max_tokens: 0,
        });
    }

    let mut raw_tokens_total = 0usize;
    let mut capped_tokens_total = 0usize;
    let mut min_tokens = usize::MAX;
    let mut max_tokens = 0usize;

    for token_len in token_lengths {
        raw_tokens_total += token_len.raw_tokens;
        capped_tokens_total += token_len.capped_tokens;
        min_tokens = min_tokens.min(token_len.capped_tokens);
        max_tokens = max_tokens.max(token_len.capped_tokens);
    }

    Some(TokenLengthSummary {
        raw_tokens_total,
        capped_tokens_total,
        padded_tokens_total: capped_tokens_total,
        min_tokens,
        max_tokens,
    })
}

fn plan_embedding_batches(
    ordered_texts: &[(usize, String, Option<EmbeddingTextLen>)],
    policy: BatchingPolicy,
) -> Vec<EmbeddingBatchPlan> {
    policy.plan(ordered_texts, |(_, text, token_len)| {
        token_len
            .map(|len| len.capped_tokens)
            .unwrap_or_else(|| text.len())
    })
}

fn sort_embedding_inputs(ordered_texts: &mut [(usize, String, Option<EmbeddingTextLen>)]) {
    ordered_texts.sort_by_key(|(original_idx, text, token_len)| {
        (
            token_len
                .map(|len| len.capped_tokens)
                .unwrap_or_else(|| text.len()),
            *original_idx,
        )
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmc_engine::embeddings::batching::BatchRows;

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

    #[test]
    fn test_sort_embedding_inputs_uses_capped_token_length() {
        let mut inputs = vec![
            (
                0,
                "aaaa".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 100,
                    capped_tokens: 9,
                }),
            ),
            (
                1,
                "b".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 2,
                    capped_tokens: 2,
                }),
            ),
            (
                2,
                "cc".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 8,
                    capped_tokens: 8,
                }),
            ),
        ];

        sort_embedding_inputs(&mut inputs);

        let ordered_indices: Vec<usize> = inputs.iter().map(|(idx, _, _)| *idx).collect();
        assert_eq!(ordered_indices, vec![1, 2, 0]);
    }

    #[test]
    fn test_sort_embedding_inputs_preserves_equal_length_order_by_original_index() {
        let mut inputs = vec![
            (
                2,
                "c".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 4,
                    capped_tokens: 4,
                }),
            ),
            (
                0,
                "a".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 4,
                    capped_tokens: 4,
                }),
            ),
            (
                1,
                "b".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 4,
                    capped_tokens: 4,
                }),
            ),
        ];

        sort_embedding_inputs(&mut inputs);

        let ordered_indices: Vec<usize> = inputs.iter().map(|(idx, _, _)| *idx).collect();
        assert_eq!(ordered_indices, vec![0, 1, 2]);
    }

    #[test]
    fn test_summarize_token_lengths_accounts_for_padding() {
        let ordered = vec![
            (
                0,
                "a".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 3,
                    capped_tokens: 3,
                }),
            ),
            (
                1,
                "bb".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 5,
                    capped_tokens: 5,
                }),
            ),
            (
                2,
                "ccc".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 7,
                    capped_tokens: 7,
                }),
            ),
        ];

        let policy = BatchingPolicy::new(None, 2, 10);
        let batch_plan = plan_embedding_batches(&ordered, policy);
        let summary = summarize_token_lengths(&ordered, &batch_plan, None).unwrap();

        assert_eq!(summary.raw_tokens_total, 15);
        assert_eq!(summary.capped_tokens_total, 15);
        assert_eq!(summary.padded_tokens_total, 17);
        assert_eq!(summary.min_tokens, 3);
        assert_eq!(summary.max_tokens, 7);
    }

    #[test]
    fn test_summarize_unsorted_token_lengths_for_remote_path() {
        let lengths = vec![
            EmbeddingTextLen {
                raw_tokens: 10,
                capped_tokens: 8,
            },
            EmbeddingTextLen {
                raw_tokens: 3,
                capped_tokens: 3,
            },
        ];

        let summary = summarize_unsorted_token_lengths(Some(&lengths)).unwrap();

        assert_eq!(summary.raw_tokens_total, 13);
        assert_eq!(summary.capped_tokens_total, 11);
        assert_eq!(summary.padded_tokens_total, 11);
        assert_eq!(summary.min_tokens, 3);
        assert_eq!(summary.max_tokens, 8);
    }

    #[test]
    fn test_plan_embedding_batches_respects_token_budget() {
        let ordered = vec![
            (
                0,
                "a".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 4,
                    capped_tokens: 4,
                }),
            ),
            (
                1,
                "b".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 4,
                    capped_tokens: 4,
                }),
            ),
            (
                2,
                "c".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 8,
                    capped_tokens: 8,
                }),
            ),
            (
                3,
                "d".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 8,
                    capped_tokens: 8,
                }),
            ),
        ];

        let plan = plan_embedding_batches(&ordered, BatchingPolicy::new(None, 4, 16));

        assert_eq!(
            plan,
            vec![
                EmbeddingBatchPlan { start: 0, end: 2 },
                EmbeddingBatchPlan { start: 2, end: 4 },
            ]
        );
    }

    #[test]
    fn test_plan_embedding_batches_keeps_oversize_single_item() {
        let ordered = vec![
            (
                0,
                "a".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 32,
                    capped_tokens: 32,
                }),
            ),
            (
                1,
                "b".to_string(),
                Some(EmbeddingTextLen {
                    raw_tokens: 2,
                    capped_tokens: 2,
                }),
            ),
        ];

        let plan = plan_embedding_batches(&ordered, BatchingPolicy::new(None, 4, 16));

        assert_eq!(
            plan,
            vec![
                EmbeddingBatchPlan { start: 0, end: 1 },
                EmbeddingBatchPlan { start: 1, end: 2 },
            ]
        );
    }

    /// With a fixed shape the cost of a run is `rows × seq_len` for EACH batch,
    /// including a partial one: the padding rows go through the model just the same.
    ///
    /// A gate on the honesty of the metric, not on arithmetic: counting by the longest
    /// row in the batch on the GPU profile would understate the cost several times over, and
    /// `padding_waste_tokens` would show near-zero exactly where padding
    /// is at its maximum.
    #[test]
    fn summarize_counts_the_whole_fixed_shape_including_the_padded_tail() {
        let ordered: Vec<(usize, String, Option<EmbeddingTextLen>)> = (0..3)
            .map(|idx| {
                (
                    idx,
                    "x".to_string(),
                    Some(EmbeddingTextLen {
                        raw_tokens: 5,
                        capped_tokens: 5,
                    }),
                )
            })
            .collect();

        let shape = FixedInputShape {
            rows: BatchRows(2),
            seq_len: 512,
        };
        let policy = BatchingPolicy::new(Some(shape.rows), 100, 10);
        let batch_plan = plan_embedding_batches(&ordered, policy);
        // Three inputs at height 2: two batches, the second one half full.
        assert_eq!(batch_plan.len(), 2);

        let summary = summarize_token_lengths(&ordered, &batch_plan, Some(shape)).unwrap();
        assert_eq!(summary.capped_tokens_total, 15);
        assert_eq!(
            summary.padded_tokens_total,
            2 * 2 * 512,
            "the partial batch was counted cheaper than a full one"
        );
    }

    /// Positive control for the previous one: without a fixed shape the count stays
    /// as before, by the longest row IN THE BATCH.
    #[test]
    fn summarize_without_fixed_shape_counts_by_the_longest_row() {
        let ordered: Vec<(usize, String, Option<EmbeddingTextLen>)> = (0..3)
            .map(|idx| {
                (
                    idx,
                    "x".to_string(),
                    Some(EmbeddingTextLen {
                        raw_tokens: 5,
                        capped_tokens: 5,
                    }),
                )
            })
            .collect();
        let batch_plan = plan_embedding_batches(&ordered, BatchingPolicy::new(None, 2, 1000));
        let summary = summarize_token_lengths(&ordered, &batch_plan, None).unwrap();
        assert_eq!(summary.padded_tokens_total, 15);
    }
}

//! Batch planning, per-input bookkeeping, and order-restoration helpers for the
//! OpenRouter embeddings backend.

use crate::embeddings::Embedding;
use crate::embeddings::batching::{BatchPlan as OpenRouterBatchPlan, plan_batches};
use crate::embeddings::openrouter::config::OpenRouterRuntimeConfig;

#[derive(Debug)]
pub(super) enum OpenRouterBatchError {
    PayloadTooLarge(String),
    Fatal(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenRouterInput {
    pub(super) original_index: usize,
    pub(super) text: String,
    pub(super) token_len: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct OpenRouterInputBatch {
    pub(super) inputs: Vec<OpenRouterInput>,
}

impl OpenRouterInputBatch {
    pub(super) fn texts(&self) -> Vec<String> {
        self.inputs
            .iter()
            .map(|input| input.text.clone())
            .collect()
    }

    pub(super) fn len(&self) -> usize {
        self.inputs.len()
    }

    pub(super) fn max_token_len(&self) -> usize {
        self.inputs
            .iter()
            .map(|input| input.token_len)
            .max()
            .unwrap_or(0)
    }

    pub(super) fn padded_tokens(&self) -> usize {
        self.len() * self.max_token_len()
    }

    pub(super) fn split_at(self, mid: usize) -> (Self, Self) {
        let right = self.inputs[mid..].to_vec();
        let left = self.inputs[..mid].to_vec();
        (Self { inputs: left }, Self { inputs: right })
    }
}

pub(super) fn plan_remote_input_batches(
    texts: Vec<String>,
    token_lengths: Vec<usize>,
    config: OpenRouterRuntimeConfig,
) -> Vec<OpenRouterInputBatch> {
    assert_eq!(
        texts.len(),
        token_lengths.len(),
        "OpenRouter planner requires one token length per text"
    );

    let mut inputs: Vec<OpenRouterInput> = texts
        .into_iter()
        .zip(token_lengths)
        .enumerate()
        .map(|(original_index, (text, token_len))| OpenRouterInput {
            original_index,
            text,
            token_len: token_len.max(1),
        })
        .collect();

    sort_openrouter_inputs(&mut inputs);
    plan_openrouter_batches(&inputs, config)
        .into_iter()
        .map(|plan| OpenRouterInputBatch {
            inputs: inputs[plan.start..plan.end].to_vec(),
        })
        .collect()
}

pub(super) fn sort_openrouter_inputs(inputs: &mut [OpenRouterInput]) {
    inputs.sort_by_key(|input| (input.token_len, input.original_index));
}

fn plan_openrouter_batches(
    inputs: &[OpenRouterInput],
    config: OpenRouterRuntimeConfig,
) -> Vec<OpenRouterBatchPlan> {
    plan_batches(
        inputs,
        config.max_batch_inputs,
        config.max_batch_tokens,
        |input| input.token_len,
    )
}

pub(super) fn fallback_token_estimate(text: &str) -> usize {
    text.len().div_ceil(4).max(1)
}

pub(super) fn restore_original_embedding_order(
    expected_count: usize,
    embeddings: Vec<(usize, Embedding)>,
) -> Result<Vec<Embedding>, String> {
    let mut output: Vec<Option<Embedding>> = vec![None; expected_count];
    for (original_index, embedding) in embeddings {
        if original_index >= expected_count {
            return Err(format!(
                "OpenRouter embedding result had out-of-range original index {} for {} inputs",
                original_index, expected_count
            ));
        }
        output[original_index] = Some(embedding);
    }

    output
        .into_iter()
        .enumerate()
        .map(|(idx, maybe)| {
            maybe.ok_or_else(|| {
                format!("OpenRouter embedding result omitted original index {idx}")
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embeddings::openrouter::config::OpenRouterEncodingFormat;

    #[test]
    fn sorts_openrouter_inputs_by_token_length_then_original_index() {
        let mut inputs = vec![
            openrouter_input(2, "c", 4),
            openrouter_input(0, "a", 8),
            openrouter_input(1, "b", 4),
        ];

        sort_openrouter_inputs(&mut inputs);

        let original_indices: Vec<usize> =
            inputs.iter().map(|input| input.original_index).collect();
        assert_eq!(original_indices, vec![1, 2, 0]);
    }

    #[test]
    fn plans_openrouter_batches_by_input_count() {
        let batches = plan_remote_input_batches(
            vec!["a", "b", "c", "d", "e"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![1, 1, 1, 1, 1],
            test_config(2, 100, 4),
        );

        let batch_lens: Vec<usize> = batches.iter().map(OpenRouterInputBatch::len).collect();
        assert_eq!(batch_lens, vec![2, 2, 1]);
    }

    #[test]
    fn plans_openrouter_batches_by_padded_token_budget() {
        let batches = plan_remote_input_batches(
            vec!["a", "b", "c", "d"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![4, 4, 8, 8],
            test_config(4, 16, 4),
        );

        let batch_lens: Vec<usize> = batches.iter().map(OpenRouterInputBatch::len).collect();
        let padded_tokens: Vec<usize> =
            batches.iter().map(OpenRouterInputBatch::padded_tokens).collect();

        assert_eq!(batch_lens, vec![2, 2]);
        assert_eq!(padded_tokens, vec![8, 16]);
    }

    #[test]
    fn plans_openrouter_oversize_input_as_single_batch() {
        let batches = plan_remote_input_batches(
            vec!["oversize", "small"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![32, 2],
            test_config(4, 16, 4),
        );

        assert!(batches.iter().any(|batch| {
            batch.len() == 1
                && batch.inputs[0].original_index == 0
                && batch.inputs[0].token_len == 32
        }));
    }

    #[test]
    fn openrouter_planner_keeps_original_indices_for_order_restoration() {
        let batches = plan_remote_input_batches(
            vec!["third", "first", "second"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            vec![9, 3, 6],
            test_config(8, 100, 4),
        );
        let pairs: Vec<(usize, Embedding)> = batches
            .into_iter()
            .flat_map(|batch| batch.inputs)
            .map(|input| (input.original_index, vec![input.original_index as f32]))
            .collect();

        let restored = restore_original_embedding_order(3, pairs).unwrap();

        assert_eq!(restored, vec![vec![0.0], vec![1.0], vec![2.0]]);
    }

    #[test]
    fn openrouter_planner_benchmark_shape_targets_fewer_requests() {
        let text_count = 2084;
        let batches = plan_remote_input_batches(
            (0..text_count)
                .map(|idx| format!("chunk {idx}"))
                .collect(),
            vec![300; text_count],
            test_config(128, 131_072, 4),
        );

        assert!(
            (8..=20).contains(&batches.len()),
            "expected 8-20 batches, got {}",
            batches.len()
        );
    }

    #[test]
    fn fallback_token_estimate_is_deterministic_and_nonzero() {
        assert_eq!(fallback_token_estimate(""), 1);
        assert_eq!(fallback_token_estimate("abcd"), 1);
        assert_eq!(fallback_token_estimate("abcde"), 2);
    }

    fn openrouter_input(original_index: usize, text: &str, token_len: usize) -> OpenRouterInput {
        OpenRouterInput {
            original_index,
            text: text.to_string(),
            token_len,
        }
    }

    fn test_config(
        max_batch_inputs: usize,
        max_batch_tokens: usize,
        concurrency: usize,
    ) -> OpenRouterRuntimeConfig {
        OpenRouterRuntimeConfig {
            max_batch_inputs,
            max_batch_tokens,
            concurrency,
            encoding_format: OpenRouterEncodingFormat::Float,
            provider: None,
        }
    }
}

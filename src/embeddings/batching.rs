#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BatchPlan {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

pub(crate) fn plan_batches<T>(
    items: &[T],
    max_batch_size: usize,
    max_tokens_per_batch: usize,
    mut token_len: impl FnMut(&T) -> usize,
) -> Vec<BatchPlan> {
    if items.is_empty() {
        return Vec::new();
    }

    let max_batch_size = max_batch_size.max(1);
    let max_tokens_per_batch = max_tokens_per_batch.max(1);
    let mut plans = Vec::new();
    let mut start = 0usize;
    let mut batch_len = 0usize;
    let mut batch_max_tokens = 0usize;

    for (idx, item) in items.iter().enumerate() {
        let item_tokens = token_len(item).max(1);
        let next_len = batch_len + 1;
        let next_max_tokens = batch_max_tokens.max(item_tokens);
        let exceeds_count = next_len > max_batch_size;
        let exceeds_token_budget = next_len * next_max_tokens > max_tokens_per_batch;

        if batch_len > 0 && (exceeds_count || exceeds_token_budget) {
            plans.push(BatchPlan { start, end: idx });
            start = idx;
            batch_len = 0;
            batch_max_tokens = 0;
        }

        batch_len += 1;
        batch_max_tokens = batch_max_tokens.max(item_tokens);
    }

    if batch_len > 0 {
        plans.push(BatchPlan {
            start,
            end: items.len(),
        });
    }

    plans
}

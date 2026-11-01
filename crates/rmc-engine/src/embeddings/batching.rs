#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchPlan {
    pub start: usize,
    pub end: usize,
}

pub fn plan_batches<T>(
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

/// Batch height — the number of ROWS of model input.
///
/// A newtype rather than `usize`, because two more 'batch sizes' live nearby,
/// and nothing else would stop the compiler from mixing them up: the TOKEN budget per batch
/// (`max_tokens_per_batch`) and the number of CHUNKS that arrived for indexing. All three
/// arrive at the same planner, and all three are `usize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BatchRows(pub usize);

/// Constant model input shape: `rows × seq_len`.
///
/// Exists only for runtimes that compile kernels FOR THE SHAPE
/// (today — MIGraphX). The others have no shape at all: there padding to the
/// longest row in the batch is free, and there is nothing to fix.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FixedInputShape {
    pub rows: BatchRows,
    pub seq_len: usize,
}

/// Who dictates the batch shape.
///
/// # Why this is a separate type
/// TWO parties know how to cut inputs into batches: the indexer (by its `gpu_batch_size` and
/// token budget) and fastembed itself (by the constant shape, if one is set).
/// As long as both sides cut silently, the coincidence of their defaults (both 32) looks like
/// consistency, but in fact rests on nothing: raise `gpu_batch_size` to
/// 100 and you get two cuts in a row — the indexer hands over 100 rows, the model
/// cuts them into 32+32+32+4 and pads the tail to 32. A tail appears in EVERY
/// indexer batch, not once per run.
///
/// So the decision 'who cuts' is made ONCE and explicitly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatchingPolicy {
    /// The MODEL dictates the shape: exactly `rows` rows per batch, and it pads the tail itself.
    ///
    /// The token budget and sorting inputs by length are dead work here:
    /// every row is padded to `seq_len` anyway, so whether long and short inputs are neighbours
    /// does not affect the cost of the run.
    FixedByModel(BatchRows),
    /// The INDEXER dictates the shape: a height plus a cap on 'rows × longest
    /// row', so that padding to the longest one does not eat up memory.
    ByBudget {
        max_rows: usize,
        max_tokens_per_batch: usize,
    },
}

impl BatchingPolicy {
    /// `fixed_rows` is the shape the model runtime requires (`None` if it
    /// requires no shape). It OVERRIDES the indexer settings: a setting
    /// that would produce a second cut is not a 'compromise' but extra work.
    pub fn new(
        fixed_rows: Option<BatchRows>,
        max_rows: usize,
        max_tokens_per_batch: usize,
    ) -> Self {
        match fixed_rows {
            Some(rows) => Self::FixedByModel(rows),
            None => Self::ByBudget {
                max_rows: max_rows.max(1),
                max_tokens_per_batch: max_tokens_per_batch.max(1),
            },
        }
    }

    /// Number of rows in a full batch — what the model run actually executes on.
    pub fn rows_per_batch(&self) -> BatchRows {
        match *self {
            Self::FixedByModel(rows) => rows,
            Self::ByBudget { max_rows, .. } => BatchRows(max_rows),
        }
    }

    /// Whether it makes sense to keep inputs of similar length together.
    ///
    /// With a constant shape — no: every row's length is brought up to
    /// `seq_len` anyway, and sorting only wastes cycles on an input of several thousand
    /// chunks.
    pub fn sorts_by_length(&self) -> bool {
        matches!(self, Self::ByBudget { .. })
    }

    pub fn plan<T>(&self, items: &[T], token_len: impl FnMut(&T) -> usize) -> Vec<BatchPlan> {
        match *self {
            Self::FixedByModel(BatchRows(rows)) => plan_fixed_rows(items.len(), rows),
            Self::ByBudget {
                max_rows,
                max_tokens_per_batch,
            } => plan_batches(items, max_rows, max_tokens_per_batch, token_len),
        }
    }
}

impl std::fmt::Display for BatchingPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::FixedByModel(BatchRows(rows)) => write!(f, "model-fixed(rows={rows})"),
            Self::ByBudget {
                max_rows,
                max_tokens_per_batch,
            } => write!(f, "budget(rows={max_rows},tokens={max_tokens_per_batch})"),
        }
    }
}

/// Cut exactly by the shape height: all batches are full except the last one.
///
/// That is the whole point of the `FixedByModel` policy — padding happens once
/// per run, not once per indexer batch.
fn plan_fixed_rows(len: usize, rows: usize) -> Vec<BatchPlan> {
    let rows = rows.max(1);
    (0..len)
        .step_by(rows)
        .map(|start| BatchPlan {
            start,
            end: (start + rows).min(len),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUDGET: usize = 49_152;

    /// Token length plays no part in cutting with a constant shape — here it is
    /// deliberately DIFFERENT, so that the budget would kick in if it were in play.
    fn items(n: usize) -> Vec<usize> {
        (0..n).map(|i| 8 + (i % 17) * 64).collect()
    }

    fn plan(policy: BatchingPolicy, n: usize) -> Vec<BatchPlan> {
        policy.plan(&items(n), |len| *len)
    }

    /// 🔑 DoD gate: with a constant shape the indexer's cut setting has NO EFFECT.
    ///
    /// The mutant on which the test must go red: make `new` respect
    /// `max_rows` when `Some(rows)` (i.e. bring back two cuts in a row).
    #[test]
    fn fixed_shape_ignores_indexer_batch_size() {
        let rows = BatchRows(32);
        let reference = plan(BatchingPolicy::new(Some(rows), 32, BUDGET), 1000);
        for configured_rows in [1usize, 7, 31, 32, 64, 100, 1000] {
            for configured_tokens in [1usize, 4096, BUDGET] {
                let policy = BatchingPolicy::new(Some(rows), configured_rows, configured_tokens);
                assert_eq!(
                    plan(policy, 1000),
                    reference,
                    "indexer settings ({configured_rows}, {configured_tokens}) \
                     changed the cut under a constant model shape"
                );
            }
        }
    }

    /// EXACTLY one batch per run gets padded, not one per indexer
    /// cut. That is exactly the cost this policy was introduced to remove.
    #[test]
    fn fixed_shape_pads_only_the_tail() {
        let rows = 32usize;
        for n in [1usize, 31, 32, 33, 64, 100, 999, 1000] {
            let plans = plan(BatchingPolicy::new(Some(BatchRows(rows)), 100, BUDGET), n);
            let padded_rows: usize = plans.iter().map(|p| rows - (p.end - p.start)).sum();
            assert_eq!(
                padded_rows,
                rows * n.div_ceil(rows) - n,
                "padding beyond a single tail at n={n}"
            );
            assert!(
                plans
                    .iter()
                    .take(plans.len().saturating_sub(1))
                    .all(|p| p.end - p.start == rows),
                "an incomplete batch is not the last one at n={n}"
            );
        }
    }

    /// Coverage: the cut must cover the input entirely and without overlaps — otherwise
    /// 'fewer embeddings than chunks' would only surface in the index.
    #[test]
    fn fixed_shape_covers_every_input_exactly_once() {
        for n in [0usize, 1, 5, 32, 33, 257] {
            let plans = plan(BatchingPolicy::new(Some(BatchRows(32)), 32, BUDGET), n);
            let covered: Vec<usize> = plans.iter().flat_map(|p| p.start..p.end).collect();
            assert_eq!(
                covered,
                (0..n).collect::<Vec<_>>(),
                "the cut did not cover the input at n={n}"
            );
        }
    }

    /// Without a constant shape the policy stays as before: both the height and the token
    /// budget apply. Positive control — without it the tests above are green even on
    /// code that simply ignores the settings ALWAYS.
    #[test]
    fn budget_policy_still_honours_both_limits() {
        let policy = BatchingPolicy::new(None, 8, BUDGET);
        assert!(policy.sorts_by_length());
        let plans = plan(policy, 20);
        assert!(plans.iter().all(|p| p.end - p.start <= 8));
        assert_eq!(plans.iter().map(|p| p.end - p.start).sum::<usize>(), 20);

        // The token budget cuts BEFORE the height does: 4 rows of 1000 tokens
        // do not fit into a budget of 2048.
        let long = vec![1000usize; 4];
        let plans = BatchingPolicy::new(None, 8, 2048).plan(&long, |len| *len);
        assert_eq!(plans.len(), 2, "token budget stopped cutting");
    }

    #[test]
    fn fixed_policy_does_not_sort_and_reports_its_rows() {
        let policy = BatchingPolicy::new(Some(BatchRows(32)), 100, BUDGET);
        assert!(!policy.sorts_by_length());
        assert_eq!(policy.rows_per_batch(), BatchRows(32));
        assert_eq!(policy.to_string(), "model-fixed(rows=32)");
    }
}

//! RRF (Reciprocal Rank Fusion) parameter tuning framework
//!
//! Automatically tunes the k parameter for optimal hybrid search quality
//! using a test dataset and NDCG evaluation metrics.

use crate::search::{HybridSearch, SearchResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Test query with ground truth relevant results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestQuery {
    /// The search query text
    pub query: String,
    /// List of symbol names that are relevant for this query
    pub relevant_chunk_ids: Vec<String>,
}

/// RRF parameter tuner for hybrid search
pub struct RRFTuner {
    test_queries: Vec<TestQuery>,
}

/// Tuning results with metrics for each k value tested
#[derive(Debug, Clone)]
pub struct TuningResult {
    pub best_k: f32,
    pub best_ndcg: f64,
    pub k_values_tested: Vec<(f32, f64)>, // (k, ndcg@10)
}

impl RRFTuner {
    /// Create a tuner with a custom test dataset
    pub fn new(test_queries: Vec<TestQuery>) -> Self {
        Self { test_queries }
    }

    /// Create a tuner with default Rust code search queries
    ///
    /// These queries cover common Rust programming tasks and are useful
    /// for tuning a general-purpose Rust code search system.
    pub fn default_rust_queries() -> Self {
        Self {
            test_queries: vec![
                TestQuery {
                    query: "parse command line arguments".to_string(),
                    relevant_chunk_ids: vec![
                        "clap_parser".to_string(),
                        "parse_args".to_string(),
                        "Args".to_string(),
                    ],
                },
                TestQuery {
                    query: "async http request".to_string(),
                    relevant_chunk_ids: vec![
                        "reqwest".to_string(),
                        "http_client".to_string(),
                        "async_request".to_string(),
                    ],
                },
                TestQuery {
                    query: "error handling with Result".to_string(),
                    relevant_chunk_ids: vec![
                        "Result".to_string(),
                        "error_handling".to_string(),
                        "Error".to_string(),
                    ],
                },
                TestQuery {
                    query: "serialize json data".to_string(),
                    relevant_chunk_ids: vec![
                        "serde_json".to_string(),
                        "to_json".to_string(),
                        "Serialize".to_string(),
                    ],
                },
                TestQuery {
                    query: "read file from filesystem".to_string(),
                    relevant_chunk_ids: vec![
                        "read_to_string".to_string(),
                        "fs::read".to_string(),
                        "File::open".to_string(),
                    ],
                },
                TestQuery {
                    query: "vector search with embeddings".to_string(),
                    relevant_chunk_ids: vec![
                        "VectorStore".to_string(),
                        "search".to_string(),
                        "embeddings".to_string(),
                    ],
                },
                TestQuery {
                    query: "parse rust source code with tree-sitter".to_string(),
                    relevant_chunk_ids: vec![
                        "RustParser".to_string(),
                        "parse_source".to_string(),
                        "tree_sitter".to_string(),
                    ],
                },
                TestQuery {
                    query: "create index for search".to_string(),
                    relevant_chunk_ids: vec![
                        "index_directory".to_string(),
                        "UnifiedIndexer".to_string(),
                        "create_index".to_string(),
                    ],
                },
            ],
        }
    }

    /// Tune the RRF k parameter by testing multiple values
    ///
    /// Tests k values: [10.0, 20.0, 40.0, 60.0, 80.0, 100.0]
    /// Returns the k value that achieves the highest NDCG@10 score.
    pub async fn tune_k(&self, hybrid_search: &HybridSearch) -> Result<TuningResult, Box<dyn std::error::Error + Send + Sync>> {
        let k_values = vec![10.0, 20.0, 40.0, 60.0, 80.0, 100.0];

        let mut best_k = 60.0;
        let mut best_ndcg = 0.0;
        let mut results = Vec::new();

        tracing::info!("🔬 Starting RRF k parameter tuning with {} test queries...", self.test_queries.len());

        for k in &k_values {
            let mut total_ndcg = 0.0;

            for test_query in &self.test_queries {
                // Search with this k value
                let search_results = hybrid_search.search_with_k(&test_query.query, 20, *k).await
                    .map_err(|e| format!("Search failed: {}", e))?;

                // Calculate NDCG@10
                let ndcg = calculate_ndcg(&search_results, &test_query.relevant_chunk_ids, 10);
                total_ndcg += ndcg;
            }

            let avg_ndcg = total_ndcg / self.test_queries.len() as f64;
            results.push((*k, avg_ndcg));

            tracing::info!("  k={:5.1}: NDCG@10={:.4}", k, avg_ndcg);

            if avg_ndcg > best_ndcg {
                best_ndcg = avg_ndcg;
                best_k = *k;
            }
        }

        tracing::info!("✓ Optimal k={} with NDCG@10={:.4}", best_k, best_ndcg);

        Ok(TuningResult {
            best_k,
            best_ndcg,
            k_values_tested: results,
        })
    }

    /// Tune k parameter with detailed per-query analysis
    pub async fn tune_k_verbose(&self, hybrid_search: &HybridSearch) -> Result<TuningResult, Box<dyn std::error::Error + Send + Sync>> {
        let k_values = vec![10.0, 20.0, 40.0, 60.0, 80.0, 100.0];

        let mut best_k = 60.0;
        let mut best_ndcg = 0.0;
        let mut results = Vec::new();

        tracing::info!("🔬 Starting RRF k parameter tuning (verbose mode)...");

        for k in &k_values {
            let mut total_ndcg = 0.0;
            let mut query_results = Vec::new();

            for test_query in &self.test_queries {
                let search_results = hybrid_search.search_with_k(&test_query.query, 20, *k).await
                    .map_err(|e| format!("Search failed: {}", e))?;

                let ndcg = calculate_ndcg(&search_results, &test_query.relevant_chunk_ids, 10);
                let mrr = calculate_mrr(&search_results, &test_query.relevant_chunk_ids);

                total_ndcg += ndcg;
                query_results.push((test_query.query.clone(), ndcg, mrr));
            }

            let avg_ndcg = total_ndcg / self.test_queries.len() as f64;
            results.push((*k, avg_ndcg));

            tracing::info!("  k={:5.1}: NDCG@10={:.4}", k, avg_ndcg);

            // Print per-query breakdown
            for (query, ndcg, mrr) in &query_results {
                tracing::debug!("    \"{}\": NDCG={:.3}, MRR={:.3}", query, ndcg, mrr);
            }

            if avg_ndcg > best_ndcg {
                best_ndcg = avg_ndcg;
                best_k = *k;
            }
        }

        tracing::info!("✓ Optimal k={} with NDCG@10={:.4}", best_k, best_ndcg);

        Ok(TuningResult {
            best_k,
            best_ndcg,
            k_values_tested: results,
        })
    }

    /// Get the number of test queries
    pub fn query_count(&self) -> usize {
        self.test_queries.len()
    }
}

/// Calculate NDCG@k (Normalized Discounted Cumulative Gain)
///
/// NDCG measures ranking quality, with values from 0 to 1.
/// Higher is better. 1.0 means perfect ranking.
fn calculate_ndcg(results: &[SearchResult], relevant: &[String], k: usize) -> f64 {
    // DCG: Discounted Cumulative Gain
    let dcg: f64 = results
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, r)| relevant.contains(&r.chunk.context.symbol_name))
        .map(|(i, _)| 1.0 / ((i + 2) as f64).log2())
        .sum();

    // IDCG: Ideal DCG (all relevant results at top)
    let ideal_dcg: f64 = (0..k.min(relevant.len()))
        .map(|i| 1.0 / ((i + 2) as f64).log2())
        .sum();

    if ideal_dcg == 0.0 {
        0.0
    } else {
        dcg / ideal_dcg
    }
}

/// Calculate MRR (Mean Reciprocal Rank)
///
/// MRR measures how quickly the first relevant result appears.
/// Values from 0 to 1. Higher is better.
fn calculate_mrr(results: &[SearchResult], relevant: &[String]) -> f64 {
    results
        .iter()
        .position(|r| relevant.contains(&r.chunk.context.symbol_name))
        .map(|pos| 1.0 / (pos + 1) as f64)
        .unwrap_or(0.0)
}

/// Calculate MAP (Mean Average Precision)
fn calculate_map(results: &[SearchResult], relevant: &[String]) -> f64 {
    let mut relevant_found = 0;
    let mut sum_precision = 0.0;

    for (i, result) in results.iter().enumerate() {
        if relevant.contains(&result.chunk.context.symbol_name) {
            relevant_found += 1;
            let precision = relevant_found as f64 / (i + 1) as f64;
            sum_precision += precision;
        }
    }

    if relevant.is_empty() {
        0.0
    } else {
        sum_precision / relevant.len() as f64
    }
}

/// Calculate Recall@k
fn calculate_recall_at_k(results: &[SearchResult], relevant: &[String], k: usize) -> f64 {
    let found = results
        .iter()
        .take(k)
        .filter(|r| relevant.contains(&r.chunk.context.symbol_name))
        .count();

    if relevant.is_empty() {
        0.0
    } else {
        found as f64 / relevant.len() as f64
    }
}

/// Calculate Precision@k
fn calculate_precision_at_k(results: &[SearchResult], relevant: &[String], k: usize) -> f64 {
    let found = results
        .iter()
        .take(k)
        .filter(|r| relevant.contains(&r.chunk.context.symbol_name))
        .count();

    found as f64 / k.min(results.len()) as f64
}

/// Comprehensive evaluation metrics
#[derive(Debug, Clone, Serialize)]
pub struct EvaluationMetrics {
    pub ndcg_at_10: f64,
    pub mrr: f64,
    pub map: f64,
    pub recall_at_20: f64,
    pub precision_at_10: f64,
}

/// Evaluate hybrid search quality across all test queries
pub async fn evaluate_hybrid_search(
    hybrid_search: &HybridSearch,
    test_queries: &[TestQuery],
) -> Result<EvaluationMetrics, Box<dyn std::error::Error + Send + Sync>> {
    let mut ndcg_sum = 0.0;
    let mut mrr_sum = 0.0;
    let mut map_sum = 0.0;
    let mut recall_sum = 0.0;
    let mut precision_sum = 0.0;

    for test_query in test_queries {
        let results = hybrid_search.search(&test_query.query, 20).await
            .map_err(|e| format!("Search failed: {}", e))?;

        ndcg_sum += calculate_ndcg(&results, &test_query.relevant_chunk_ids, 10);
        mrr_sum += calculate_mrr(&results, &test_query.relevant_chunk_ids);
        map_sum += calculate_map(&results, &test_query.relevant_chunk_ids);
        recall_sum += calculate_recall_at_k(&results, &test_query.relevant_chunk_ids, 20);
        precision_sum += calculate_precision_at_k(&results, &test_query.relevant_chunk_ids, 10);
    }

    let n = test_queries.len() as f64;

    Ok(EvaluationMetrics {
        ndcg_at_10: ndcg_sum / n,
        mrr: mrr_sum / n,
        map: map_sum / n,
        recall_at_20: recall_sum / n,
        precision_at_10: precision_sum / n,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{ChunkContext, ChunkId, CodeChunk};
    use std::path::PathBuf;

    fn create_mock_result(symbol_name: &str, score: f32) -> SearchResult {
        SearchResult {
            chunk_id: ChunkId::new(),
            score,
            bm25_score: None,
            vector_score: Some(score),
            bm25_rank: None,
            vector_rank: Some(1),
            chunk: CodeChunk {
                id: ChunkId::new(),
                content: "test content".to_string(),
                context: ChunkContext {
                    file_path: PathBuf::from("test.rs"),
                    module_path: vec!["test".to_string()],
                    symbol_name: symbol_name.to_string(),
                    symbol_kind: "function".to_string(),
                    docstring: None,
                    imports: vec![],
                    outgoing_calls: vec![],
                    line_start: 1,
                    line_end: 10,
                },
                overlap_prev: None,
                overlap_next: None,
            },
        }
    }

    #[test]
    fn test_ndcg_perfect_ranking() {
        let results = vec![
            create_mock_result("relevant1", 1.0),
            create_mock_result("relevant2", 0.9),
            create_mock_result("irrelevant", 0.8),
        ];

        let relevant = vec!["relevant1".to_string(), "relevant2".to_string()];

        let ndcg = calculate_ndcg(&results, &relevant, 10);

        // Should be 1.0 for perfect ranking
        assert!((ndcg - 1.0).abs() < 0.01, "NDCG should be ~1.0, got {}", ndcg);
    }

    #[test]
    fn test_mrr_calculation() {
        let results = vec![
            create_mock_result("irrelevant1", 1.0),
            create_mock_result("irrelevant2", 0.9),
            create_mock_result("relevant", 0.8),
        ];

        let relevant = vec!["relevant".to_string()];

        let mrr = calculate_mrr(&results, &relevant);

        // First relevant at position 3 (index 2), so MRR = 1/3
        assert!((mrr - 0.333).abs() < 0.01, "MRR should be ~0.333, got {}", mrr);
    }

    #[test]
    fn test_recall_at_k() {
        let results = vec![
            create_mock_result("relevant1", 1.0),
            create_mock_result("irrelevant", 0.9),
            create_mock_result("relevant2", 0.8),
        ];

        let relevant = vec!["relevant1".to_string(), "relevant2".to_string(), "relevant3".to_string()];

        let recall = calculate_recall_at_k(&results, &relevant, 10);

        // Found 2 out of 3 relevant, so recall = 2/3
        assert!((recall - 0.666).abs() < 0.01, "Recall should be ~0.666, got {}", recall);
    }

    #[test]
    fn test_precision_at_k() {
        let results = vec![
            create_mock_result("relevant1", 1.0),
            create_mock_result("irrelevant", 0.9),
            create_mock_result("relevant2", 0.8),
        ];

        let relevant = vec!["relevant1".to_string(), "relevant2".to_string()];

        let precision = calculate_precision_at_k(&results, &relevant, 3);

        // 2 relevant out of 3 results, so precision = 2/3
        assert!((precision - 0.666).abs() < 0.01, "Precision should be ~0.666, got {}", precision);
    }

    #[test]
    fn test_default_rust_queries() {
        let tuner = RRFTuner::default_rust_queries();
        assert!(tuner.query_count() > 0, "Should have default queries");
        assert!(tuner.query_count() >= 5, "Should have at least 5 default queries");
    }
}

//! Hybrid search combining BM25 (lexical) and vector (semantic) search
//!
//! Implements Reciprocal Rank Fusion (RRF) to merge results from multiple search engines

pub mod bm25;
pub mod resilient;
pub mod rrf_tuner;

pub use bm25::Bm25Search;
pub use resilient::ResilientHybridSearch;
pub use rrf_tuner::{evaluate_hybrid_search, EvaluationMetrics, RRFTuner, TestQuery, TuningResult};

use crate::chunker::{ChunkId, CodeChunk};
use crate::embeddings::EmbeddingGenerator;
use crate::vector_store::{VectorStore, SearchResult as VectorSearchResult};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration for hybrid search
#[derive(Debug, Clone)]
pub struct HybridSearchConfig {
    /// Weight for BM25 search (0.0 to 1.0)
    pub bm25_weight: f32,
    /// Weight for vector search (0.0 to 1.0)
    pub vector_weight: f32,
    /// RRF k parameter (typically 60)
    pub rrf_k: f32,
    /// Number of candidates to fetch from each engine
    pub candidate_count: usize,
}

impl Default for HybridSearchConfig {
    fn default() -> Self {
        Self {
            bm25_weight: 0.5,
            vector_weight: 0.5,
            rrf_k: 60.0,
            candidate_count: 100,
        }
    }
}

/// Unified search result combining scores from multiple sources
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Chunk ID
    pub chunk_id: ChunkId,
    /// Combined score (after RRF)
    pub score: f32,
    /// BM25 score (if available)
    pub bm25_score: Option<f32>,
    /// Vector similarity score (if available)
    pub vector_score: Option<f32>,
    /// Rank in BM25 results (if found)
    pub bm25_rank: Option<usize>,
    /// Rank in vector results (if found)
    pub vector_rank: Option<usize>,
    /// The actual chunk
    pub chunk: CodeChunk,
}

/// Vector search wrapper that generates embeddings and queries Qdrant
pub struct VectorSearch {
    embedding_generator: EmbeddingGenerator,
    vector_store: VectorStore,
}

impl VectorSearch {
    /// Create a new vector search instance
    pub fn new(
        embedding_generator: EmbeddingGenerator,
        vector_store: VectorStore,
    ) -> Self {
        Self {
            embedding_generator,
            vector_store,
        }
    }

    /// Search for similar chunks using a text query
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<VectorSearchResult>, Box<dyn std::error::Error + Send>> {
        // Generate embedding for the query
        let query_embedding = self.embedding_generator.embed(query)?;

        // Search in vector store
        self.vector_store.search(query_embedding, limit).await
    }
}

/// Hybrid search combining BM25 and vector search with RRF
pub struct HybridSearch {
    vector_search: VectorSearch,
    bm25_search: Option<Bm25Search>,
    config: HybridSearchConfig,
}

impl HybridSearch {
    /// Create a new hybrid search instance
    pub fn new(
        embedding_generator: EmbeddingGenerator,
        vector_store: VectorStore,
        bm25_search: Option<Bm25Search>,
        config: HybridSearchConfig,
    ) -> Self {
        Self {
            vector_search: VectorSearch::new(embedding_generator, vector_store),
            bm25_search,
            config,
        }
    }

    /// Create with default configuration
    pub fn with_defaults(
        embedding_generator: EmbeddingGenerator,
        vector_store: VectorStore,
        bm25_search: Option<Bm25Search>,
    ) -> Self {
        Self::new(embedding_generator, vector_store, bm25_search, HybridSearchConfig::default())
    }

    /// Perform hybrid search combining BM25 and vector search
    ///
    /// If BM25 search is available, runs both searches in parallel and merges
    /// results using Reciprocal Rank Fusion. Otherwise falls back to vector-only search.
    pub async fn search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error + Send>> {
        self.search_with_k(query, limit, self.config.rrf_k).await
    }

    /// Perform hybrid search with a custom RRF k parameter
    ///
    /// This is useful for tuning the RRF k parameter to optimize search quality.
    pub async fn search_with_k(
        &self,
        query: &str,
        limit: usize,
        rrf_k: f32,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error + Send>> {
        // Run vector search and BM25 search in parallel (if BM25 is available)
        let (vector_results, bm25_results) = if let Some(bm25) = &self.bm25_search {
            // BM25 search is sync, run in blocking task
            let bm25_clone = bm25.clone();
            let query_clone = query.to_string();
            let candidate_count = self.config.candidate_count;

            let (vector_future, bm25_future) = tokio::join!(
                self.vector_search.search(query, candidate_count),
                tokio::task::spawn_blocking(move || {
                    bm25_clone.search(&query_clone, candidate_count)
                })
            );

            let vector_results = vector_future?;
            let bm25_results = bm25_future.map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send>)?
                .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, format!("{:?}", e))) as Box<dyn std::error::Error + Send>)?;

            (vector_results, bm25_results)
        } else {
            // No BM25 search available, vector only
            let vector_results = self
                .vector_search
                .search(query, self.config.candidate_count)
                .await?;

            (vector_results, vec![])
        };

        // Apply Reciprocal Rank Fusion
        let merged = self.reciprocal_rank_fusion_with_k(&vector_results, &bm25_results, rrf_k);

        // Return top N results
        Ok(merged.into_iter().take(limit).collect())
    }

    /// Reciprocal Rank Fusion (RRF) algorithm
    ///
    /// Combines rankings from multiple search systems using the formula:
    /// score(item) = sum(1 / (k + rank_i)) for all systems i where item appears
    ///
    /// Where k is a constant (typically 60) and rank_i is 1-indexed rank
    fn reciprocal_rank_fusion(
        &self,
        vector_results: &[VectorSearchResult],
        bm25_results: &[(ChunkId, f32, CodeChunk)],
    ) -> Vec<SearchResult> {
        self.reciprocal_rank_fusion_with_k(vector_results, bm25_results, self.config.rrf_k)
    }

    /// Reciprocal Rank Fusion with custom k parameter
    fn reciprocal_rank_fusion_with_k(
        &self,
        vector_results: &[VectorSearchResult],
        bm25_results: &[(ChunkId, f32, CodeChunk)],
        k: f32,
    ) -> Vec<SearchResult> {
        let mut scores: HashMap<ChunkId, RrfScore> = HashMap::new();

        // Process vector search results
        for (rank, result) in vector_results.iter().enumerate() {
            let rrf_score = 1.0 / (k + (rank + 1) as f32);
            let entry = scores.entry(result.chunk_id).or_insert_with(|| RrfScore {
                chunk_id: result.chunk_id,
                rrf_score: 0.0,
                vector_score: None,
                vector_rank: None,
                bm25_score: None,
                bm25_rank: None,
                chunk: result.chunk.clone(),
            });

            entry.rrf_score += rrf_score * self.config.vector_weight;
            entry.vector_score = Some(result.score);
            entry.vector_rank = Some(rank + 1);
        }

        // Process BM25 results
        for (rank, (chunk_id, score, chunk)) in bm25_results.iter().enumerate() {
            let rrf_score = 1.0 / (k + (rank + 1) as f32);
            let entry = scores.entry(*chunk_id).or_insert_with(|| RrfScore {
                chunk_id: *chunk_id,
                rrf_score: 0.0,
                vector_score: None,
                vector_rank: None,
                bm25_score: None,
                bm25_rank: None,
                chunk: chunk.clone(),
            });

            entry.rrf_score += rrf_score * self.config.bm25_weight;
            entry.bm25_score = Some(*score);
            entry.bm25_rank = Some(rank + 1);
        }

        // Convert to SearchResult and sort by RRF score
        let mut results: Vec<SearchResult> = scores
            .into_values()
            .map(|rrf_score| SearchResult {
                chunk_id: rrf_score.chunk_id,
                score: rrf_score.rrf_score,
                bm25_score: rrf_score.bm25_score,
                vector_score: rrf_score.vector_score,
                bm25_rank: rrf_score.bm25_rank,
                vector_rank: rrf_score.vector_rank,
                chunk: rrf_score.chunk,
            })
            .collect();

        // Sort by RRF score (descending)
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results
    }

    /// Search using only vector similarity
    pub async fn vector_only_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, Box<dyn std::error::Error + Send>> {
        let results = self.vector_search.search(query, limit).await?;

        Ok(results
            .into_iter()
            .enumerate()
            .map(|(rank, result)| SearchResult {
                chunk_id: result.chunk_id,
                score: result.score,
                bm25_score: None,
                vector_score: Some(result.score),
                bm25_rank: None,
                vector_rank: Some(rank + 1),
                chunk: result.chunk,
            })
            .collect())
    }

    /// Static version of reciprocal rank fusion (for use by ResilientHybridSearch)
    pub fn reciprocal_rank_fusion_static(
        bm25_results: Vec<SearchResult>,
        vector_results: Vec<SearchResult>,
        k: f32,
    ) -> Vec<SearchResult> {
        let mut scores: HashMap<ChunkId, RrfScore> = HashMap::new();

        // Process BM25 results
        for (rank, result) in bm25_results.iter().enumerate() {
            let rrf_score = 1.0 / (k + (rank + 1) as f32);
            let entry = scores.entry(result.chunk_id).or_insert_with(|| RrfScore {
                chunk_id: result.chunk_id,
                rrf_score: 0.0,
                vector_score: None,
                vector_rank: None,
                bm25_score: None,
                bm25_rank: None,
                chunk: result.chunk.clone(),
            });

            entry.rrf_score += rrf_score * 0.5; // Default weight
            entry.bm25_score = result.bm25_score.or(Some(result.score));
            entry.bm25_rank = Some(rank + 1);
        }

        // Process vector results
        for (rank, result) in vector_results.iter().enumerate() {
            let rrf_score = 1.0 / (k + (rank + 1) as f32);
            let entry = scores.entry(result.chunk_id).or_insert_with(|| RrfScore {
                chunk_id: result.chunk_id,
                rrf_score: 0.0,
                vector_score: None,
                vector_rank: None,
                bm25_score: None,
                bm25_rank: None,
                chunk: result.chunk.clone(),
            });

            entry.rrf_score += rrf_score * 0.5; // Default weight
            entry.vector_score = result.vector_score.or(Some(result.score));
            entry.vector_rank = Some(rank + 1);
        }

        // Convert to SearchResult and sort
        let mut results: Vec<SearchResult> = scores
            .into_values()
            .map(|rrf_score| SearchResult {
                chunk_id: rrf_score.chunk_id,
                score: rrf_score.rrf_score,
                bm25_score: rrf_score.bm25_score,
                vector_score: rrf_score.vector_score,
                bm25_rank: rrf_score.bm25_rank,
                vector_rank: rrf_score.vector_rank,
                chunk: rrf_score.chunk,
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        results
    }
}

/// Internal structure for RRF score calculation
struct RrfScore {
    chunk_id: ChunkId,
    rrf_score: f32,
    vector_score: Option<f32>,
    vector_rank: Option<usize>,
    bm25_score: Option<f32>,
    bm25_rank: Option<usize>,
    chunk: CodeChunk,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::{ChunkContext, CodeChunk};
    use std::path::PathBuf;

    fn create_test_chunk(id: ChunkId, name: &str) -> CodeChunk {
        CodeChunk {
            id,
            content: format!("fn {}() {{}}", name),
            context: ChunkContext {
                file_path: PathBuf::from("test.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: name.to_string(),
                symbol_kind: "function".to_string(),
                docstring: None,
                imports: vec![],
                outgoing_calls: vec![],
                line_start: 1,
                line_end: 1,
            },
            overlap_prev: None,
            overlap_next: None,
        }
    }

    #[test]
    fn test_hybrid_search_config() {
        let config = HybridSearchConfig::default();
        assert_eq!(config.bm25_weight, 0.5);
        assert_eq!(config.vector_weight, 0.5);
        assert_eq!(config.rrf_k, 60.0);
        assert_eq!(config.candidate_count, 100);
    }

    #[tokio::test]
    #[ignore] // Requires Qdrant server and embedding model
    async fn test_rrf_calculation() {
        let chunk_id1 = ChunkId::new();
        let chunk_id2 = ChunkId::new();
        let chunk_id3 = ChunkId::new();

        let chunk1 = create_test_chunk(chunk_id1, "func1");
        let chunk2 = create_test_chunk(chunk_id2, "func2");
        let chunk3 = create_test_chunk(chunk_id3, "func3");

        // Simulate vector results: chunk1 (rank 1), chunk2 (rank 2)
        let vector_results = vec![
            VectorSearchResult {
                chunk_id: chunk_id1,
                score: 0.95,
                chunk: chunk1.clone(),
            },
            VectorSearchResult {
                chunk_id: chunk_id2,
                score: 0.85,
                chunk: chunk2.clone(),
            },
        ];

        // Simulate BM25 results: chunk2 (rank 1), chunk3 (rank 2)
        let bm25_results = vec![
            (chunk_id2, 10.5, chunk2.clone()),
            (chunk_id3, 8.3, chunk3.clone()),
        ];

        let config = HybridSearchConfig::default();
        let embedding_generator = EmbeddingGenerator::new().unwrap();
        let vector_store = VectorStore::new(crate::vector_store::VectorStoreConfig::default())
            .await
            .unwrap();

        let hybrid_search = HybridSearch::new(embedding_generator, vector_store, None, config);

        let results = hybrid_search.reciprocal_rank_fusion(&vector_results, &bm25_results);

        // chunk2 should be first (appears in both)
        // chunk1 should be second (only in vector, but high rank)
        // chunk3 should be third (only in BM25, lower rank)
        assert_eq!(results.len(), 3);

        // Verify chunk2 has scores from both sources
        let chunk2_result = results.iter().find(|r| r.chunk_id == chunk_id2).unwrap();
        assert!(chunk2_result.vector_score.is_some());
        assert!(chunk2_result.bm25_score.is_some());
        assert!(chunk2_result.vector_rank.is_some());
        assert!(chunk2_result.bm25_rank.is_some());

        // Verify chunk1 only has vector score
        let chunk1_result = results.iter().find(|r| r.chunk_id == chunk_id1).unwrap();
        assert!(chunk1_result.vector_score.is_some());
        assert!(chunk1_result.bm25_score.is_none());

        // Verify chunk3 only has BM25 score
        let chunk3_result = results.iter().find(|r| r.chunk_id == chunk_id3).unwrap();
        assert!(chunk3_result.vector_score.is_none());
        assert!(chunk3_result.bm25_score.is_some());
    }

    #[tokio::test]
    #[ignore] // Requires Qdrant server and embedding model
    async fn test_vector_only_search() {
        let embedding_generator = EmbeddingGenerator::new().unwrap();
        let vector_store = VectorStore::new(crate::vector_store::VectorStoreConfig::default())
            .await
            .unwrap();

        let hybrid_search = HybridSearch::with_defaults(embedding_generator, vector_store, None);

        // This test would require indexed data
        let results = hybrid_search.vector_only_search("test query", 10).await;
        assert!(results.is_ok());
    }

    #[test]
    fn test_hybrid_search_with_bm25() {
        use tempfile::TempDir;
        use tantivy::doc;

        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("bm25_test");

        // Create BM25 search
        let bm25_search = Bm25Search::new(&index_path).unwrap();

        // Create test chunks and index them
        let chunk1_id = ChunkId::new();
        let chunk1 = create_test_chunk(chunk1_id, "async_function");
        let chunk1_json = serde_json::to_string(&chunk1).unwrap();

        let mut index_writer = bm25_search.index().writer(50_000_000).unwrap();
        let schema = bm25_search.schema();

        index_writer.add_document(doc!(
            schema.chunk_id => chunk1_id.to_string(),
            schema.content => chunk1.content.clone(),
            schema.symbol_name => chunk1.context.symbol_name.clone(),
            schema.symbol_kind => chunk1.context.symbol_kind.clone(),
            schema.file_path => chunk1.context.file_path.display().to_string(),
            schema.module_path => chunk1.context.module_path.join("::"),
            schema.docstring => chunk1.context.docstring.clone().unwrap_or_default(),
            schema.chunk_json => chunk1_json,
        )).unwrap();

        index_writer.commit().unwrap();

        // Verify BM25 search works independently
        let mut bm25_search_mut = bm25_search.clone();
        bm25_search_mut.reload().unwrap();

        let results = bm25_search_mut.search("async", 10).unwrap();
        assert!(!results.is_empty());
        assert_eq!(results[0].0, chunk1_id);
    }

    #[test]
    fn test_search_result_serialization() {
        let chunk_id = ChunkId::new();
        let chunk = create_test_chunk(chunk_id, "test_func");

        let result = SearchResult {
            chunk_id,
            score: 0.95,
            bm25_score: Some(12.5),
            vector_score: Some(0.92),
            bm25_rank: Some(2),
            vector_rank: Some(3),
            chunk,
        };

        // Test serialization
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("test_func"));
        assert!(json.contains("0.95"));

        // Test deserialization
        let deserialized: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.chunk_id, chunk_id);
        assert_eq!(deserialized.score, 0.95);
        assert_eq!(deserialized.bm25_score, Some(12.5));
        assert_eq!(deserialized.vector_score, Some(0.92));
    }
}

//! Hybrid search combining BM25 (lexical) and vector (semantic) search
//!
//! Implements Reciprocal Rank Fusion (RRF) to merge results from multiple search engines

mod bm25;
mod error;
#[cfg(all(test, feature = "vector-store", target_os = "linux"))]
mod fts_eval;
mod relevance;
#[cfg(feature = "hybrid-search")]
mod resilient;
#[cfg(feature = "hybrid-search")]
mod rrf_tuner;

pub use bm25::Bm25Search;
pub use error::SearchError;
pub use relevance::{CaseScore, RelevanceCase, RelevanceSummary, RepoRelPath, score_case};
#[cfg(feature = "hybrid-search")]
pub use resilient::ResilientHybridSearch;

use crate::chunker::{ChunkId, CodeChunk};
#[cfg(feature = "hybrid-search")]
use crate::embeddings::EmbeddingGenerator;
#[cfg(feature = "hybrid-search")]
use crate::vector_store::{VectorStore, VectorSearchResult};
use serde::{Deserialize, Serialize};
#[cfg(feature = "hybrid-search")]
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

/// Vector search wrapper that generates embeddings and queries the vector store
#[cfg(feature = "hybrid-search")]
pub(crate) struct VectorSearch {
    embedding_generator: EmbeddingGenerator,
    vector_store: VectorStore,
}

#[cfg(feature = "hybrid-search")]
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
    ) -> Result<Vec<VectorSearchResult>, SearchError> {
        // Generate embedding for the query (instruction-prefixed).
        let mut query_embeddings = self
            .embedding_generator
            .embed_queries(vec![query.to_string()])
            .await?;
        let query_embedding = query_embeddings
            .pop()
            .ok_or(SearchError::Embedding(
                crate::embeddings::EmbeddingError::NoEmbeddingGenerated,
            ))?;

        // Search in vector store
        self.vector_store.search(query_embedding, limit).await
            .map_err(SearchError::VectorStore)
    }
}

/// Hybrid search combining BM25 and vector search with RRF
#[cfg(feature = "hybrid-search")]
pub struct HybridSearch {
    vector_search: VectorSearch,
    bm25_search: Option<Bm25Search>,
    config: HybridSearchConfig,
}

#[cfg(feature = "hybrid-search")]
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
    ) -> Result<Vec<SearchResult>, SearchError> {
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
    ) -> Result<Vec<SearchResult>, SearchError> {
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
            let bm25_results = bm25_future
                .map_err(|e| SearchError::Bm25(Box::new(e)))?
                .map_err(|e| SearchError::Bm25(e))?;

            (vector_results, bm25_results)
        } else {
            // No BM25 search available, vector only
            let vector_results = self
                .vector_search
                .search(query, self.config.candidate_count)
                .await?;

            (vector_results, vec![])
        };

        // Apply Reciprocal Rank Fusion, on ids: which chunks make the answer
        // is decided before any chunk is read.
        let vector_hits: Vec<(ChunkId, f32)> =
            vector_results.iter().map(|r| (r.chunk_id, r.score)).collect();
        let mut fused = reciprocal_rank_fusion_core(
            &vector_hits,
            &bm25_results,
            rrf_k,
            self.config.vector_weight,
            self.config.bm25_weight,
        );
        fused.truncate(limit);

        // Vector hits carry their chunk; a hit only BM25 found is read from
        // the vector store, which is where the BM25 index's chunks live.
        let mut chunks: HashMap<ChunkId, CodeChunk> = vector_results
            .into_iter()
            .map(|r| (r.chunk_id, r.chunk))
            .collect();
        let missing: Vec<ChunkId> = fused
            .iter()
            .map(|hit| hit.chunk_id)
            .filter(|id| !chunks.contains_key(id))
            .collect();
        if !missing.is_empty() {
            chunks.extend(
                self.vector_search
                    .vector_store
                    .chunks_by_ids(&missing)
                    .await
                    .map_err(SearchError::VectorStore)?,
            );
        }

        Ok(attach_chunks(fused, chunks))
    }

    /// Search using only vector similarity
    pub async fn vector_only_search(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
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
        let bm25: Vec<_> = bm25_results
            .iter()
            .map(|r| (r.chunk_id, r.bm25_score.unwrap_or(r.score)))
            .collect();
        let vector: Vec<_> = vector_results
            .iter()
            .map(|r| (r.chunk_id, r.vector_score.unwrap_or(r.score)))
            .collect();
        let fused = reciprocal_rank_fusion_core(&vector, &bm25, k, 0.5, 0.5);
        let chunks = bm25_results
            .into_iter()
            .chain(vector_results)
            .map(|r| (r.chunk_id, r.chunk))
            .collect();
        attach_chunks(fused, chunks)
    }
}

/// Core Reciprocal Rank Fusion algorithm
///
/// Fuses two ranked id lists using the RRF formula `1/(k + rank)`. Each
/// entry is `(ChunkId, raw_score)`; the two lists are processed
/// independently and their weighted contributions are summed. Returns the
/// fused hits, best first.
#[cfg(feature = "hybrid-search")]
fn reciprocal_rank_fusion_core(
    vector_results: &[(ChunkId, f32)],
    bm25_results: &[(ChunkId, f32)],
    k: f32,
    vector_weight: f32,
    bm25_weight: f32,
) -> Vec<RrfScore> {
    let mut scores: HashMap<ChunkId, RrfScore> = HashMap::new();
    let new_entry = |chunk_id: ChunkId| RrfScore {
        chunk_id,
        rrf_score: 0.0,
        vector_score: None,
        vector_rank: None,
        bm25_score: None,
        bm25_rank: None,
    };

    for (rank, (chunk_id, score)) in vector_results.iter().enumerate() {
        let rrf_score = 1.0 / (k + (rank + 1) as f32);
        let entry = scores.entry(*chunk_id).or_insert_with(|| new_entry(*chunk_id));
        entry.rrf_score += rrf_score * vector_weight;
        entry.vector_score = Some(*score);
        entry.vector_rank = Some(rank + 1);
    }

    for (rank, (chunk_id, score)) in bm25_results.iter().enumerate() {
        let rrf_score = 1.0 / (k + (rank + 1) as f32);
        let entry = scores.entry(*chunk_id).or_insert_with(|| new_entry(*chunk_id));
        entry.rrf_score += rrf_score * bm25_weight;
        entry.bm25_score = Some(*score);
        entry.bm25_rank = Some(rank + 1);
    }

    let mut results: Vec<RrfScore> = scores.into_values().collect();
    results.sort_by(|a, b| {
        b.rrf_score
            .partial_cmp(&a.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results
}

/// Turn fused hits into results, in order, each with its chunk.
///
/// A hit whose chunk is not in `chunks` is left out and counted in a warning.
/// It is a BM25 hit for a chunk the vector store no longer (or not yet) has:
/// the two are written one after the other, and between a file's BM25 commit
/// and its vector write there is no chunk to show. Leaving it out is the only
/// answer there is; the warning is what keeps a store that drifted for good
/// from passing silently.
#[cfg(feature = "hybrid-search")]
fn attach_chunks(
    fused: Vec<RrfScore>,
    mut chunks: HashMap<ChunkId, CodeChunk>,
) -> Vec<SearchResult> {
    let mut results = Vec::with_capacity(fused.len());
    let mut without_chunk = 0usize;
    for hit in fused {
        let Some(chunk) = chunks.remove(&hit.chunk_id) else {
            without_chunk += 1;
            continue;
        };
        results.push(SearchResult {
            chunk_id: hit.chunk_id,
            score: hit.rrf_score,
            bm25_score: hit.bm25_score,
            vector_score: hit.vector_score,
            bm25_rank: hit.bm25_rank,
            vector_rank: hit.vector_rank,
            chunk,
        });
    }
    if without_chunk > 0 {
        tracing::warn!(
            without_chunk,
            "search hits dropped: the vector store holds no chunk for them"
        );
    }
    results
}

/// Internal structure for RRF score calculation
#[cfg(feature = "hybrid-search")]
struct RrfScore {
    chunk_id: ChunkId,
    rrf_score: f32,
    vector_score: Option<f32>,
    vector_rank: Option<usize>,
    bm25_score: Option<f32>,
    bm25_rank: Option<usize>,
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
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
                line_start: 1,
                line_end: 1,
            },
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

    #[test]
    fn test_hybrid_search_with_bm25() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().join("bm25_test");

        // Create BM25 search
        let bm25_search = Bm25Search::new(&index_path).unwrap();

        // Create test chunks and index them
        let chunk1_id = ChunkId::new();
        let chunk1 = create_test_chunk(chunk1_id, "async_function");

        let mut index_writer = bm25_search.index().writer(50_000_000).unwrap();
        let schema = bm25_search.schema();

        index_writer.add_document(schema.document(&chunk1)).unwrap();

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

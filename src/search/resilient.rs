//! Resilient hybrid search with graceful degradation
//!
//! Provides automatic fallback when search components fail:
//! - If vector search fails → BM25-only mode
//! - If BM25 fails → vector-only mode
//! - If both fail → clear error message
//!
//! This ensures the system remains functional even when components are degraded.

use crate::embeddings::EmbeddingGenerator;
use crate::search::{Bm25Search, HybridSearch, SearchResult};
use crate::vector_store::VectorStore;
use anyhow::{anyhow, Context, Result};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing;

/// Resilient hybrid search with automatic fallback
pub struct ResilientHybridSearch {
    bm25: Option<Arc<Bm25Search>>,
    vector_store: Option<Arc<VectorStore>>,
    embedding_generator: Option<Arc<EmbeddingGenerator>>,
    rrf_k: f32,
    fallback_mode: Arc<AtomicBool>,
}

impl ResilientHybridSearch {
    /// Create a new resilient hybrid search
    pub fn new(
        bm25: Option<Bm25Search>,
        vector_store: Option<VectorStore>,
        embedding_generator: Option<EmbeddingGenerator>,
        rrf_k: f32,
    ) -> Self {
        Self {
            bm25: bm25.map(Arc::new),
            vector_store: vector_store.map(Arc::new),
            embedding_generator: embedding_generator.map(Arc::new),
            rrf_k,
            fallback_mode: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Create with defaults (k=60)
    pub fn with_defaults(
        bm25: Option<Bm25Search>,
        vector_store: Option<VectorStore>,
        embedding_generator: Option<EmbeddingGenerator>,
    ) -> Self {
        Self::new(bm25, vector_store, embedding_generator, 60.0)
    }

    /// Perform search with automatic fallback
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Try full hybrid search first
        match self.try_hybrid_search(query, limit).await {
            Ok(results) => {
                // Reset fallback mode if we succeeded
                self.fallback_mode.store(false, Ordering::Relaxed);
                Ok(results)
            }
            Err(e) => {
                tracing::warn!("Hybrid search failed: {}, attempting fallback", e);
                self.fallback_mode.store(true, Ordering::Relaxed);
                self.fallback_search(query, limit).await
            }
        }
    }

    /// Check if currently in fallback mode
    pub fn is_fallback_mode(&self) -> bool {
        self.fallback_mode.load(Ordering::Relaxed)
    }

    /// Try full hybrid search (both BM25 and vector)
    async fn try_hybrid_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let (bm25_result, vector_result) = tokio::join!(
            self.bm25_search(query, limit),
            self.vector_search(query, limit)
        );

        match (bm25_result, vector_result) {
            (Ok(bm25_results), Ok(vector_results)) => {
                // Both succeeded - full hybrid search
                Ok(self.merge_results(bm25_results, vector_results))
            }
            (Ok(bm25_results), Err(vector_err)) => {
                // Vector failed, BM25 works
                tracing::warn!("Vector search failed: {}, using BM25 only", vector_err);
                Ok(bm25_results)
            }
            (Err(bm25_err), Ok(vector_results)) => {
                // BM25 failed, vector works
                tracing::warn!("BM25 search failed: {}, using vector only", bm25_err);
                Ok(vector_results)
            }
            (Err(bm25_err), Err(vector_err)) => {
                // Both failed
                Err(anyhow!(
                    "Both search engines failed - BM25: {}, Vector: {}",
                    bm25_err,
                    vector_err
                ))
            }
        }
    }

    /// Fallback search when hybrid fails
    async fn fallback_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        // Try BM25 first (more reliable, no external dependencies)
        if let Ok(results) = self.bm25_search(query, limit).await {
            tracing::info!("Fallback: using BM25-only search");
            return Ok(results);
        }

        // Try vector as last resort
        if let Ok(results) = self.vector_search(query, limit).await {
            tracing::info!("Fallback: using vector-only search");
            return Ok(results);
        }

        Err(anyhow!(
            "All search engines unavailable - both BM25 and vector failed"
        ))
    }

    /// Perform BM25 search
    async fn bm25_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let bm25 = self
            .bm25
            .as_ref()
            .ok_or_else(|| anyhow!("BM25 search not configured"))?;

        // BM25 search is synchronous, wrap in spawn_blocking
        let bm25_clone = Arc::clone(bm25);
        let query_clone = query.to_string();

        let results = tokio::task::spawn_blocking(move || {
            bm25_clone.search(&query_clone, limit)
        })
        .await
        .context("BM25 search task failed")?
        .map_err(|e| anyhow!("BM25 search failed: {:?}", e))?;

        Ok(results
            .into_iter()
            .map(|(chunk_id, score, chunk)| SearchResult {
                chunk_id,
                score,
                bm25_score: Some(score),
                vector_score: None,
                bm25_rank: None,
                vector_rank: None,
                chunk,
            })
            .collect())
    }

    /// Perform vector search
    async fn vector_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let vector_store = self
            .vector_store
            .as_ref()
            .ok_or_else(|| anyhow!("Vector store not configured"))?;

        let embedding_generator = self
            .embedding_generator
            .as_ref()
            .ok_or_else(|| anyhow!("Embedding generator not configured"))?;

        // Generate query embedding
        let query_embedding = embedding_generator
            .embed(query)
            .map_err(|e| anyhow!("Failed to generate query embedding: {:?}", e))?;

        // Search vector store
        let results = vector_store
            .search(query_embedding, limit)
            .await
            .map_err(|e| anyhow!("Vector search failed: {:?}", e))?;

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

    /// Merge BM25 and vector results using reciprocal rank fusion
    fn merge_results(
        &self,
        bm25_results: Vec<SearchResult>,
        vector_results: Vec<SearchResult>,
    ) -> Vec<SearchResult> {
        // Use HybridSearch's reciprocal_rank_fusion
        HybridSearch::reciprocal_rank_fusion_static(bm25_results, vector_results, self.rrf_k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resilient_search_creation() {
        let resilient = ResilientHybridSearch::with_defaults(None, None, None);
        assert!(!resilient.is_fallback_mode());
    }

    #[test]
    fn test_fallback_mode_tracking() {
        let resilient = ResilientHybridSearch::with_defaults(None, None, None);

        assert!(!resilient.is_fallback_mode());

        // Simulate fallback
        resilient.fallback_mode.store(true, Ordering::Relaxed);
        assert!(resilient.is_fallback_mode());

        // Reset
        resilient.fallback_mode.store(false, Ordering::Relaxed);
        assert!(!resilient.is_fallback_mode());
    }

    #[tokio::test]
    async fn test_search_with_no_components() {
        let resilient = ResilientHybridSearch::with_defaults(None, None, None);

        let result = resilient.search("test", 10).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("All search engines unavailable"));
    }
}

//! Search error types

use thiserror::Error;

use rust_code_mcp_embeddings::EmbeddingError;
use rust_code_mcp_vector_store::VectorStoreError;

/// Errors that can occur during search operations
#[derive(Error, Debug)]
pub enum SearchError {
    /// Embedding generation failed
    #[error("Embedding error: {0}")]
    Embedding(#[from] EmbeddingError),

    /// Vector store operation failed
    #[error("Vector store error: {0}")]
    VectorStore(#[from] VectorStoreError),

    /// BM25 search failed
    #[error("BM25 search error: {0}")]
    Bm25(Box<dyn std::error::Error + Send>),

    /// No results found
    #[error("No results found")]
    NoResults,
}

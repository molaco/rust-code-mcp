//! Indexing error types

use thiserror::Error;

use rmc_engine::embeddings::EmbeddingError;
use rmc_engine::vector_store::VectorStoreError;

/// Errors that can occur during indexing operations
#[derive(Error, Debug)]
pub(crate) enum IndexingError {
    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Embedding generation failed
    #[error("Embedding error: {0}")]
    Embedding(#[from] EmbeddingError),

    /// Vector store operation failed
    #[error("Vector store error: {0}")]
    VectorStore(#[from] VectorStoreError),

    /// Parser or chunker error
    #[error("Parser error: {0}")]
    Parser(String),

    /// Metadata cache error
    #[error("Cache error: {0}")]
    Cache(String),
}

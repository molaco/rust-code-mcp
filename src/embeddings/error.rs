//! Embedding error types
//!
//! Unified error type for all embedding operations.

use thiserror::Error;

/// Errors that can occur during embedding operations
#[derive(Error, Debug)]
pub enum EmbeddingError {
    /// Model initialization failed
    #[error("Model initialization failed: {0}")]
    ModelInit(String),

    /// Embedding generation failed
    #[error("Embedding generation failed: {0}")]
    EmbedFailed(String),

    /// No embedding was generated (empty result from model)
    #[error("No embedding generated")]
    NoEmbeddingGenerated,

    /// Async task join failed
    #[error("Async task failed: {0}")]
    TaskJoin(String),
}

impl EmbeddingError {
    /// Create a model initialization error
    pub fn model_init(msg: impl Into<String>) -> Self {
        Self::ModelInit(msg.into())
    }

    /// Create an embed failed error
    pub fn embed_failed(msg: impl Into<String>) -> Self {
        Self::EmbedFailed(msg.into())
    }

    /// Create a task join error
    pub fn task_join(msg: impl Into<String>) -> Self {
        Self::TaskJoin(msg.into())
    }
}

// Convert to Box<dyn Error + Send> for compatibility with existing code
impl From<EmbeddingError> for Box<dyn std::error::Error + Send> {
    fn from(err: EmbeddingError) -> Self {
        Box::new(err)
    }
}

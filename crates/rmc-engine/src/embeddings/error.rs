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

    /// The model answered, but the vector contains NaN or infinity.
    ///
    /// A separate variant rather than [`EmbeddingError::EmbedFailed`] because
    /// it is a different kind of failure: nothing errored, the model produced
    /// a result, and without a check that result would have been written to
    /// the index — where it costs not a crash but silent rot, since a
    /// non-finite vector loses every cosine comparison and the chunk simply
    /// stops being findable.
    #[error(
        "Model returned a non-finite embedding: item {index} of {batch_len}, component {component} = {value}"
    )]
    NonFiniteEmbedding {
        index: usize,
        batch_len: usize,
        component: usize,
        value: f32,
    },

    /// Async task join failed
    #[error("Async task failed: {0}")]
    TaskJoin(String),

    /// GPU device construction failed (no fallback path)
    #[error("{0}")]
    GpuRequired(String),

    /// Failed to parse an embedder identity string back into an
    /// `EmbeddingBackend`. Used when reading `metadata.json` next to a
    /// LanceDB table.
    #[error("Invalid embedder identity: {0}")]
    InvalidIdentity(String),
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

    /// Create a GPU-required error
    pub fn gpu_required(msg: impl Into<String>) -> Self {
        Self::GpuRequired(msg.into())
    }

    /// Create an invalid-identity error.
    pub fn invalid_identity(msg: impl Into<String>) -> Self {
        Self::InvalidIdentity(msg.into())
    }
}

// Convert to Box<dyn Error + Send> for compatibility with existing code
impl From<EmbeddingError> for Box<dyn std::error::Error + Send> {
    fn from(err: EmbeddingError) -> Self {
        Box::new(err)
    }
}

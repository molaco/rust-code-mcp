//! Vector store error types
//!
//! Unified error type for all vector storage backends.

use thiserror::Error;

/// Errors that can occur during vector store operations
#[derive(Error, Debug)]
pub enum VectorStoreError {
    /// Failed to connect to the backend
    #[error("Connection failed: {0}")]
    Connection(String),

    /// Query execution failed
    #[error("Query failed: {0}")]
    Query(String),

    /// Serialization/deserialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Resource not found
    #[error("Not found: {0}")]
    NotFound(String),

    /// I/O error
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Generic backend error
    #[error("Backend error: {0}")]
    Backend(String),

    /// On-disk vector store was built with a different embedder than the
    /// one currently configured. The cached vectors are dimension- and/or
    /// instruction-incompatible and must be discarded before reindexing.
    #[error(
        "vector store embedder mismatch: stored={stored}, configured={configured}. \
         Run clear_cache to discard and rebuild."
    )]
    VersionMismatch {
        stored: String,
        configured: String,
    },
}

impl VectorStoreError {
    /// Create a connection error
    pub fn connection(msg: impl Into<String>) -> Self {
        Self::Connection(msg.into())
    }

    /// Create a query error
    pub fn query(msg: impl Into<String>) -> Self {
        Self::Query(msg.into())
    }

    /// Create a serialization error
    pub fn serialization(msg: impl Into<String>) -> Self {
        Self::Serialization(msg.into())
    }

    /// Create a not found error
    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::NotFound(msg.into())
    }

    /// Create a backend error
    pub fn backend(msg: impl Into<String>) -> Self {
        Self::Backend(msg.into())
    }

    /// Create a version-mismatch error between the stored embedder
    /// identity and the currently configured one.
    pub fn version_mismatch(
        stored: impl Into<String>,
        configured: impl Into<String>,
    ) -> Self {
        Self::VersionMismatch {
            stored: stored.into(),
            configured: configured.into(),
        }
    }
}

// Convert to Box<dyn Error + Send> for compatibility with existing code
impl From<VectorStoreError> for Box<dyn std::error::Error + Send> {
    fn from(err: VectorStoreError) -> Self {
        Box::new(err)
    }
}

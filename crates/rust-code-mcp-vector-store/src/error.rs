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
}

// Convert to Box<dyn Error + Send> for compatibility with existing code
impl From<VectorStoreError> for Box<dyn std::error::Error + Send> {
    fn from(err: VectorStoreError) -> Self {
        Box::new(err)
    }
}

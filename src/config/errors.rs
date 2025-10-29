//! Unified error handling utilities
//!
//! Provides centralized error types and utilities for consistent error handling
//! across the codebase. Uses anyhow for flexibility while providing domain-specific
//! error categorization.

pub use anyhow::{anyhow, bail, Context, Error, Result};

// Re-export indexing errors for convenience
pub use crate::indexing::errors::{categorize_error, ErrorCategory, ErrorCollector, ErrorDetail};

/// Common error message formatting
pub trait ErrorMessage {
    fn to_user_message(&self) -> String;
}

impl ErrorMessage for Error {
    fn to_user_message(&self) -> String {
        format!("Error: {}", self)
    }
}

/// Error context builders for common operations
pub trait ErrorContextExt<T> {
    /// Add indexing operation context
    fn indexing_context(self, operation: &str) -> Result<T>;

    /// Add search operation context
    fn search_context(self, query: &str) -> Result<T>;

    /// Add file operation context
    fn file_context(self, path: &std::path::Path) -> Result<T>;

    /// Add vector store operation context
    fn vector_store_context(self, operation: &str) -> Result<T>;
}

impl<T> ErrorContextExt<T> for Result<T> {
    fn indexing_context(self, operation: &str) -> Result<T> {
        self.with_context(|| format!("Indexing operation failed: {}", operation))
    }

    fn search_context(self, query: &str) -> Result<T> {
        self.with_context(|| format!("Search operation failed for query: {}", query))
    }

    fn file_context(self, path: &std::path::Path) -> Result<T> {
        self.with_context(|| format!("File operation failed: {}", path.display()))
    }

    fn vector_store_context(self, operation: &str) -> Result<T> {
        self.with_context(|| format!("Vector store operation failed: {}", operation))
    }
}

/// Convert `Box<dyn Error>` to anyhow::Error
pub fn box_error_to_anyhow(e: Box<dyn std::error::Error + Send + Sync>) -> Error {
    anyhow!("{}", e)
}

/// Check if an error is retryable
pub fn is_retryable(error: &Error) -> bool {
    let error_str = error.to_string().to_lowercase();

    // Network and temporary errors are retryable
    error_str.contains("timeout")
        || error_str.contains("connection")
        || error_str.contains("would block")
        || error_str.contains("try again")
        || error_str.contains("unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_context_builders() {
        let result: Result<()> = Err(anyhow!("test error"));

        let with_context = result.indexing_context("test operation");
        assert!(with_context.is_err());
        let error_msg = with_context.unwrap_err().to_string();
        assert!(error_msg.contains("Indexing operation failed"));
    }

    #[test]
    fn test_is_retryable() {
        let retryable = anyhow!("connection timeout");
        assert!(is_retryable(&retryable));

        let permanent = anyhow!("permission denied");
        assert!(!is_retryable(&permanent));
    }

    #[test]
    fn test_error_message() {
        let error = anyhow!("test error");
        let message = error.to_user_message();
        assert!(message.contains("Error:"));
        assert!(message.contains("test error"));
    }
}

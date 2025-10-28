//! Thread-safe error collection for parallel indexing
//!
//! Provides error tracking that can be safely shared across threads

use std::sync::{Arc, Mutex};
use std::path::PathBuf;

/// Category of indexing error
#[derive(Debug, Clone, PartialEq)]
pub enum ErrorCategory {
    /// Permanent error (permission denied, invalid UTF-8)
    Permanent,
    /// Transient error (network timeout, would block)
    Transient,
}

/// Details of a single indexing error
#[derive(Debug, Clone)]
pub struct ErrorDetail {
    /// Path to the file that failed
    pub file_path: PathBuf,
    /// Category of the error
    pub category: ErrorCategory,
    /// Error message
    pub message: String,
}

/// Thread-safe collector for indexing errors
#[derive(Clone)]
pub struct ErrorCollector {
    errors: Arc<Mutex<Vec<ErrorDetail>>>,
}

impl ErrorCollector {
    /// Create a new error collector
    pub fn new() -> Self {
        Self {
            errors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Record an error
    pub fn record(&self, error: ErrorDetail) {
        self.errors.lock().unwrap().push(error);
    }

    /// Get all collected errors
    pub fn get_errors(&self) -> Vec<ErrorDetail> {
        self.errors.lock().unwrap().clone()
    }

    /// Get the number of errors
    pub fn error_count(&self) -> usize {
        self.errors.lock().unwrap().len()
    }

    /// Get errors by category
    pub fn errors_by_category(&self, category: ErrorCategory) -> Vec<ErrorDetail> {
        self.errors
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.category == category)
            .cloned()
            .collect()
    }

    /// Clear all errors
    pub fn clear(&self) {
        self.errors.lock().unwrap().clear();
    }
}

impl Default for ErrorCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Categorize an error based on its message
pub fn categorize_error(error: &anyhow::Error) -> ErrorCategory {
    let error_str = error.to_string().to_lowercase();

    // Permanent errors
    if error_str.contains("permission denied")
        || error_str.contains("not found")
        || error_str.contains("invalid utf")
        || error_str.contains("is a directory")
    {
        return ErrorCategory::Permanent;
    }

    // Default to transient
    ErrorCategory::Transient
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_collector_creation() {
        let collector = ErrorCollector::new();
        assert_eq!(collector.error_count(), 0);
    }

    #[test]
    fn test_record_error() {
        let collector = ErrorCollector::new();

        collector.record(ErrorDetail {
            file_path: PathBuf::from("test.rs"),
            category: ErrorCategory::Permanent,
            message: "Permission denied".to_string(),
        });

        assert_eq!(collector.error_count(), 1);
    }

    #[test]
    fn test_get_errors() {
        let collector = ErrorCollector::new();

        collector.record(ErrorDetail {
            file_path: PathBuf::from("test1.rs"),
            category: ErrorCategory::Permanent,
            message: "Error 1".to_string(),
        });

        collector.record(ErrorDetail {
            file_path: PathBuf::from("test2.rs"),
            category: ErrorCategory::Transient,
            message: "Error 2".to_string(),
        });

        let errors = collector.get_errors();
        assert_eq!(errors.len(), 2);
    }

    #[test]
    fn test_errors_by_category() {
        let collector = ErrorCollector::new();

        collector.record(ErrorDetail {
            file_path: PathBuf::from("test1.rs"),
            category: ErrorCategory::Permanent,
            message: "Error 1".to_string(),
        });

        collector.record(ErrorDetail {
            file_path: PathBuf::from("test2.rs"),
            category: ErrorCategory::Transient,
            message: "Error 2".to_string(),
        });

        let permanent = collector.errors_by_category(ErrorCategory::Permanent);
        assert_eq!(permanent.len(), 1);

        let transient = collector.errors_by_category(ErrorCategory::Transient);
        assert_eq!(transient.len(), 1);
    }

    #[test]
    fn test_clear() {
        let collector = ErrorCollector::new();

        collector.record(ErrorDetail {
            file_path: PathBuf::from("test.rs"),
            category: ErrorCategory::Permanent,
            message: "Error".to_string(),
        });

        assert_eq!(collector.error_count(), 1);

        collector.clear();
        assert_eq!(collector.error_count(), 0);
    }

    #[test]
    fn test_categorize_permanent_errors() {
        let error = anyhow::anyhow!("Permission denied");
        assert_eq!(categorize_error(&error), ErrorCategory::Permanent);

        let error = anyhow::anyhow!("File not found");
        assert_eq!(categorize_error(&error), ErrorCategory::Permanent);
    }

    #[test]
    fn test_categorize_transient_errors() {
        let error = anyhow::anyhow!("Network timeout");
        assert_eq!(categorize_error(&error), ErrorCategory::Transient);
    }
}

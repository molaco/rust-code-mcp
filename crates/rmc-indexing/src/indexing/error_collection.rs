//! Thread-safe error collection for parallel indexing.
//!
//! Provides cross-thread error tracking (`ErrorCollector`) and error
//! categorization (`ErrorCategory`, `ErrorDetail`, `categorize_error`).
//!
//! This is distinct from `indexing::error::IndexingError`, which is the
//! `thiserror` enum for hard indexing failures.

use crate::indexing::error::SETTLED_OUTCOMES;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Category of indexing error
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ErrorCategory {
    /// Permanent error (permission denied, invalid UTF-8)
    Permanent,
    /// Transient error (network timeout, would block)
    Transient,
}

/// Details of a single indexing error
#[derive(Debug, Clone)]
pub(crate) struct ErrorDetail {
    /// Path to the file that failed
    pub(crate) file_path: PathBuf,
    /// Category of the error
    pub(crate) category: ErrorCategory,
    /// Error message
    pub(crate) message: String,
}

/// Thread-safe collector for indexing errors
#[derive(Clone)]
pub(crate) struct ErrorCollector {
    errors: Arc<Mutex<Vec<ErrorDetail>>>,
}

impl ErrorCollector {
    /// Create a new error collector
    pub(crate) fn new() -> Self {
        Self {
            errors: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Record an error
    pub(crate) fn record(&self, error: ErrorDetail) {
        self.errors.lock().unwrap().push(error);
    }

    /// Get all collected errors
    pub(crate) fn get_errors(&self) -> Vec<ErrorDetail> {
        self.errors.lock().unwrap().clone()
    }

    /// Get the number of errors
    pub(crate) fn error_count(&self) -> usize {
        self.errors.lock().unwrap().len()
    }

    /// Get errors by category
    pub(crate) fn errors_by_category(&self, category: ErrorCategory) -> Vec<ErrorDetail> {
        self.errors
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.category == category)
            .cloned()
            .collect()
    }

    /// Clear all errors
    pub(crate) fn clear(&self) {
        self.errors.lock().unwrap().clear();
    }
}

impl Default for ErrorCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Categorize an error based on its message
pub(crate) fn categorize_error(error: &dyn std::error::Error) -> ErrorCategory {
    let error_str = error.to_string().to_lowercase();

    // Settled outcomes: the file was considered and deliberately not
    // indexed. Retrying is guaranteed to reach the same answer, and a
    // retried path stays out of the committed Merkle snapshot — so
    // "transient" here would mean "re-attempted every five minutes for
    // as long as the daemon lives". See `SETTLED_OUTCOMES`.
    if SETTLED_OUTCOMES
        .iter()
        .any(|outcome| error_str.contains(&outcome.to_lowercase()))
    {
        return ErrorCategory::Permanent;
    }

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
        let error = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "Permission denied");
        assert_eq!(categorize_error(&error), ErrorCategory::Permanent);

        let error = std::io::Error::new(std::io::ErrorKind::NotFound, "File not found");
        assert_eq!(categorize_error(&error), ErrorCategory::Permanent);
    }

    #[test]
    fn test_categorize_transient_errors() {
        let error = std::io::Error::new(std::io::ErrorKind::TimedOut, "Network timeout");
        assert_eq!(categorize_error(&error), ErrorCategory::Transient);
    }

    /// A settled outcome must never be called transient.
    ///
    /// This is not a style point. A transient verdict keeps the path out
    /// of the committed Merkle snapshot, so the next run tries it again —
    /// and since these outcomes are deterministic, "again" means every
    /// five minutes for the life of the daemon, each attempt writing a
    /// fragment and a version to the vector store that nothing removes.
    /// That loop is what took the live index of a large workspace to 15 GB.
    #[test]
    fn settled_outcomes_are_permanent_not_retried_forever() {
        for outcome in SETTLED_OUTCOMES {
            let error = crate::indexing::IndexingError::Parser(outcome.to_string());
            assert_eq!(
                categorize_error(&error),
                ErrorCategory::Permanent,
                "{outcome:?} is deterministic — retrying it can only reach the same answer"
            );
        }
    }

    /// The control for the test above: a real parse failure MUST stay
    /// transient. Without this, "classify every Parser error as
    /// permanent" would pass the assertion above while silently dropping
    /// files that a retry would have recovered.
    #[test]
    fn a_genuine_parse_failure_is_still_transient() {
        let error =
            crate::indexing::IndexingError::Parser("tree-sitter: unexpected end of input".into());
        assert_eq!(categorize_error(&error), ErrorCategory::Transient);
    }
}

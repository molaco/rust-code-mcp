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

/// Outcomes that travel as `Parser` errors but are NOT failures to retry.
///
/// Each one means the file was considered and deliberately not indexed:
/// filtered, unchanged since last time, or holding nothing indexable.
/// They arrive as errors only because `process_file_sync` has a single
/// return channel.
///
/// Classifying them as transient costs more than a wasted retry. A
/// retried path is held OUT of the committed Merkle snapshot so the next
/// run will try it again — so "retry" here means "forever", and every
/// attempt writes a fresh fragment and version to the vector store.
/// Measured on the live index of a large workspace (2026-09-01): 62 such files
/// retried every five minutes, and with nothing compacting the store
/// that is what grew it to 15 GB against 4.2 GB of actual content.
///
/// The literals are shared with `categorize_error` deliberately. Spelled
/// out at both ends, a reworded message would quietly stop matching and
/// the loop would come back with no signal that anything had changed.
pub(crate) const OUTCOME_FILE_UNCHANGED: &str = "File unchanged";
pub(crate) const OUTCOME_NO_CHUNKS: &str = "No chunks generated";
pub(crate) const OUTCOME_CONTAINS_SECRETS: &str = "Contains secrets";
pub(crate) const OUTCOME_SECURITY_FILTERED: &str = "File filtered: security check failed";

/// Every outcome above, for the classifier to consult.
pub(crate) const SETTLED_OUTCOMES: [&str; 4] = [
    OUTCOME_FILE_UNCHANGED,
    OUTCOME_NO_CHUNKS,
    OUTCOME_CONTAINS_SECRETS,
    OUTCOME_SECURITY_FILTERED,
];

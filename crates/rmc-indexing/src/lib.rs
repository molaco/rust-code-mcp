//! rmc-indexing — Indexing pipeline plus supporting utilities.
//!
//! Contains the unified Tantivy + vector indexing pipeline (`indexing`),
//! the runtime monitoring surface (`monitoring`), the file metadata cache
//! used for incremental updates (`metadata_cache`), the indexing metrics
//! helpers (`metrics`), and the secrets/sensitive-file filters (`security`).

#![warn(unreachable_pub, dead_code)]

pub mod indexing;
pub mod metadata_cache;
pub mod metrics;
pub mod monitoring;
pub mod security;

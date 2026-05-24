//! rmc-indexing — Indexing pipeline plus supporting utilities.
//!
//! Contains the unified Tantivy + vector indexing pipeline (`indexing`),
//! the runtime monitoring surface (`monitoring`), indexing metrics helpers
//! (`metrics`), and internal caches/security filters used by indexing.

#![warn(unreachable_pub, dead_code)]

pub mod indexing;
mod metadata_cache;
pub mod metrics;
pub mod monitoring;
mod security;

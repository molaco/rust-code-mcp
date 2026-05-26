//! Read-path queries on a published snapshot.
//!
//! Split from the pre-refactor `graph::queries` mega-file across PRs 08-11.
//! Result types live in `model`; method implementations on `OpenedSnapshot`
//! are partitioned by concern (imports, usage, calls, crates, surface,
//! audits, functions, modules, overlaps).

pub(super) mod audits;
pub(super) mod calls;
pub(super) mod crates;
pub(super) mod enrichment;
pub(super) mod functions;
pub(super) mod imports;
pub(super) mod model;
pub(super) mod modules;
pub(super) mod navigation;
pub(super) mod overlaps;
pub(super) mod shared;
#[cfg(feature = "semantic-embeddings")]
pub(super) mod similarity;
pub(super) mod surface;
pub(super) mod usage;

#[cfg(test)]
mod tests;

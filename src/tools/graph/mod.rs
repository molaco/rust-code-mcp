//! Hypergraph-backed MCP tools, decomposed by endpoint family.
//!
//! PR 04 extracted the `core` family (module/import/usage/call-site
//! navigation) and the cross-family `response` helpers (pagination,
//! snapshot opening, error mapping, JSON serialization, common parsing).
//! PR 05 added the `crates`, `audits`, and `surface` families. PR 06
//! finished the split by extracting `similarity` and `codemap`, leaving
//! `graph_tools.rs` as a pure facade over these submodules.

pub(super) mod audits;
pub(super) mod codemap;
pub(super) mod core;
pub(super) mod crates;
pub(super) mod response;
pub(super) mod similarity;
pub(super) mod surface;

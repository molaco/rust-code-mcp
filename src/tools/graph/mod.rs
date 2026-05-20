//! Hypergraph-backed MCP tools, decomposed by endpoint family.
//!
//! PR 04 extracted the `core` family (module/import/usage/call-site
//! navigation) and the cross-family `response` helpers (pagination,
//! snapshot opening, error mapping, JSON serialization, common parsing).
//! Subsequent PRs will move the remaining families out of `graph_tools.rs`.

pub(super) mod core;
pub(super) mod response;

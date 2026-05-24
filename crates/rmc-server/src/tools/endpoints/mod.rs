//! Endpoint implementations for standalone MCP tools.
//!
//! The router (and compatibility facades in `src/tools/`) reach the
//! implementations through this module. Each submodule owns one
//! endpoint family — a single tool plus its supporting helpers
//! (`cache`, `health`, `index`), or a coherent cluster of related
//! tools (`analysis`, `query`).

pub(super) mod analysis;
pub(super) mod cache;
pub(super) mod health;
pub(super) mod index;
pub(super) mod query;

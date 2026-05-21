//! Task-conditioned codemap response types and query-time algorithm.
//!
//! Split across five sibling files in PRs 12-13:
//!   - `model`: serializable response types (Codemap, CodemapNode, ...).
//!   - `seeds`: SeedHit DTO + seed resolution (override + search-hit) + path/span helpers.
//!   - `build`: build_codemap algorithm + pre-build freshness check.
//!   - `hierarchy`: filtered module-tree projection used by build_codemap.
//!   - `render`: mermaid + outline output formatting.

mod model;
pub(super) mod seeds;
pub(super) mod build;
pub(super) mod hierarchy;
pub(super) mod render;

#[cfg(test)]
mod test_support;

pub use model::*;
pub use seeds::SeedHit;
pub use build::{build_codemap, newest_source_mtime};
pub use render::{render_mermaid, render_outline};

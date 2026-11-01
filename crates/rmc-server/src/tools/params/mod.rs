//! Parameter and schema structs for the MCP tool router.
//!
//! Split by endpoint family. Submodules are flat-re-exported so callers can
//! continue to use `crate::tools::params::FooParams` regardless of family.

mod audit;
mod graph;
mod indexing;
mod search;

pub(crate) use audit::*;
pub(crate) use graph::*;
pub(crate) use indexing::*;
pub(crate) use search::*;

use rmcp::schemars;

// Flattened into ~30 list tools, so every word here is paid ~30 times in
// `tools/list`. The full contract (slice after sort, `total_match_count`,
// how to detect more pages) is stated once in the server `instructions`.
#[derive(Debug, Default, Clone, Copy, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub(crate) struct ListPaginationParams {
    #[schemars(description = "Page size; default 50.")]
    #[serde(default)]
    pub limit: Option<usize>,
    #[schemars(description = "Items to skip; default 0.")]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Drop bulky per-item fields.")]
    #[serde(default)]
    pub summary: Option<bool>,
}

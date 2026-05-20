//! Parameter and schema structs for the MCP tool router.
//!
//! Split by endpoint family. Submodules are flat-re-exported so callers can
//! continue to use `crate::tools::params::FooParams` regardless of family.

mod audit;
mod graph;
mod indexing;
mod search;

pub use audit::*;
pub use graph::*;
pub use indexing::*;
pub use search::*;

use rmcp::schemars;

#[derive(Debug, Default, Clone, Copy, serde::Deserialize, serde::Serialize, schemars::JsonSchema)]
pub struct ListPaginationParams {
    #[schemars(description = "Optional cap on returned items after slicing. Default: 50.")]
    #[serde(default)]
    pub limit: Option<usize>,
    #[schemars(description = "Optional offset into the sorted item list, applied before `limit`. Default: 0.")]
    #[serde(default)]
    pub offset: Option<usize>,
    #[schemars(description = "Optional summary mode. When true, tools omit bulky per-item payload fields where applicable. Default: false.")]
    #[serde(default)]
    pub summary: Option<bool>,
}

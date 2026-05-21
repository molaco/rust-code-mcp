//! Indexing primitive parameter structs.

use rmcp::schemars;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct BuildHypergraphParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Force a rebuild even if a snapshot for the current fingerprint already exists")]
    pub force_rebuild: Option<bool>,
}

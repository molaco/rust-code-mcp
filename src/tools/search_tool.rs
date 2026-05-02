//! Search tool compatibility wrapper
//!
//! This module provides backward compatibility by re-exporting the new modular structure.
//! All functionality has been split into focused modules:
//! - `indexing_tools`: Indexing operations
//! - `query_tools`: Search and query operations
//! - `analysis_tools`: Code analysis operations
//! - `search_tool_router`: MCP tool routing
//!
//! This wrapper ensures existing code using `SearchTool` continues to work.

use rmcp::schemars;

// Re-export the router as SearchTool for backward compatibility
pub use crate::tools::search_tool_router::SearchToolRouter as SearchTool;

// Re-export all parameter types for backward compatibility
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Path to the directory to search")]
    pub directory: String,
    #[schemars(description = "Keyword to search for")]
    pub keyword: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FileContentParams {
    #[schemars(description = "Path to the file to read")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindDefinitionParams {
    #[schemars(description = "Symbol name to find the definition for (function, struct, trait, const, etc.)")]
    pub symbol_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindReferencesParams {
    #[schemars(description = "Symbol name to find all references for")]
    pub symbol_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetDependenciesParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetCallGraphParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
    #[schemars(description = "Optional: specific symbol to get call graph for")]
    pub symbol_name: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AnalyzeComplexityParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSimilarCodeParams {
    #[schemars(description = "Code snippet or query to find similar code")]
    pub query: String,
    #[schemars(description = "Directory containing the codebase")]
    pub directory: String,
    #[schemars(description = "Number of similar results to return (default 5)")]
    pub limit: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BuildHypergraphParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Force a rebuild even if a snapshot for the current fingerprint already exists")]
    pub force_rebuild: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphImportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module qualified name, e.g. `my_crate::sub::module`")]
    pub module: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphExportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module to enumerate exports from (qualified name)")]
    pub module: String,
    #[schemars(description = "Consumer module from whose viewpoint visibility is checked (qualified name)")]
    pub consumer: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GraphReexportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module to enumerate re-exports from (qualified name)")]
    pub module: String,
    #[schemars(description = "Consumer module from whose viewpoint visibility is checked (qualified name)")]
    pub consumer: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoImportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose importers you want")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoUsesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose non-import references you want (file:byte-range hits)")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeadPubParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the local crate to scan (e.g. `my_crate`). Items declared `pub` with no cross-crate consumers are returned as candidates for downgrading to `pub(crate)`.")]
    pub krate: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_tool_backward_compat() {
        // Verify SearchTool can still be created
        let _tool = SearchTool::new();
        // Just verify construction works
        assert!(true);
    }
}

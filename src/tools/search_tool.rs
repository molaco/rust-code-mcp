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
    #[schemars(description = "Symbol name to find the definition for")]
    pub symbol_name: String,
    #[schemars(description = "Directory to search in")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindReferencesParams {
    #[schemars(description = "Symbol name to find references to")]
    pub symbol_name: String,
    #[schemars(description = "Directory to search in")]
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

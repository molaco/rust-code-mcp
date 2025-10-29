//! Search tool router module
//!
//! This module provides the MCP tool routing for all search and analysis tools.
//! It delegates actual implementation to specialized modules:
//! - `indexing_tools`: Indexing operations
//! - `query_tools`: Search and query operations
//! - `analysis_tools`: Code analysis operations

use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    tool, tool_handler, tool_router,
};

// Re-export parameter types from search_tool for compatibility
pub use crate::tools::search_tool::{
    AnalyzeComplexityParams, FileContentParams, FindDefinitionParams, FindReferencesParams,
    GetCallGraphParams, GetDependenciesParams, GetSimilarCodeParams, SearchParams,
};

/// Main tool router struct
#[derive(Clone)]
pub struct SearchToolRouter {
    tool_router: ToolRouter<Self>,
    /// Optional sync manager for automatic directory tracking
    sync_manager: Option<std::sync::Arc<crate::mcp::SyncManager>>,
}

impl SearchToolRouter {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            sync_manager: None,
        }
    }

    /// Create a new SearchToolRouter with background sync manager
    pub fn with_sync_manager(sync_manager: std::sync::Arc<crate::mcp::SyncManager>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            sync_manager: Some(sync_manager),
        }
    }
}

#[tool_router]
impl SearchToolRouter {
    /// Read and return the content of a specified file
    #[tool(description = "Read the content of a file from the specified path")]
    async fn read_file_content(
        &self,
        Parameters(FileContentParams { file_path }): Parameters<FileContentParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::query_tools::read_file_content(&file_path).await
    }

    /// Perform hybrid search (BM25 + Vector) on Rust code in the specified directory
    #[tool(description = "Search for keywords in Rust code using hybrid search (BM25 + semantic vectors)")]
    async fn search(
        &self,
        Parameters(SearchParams { directory, keyword }): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::query_tools::search(&directory, &keyword, self.sync_manager.as_ref()).await
    }

    /// Find the definition of a symbol in Rust code
    #[tool(description = "Find where a Rust symbol (function, struct, trait, etc.) is defined")]
    async fn find_definition(
        &self,
        Parameters(FindDefinitionParams {
            symbol_name,
            directory,
        }): Parameters<FindDefinitionParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::find_definition(&symbol_name, &directory).await
    }

    /// Find all references to a symbol in the codebase
    #[tool(
        description = "Find all places where a symbol is referenced or called (includes function calls and type usage)"
    )]
    async fn find_references(
        &self,
        Parameters(FindReferencesParams {
            symbol_name,
            directory,
        }): Parameters<FindReferencesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::find_references(&symbol_name, &directory).await
    }

    /// Get dependencies for a file (imports and files that depend on it)
    #[tool(description = "Get import dependencies for a Rust source file")]
    async fn get_dependencies(
        &self,
        Parameters(GetDependenciesParams { file_path }): Parameters<GetDependenciesParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::get_dependencies(&file_path).await
    }

    /// Get call graph for a file or specific symbol
    #[tool(description = "Get the call graph showing function call relationships")]
    async fn get_call_graph(
        &self,
        Parameters(GetCallGraphParams {
            file_path,
            symbol_name,
        }): Parameters<GetCallGraphParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::get_call_graph(&file_path, symbol_name.as_deref()).await
    }

    /// Analyze code complexity metrics for a file
    #[tool(
        description = "Analyze code complexity metrics (LOC, cyclomatic complexity, function count)"
    )]
    async fn analyze_complexity(
        &self,
        Parameters(AnalyzeComplexityParams { file_path }): Parameters<AnalyzeComplexityParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::analyze_complexity(&file_path).await
    }

    /// Check system health status
    #[tool(description = "Check the health status of the code search system (BM25, Vector store, Merkle tree)")]
    async fn health_check(
        &self,
        Parameters(params): Parameters<crate::tools::health_tool::HealthCheckParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::health_tool::health_check(Parameters(params)).await
    }

    /// Find semantically similar code using vector search
    #[tool(description = "Find code snippets semantically similar to a query using embeddings")]
    async fn get_similar_code(
        &self,
        Parameters(GetSimilarCodeParams {
            query,
            directory,
            limit,
        }): Parameters<GetSimilarCodeParams>,
    ) -> Result<CallToolResult, McpError> {
        let limit = limit.unwrap_or(5);
        crate::tools::query_tools::get_similar_code(&query, &directory, limit).await
    }

    /// Manually index a codebase directory with automatic change detection
    #[tool(description = "Manually index a codebase directory (incremental indexing with Merkle tree change detection)")]
    async fn index_codebase(
        &self,
        Parameters(params): Parameters<crate::tools::index_tool::IndexCodebaseParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::index_tool::index_codebase(params, self.sync_manager.as_ref()).await
    }
}

#[tool_handler]
impl ServerHandler for SearchToolRouter {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "This server provides code search and analysis tools: 1) search - keyword search in files, 2) read_file_content - read file contents, 3) find_definition - locate symbol definitions, 4) find_references - find symbol references, 5) get_dependencies - analyze imports, 6) get_call_graph - show function call relationships, 7) analyze_complexity - calculate code metrics, 8) health_check - check system health status, 9) get_similar_code - semantic similarity search, 10) index_codebase - manually index a codebase with incremental change detection"
                    .into(),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_router_creation() {
        let router = SearchToolRouter::new();
        // Just verify it can be created
        assert!(true);
    }

    #[test]
    fn test_router_with_sync_manager() {
        use std::sync::Arc;
        let sync_mgr = Arc::new(crate::mcp::SyncManager::new());
        let router = SearchToolRouter::with_sync_manager(sync_mgr);
        assert!(router.sync_manager.is_some());
    }
}

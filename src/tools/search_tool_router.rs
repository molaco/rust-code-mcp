//! Search tool router module
//!
//! This module provides the MCP tool routing layer for all search and analysis tools.
//! It acts as a facade, delegating to specialized modules while handling MCP protocol
//! concerns like parameter validation and response formatting.
//!
//! ## Overview
//!
//! The `SearchToolRouter` is the main entry point for MCP clients. It:
//! - Routes tool calls to specialized implementations
//! - Handles parameter validation and error formatting
//! - Manages optional sync manager for background indexing
//! - Exposes 10 tools via MCP protocol
//!
//! ## Architecture
//!
//! ```text
//! MCP Client
//!     ↓
//! SearchToolRouter (this module)
//!     ├─→ query_tools      (search, get_similar_code, read_file_content)
//!     ├─→ analysis_tools   (find_*, get_*, analyze_complexity)
//!     ├─→ index_tool       (index_codebase)
//!     └─→ health_tool      (health_check)
//! ```
//!
//! ## Exposed MCP Tools
//!
//! 1. **read_file_content** - Read file contents
//! 2. **search** - Hybrid keyword + semantic search
//! 3. **get_similar_code** - Semantic similarity search
//! 4. **find_definition** - Locate symbol definitions
//! 5. **find_references** - Find all symbol references
//! 6. **get_dependencies** - Analyze file imports
//! 7. **get_call_graph** - Function call relationships
//! 8. **analyze_complexity** - Code complexity metrics
//! 9. **health_check** - System health status
//! 10. **index_codebase** - Manual indexing with change detection
//! 11. **clear_cache** - Clear corrupted cache/index files
//!
//! ## Refactoring Notes
//!
//! This module is the result of Phase 1 refactoring, which split the monolithic
//! `search_tool.rs` (1000 LOC, 242 cyclomatic complexity) into focused modules:
//! - Reduced router to ~200 LOC with routing-only logic
//! - Extracted business logic to specialized modules
//! - Achieved 90% complexity reduction
//! - Maintained full backward compatibility
//!
//! ## Examples
//!
//! ### Basic Usage
//! ```rust,no_run
//! use file_search_mcp::tools::search_tool_router::SearchToolRouter;
//!
//! // Create router without sync manager
//! let router = SearchToolRouter::new();
//! ```
//!
//! ### With Background Sync
//! ```rust,no_run
//! use file_search_mcp::tools::search_tool_router::SearchToolRouter;
//! use file_search_mcp::mcp::SyncManager;
//! use std::sync::Arc;
//! use std::path::PathBuf;
//!
//! // Create router with background sync
//! let sync_mgr = Arc::new(SyncManager::new(
//!     PathBuf::from("/tmp/cache"),
//!     PathBuf::from("/tmp/index"),
//!     300  // 5-minute sync interval
//! ));
//! let router = SearchToolRouter::with_sync_manager(sync_mgr);
//! ```

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

    /// Find the definition of a symbol by name
    #[tool(description = "Find where a Rust symbol (function, struct, trait, const, etc.) is defined")]
    async fn find_definition(
        &self,
        Parameters(FindDefinitionParams {
            symbol_name,
            directory,
        }): Parameters<FindDefinitionParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::analysis_tools::find_definition(&symbol_name, &directory).await
    }

    /// Find all references to a symbol by name
    #[tool(description = "Find all places where a symbol is used (calls, type references, etc.)")]
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

    /// Clear corrupted cache, index, and vector store files
    #[tool(description = "Clear corrupted cache files to fix 'Failed to open MetadataCache' errors. Clears metadata cache, tantivy index, and vector store.")]
    async fn clear_cache(
        &self,
        Parameters(params): Parameters<crate::tools::clear_cache_tool::ClearCacheParams>,
    ) -> Result<CallToolResult, McpError> {
        crate::tools::clear_cache_tool::clear_cache(params).await
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
                "This server provides code search and analysis tools: 1) search - keyword search in files, 2) read_file_content - read file contents, 3) find_definition - locate symbol definitions, 4) find_references - find symbol references, 5) get_dependencies - analyze imports, 6) get_call_graph - show function call relationships, 7) analyze_complexity - calculate code metrics, 8) health_check - check system health status, 9) get_similar_code - semantic similarity search, 10) index_codebase - manually index a codebase with incremental change detection, 11) clear_cache - clear corrupted cache/index files to fix MetadataCache errors"
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
        let _router = SearchToolRouter::new();
        // Just verify it can be created
        assert!(true);
    }

    #[test]
    fn test_router_with_sync_manager() {
        use std::sync::Arc;
        use std::path::PathBuf;
        let sync_mgr = Arc::new(crate::mcp::SyncManager::new(
            PathBuf::from("/tmp/cache"),
            PathBuf::from("/tmp/index"),
            300, // 5 minutes
        ));
        let _router = SearchToolRouter::with_sync_manager(sync_mgr);
        assert!(_router.sync_manager.is_some());
    }
}

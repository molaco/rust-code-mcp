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
pub struct GraphDeclaredReexportsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Module to enumerate explicit `pub use` declarations from (qualified name)")]
    pub module: String,
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
pub struct WhoUsesSummaryParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the symbol whose non-import references you want, aggregated per consumer module with per-category counts")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WhoCallsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose callers you want (Layer 10 call graph)")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CallsFromParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the caller function whose outgoing references you want (Layer 10 call graph)")]
    pub caller: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CallGraphParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the root function to descend from")]
    pub root: String,
    #[schemars(description = "Optional max recursion depth (default 3, capped at 8)")]
    pub depth: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CallersInCrateParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose callers you want")]
    pub target: String,
    #[schemars(description = "Qualified name of the crate to filter callers by (matches the *caller's* crate, not the target's)")]
    pub krate: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct RecursiveCallersCountParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the target function whose transitive callers you want to count")]
    pub target: String,
    #[schemars(description = "Optional max BFS depth in caller hops (default 3, capped at 8)")]
    pub depth: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeadPubParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the local crate to scan (e.g. `my_crate`). Items declared `pub` with no cross-crate consumers are returned as candidates for downgrading to `pub(crate)`.")]
    pub krate: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct DeadPubReportParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml). Runs dead_pub_in_crate over every local crate and returns aggregated findings per crate.")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CrateEdgesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct OverlapsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

/// One architectural rule for `forbidden_dependency_check`. Patterns in
/// `consumer`, `producer`, and `except` are glob-style with `*` wildcards.
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ForbiddenDependencyRuleParam {
    #[schemars(description = "Glob pattern matched against the consumer crate name (e.g. `domain*`)")]
    pub consumer: String,
    #[schemars(description = "Glob pattern matched against the producer crate name (e.g. `tokio`)")]
    pub producer: String,
    #[schemars(description = "Optional consumer-side glob exception: edges whose consumer matches this pattern are NOT flagged, even if `consumer`/`producer` match")]
    pub except: Option<String>,
    #[schemars(description = "Optional severity tag passed through to violations (e.g. `error` / `warn`)")]
    pub severity: Option<String>,
    #[schemars(description = "Optional human-readable rationale, passed through unchanged")]
    pub message: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ForbiddenDependencyCheckParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Architectural rules to enforce against the workspace's cross-crate edges")]
    pub rules: Vec<ForbiddenDependencyRuleParam>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct EnumVariantsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the enum whose variants you want (e.g. `my_crate::module::MyEnum`)")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ItemAttributesParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the item whose outer attributes (and doc-comment lines) you want, e.g. `my_crate::Foo`")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ItemsWithAttributeParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scan (e.g. `my_crate`); accepts the crate root module name as an alias")]
    pub crate_name: String,
    #[schemars(description = "Substring to match against each item's attribute strings, e.g. `#[must_use]`, `must_use`, `derive(Debug`, or `/// SAFETY:`")]
    pub attribute_pattern: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct PubUsePubTypeAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scan (e.g. `my_crate`); accepts the crate root module name as an alias")]
    pub crate_name: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ReExportChainParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the canonical declaration whose re-export chain you want to walk (e.g. `my_crate::module::Token`)")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct CrateDependencyMetricParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ModuleTreeParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name (e.g. `my_crate`)")]
    pub krate: String,
    #[schemars(description = "Optional max depth below the crate root (None walks the full tree)")]
    pub depth: Option<usize>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct WorkspaceStatsParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FunctionSignatureParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Qualified name of the function (e.g. `crate::module::fn_name` or `crate::Type::method`)")]
    pub target: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct UnsafeAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct MutStaticAuditParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FunctionsWithFilterParams {
    #[schemars(description = "Workspace root (directory containing Cargo.toml)")]
    pub directory: String,
    #[schemars(description = "Crate qualified name to scope the search (accepts the crate name or its root module)")]
    pub krate: String,
    #[schemars(description = "Optional minimum non-self param count")]
    #[serde(default)]
    pub min_param_count: Option<usize>,
    #[schemars(description = "Optional substring pattern that must appear in at least one param's stringified type")]
    #[serde(default)]
    pub has_param_type: Option<String>,
    #[schemars(description = "Optional substring pattern that must appear in the function's stringified return type")]
    #[serde(default)]
    pub returns_type_pattern: Option<String>,
    #[schemars(description = "Optional async filter — true to require `async fn`, false to require non-async")]
    #[serde(default)]
    pub is_async: Option<bool>,
    #[schemars(description = "Optional self-kind filter: \"none\" | \"owned\" | \"ref\" | \"ref_mut\"")]
    #[serde(default)]
    pub self_kind: Option<String>,
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

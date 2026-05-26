//! Search and navigation parameter structs.

use rmcp::schemars;

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct SearchParams {
    #[schemars(description = "Path to the directory to search")]
    pub directory: String,
    #[schemars(description = "Keyword to search for")]
    pub keyword: String,
    #[schemars(
        description = "Optional embedding profile for vector search. Default: \"local-cpu-small\". One of: \"local-gpu-small\", \"local-cpu-small\", \"openrouter-qwen3-8b\", \"local-qwen3-4b\", \"local-qwen3-8b\"."
    )]
    pub embedding_profile: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct FileContentParams {
    #[schemars(description = "Path to the file to read")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct FindDefinitionParams {
    #[schemars(description = "Symbol name to find the definition for (function, struct, trait, const, etc.)")]
    pub symbol_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
    #[schemars(description = "When true, only return full symbol-name matches. Default false preserves substring/fuzzy search behavior.")]
    #[serde(default)]
    pub exact: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct FindReferencesParams {
    #[schemars(description = "Symbol name to find all references for")]
    pub symbol_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
    #[schemars(description = "When true, only resolve references for full symbol-name matches. Default false preserves substring/fuzzy search behavior.")]
    #[serde(default)]
    pub exact: Option<bool>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct RenameSymbolParams {
    #[schemars(description = "Symbol leaf name to rename. Ambiguous names are rejected unless file_path, line, and column select a concrete position.")]
    pub symbol_name: String,
    #[schemars(description = "New name for the symbol (valid Rust identifier)")]
    pub new_name: String,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
    #[schemars(description = "Optional file path for position-based disambiguation. Provide together with line and column; relative paths are resolved from directory.")]
    pub file_path: Option<String>,
    #[schemars(description = "Optional 1-based line for position-based disambiguation. Provide together with file_path and column.")]
    pub line: Option<u32>,
    #[schemars(description = "Optional 1-based column for position-based disambiguation. Provide together with file_path and line.")]
    pub column: Option<u32>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GetDependenciesParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GetCallGraphParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
    #[schemars(description = "Optional: specific symbol to get call graph for")]
    pub symbol_name: Option<String>,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct AnalyzeComplexityParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub(crate) struct GetSimilarCodeParams {
    #[schemars(description = "Code snippet or query to find similar code")]
    pub query: String,
    #[schemars(description = "Directory containing the codebase")]
    pub directory: String,
    #[schemars(description = "Number of similar results to return (default 5)")]
    pub limit: Option<usize>,
    #[schemars(
        description = "Optional embedding profile for vector search. Default: \"local-cpu-small\". One of: \"local-gpu-small\", \"local-cpu-small\", \"openrouter-qwen3-8b\", \"local-qwen3-4b\", \"local-qwen3-8b\"."
    )]
    pub embedding_profile: Option<String>,
}

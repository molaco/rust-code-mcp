use rmcp::{
    ErrorData as McpError, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
    },
    schemars, tool, tool_handler, tool_router,
};
use std::fs;
use std::path::{Path, PathBuf};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{Index, TantivyDocument, doc};
use tracing;

// Phase 1: Import our new modules
use crate::embeddings::EmbeddingGenerator;
use crate::metadata_cache::{FileMetadata, MetadataCache};
use crate::parser::RustParser;
use crate::schema::FileSchema;
use crate::search::HybridSearch;
use crate::vector_store::{VectorStore, VectorStoreConfig};
use directories::ProjectDirs;

// Search parameters: directory path and search keyword
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    #[schemars(description = "Path to the directory to search")]
    pub directory: String,
    #[schemars(description = "Keyword to search for")]
    pub keyword: String,
}

// File content parameters: file path
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FileContentParams {
    #[schemars(description = "Path to the file to read")]
    pub file_path: String,
}

// Find definition parameters: symbol name and directory
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindDefinitionParams {
    #[schemars(description = "Symbol name to find the definition for")]
    pub symbol_name: String,
    #[schemars(description = "Directory to search in")]
    pub directory: String,
}

// Find references parameters: symbol name and directory
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindReferencesParams {
    #[schemars(description = "Symbol name to find references to")]
    pub symbol_name: String,
    #[schemars(description = "Directory to search in")]
    pub directory: String,
}

// Get dependencies parameters: file path
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetDependenciesParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

// Get call graph parameters: file path
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetCallGraphParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
    #[schemars(description = "Optional: specific symbol to get call graph for")]
    pub symbol_name: Option<String>,
}

// Analyze complexity parameters: file path
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct AnalyzeComplexityParams {
    #[schemars(description = "Path to the file to analyze")]
    pub file_path: String,
}

// Get similar code parameters: code snippet and directory
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct GetSimilarCodeParams {
    #[schemars(description = "Code snippet or query to find similar code")]
    pub query: String,
    #[schemars(description = "Directory containing the codebase")]
    pub directory: String,
    #[schemars(description = "Number of similar results to return (default 5)")]
    pub limit: Option<usize>,
}

// Main tool struct
#[derive(Clone)]
pub struct SearchTool {
    tool_router: ToolRouter<Self>,
    /// Optional sync manager for automatic directory tracking
    sync_manager: Option<std::sync::Arc<crate::mcp::SyncManager>>,
}

impl SearchTool {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            sync_manager: None,
        }
    }

    /// Create a new SearchTool with background sync manager
    pub fn with_sync_manager(sync_manager: std::sync::Arc<crate::mcp::SyncManager>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            sync_manager: Some(sync_manager),
        }
    }

    /// Get the path for storing persistent index and cache
    fn data_dir() -> PathBuf {
        // Use XDG-compliant data directory, or fallback to current directory
        ProjectDirs::from("dev", "rust-code-mcp", "search")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"))
    }

    /// Open or create a persistent Tantivy index
    fn open_or_create_index() -> Result<(Index, FileSchema), String> {
        let schema = FileSchema::new();
        let index_path = Self::data_dir().join("index");

        // Ensure directory exists
        std::fs::create_dir_all(&index_path)
            .map_err(|e| format!("Failed to create index directory: {}", e))?;

        let index = if index_path.join("meta.json").exists() {
            // Open existing index
            tracing::debug!("Opening existing index at: {}", index_path.display());
            Index::open_in_dir(&index_path).map_err(|e| format!("Failed to open index: {}", e))?
        } else {
            // Create new index
            tracing::info!("Creating new index at: {}", index_path.display());
            Index::create_in_dir(&index_path, schema.schema())
                .map_err(|e| format!("Failed to create index: {}", e))?
        };

        Ok((index, schema))
    }

    /// Open or create metadata cache
    fn open_cache() -> Result<MetadataCache, String> {
        let cache_path = Self::data_dir().join("cache");
        MetadataCache::new(&cache_path).map_err(|e| format!("Failed to open metadata cache: {}", e))
    }
}

#[tool_router]
impl SearchTool {
    /// Read and return the content of a specified file
    #[tool(description = "Read the content of a file from the specified path")]
    async fn read_file_content(
        &self,
        Parameters(FileContentParams { file_path }): Parameters<FileContentParams>,
    ) -> Result<CallToolResult, McpError> {
        // Validate file path
        let file_path_obj = Path::new(&file_path);

        // Check if the path exists
        if !file_path_obj.exists() {
            return Err(McpError::invalid_params(
                format!("The specified path '{}' does not exist", file_path),
                None,
            ));
        }

        // Check if the path is a file
        if !file_path_obj.is_file() {
            return Err(McpError::invalid_params(
                format!("The specified path '{}' is not a file", file_path),
                None,
            ));
        }

        // Try to read the file content
        match fs::read_to_string(file_path_obj) {
            Ok(content) => {
                if content.is_empty() {
                    Ok(CallToolResult::success(vec![Content::text(
                        "File is empty.",
                    )]))
                } else {
                    Ok(CallToolResult::success(vec![Content::text(content)]))
                }
            }
            Err(e) => {
                // Handle binary files or read errors
                tracing::error!("Error reading file '{}': {}", file_path_obj.display(), e);

                // Try to read as binary and check if it's a binary file
                match fs::read(file_path_obj) {
                    Ok(bytes) => {
                        // Check if it seems to be a binary file
                        if bytes.iter().any(|&b| b == 0)
                            || bytes
                                .iter()
                                .filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13)
                                .count()
                                > bytes.len() / 10
                        {
                            Err(McpError::invalid_params(
                                format!(
                                    "The file '{}' appears to be a binary file and cannot be displayed as text",
                                    file_path
                                ),
                                None,
                            ))
                        } else {
                            Err(McpError::invalid_params(
                                format!(
                                    "The file '{}' could not be read as text: {}",
                                    file_path, e
                                ),
                                None,
                            ))
                        }
                    }
                    Err(read_err) => Err(McpError::invalid_params(
                        format!("Error reading file '{}': {}", file_path, read_err),
                        None,
                    )),
                }
            }
        }
    }

    /// Perform hybrid search (BM25 + Vector) on Rust code in the specified directory
    #[tool(description = "Search for keywords in Rust code using hybrid search (BM25 + semantic vectors)")]
    async fn search(
        &self,
        Parameters(SearchParams { directory, keyword }): Parameters<SearchParams>,
    ) -> Result<CallToolResult, McpError> {
        use crate::indexing::unified::UnifiedIndexer;

        let dir_path = Path::new(&directory);
        if !dir_path.is_dir() {
            return Err(McpError::invalid_params(
                format!("The specified path '{}' is not a directory", directory),
                None,
            ));
        }

        // Ensure the keyword is not empty
        if keyword.trim().is_empty() {
            return Err(McpError::invalid_params(
                "Search keyword is empty. Please enter a valid keyword.".to_string(),
                None,
            ));
        }

        // 1. Initialize unified indexer
        let qdrant_url = std::env::var("QDRANT_URL")
            .unwrap_or_else(|_| "http://localhost:6334".to_string());

        // Sanitize project name for collection
        let project_name = dir_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("default")
            .replace(|c: char| !c.is_alphanumeric(), "_");

        let collection_name = format!("code_chunks_{}", project_name);

        tracing::info!("Initializing unified indexer for {}", dir_path.display());

        let mut indexer = UnifiedIndexer::new(
            &Self::data_dir().join("cache"),
            &Self::data_dir().join("index"),
            &qdrant_url,
            &collection_name,
            384, // all-MiniLM-L6-v2 vector size
        )
        .await
        .map_err(|e| McpError::invalid_params(format!("Failed to initialize indexer: {}", e), None))?;

        // 2. Index directory (incremental - only changed files)
        tracing::info!("Indexing directory: {}", dir_path.display());
        let stats = indexer
            .index_directory(dir_path)
            .await
            .map_err(|e| McpError::invalid_params(format!("Indexing failed: {}", e), None))?;

        tracing::info!(
            "Indexed {} files ({} chunks), {} unchanged, {} skipped",
            stats.indexed_files,
            stats.total_chunks,
            stats.unchanged_files,
            stats.skipped_files
        );

        // Track directory for background sync if indexing was successful
        if let Some(ref sync_mgr) = self.sync_manager {
            if stats.indexed_files > 0 || stats.unchanged_files > 0 {
                sync_mgr.track_directory(dir_path.to_path_buf()).await;
                tracing::info!("Directory tracked for background sync: {}", dir_path.display());
            }
        }

        if stats.total_chunks == 0 && stats.unchanged_files == 0 {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No Rust files suitable for indexing were found in '{}'.\nSkipped files: {}",
                directory, stats.skipped_files
            ))]));
        }

        // 3. Perform hybrid search
        let bm25_search = indexer.create_bm25_search()
            .map_err(|e| McpError::invalid_params(format!("Failed to create BM25 search: {}", e), None))?;

        let hybrid_search = HybridSearch::with_defaults(
            indexer.embedding_generator_cloned(),
            indexer.vector_store_cloned(),
            Some(bm25_search),
        );

        tracing::info!("Performing hybrid search for: {}", keyword);
        let results = hybrid_search
            .search(&keyword, 10)
            .await
            .map_err(|e| McpError::invalid_params(format!("Search failed: {}", e), None))?;

        // 4. Format results
        if results.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "No results found for '{}'.\nIndexed {} files ({} chunks), {} unchanged, {} skipped",
                keyword, stats.indexed_files, stats.total_chunks, stats.unchanged_files, stats.skipped_files
            ))]))
        } else {
            let mut result_str = format!("Found {} results for '{}':\n\n", results.len(), keyword);

            for (idx, result) in results.iter().enumerate() {
                result_str.push_str(&format!(
                    "{}. Score: {:.4} | File: {} | Symbol: {} ({})\n",
                    idx + 1,
                    result.score,
                    result.chunk.context.file_path.display(),
                    result.chunk.context.symbol_name,
                    result.chunk.context.symbol_kind,
                ));
                result_str.push_str(&format!(
                    "   Lines: {}-{}\n",
                    result.chunk.context.line_start,
                    result.chunk.context.line_end
                ));
                if let Some(ref doc) = result.chunk.context.docstring {
                    result_str.push_str(&format!("   Doc: {}\n", doc));
                }
                result_str.push_str(&format!(
                    "   Preview:\n   {}\n\n",
                    result.chunk.content.lines().take(3).collect::<Vec<_>>().join("\n   ")
                ));
            }

            result_str.push_str(&format!(
                "\n--- Indexing stats: {} files indexed ({} chunks), {} unchanged, {} skipped ---",
                stats.indexed_files, stats.total_chunks, stats.unchanged_files, stats.skipped_files
            ));

            Ok(CallToolResult::success(vec![Content::text(result_str)]))
        }
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
        let dir_path = Path::new(&directory);
        if !dir_path.is_dir() {
            return Err(McpError::invalid_params(
                format!("The specified path '{}' is not a directory", directory),
                None,
            ));
        }

        tracing::debug!("Searching for definition of '{}'", symbol_name);

        let mut found_definitions = Vec::new();
        let mut parser = RustParser::new().map_err(|e| {
            McpError::invalid_params(format!("Parser initialization error: {}", e), None)
        })?;

        // Recursively search .rs files
        fn visit_rust_files(
            dir: &Path,
            symbol_name: &str,
            parser: &mut RustParser,
            found: &mut Vec<(PathBuf, usize, String)>,
        ) -> Result<(), String> {
            for entry in fs::read_dir(dir).map_err(|e| format!("Directory read error: {}", e))? {
                let entry = entry.map_err(|e| format!("Entry read error: {}", e))?;
                let path = entry.path();

                if path.is_dir() {
                    visit_rust_files(&path, symbol_name, parser, found)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    if let Ok(symbols) = parser.parse_file(&path) {
                        for symbol in symbols {
                            if symbol.name == symbol_name {
                                found.push((
                                    path.clone(),
                                    symbol.range.start_line,
                                    symbol.kind.as_str().to_string(),
                                ));
                            }
                        }
                    }
                }
            }
            Ok(())
        }

        visit_rust_files(dir_path, &symbol_name, &mut parser, &mut found_definitions)
            .map_err(|e| McpError::invalid_params(e, None))?;

        if found_definitions.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "No definition found for symbol '{}'",
                symbol_name
            ))]))
        } else {
            let mut result = format!(
                "Found {} definition(s) for '{}':\n",
                found_definitions.len(),
                symbol_name
            );
            for (path, line, kind) in found_definitions {
                result.push_str(&format!("- {}:{} ({})\n", path.display(), line, kind));
            }
            Ok(CallToolResult::success(vec![Content::text(result)]))
        }
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
        let dir_path = Path::new(&directory);
        if !dir_path.is_dir() {
            return Err(McpError::invalid_params(
                format!("The specified path '{}' is not a directory", directory),
                None,
            ));
        }

        tracing::debug!("Searching for references to '{}'", symbol_name);

        let mut found_call_refs = Vec::new();
        let mut found_type_refs = Vec::new();
        let mut parser = RustParser::new().map_err(|e| {
            McpError::invalid_params(format!("Parser initialization error: {}", e), None)
        })?;

        // Recursively search .rs files for references (both function calls and type usage)
        fn visit_rust_files(
            dir: &Path,
            symbol_name: &str,
            parser: &mut RustParser,
            call_refs: &mut Vec<(PathBuf, Vec<String>)>,
            type_refs: &mut Vec<(PathBuf, Vec<String>)>,
        ) -> Result<(), String> {
            for entry in fs::read_dir(dir).map_err(|e| format!("Directory read error: {}", e))? {
                let entry = entry.map_err(|e| format!("Entry read error: {}", e))?;
                let path = entry.path();

                if path.is_dir() {
                    visit_rust_files(&path, symbol_name, parser, call_refs, type_refs)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    if let Ok(parse_result) = parser.parse_file_complete(&path) {
                        // Check function call references
                        let callers = parse_result.call_graph.get_callers(symbol_name);
                        if !callers.is_empty() {
                            call_refs.push((
                                path.clone(),
                                callers.into_iter().map(String::from).collect(),
                            ));
                        }

                        // Check type references
                        let type_usages: Vec<String> = parse_result
                            .type_references
                            .iter()
                            .filter(|r| r.type_name == symbol_name)
                            .map(|r| match &r.usage_context {
                                crate::parser::TypeUsageContext::FunctionParameter {
                                    function_name,
                                } => {
                                    format!("parameter in {}", function_name)
                                }
                                crate::parser::TypeUsageContext::FunctionReturn {
                                    function_name,
                                } => {
                                    format!("return type of {}", function_name)
                                }
                                crate::parser::TypeUsageContext::StructField {
                                    struct_name,
                                    field_name,
                                } => {
                                    format!("field '{}' in struct {}", field_name, struct_name)
                                }
                                crate::parser::TypeUsageContext::ImplBlock { trait_name } => {
                                    if let Some(trait_name) = trait_name {
                                        format!("impl {} for type", trait_name)
                                    } else {
                                        "impl block".to_string()
                                    }
                                }
                                crate::parser::TypeUsageContext::LetBinding => {
                                    "let binding".to_string()
                                }
                                crate::parser::TypeUsageContext::GenericArgument => {
                                    "generic type argument".to_string()
                                }
                            })
                            .collect();

                        if !type_usages.is_empty() {
                            type_refs.push((path.clone(), type_usages));
                        }
                    }
                }
            }
            Ok(())
        }

        visit_rust_files(
            dir_path,
            &symbol_name,
            &mut parser,
            &mut found_call_refs,
            &mut found_type_refs,
        )
        .map_err(|e| McpError::invalid_params(e, None))?;

        if found_call_refs.is_empty() && found_type_refs.is_empty() {
            Ok(CallToolResult::success(vec![Content::text(format!(
                "No references found for symbol '{}'",
                symbol_name
            ))]))
        } else {
            let total_call_refs: usize = found_call_refs
                .iter()
                .map(|(_, callers)| callers.len())
                .sum();
            let total_type_refs: usize =
                found_type_refs.iter().map(|(_, usages)| usages.len()).sum();
            let total_refs = total_call_refs + total_type_refs;
            let total_files = found_call_refs
                .iter()
                .map(|(p, _)| p)
                .chain(found_type_refs.iter().map(|(p, _)| p))
                .collect::<std::collections::HashSet<_>>()
                .len();

            let mut result = format!(
                "Found {} reference(s) to '{}' in {} file(s):\n\n",
                total_refs, symbol_name, total_files
            );

            // Show function call references
            if !found_call_refs.is_empty() {
                result.push_str(&format!(
                    "Function Calls ({} references):\n",
                    total_call_refs
                ));
                for (path, callers) in found_call_refs {
                    result.push_str(&format!(
                        "- {} (called by: {})\n",
                        path.display(),
                        callers.join(", ")
                    ));
                }
                result.push('\n');
            }

            // Show type usage references
            if !found_type_refs.is_empty() {
                result.push_str(&format!("Type Usage ({} references):\n", total_type_refs));
                for (path, usages) in found_type_refs {
                    result.push_str(&format!("- {} ({})\n", path.display(), usages.join(", ")));
                }
            }

            Ok(CallToolResult::success(vec![Content::text(result)]))
        }
    }

    /// Get dependencies for a file (imports and files that depend on it)
    #[tool(description = "Get import dependencies for a Rust source file")]
    async fn get_dependencies(
        &self,
        Parameters(GetDependenciesParams { file_path }): Parameters<GetDependenciesParams>,
    ) -> Result<CallToolResult, McpError> {
        let file_path_obj = Path::new(&file_path);

        if !file_path_obj.exists() {
            return Err(McpError::invalid_params(
                format!("File '{}' does not exist", file_path),
                None,
            ));
        }

        if !file_path_obj.is_file() {
            return Err(McpError::invalid_params(
                format!("'{}' is not a file", file_path),
                None,
            ));
        }

        let mut parser = RustParser::new().map_err(|e| {
            McpError::invalid_params(format!("Parser initialization error: {}", e), None)
        })?;
        let parse_result = parser
            .parse_file_complete(file_path_obj)
            .map_err(|e| McpError::invalid_params(format!("Parse error: {}", e), None))?;

        let mut result = format!("Dependencies for '{}':\n\n", file_path_obj.display());

        // List imports
        if parse_result.imports.is_empty() {
            result.push_str("No imports found.\n");
        } else {
            result.push_str(&format!("Imports ({}):\n", parse_result.imports.len()));
            for import in &parse_result.imports {
                result.push_str(&format!("- {}\n", import.path));
            }
        }

        Ok(CallToolResult::success(vec![Content::text(result)]))
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
        let file_path_obj = Path::new(&file_path);

        if !file_path_obj.exists() {
            return Err(McpError::invalid_params(
                format!("File '{}' does not exist", file_path),
                None,
            ));
        }

        if !file_path_obj.is_file() {
            return Err(McpError::invalid_params(
                format!("'{}' is not a file", file_path),
                None,
            ));
        }

        let mut parser = RustParser::new().map_err(|e| {
            McpError::invalid_params(format!("Parser initialization error: {}", e), None)
        })?;
        let parse_result = parser
            .parse_file_complete(file_path_obj)
            .map_err(|e| McpError::invalid_params(format!("Parse error: {}", e), None))?;

        let mut result = format!("Call graph for '{}':\n\n", file_path_obj.display());

        if let Some(ref symbol_name) = symbol_name {
            // Show call graph for specific symbol
            let callees = parse_result.call_graph.get_callees(symbol_name);
            let callers = parse_result.call_graph.get_callers(symbol_name);

            result.push_str(&format!("Symbol: {}\n\n", symbol_name));

            if callees.is_empty() && callers.is_empty() {
                result.push_str("No call relationships found.\n");
            } else {
                if !callees.is_empty() {
                    result.push_str(&format!("Calls ({}):\n", callees.len()));
                    for callee in callees {
                        result.push_str(&format!("  → {}\n", callee));
                    }
                }

                if !callers.is_empty() {
                    result.push_str(&format!("\nCalled by ({}):\n", callers.len()));
                    for caller in callers {
                        result.push_str(&format!("  ← {}\n", caller));
                    }
                }
            }
        } else {
            // Show entire call graph for file
            let all_functions = parse_result.call_graph.all_functions();
            let edge_count = parse_result.call_graph.edge_count();

            result.push_str(&format!("Functions: {}\n", all_functions.len()));
            result.push_str(&format!("Call relationships: {}\n\n", edge_count));

            if edge_count == 0 {
                result.push_str("No function calls found.\n");
            } else {
                result.push_str("Call relationships:\n");
                for function in all_functions {
                    let callees = parse_result.call_graph.get_callees(function);
                    if !callees.is_empty() {
                        result.push_str(&format!("{} → [{}]\n", function, callees.join(", ")));
                    }
                }
            }
        }

        Ok(CallToolResult::success(vec![Content::text(result)]))
    }

    /// Analyze code complexity metrics for a file
    #[tool(
        description = "Analyze code complexity metrics (LOC, cyclomatic complexity, function count)"
    )]
    async fn analyze_complexity(
        &self,
        Parameters(AnalyzeComplexityParams { file_path }): Parameters<AnalyzeComplexityParams>,
    ) -> Result<CallToolResult, McpError> {
        let file_path_obj = Path::new(&file_path);

        if !file_path_obj.exists() {
            return Err(McpError::invalid_params(
                format!("File '{}' does not exist", file_path),
                None,
            ));
        }

        if !file_path_obj.is_file() {
            return Err(McpError::invalid_params(
                format!("'{}' is not a file", file_path),
                None,
            ));
        }

        // Read source file
        let source = fs::read_to_string(file_path_obj)
            .map_err(|e| McpError::invalid_params(format!("Failed to read file: {}", e), None))?;

        // Parse file to get symbols
        let mut parser = RustParser::new().map_err(|e| {
            McpError::invalid_params(format!("Parser initialization error: {}", e), None)
        })?;
        let parse_result = parser
            .parse_file_complete(file_path_obj)
            .map_err(|e| McpError::invalid_params(format!("Parse error: {}", e), None))?;

        // Calculate metrics
        let lines_of_code = source.lines().count();
        let non_empty_loc = source.lines().filter(|l| !l.trim().is_empty()).count();
        let comment_lines = source
            .lines()
            .filter(|l| l.trim().starts_with("//"))
            .count();
        let function_count = parse_result
            .symbols
            .iter()
            .filter(|s| matches!(s.kind, crate::parser::SymbolKind::Function { .. }))
            .count();
        let struct_count = parse_result
            .symbols
            .iter()
            .filter(|s| matches!(s.kind, crate::parser::SymbolKind::Struct))
            .count();
        let trait_count = parse_result
            .symbols
            .iter()
            .filter(|s| matches!(s.kind, crate::parser::SymbolKind::Trait))
            .count();

        // Calculate cyclomatic complexity (simplified - count decision points)
        let complexity_keywords = ["if", "else if", "while", "for", "match", "&&", "||"];
        let cyclomatic_complexity: usize = source
            .lines()
            .map(|line| {
                complexity_keywords
                    .iter()
                    .map(|kw| line.matches(kw).count())
                    .sum::<usize>()
            })
            .sum();

        // Calculate average function complexity
        let avg_complexity = if function_count > 0 {
            cyclomatic_complexity as f64 / function_count as f64
        } else {
            0.0
        };

        let mut result = format!("Complexity analysis for '{}':\n\n", file_path_obj.display());
        result.push_str("=== Code Metrics ===\n");
        result.push_str(&format!("Total lines:           {}\n", lines_of_code));
        result.push_str(&format!("Non-empty lines:       {}\n", non_empty_loc));
        result.push_str(&format!("Comment lines:         {}\n", comment_lines));
        result.push_str(&format!(
            "Code lines (approx):   {}\n\n",
            non_empty_loc - comment_lines
        ));

        result.push_str("=== Symbol Counts ===\n");
        result.push_str(&format!("Functions:             {}\n", function_count));
        result.push_str(&format!("Structs:               {}\n", struct_count));
        result.push_str(&format!("Traits:                {}\n\n", trait_count));

        result.push_str("=== Complexity ===\n");
        result.push_str(&format!(
            "Total cyclomatic:      {}\n",
            cyclomatic_complexity
        ));
        result.push_str(&format!("Avg per function:      {:.2}\n", avg_complexity));

        // Add call graph complexity
        let edge_count = parse_result.call_graph.edge_count();
        result.push_str(&format!("Function calls:        {}\n", edge_count));

        Ok(CallToolResult::success(vec![Content::text(result)]))
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
        let dir_path = Path::new(&directory);
        if !dir_path.is_dir() {
            return Err(McpError::invalid_params(
                format!("The specified path '{}' is not a directory", directory),
                None,
            ));
        }

        let limit = limit.unwrap_or(5);

        tracing::debug!("Searching for similar code to: {}", query);

        // Initialize components
        let embedding_generator = EmbeddingGenerator::new().map_err(|e| {
            McpError::invalid_params(
                format!("Failed to initialize embedding generator: {}", e),
                None,
            )
        })?;

        let vector_store_config = VectorStoreConfig::default();
        let vector_store = VectorStore::new(vector_store_config).await.map_err(|e| {
            McpError::invalid_params(format!("Failed to initialize vector store: {}", e), None)
        })?;

        // Create hybrid search (vector-only mode)
        let hybrid_search = HybridSearch::with_defaults(
            embedding_generator,
            vector_store,
            None, // No BM25 for this tool
        );

        // Perform vector search
        let results = hybrid_search
            .vector_only_search(&query, limit)
            .await
            .map_err(|e| McpError::invalid_params(format!("Search error: {}", e), None))?;

        if results.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "No similar code found for query: '{}'",
                query
            ))]));
        }

        let mut result = format!(
            "Found {} similar code snippet(s) for query '{}':\n\n",
            results.len(),
            query
        );

        for (idx, search_result) in results.iter().enumerate() {
            let chunk = &search_result.chunk;
            result.push_str(&format!("{}. ", idx + 1));
            result.push_str(&format!(
                "Score: {:.4} | File: {} | Symbol: {} ({})\n",
                search_result.score,
                chunk.context.file_path.display(),
                chunk.context.symbol_name,
                chunk.context.symbol_kind
            ));
            result.push_str(&format!(
                "   Lines: {}-{}\n",
                chunk.context.line_start, chunk.context.line_end
            ));
            if let Some(ref doc) = chunk.context.docstring {
                result.push_str(&format!("   Doc: {}\n", doc));
            }
            result.push_str(&format!(
                "   Code preview:\n   {}\n\n",
                chunk
                    .content
                    .lines()
                    .take(3)
                    .collect::<Vec<_>>()
                    .join("\n   ")
            ));
        }

        Ok(CallToolResult::success(vec![Content::text(result)]))
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
impl ServerHandler for SearchTool {
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

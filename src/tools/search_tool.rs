use rmcp::model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, schemars, tool};
use std::fs;
use std::path::{Path, PathBuf};
use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::Value;
use tantivy::{Index, TantivyDocument, doc};
use tracing;

// Phase 1: Import our new modules
use file_search_mcp::metadata_cache::{FileMetadata, MetadataCache};
use file_search_mcp::schema::FileSchema;
use file_search_mcp::parser::RustParser;
use file_search_mcp::embeddings::EmbeddingGenerator;
use file_search_mcp::vector_store::{VectorStore, VectorStoreConfig};
use file_search_mcp::search::HybridSearch;
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
#[derive(Debug, Clone)]
pub struct SearchTool;

#[tool(tool_box)]
impl SearchTool {
    pub fn new() -> Self {
        Self {}
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
            Index::open_in_dir(&index_path)
                .map_err(|e| format!("Failed to open index: {}", e))?
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
        MetadataCache::new(&cache_path)
            .map_err(|e| format!("Failed to open metadata cache: {}", e))
    }

    /// Read and return the content of a specified file
    #[tool(description = "Read the content of a file from the specified path")]
    async fn read_file_content(
        &self,
        #[tool(aggr)] params: FileContentParams,
    ) -> Result<String, String> {
        // Validate file path
        let file_path = Path::new(&params.file_path);

        // Check if the path exists
        if !file_path.exists() {
            return Err(format!(
                "The specified path '{}' does not exist",
                params.file_path
            ));
        }

        // Check if the path is a file
        if !file_path.is_file() {
            return Err(format!(
                "The specified path '{}' is not a file",
                params.file_path
            ));
        }

        // Try to read the file content
        match fs::read_to_string(file_path) {
            Ok(content) => {
                if content.is_empty() {
                    Ok("File is empty.".to_string())
                } else {
                    Ok(content)
                }
            }
            Err(e) => {
                // Handle binary files or read errors
                tracing::error!("Error reading file '{}': {}", file_path.display(), e);

                // Try to read as binary and check if it's a binary file
                match fs::read(file_path) {
                    Ok(bytes) => {
                        // Check if it seems to be a binary file
                        if bytes.iter().any(|&b| b == 0)
                            || bytes
                                .iter()
                                .filter(|&&b| b < 32 && b != 9 && b != 10 && b != 13)
                                .count()
                                > bytes.len() / 10
                        {
                            Err(format!(
                                "The file '{}' appears to be a binary file and cannot be displayed as text",
                                params.file_path
                            ))
                        } else {
                            Err(format!(
                                "The file '{}' could not be read as text: {}",
                                params.file_path, e
                            ))
                        }
                    }
                    Err(read_err) => Err(format!(
                        "Error reading file '{}': {}",
                        params.file_path, read_err
                    )),
                }
            }
        }
    }

    /// Perform full-text search for keywords on text files (such as .txt, .md, etc.) in the specified directory
    #[tool(description = "Search for keywords in text files within the specified directory")]
    async fn search(&self, #[tool(aggr)] params: SearchParams) -> Result<String, String> {
        // 1. Open or create persistent index with FileSchema
        let (index, file_schema) = Self::open_or_create_index()?;

        // 2. Open metadata cache for incremental indexing
        let cache = Self::open_cache()?;

        // 3. Create index writer (adjust buffer size as needed)
        let mut index_writer = index
            .writer(50_000_000)
            .map_err(|e| format!("Index writer error: {}", e))?;

        // 4. Count the number of files added to the index
        let mut indexed_files_count = 0;
        let mut reindexed_files_count = 0;  // Track how many were reindexed (changed)
        let mut unchanged_files_count = 0;  // Track how many were skipped (unchanged)
        // Track directory processing status (for debugging)
        let mut found_files_count = 0;
        let mut skipped_files_count = 0;

        // 4. Read text files in the specified directory and add them to the index
        let dir_path = Path::new(&params.directory);
        if !dir_path.is_dir() {
            return Err(format!(
                "The specified path '{}' is not a directory",
                params.directory
            ));
        }

        // Blacklist of extensions likely to be binary files
        // Skip extensions that are clearly binary files
        let binary_extensions = [
            "exe", "dll", "so", "dylib", "bin", "obj", "o", "a", "lib", "png", "jpg", "jpeg",
            "gif", "bmp", "tiff", "webp", "ico", "mp3", "mp4", "wav", "ogg", "flac", "avi", "mov",
            "mkv", "zip", "gz", "tar", "7z", "rar", "jar", "war", "pdf", "doc", "docx", "xls",
            "xlsx", "ppt", "pptx", "db", "sqlite", "mdb", "iso", "dmg", "class",
        ];

        // Function to determine if a file is a text file
        fn is_text_file(path: &Path, binary_extensions: &[&str]) -> bool {
            // 1. First check extensions that are clearly binary
            if let Some(ext) = path.extension() {
                let ext_str = ext.to_string_lossy().to_lowercase();
                if binary_extensions.iter().any(|&bin_ext| bin_ext == ext_str) {
                    return false;
                }
            }

            // 2. Read the beginning of the file and determine if it is binary
            match fs::read(path) {
                Ok(bytes) if !bytes.is_empty() => {
                    // Sample size (read up to 8KB)
                    let sample_size = std::cmp::min(bytes.len(), 8192);
                    let sample = &bytes[..sample_size];

                    // Detect binary characteristics
                    // 1. Detect NULL bytes (text files do not have NULL bytes)
                    if sample.iter().any(|&b| b == 0) {
                        return false;
                    }

                    // 2. Check the ratio of control characters
                    let control_chars_count = sample
                        .iter()
                        .filter(|&&b| {
                            b < 32 && b != 9 && b != 10 && b != 13 // Exclude Tab, LF, CR
                        })
                        .count();

                    // If the ratio of control characters is too high, consider it binary
                    if (control_chars_count as f32 / sample_size as f32) > 0.3 {
                        return false;
                    }

                    // 3. Check if it is valid UTF-8
                    let is_valid_utf8 = std::str::from_utf8(sample).is_ok();

                    // 4. Check the ASCII ratio
                    let ascii_ratio =
                        sample.iter().filter(|&&b| b <= 127).count() as f32 / sample_size as f32;

                    // Valid UTF-8 with a high ASCII ratio, or specific non-UTF-8 encoding characteristics
                    is_valid_utf8 || ascii_ratio > 0.8
                }
                _ => false, // Do not consider files with read errors or size 0 as text
            }
        }

        // Function to recursively process directory entries
        fn process_directory(
            dir_path: &Path,
            index_writer: &mut tantivy::IndexWriter,
            file_schema: &FileSchema,
            cache: &MetadataCache,
            binary_extensions: &[&str],
            indexed_files_count: &mut usize,
            reindexed_files_count: &mut usize,
            unchanged_files_count: &mut usize,
            found_files_count: &mut usize,
            skipped_files_count: &mut usize,
        ) -> Result<(), String> {
            for entry in fs::read_dir(dir_path)
                .map_err(|e| format!("Directory read error '{}': {}", dir_path.display(), e))?
            {
                let entry = entry.map_err(|e| format!("Entry read error: {}", e))?;
                let path = entry.path();

                if path.is_dir() {
                    // Recursively process subdirectories (add depth limit if needed)
                    process_directory(
                        &path,
                        index_writer,
                        file_schema,
                        cache,
                        binary_extensions,
                        indexed_files_count,
                        reindexed_files_count,
                        unchanged_files_count,
                        found_files_count,
                        skipped_files_count,
                    )?;
                } else if path.is_file() {
                    *found_files_count += 1;

                    // More universal text file determination
                    if is_text_file(&path, binary_extensions) {
                        match fs::read_to_string(&path) {
                            Ok(content) => {
                                if !content.trim().is_empty() {
                                    let file_path_str = path.to_string_lossy().to_string();

                                    // Check if file has changed using metadata cache
                                    let has_changed = cache.has_changed(&file_path_str, &content)
                                        .map_err(|e| format!("Cache check error: {}", e))?;

                                    if !has_changed {
                                        // File unchanged, skip indexing
                                        *unchanged_files_count += 1;
                                        tracing::debug!("Skipped (unchanged): {}", path.display());
                                    } else {
                                        // File is new or changed, index it
                                        // Check if file was in cache (for reindex tracking)
                                        let was_cached = cache.get(&file_path_str)
                                            .map_err(|e| format!("Cache get error: {}", e))?
                                            .is_some();

                                        // Get file metadata
                                        let metadata = fs::metadata(&path)
                                            .map_err(|e| format!("Metadata error: {}", e))?;
                                        let last_modified = metadata.modified()
                                            .map_err(|e| format!("Modified time error: {}", e))?
                                            .duration_since(std::time::UNIX_EPOCH)
                                            .map_err(|e| format!("Time conversion error: {}", e))?
                                            .as_secs();

                                        // Create FileMetadata for cache
                                        let file_meta = FileMetadata::from_content(
                                            &content,
                                            last_modified,
                                            metadata.len()
                                        );

                                        // Add document to index
                                        index_writer
                                            .add_document(doc!(
                                                file_schema.relative_path => file_path_str.clone(),
                                                file_schema.content => content,
                                                file_schema.unique_hash => file_meta.hash.clone(),
                                                file_schema.last_modified => file_meta.last_modified,
                                                file_schema.file_size => file_meta.size,
                                            ))
                                            .map_err(|e| format!("Document addition error: {}", e))?;

                                        // Update cache
                                        cache.set(&file_path_str, &file_meta)
                                            .map_err(|e| format!("Cache update error: {}", e))?;

                                        *indexed_files_count += 1;

                                        // Track if this was a reindex or new file
                                        if was_cached {
                                            *reindexed_files_count += 1;
                                            tracing::debug!("Reindexed (changed): {}", path.display());
                                        } else {
                                            tracing::debug!("Indexed (new): {}", path.display());
                                        }
                                    }
                                } else {
                                    *skipped_files_count += 1;
                                    tracing::debug!("Skipped (empty file): {}", path.display());
                                }
                            }
                            Err(e) => {
                                // Skip and continue on read errors
                                *skipped_files_count += 1;
                                tracing::debug!("Skipped (read error): {} - {}", path.display(), e);
                            }
                        }
                    } else {
                        *skipped_files_count += 1;
                        tracing::debug!("Skipped (non-text): {}", path.display());
                    }
                }
            }
            Ok(())
        }

        // Execute directory processing
        tracing::info!("Target directory for search: {}", dir_path.display());
        process_directory(
            dir_path,
            &mut index_writer,
            &file_schema,
            &cache,
            &binary_extensions,
            &mut indexed_files_count,
            &mut reindexed_files_count,
            &mut unchanged_files_count,
            &mut found_files_count,
            &mut skipped_files_count,
        )?;

        tracing::info!(
            "Processing complete: Found={}, New/Changed={}, Reindexed={}, Unchanged={}, Skipped={}",
            found_files_count,
            indexed_files_count,
            reindexed_files_count,
            unchanged_files_count,
            skipped_files_count
        );

        // Return an error if no files were indexed
        if indexed_files_count == 0 {
            return Ok(format!(
                "No text files suitable for indexing were found in the specified directory '{}'.\nFound files: {}, Skipped: {}\nSupported extensions: {:?}",
                params.directory, found_files_count, skipped_files_count, binary_extensions
            ));
        }

        // 5. Commit the index
        index_writer
            .commit()
            .map_err(|e| format!("Commit error: {}", e))?;

        // 6. Generate reader and searcher for searching
        let reader = index.reader().map_err(|e| e.to_string())?;
        let searcher = reader.searcher();

        // 7. Parse query containing the keyword
        let query_parser = QueryParser::for_index(&index, vec![file_schema.content]);

        // Ensure the keyword is not empty
        if params.keyword.trim().is_empty() {
            return Err("Search keyword is empty. Please enter a valid keyword.".into());
        }

        let query = query_parser
            .parse_query(&params.keyword)
            .map_err(|e| format!("Query parse error: {}", e))?;

        // 8. Retrieve top 10 search results
        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(10))
            .map_err(|e| format!("Search error: {}", e))?;

        // 9. Concatenate file paths from search results into a string
        let mut result_str = String::new();
        for (score, doc_address) in &top_docs {
            let retrieved_doc: TantivyDocument =
                searcher.doc(*doc_address).map_err(|e| e.to_string())?;
            let path_value = retrieved_doc
                .get_first(file_schema.relative_path)
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown path");
            result_str.push_str(&format!("Hit: {} (Score: {:.2})\n", path_value, score));
        }

        if result_str.is_empty() {
            Ok(format!(
                "No search results for keyword '{}'. Number of indexed files: {}",
                params.keyword, indexed_files_count
            ))
        } else {
            Ok(format!(
                "Search results ({} hits):\n{}",
                top_docs.len(),
                result_str
            ))
        }
    }

    /// Find the definition of a symbol in Rust code
    #[tool(description = "Find where a Rust symbol (function, struct, trait, etc.) is defined")]
    async fn find_definition(
        &self,
        #[tool(aggr)] params: FindDefinitionParams,
    ) -> Result<String, String> {
        let dir_path = Path::new(&params.directory);
        if !dir_path.is_dir() {
            return Err(format!(
                "The specified path '{}' is not a directory",
                params.directory
            ));
        }

        tracing::debug!("Searching for definition of '{}'", params.symbol_name);

        let mut found_definitions = Vec::new();
        let mut parser = RustParser::new().map_err(|e| format!("Parser initialization error: {}", e))?;

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

        visit_rust_files(dir_path, &params.symbol_name, &mut parser, &mut found_definitions)?;

        if found_definitions.is_empty() {
            Ok(format!(
                "No definition found for symbol '{}'",
                params.symbol_name
            ))
        } else {
            let mut result = format!(
                "Found {} definition(s) for '{}':\n",
                found_definitions.len(),
                params.symbol_name
            );
            for (path, line, kind) in found_definitions {
                result.push_str(&format!(
                    "- {}:{} ({})\n",
                    path.display(),
                    line,
                    kind
                ));
            }
            Ok(result)
        }
    }

    /// Find all references to a symbol in the codebase
    #[tool(description = "Find all places where a symbol is referenced or called")]
    async fn find_references(
        &self,
        #[tool(aggr)] params: FindReferencesParams,
    ) -> Result<String, String> {
        let dir_path = Path::new(&params.directory);
        if !dir_path.is_dir() {
            return Err(format!(
                "The specified path '{}' is not a directory",
                params.directory
            ));
        }

        tracing::debug!("Searching for references to '{}'", params.symbol_name);

        let mut found_references = Vec::new();
        let mut parser = RustParser::new().map_err(|e| format!("Parser initialization error: {}", e))?;

        // Recursively search .rs files for references in call graphs
        fn visit_rust_files(
            dir: &Path,
            symbol_name: &str,
            parser: &mut RustParser,
            found: &mut Vec<(PathBuf, Vec<String>)>,
        ) -> Result<(), String> {
            for entry in fs::read_dir(dir).map_err(|e| format!("Directory read error: {}", e))? {
                let entry = entry.map_err(|e| format!("Entry read error: {}", e))?;
                let path = entry.path();

                if path.is_dir() {
                    visit_rust_files(&path, symbol_name, parser, found)?;
                } else if path.extension().and_then(|s| s.to_str()) == Some("rs") {
                    if let Ok(parse_result) = parser.parse_file_complete(&path) {
                        let callers = parse_result.call_graph.get_callers(symbol_name);
                        if !callers.is_empty() {
                            found.push((
                                path.clone(),
                                callers.into_iter().map(String::from).collect(),
                            ));
                        }
                    }
                }
            }
            Ok(())
        }

        visit_rust_files(dir_path, &params.symbol_name, &mut parser, &mut found_references)?;

        if found_references.is_empty() {
            Ok(format!(
                "No references found for symbol '{}'",
                params.symbol_name
            ))
        } else {
            let total_refs: usize = found_references.iter().map(|(_, callers)| callers.len()).sum();
            let mut result = format!(
                "Found {} reference(s) to '{}' in {} file(s):\n",
                total_refs,
                params.symbol_name,
                found_references.len()
            );
            for (path, callers) in found_references {
                result.push_str(&format!(
                    "- {} (called by: {})\n",
                    path.display(),
                    callers.join(", ")
                ));
            }
            Ok(result)
        }
    }

    /// Get dependencies for a file (imports and files that depend on it)
    #[tool(description = "Get import dependencies for a Rust source file")]
    async fn get_dependencies(
        &self,
        #[tool(aggr)] params: GetDependenciesParams,
    ) -> Result<String, String> {
        let file_path = Path::new(&params.file_path);

        if !file_path.exists() {
            return Err(format!("File '{}' does not exist", params.file_path));
        }

        if !file_path.is_file() {
            return Err(format!("'{}' is not a file", params.file_path));
        }

        let mut parser = RustParser::new().map_err(|e| format!("Parser initialization error: {}", e))?;
        let parse_result = parser
            .parse_file_complete(file_path)
            .map_err(|e| format!("Parse error: {}", e))?;

        let mut result = format!("Dependencies for '{}':\n\n", file_path.display());

        // List imports
        if parse_result.imports.is_empty() {
            result.push_str("No imports found.\n");
        } else {
            result.push_str(&format!("Imports ({}):\n", parse_result.imports.len()));
            for import in &parse_result.imports {
                result.push_str(&format!("- {}\n", import.path));
            }
        }

        Ok(result)
    }

    /// Get call graph for a file or specific symbol
    #[tool(description = "Get the call graph showing function call relationships")]
    async fn get_call_graph(
        &self,
        #[tool(aggr)] params: GetCallGraphParams,
    ) -> Result<String, String> {
        let file_path = Path::new(&params.file_path);

        if !file_path.exists() {
            return Err(format!("File '{}' does not exist", params.file_path));
        }

        if !file_path.is_file() {
            return Err(format!("'{}' is not a file", params.file_path));
        }

        let mut parser = RustParser::new().map_err(|e| format!("Parser initialization error: {}", e))?;
        let parse_result = parser
            .parse_file_complete(file_path)
            .map_err(|e| format!("Parse error: {}", e))?;

        let mut result = format!("Call graph for '{}':\n\n", file_path.display());

        if let Some(ref symbol_name) = params.symbol_name {
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

        Ok(result)
    }

    /// Analyze code complexity metrics for a file
    #[tool(description = "Analyze code complexity metrics (LOC, cyclomatic complexity, function count)")]
    async fn analyze_complexity(
        &self,
        #[tool(aggr)] params: AnalyzeComplexityParams,
    ) -> Result<String, String> {
        let file_path = Path::new(&params.file_path);

        if !file_path.exists() {
            return Err(format!("File '{}' does not exist", params.file_path));
        }

        if !file_path.is_file() {
            return Err(format!("'{}' is not a file", params.file_path));
        }

        // Read source file
        let source = fs::read_to_string(file_path)
            .map_err(|e| format!("Failed to read file: {}", e))?;

        // Parse file to get symbols
        let mut parser = RustParser::new()
            .map_err(|e| format!("Parser initialization error: {}", e))?;
        let parse_result = parser
            .parse_file_complete(file_path)
            .map_err(|e| format!("Parse error: {}", e))?;

        // Calculate metrics
        let lines_of_code = source.lines().count();
        let non_empty_loc = source.lines().filter(|l| !l.trim().is_empty()).count();
        let comment_lines = source.lines().filter(|l| l.trim().starts_with("//")).count();
        let function_count = parse_result
            .symbols
            .iter()
            .filter(|s| matches!(s.kind, file_search_mcp::parser::SymbolKind::Function { .. }))
            .count();
        let struct_count = parse_result
            .symbols
            .iter()
            .filter(|s| matches!(s.kind, file_search_mcp::parser::SymbolKind::Struct))
            .count();
        let trait_count = parse_result
            .symbols
            .iter()
            .filter(|s| matches!(s.kind, file_search_mcp::parser::SymbolKind::Trait))
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

        let mut result = format!("Complexity analysis for '{}':\n\n", file_path.display());
        result.push_str("=== Code Metrics ===\n");
        result.push_str(&format!("Total lines:           {}\n", lines_of_code));
        result.push_str(&format!("Non-empty lines:       {}\n", non_empty_loc));
        result.push_str(&format!("Comment lines:         {}\n", comment_lines));
        result.push_str(&format!("Code lines (approx):   {}\n\n", non_empty_loc - comment_lines));

        result.push_str("=== Symbol Counts ===\n");
        result.push_str(&format!("Functions:             {}\n", function_count));
        result.push_str(&format!("Structs:               {}\n", struct_count));
        result.push_str(&format!("Traits:                {}\n\n", trait_count));

        result.push_str("=== Complexity ===\n");
        result.push_str(&format!("Total cyclomatic:      {}\n", cyclomatic_complexity));
        result.push_str(&format!("Avg per function:      {:.2}\n", avg_complexity));

        // Add call graph complexity
        let edge_count = parse_result.call_graph.edge_count();
        result.push_str(&format!("Function calls:        {}\n", edge_count));

        Ok(result)
    }

    /// Find semantically similar code using vector search
    #[tool(description = "Find code snippets semantically similar to a query using embeddings")]
    async fn get_similar_code(
        &self,
        #[tool(aggr)] params: GetSimilarCodeParams,
    ) -> Result<String, String> {
        let dir_path = Path::new(&params.directory);
        if !dir_path.is_dir() {
            return Err(format!(
                "The specified path '{}' is not a directory",
                params.directory
            ));
        }

        let limit = params.limit.unwrap_or(5);

        tracing::debug!("Searching for similar code to: {}", params.query);

        // Initialize components
        let embedding_generator = EmbeddingGenerator::new()
            .map_err(|e| format!("Failed to initialize embedding generator: {}", e))?;

        let vector_store_config = VectorStoreConfig::default();
        let vector_store = VectorStore::new(vector_store_config)
            .await
            .map_err(|e| format!("Failed to initialize vector store: {}", e))?;

        // Create hybrid search (vector-only mode)
        let hybrid_search = HybridSearch::with_defaults(
            embedding_generator,
            vector_store,
            None, // No BM25 for this tool
        );

        // Perform vector search
        let results = hybrid_search
            .vector_only_search(&params.query, limit)
            .await
            .map_err(|e| format!("Search error: {}", e))?;

        if results.is_empty() {
            return Ok(format!(
                "No similar code found for query: '{}'",
                params.query
            ));
        }

        let mut result = format!(
            "Found {} similar code snippet(s) for query '{}':\n\n",
            results.len(),
            params.query
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
            result.push_str(&format!("   Lines: {}-{}\n", chunk.context.line_start, chunk.context.line_end));
            if let Some(ref doc) = chunk.context.docstring {
                result.push_str(&format!("   Doc: {}\n", doc));
            }
            result.push_str(&format!("   Code preview:\n   {}\n\n",
                chunk.content.lines().take(3).collect::<Vec<_>>().join("\n   ")
            ));
        }

        Ok(result)
    }
}

#[tool(tool_box)]
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
                "This server provides code search and analysis tools: 1) search - keyword search in files, 2) read_file_content - read file contents, 3) find_definition - locate symbol definitions, 4) find_references - find symbol references, 5) get_dependencies - analyze imports, 6) get_call_graph - show function call relationships, 7) analyze_complexity - calculate code metrics, 8) get_similar_code - semantic similarity search"
                    .into(),
            ),
        }
    }
}

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
                "This server provides two tools: 1) Search for keywords in text files within a directory, 2) Read and display the content of a specific file."
                    .into(),
            ),
        }
    }
}

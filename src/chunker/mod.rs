//! Semantic code chunking with context enrichment
//!
//! Chunks code by symbols (functions, structs, etc.) and adds rich context
//! for better embedding and retrieval quality.

use crate::parser::{CallGraph, Import, ParseResult, Symbol, SymbolKind};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A unique identifier for a code chunk
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChunkId(Uuid);

impl ChunkId {
    /// Create a new random chunk ID
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Convert to string representation
    pub fn to_string(&self) -> String {
        self.0.to_string()
    }
}

impl Default for ChunkId {
    fn default() -> Self {
        Self::new()
    }
}

/// Context information for a code chunk
#[derive(Debug, Clone)]
pub struct ChunkContext {
    /// File path
    pub file_path: PathBuf,
    /// Module path (e.g., ["crate", "parser", "mod"])
    pub module_path: Vec<String>,
    /// Symbol that this chunk represents
    pub symbol_name: String,
    /// Kind of symbol
    pub symbol_kind: String,
    /// Documentation string
    pub docstring: Option<String>,
    /// Import statements in the file
    pub imports: Vec<String>,
    /// Functions this symbol calls
    pub outgoing_calls: Vec<String>,
    /// Line range in source file
    pub line_start: usize,
    pub line_end: usize,
}

/// A code chunk with content and context
#[derive(Debug, Clone)]
pub struct CodeChunk {
    /// Unique identifier
    pub id: ChunkId,
    /// The code content
    pub content: String,
    /// Rich context for this chunk
    pub context: ChunkContext,
    /// Overlap from previous chunk (for continuity)
    pub overlap_prev: Option<String>,
    /// Overlap to next chunk (for continuity)
    pub overlap_next: Option<String>,
}

impl CodeChunk {
    /// Format chunk for embedding using contextual retrieval approach
    ///
    /// This follows Anthropic's contextual retrieval pattern, which reduces
    /// retrieval errors by up to 49% by adding context to each chunk.
    pub fn format_for_embedding(&self) -> String {
        let mut parts = Vec::new();

        // File and location context
        parts.push(format!("// File: {}", self.context.file_path.display()));
        parts.push(format!(
            "// Location: lines {}-{}",
            self.context.line_start, self.context.line_end
        ));

        // Module context
        if !self.context.module_path.is_empty() {
            parts.push(format!(
                "// Module: {}",
                self.context.module_path.join("::")
            ));
        }

        // Symbol context
        parts.push(format!(
            "// Symbol: {} ({})",
            self.context.symbol_name, self.context.symbol_kind
        ));

        // Documentation if available
        if let Some(ref doc) = self.context.docstring {
            parts.push(format!("// Purpose: {}", doc));
        }

        // Import context (first 5 imports)
        if !self.context.imports.is_empty() {
            let imports_str = self
                .context
                .imports
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("// Imports: {}", imports_str));
        }

        // Call context (first 5 calls)
        if !self.context.outgoing_calls.is_empty() {
            let calls_str = self
                .context
                .outgoing_calls
                .iter()
                .take(5)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("// Calls: {}", calls_str));
        }

        // Add separator
        parts.push(String::new());

        // Add the actual code content
        parts.push(self.content.clone());

        parts.join("\n")
    }
}

/// Code chunker that splits by symbols and adds context
pub struct Chunker {
    /// Overlap percentage (0.0 to 1.0)
    overlap_percentage: f64,
}

impl Chunker {
    /// Create a new chunker with default settings
    pub fn new() -> Self {
        Self {
            overlap_percentage: 0.2, // 20% overlap
        }
    }

    /// Create a chunker with custom overlap percentage
    pub fn with_overlap(overlap_percentage: f64) -> Self {
        Self {
            overlap_percentage: overlap_percentage.clamp(0.0, 0.5),
        }
    }

    /// Chunk a file based on its parse result
    pub fn chunk_file(
        &self,
        file_path: &Path,
        source: &str,
        parse_result: &ParseResult,
    ) -> Result<Vec<CodeChunk>, Box<dyn std::error::Error>> {
        let mut chunks = Vec::new();

        // Get module path from file path
        let module_path = self.extract_module_path(file_path);

        // Convert imports to strings
        let import_strings: Vec<String> = parse_result
            .imports
            .iter()
            .map(|i| i.path.clone())
            .collect();

        // Process each symbol
        for symbol in &parse_result.symbols {
            // Extract the code for this symbol
            let code = self.extract_symbol_code(source, symbol)?;

            // Get outgoing calls for this symbol
            let outgoing_calls = parse_result
                .call_graph
                .get_callees(&symbol.name)
                .into_iter()
                .map(String::from)
                .collect();

            // Create chunk context
            let context = ChunkContext {
                file_path: file_path.to_path_buf(),
                module_path: module_path.clone(),
                symbol_name: symbol.name.clone(),
                symbol_kind: symbol.kind.as_str().to_string(),
                docstring: symbol.docstring.clone(),
                imports: import_strings.clone(),
                outgoing_calls,
                line_start: symbol.range.start_line,
                line_end: symbol.range.end_line,
            };

            // Create the chunk
            let chunk = CodeChunk {
                id: ChunkId::new(),
                content: code,
                context,
                overlap_prev: None, // Will be filled in later
                overlap_next: None, // Will be filled in later
            };

            chunks.push(chunk);
        }

        // Add overlap between adjacent chunks
        self.add_overlap(&mut chunks);

        Ok(chunks)
    }

    /// Extract code for a specific symbol from source
    fn extract_symbol_code(
        &self,
        source: &str,
        symbol: &Symbol,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let lines: Vec<&str> = source.lines().collect();

        // Convert to 0-indexed
        let start = symbol.range.start_line.saturating_sub(1);
        let end = symbol.range.end_line;

        if start >= lines.len() {
            return Ok(String::new());
        }

        let end = end.min(lines.len());
        let symbol_lines = &lines[start..end];

        Ok(symbol_lines.join("\n"))
    }

    /// Extract module path from file path
    fn extract_module_path(&self, file_path: &Path) -> Vec<String> {
        let mut parts = Vec::new();

        // Try to extract from "src/" onwards
        let mut found_src = false;
        for component in file_path.components() {
            if let Some(name) = component.as_os_str().to_str() {
                if name == "src" {
                    found_src = true;
                    parts.push("crate".to_string());
                    continue;
                }

                if found_src {
                    // Remove .rs extension
                    let clean_name = name.strip_suffix(".rs").unwrap_or(name);
                    if clean_name != "mod" {
                        parts.push(clean_name.to_string());
                    }
                }
            }
        }

        if parts.is_empty() {
            // Fallback: use filename
            if let Some(name) = file_path.file_stem().and_then(|s| s.to_str()) {
                parts.push(name.to_string());
            }
        }

        parts
    }

    /// Add overlap between adjacent chunks
    fn add_overlap(&self, chunks: &mut [CodeChunk]) {
        for i in 0..chunks.len() {
            // Add overlap from previous chunk
            if i > 0 {
                let overlap = self.calculate_overlap(&chunks[i - 1].content, false);
                chunks[i].overlap_prev = overlap;
            }

            // Add overlap to next chunk
            if i < chunks.len() - 1 {
                let overlap = self.calculate_overlap(&chunks[i].content, true);
                chunks[i].overlap_next = overlap;
            }
        }
    }

    /// Calculate overlap text from a chunk
    /// If `from_end` is true, take from the end; otherwise from the beginning
    fn calculate_overlap(&self, content: &str, from_end: bool) -> Option<String> {
        let lines: Vec<&str> = content.lines().collect();
        let overlap_lines = ((lines.len() as f64) * self.overlap_percentage).ceil() as usize;

        if overlap_lines == 0 || lines.is_empty() {
            return None;
        }

        let overlap = if from_end {
            // Take last N lines for next chunk
            &lines[lines.len().saturating_sub(overlap_lines)..]
        } else {
            // Take first N lines from previous chunk
            &lines[..overlap_lines.min(lines.len())]
        };

        Some(overlap.join("\n"))
    }
}

impl Default for Chunker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::RustParser;

    #[test]
    fn test_chunk_creation() {
        let chunk = CodeChunk {
            id: ChunkId::new(),
            content: "fn test() {}".to_string(),
            context: ChunkContext {
                file_path: PathBuf::from("test.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: "test".to_string(),
                symbol_kind: "function".to_string(),
                docstring: None,
                imports: vec![],
                outgoing_calls: vec![],
                line_start: 1,
                line_end: 1,
            },
            overlap_prev: None,
            overlap_next: None,
        };

        assert_eq!(chunk.context.symbol_name, "test");
        assert_eq!(chunk.context.symbol_kind, "function");
    }

    #[test]
    fn test_format_for_embedding() {
        let chunk = CodeChunk {
            id: ChunkId::new(),
            content: "fn test() {\n    println!(\"hello\");\n}".to_string(),
            context: ChunkContext {
                file_path: PathBuf::from("src/main.rs"),
                module_path: vec!["crate".to_string(), "main".to_string()],
                symbol_name: "test".to_string(),
                symbol_kind: "function".to_string(),
                docstring: Some("A test function".to_string()),
                imports: vec!["std::io".to_string()],
                outgoing_calls: vec!["println".to_string()],
                line_start: 10,
                line_end: 12,
            },
            overlap_prev: None,
            overlap_next: None,
        };

        let formatted = chunk.format_for_embedding();

        assert!(formatted.contains("File: src/main.rs"));
        assert!(formatted.contains("lines 10-12"));
        assert!(formatted.contains("Module: crate::main"));
        assert!(formatted.contains("Symbol: test (function)"));
        assert!(formatted.contains("Purpose: A test function"));
        assert!(formatted.contains("Imports: std::io"));
        assert!(formatted.contains("Calls: println"));
        assert!(formatted.contains("fn test()"));
    }

    #[test]
    fn test_chunker_creation() {
        let chunker = Chunker::new();
        assert_eq!(chunker.overlap_percentage, 0.2);

        let chunker = Chunker::with_overlap(0.3);
        assert_eq!(chunker.overlap_percentage, 0.3);

        // Test clamping
        let chunker = Chunker::with_overlap(0.8);
        assert_eq!(chunker.overlap_percentage, 0.5);
    }

    #[test]
    fn test_extract_module_path() {
        let chunker = Chunker::new();

        let path = PathBuf::from("/home/user/project/src/parser/mod.rs");
        let module_path = chunker.extract_module_path(&path);
        assert_eq!(module_path, vec!["crate", "parser"]);

        let path = PathBuf::from("/home/user/project/src/lib.rs");
        let module_path = chunker.extract_module_path(&path);
        assert_eq!(module_path, vec!["crate", "lib"]);
    }

    #[test]
    fn test_chunk_file() {
        let source = r#"
use std::collections::HashMap;

/// A test function
fn test_function() {
    helper();
}

fn helper() {
    println!("help");
}
        "#;

        let mut parser = RustParser::new().unwrap();
        let parse_result = parser.parse_source_complete(source).unwrap();

        let chunker = Chunker::new();
        let chunks = chunker
            .chunk_file(Path::new("test.rs"), source, &parse_result)
            .unwrap();

        // Should have chunks for both functions
        assert!(chunks.len() >= 2, "Expected at least 2 chunks, got {}", chunks.len());

        // Find the test_function chunk
        let test_chunk = chunks.iter().find(|c| c.context.symbol_name == "test_function");
        assert!(test_chunk.is_some(), "Should find test_function chunk");

        let test_chunk = test_chunk.unwrap();
        assert_eq!(test_chunk.context.symbol_kind, "function");
        assert!(test_chunk.context.docstring.is_some());
        assert!(test_chunk.context.imports.contains(&"std::collections::HashMap".to_string()));
        assert!(test_chunk.context.outgoing_calls.contains(&"helper".to_string()));
    }

    #[test]
    fn test_overlap() {
        let source = r#"
fn first() {
    line1();
    line2();
    line3();
}

fn second() {
    other();
}
        "#;

        let mut parser = RustParser::new().unwrap();
        let parse_result = parser.parse_source_complete(source).unwrap();

        let chunker = Chunker::with_overlap(0.2);
        let chunks = chunker
            .chunk_file(Path::new("test.rs"), source, &parse_result)
            .unwrap();

        // Check that chunks have overlap
        if chunks.len() >= 2 {
            // First chunk should have overlap_next
            assert!(chunks[0].overlap_next.is_some(), "First chunk should have overlap_next");

            // Second chunk should have overlap_prev
            assert!(chunks[1].overlap_prev.is_some(), "Second chunk should have overlap_prev");
        }
    }
}

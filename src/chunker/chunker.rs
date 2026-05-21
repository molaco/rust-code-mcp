//! Core `Chunker` engine: file/symbol chunking with overlap and module-path extraction.

use std::path::Path;

use crate::parser::{ParseResult, Symbol};

use super::types::{ChunkContext, ChunkId, CodeChunk};

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
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
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
    pub(super) fn add_overlap(&self, chunks: &mut [CodeChunk]) {
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
    use std::path::PathBuf;

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

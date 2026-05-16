//! Semantic code chunking with context enrichment
//!
//! Chunks code by symbols (functions, structs, etc.) and adds rich context
//! for better embedding and retrieval quality.

use crate::parser::{ParseResult, Symbol};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use uuid::Uuid;

/// A unique identifier for a code chunk
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

    /// Parse from string representation
    pub fn from_string(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Default for ChunkId {
    fn default() -> Self {
        Self::new()
    }
}

/// Context information for a code chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Parent symbol omitted or split to create this smaller chunk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_symbol_name: Option<String>,
    /// One-based part number when a single symbol is line-split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_part: Option<usize>,
    /// Total number of parts when a single symbol is line-split.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split_total: Option<usize>,
    /// Line range in source file
    pub line_start: usize,
    pub line_end: usize,
}

/// A code chunk with content and context
#[derive(Debug, Clone, Serialize, Deserialize)]
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

        if let Some(ref parent) = self.context.parent_symbol_name {
            parts.push(format!("// Parent: {}", parent));
        }

        if let (Some(part), Some(total)) =
            (self.context.split_part, self.context.split_total)
        {
            parts.push(format!("// Chunk part: {}/{}", part, total));
        }

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

/// Token limits for splitting oversized chunks before embedding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkSplitConfig {
    /// Preferred upper bound for formatted embedding text.
    pub target_tokens: usize,
    /// Hard upper bound; single-line chunks may still exceed this.
    pub hard_max_tokens: usize,
}

impl ChunkSplitConfig {
    pub fn new(target_tokens: usize, hard_max_tokens: usize) -> Self {
        let target_tokens = target_tokens.max(1);
        Self {
            target_tokens,
            hard_max_tokens: hard_max_tokens.max(target_tokens),
        }
    }
}

impl Default for ChunkSplitConfig {
    fn default() -> Self {
        Self::new(768, 1024)
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

    /// Split or elide chunks whose formatted embedding text exceeds the target.
    ///
    /// Container chunks (`impl`, `module`, `trait`) are dropped when they are
    /// oversized and their child chunks already cover the useful semantic
    /// boundaries. Oversized leaf chunks are split by line ranges as a fallback.
    pub fn split_oversized_chunks<F>(
        &self,
        chunks: Vec<CodeChunk>,
        config: ChunkSplitConfig,
        token_count: F,
    ) -> Vec<CodeChunk>
    where
        F: Fn(&CodeChunk) -> Option<usize>,
    {
        if chunks.is_empty() {
            return chunks;
        }

        let token_counts: Vec<usize> = chunks
            .iter()
            .map(|chunk| token_count_or_estimate(chunk, &token_count))
            .collect();
        let mut skip_container = vec![false; chunks.len()];

        for (idx, chunk) in chunks.iter().enumerate() {
            if token_counts[idx] <= config.target_tokens
                || !is_container_kind(&chunk.context.symbol_kind)
            {
                continue;
            }

            let has_child = chunks.iter().enumerate().any(|(child_idx, child)| {
                child_idx != idx && strictly_contains(chunk, child)
            });
            if has_child {
                skip_container[idx] = true;
            }
        }

        let mut output = Vec::new();
        for (idx, chunk) in chunks.iter().enumerate() {
            if skip_container[idx] {
                continue;
            }

            let mut chunk = chunk.clone();
            if let Some(parent_idx) = nearest_skipped_parent(idx, &chunks, &skip_container) {
                chunk.context.parent_symbol_name =
                    Some(chunks[parent_idx].context.symbol_name.clone());
            }

            let count = token_count_or_estimate(&chunk, &token_count);
            if count > config.target_tokens {
                output.extend(self.split_leaf_chunk(chunk, config, &token_count));
            } else {
                output.push(chunk);
            }
        }

        for chunk in &mut output {
            chunk.overlap_prev = None;
            chunk.overlap_next = None;
        }
        self.add_overlap(&mut output);

        output
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

    fn split_leaf_chunk<F>(
        &self,
        chunk: CodeChunk,
        config: ChunkSplitConfig,
        token_count: &F,
    ) -> Vec<CodeChunk>
    where
        F: Fn(&CodeChunk) -> Option<usize>,
    {
        let lines: Vec<&str> = chunk.content.lines().collect();
        if lines.len() <= 1 {
            return vec![chunk];
        }

        let mut parts = Vec::new();
        let mut part_start = 0usize;
        let mut current_lines: Vec<&str> = Vec::new();

        for (line_idx, line) in lines.iter().enumerate() {
            let mut candidate_lines = current_lines.clone();
            candidate_lines.push(*line);
            let candidate = make_part_chunk(
                &chunk,
                part_start,
                line_idx + 1,
                &candidate_lines.join("\n"),
            );
            let candidate_tokens = token_count_or_estimate(&candidate, token_count);

            if !current_lines.is_empty() && candidate_tokens > config.target_tokens {
                let content = current_lines.join("\n");
                parts.push(make_part_chunk(&chunk, part_start, line_idx, &content));
                part_start = line_idx;
                current_lines.clear();
            }

            current_lines.push(*line);
        }

        if !current_lines.is_empty() {
            let content = current_lines.join("\n");
            parts.push(make_part_chunk(&chunk, part_start, lines.len(), &content));
        }

        if parts.len() <= 1 {
            return vec![chunk];
        }

        let total = parts.len();
        for (idx, part) in parts.iter_mut().enumerate() {
            part.context.split_part = Some(idx + 1);
            part.context.split_total = Some(total);
        }

        parts
    }
}

fn make_part_chunk(
    source: &CodeChunk,
    start_line_offset: usize,
    end_line_offset: usize,
    content: &str,
) -> CodeChunk {
    let mut part = source.clone();
    part.id = ChunkId::new();
    part.content = content.to_string();
    part.context.line_start = source.context.line_start + start_line_offset;
    part.context.line_end = source.context.line_start + end_line_offset.saturating_sub(1);
    part.overlap_prev = None;
    part.overlap_next = None;
    part
}

fn token_count_or_estimate<F>(chunk: &CodeChunk, token_count: &F) -> usize
where
    F: Fn(&CodeChunk) -> Option<usize>,
{
    token_count(chunk)
        .unwrap_or_else(|| chunk.format_for_embedding().len().div_ceil(4))
        .max(1)
}

fn is_container_kind(symbol_kind: &str) -> bool {
    matches!(symbol_kind, "impl" | "module" | "trait")
}

fn strictly_contains(parent: &CodeChunk, child: &CodeChunk) -> bool {
    parent.context.file_path == child.context.file_path
        && parent.context.line_start <= child.context.line_start
        && parent.context.line_end >= child.context.line_end
        && (parent.context.line_start < child.context.line_start
            || parent.context.line_end > child.context.line_end)
}

fn nearest_skipped_parent(
    idx: usize,
    chunks: &[CodeChunk],
    skip_container: &[bool],
) -> Option<usize> {
    chunks
        .iter()
        .enumerate()
        .filter(|(parent_idx, parent)| {
            skip_container[*parent_idx] && *parent_idx != idx && strictly_contains(parent, &chunks[idx])
        })
        .min_by_key(|(_, parent)| parent.context.line_end - parent.context.line_start)
        .map(|(parent_idx, _)| parent_idx)
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
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
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
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
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

    fn synthetic_chunk(name: &str, kind: &str, start: usize, end: usize) -> CodeChunk {
        let content = (start..=end)
            .map(|line| format!("line_{line}();"))
            .collect::<Vec<_>>()
            .join("\n");
        CodeChunk {
            id: ChunkId::new(),
            content,
            context: ChunkContext {
                file_path: PathBuf::from("src/lib.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: name.to_string(),
                symbol_kind: kind.to_string(),
                docstring: None,
                imports: vec![],
                outgoing_calls: vec![],
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
                line_start: start,
                line_end: end,
            },
            overlap_prev: None,
            overlap_next: None,
        }
    }

    #[test]
    fn test_split_oversized_container_uses_child_chunks() {
        let chunker = Chunker::with_overlap(0.0);
        let parent = synthetic_chunk("impl Foo", "impl", 1, 12);
        let child_a = synthetic_chunk("a", "function", 2, 4);
        let child_b = synthetic_chunk("b", "function", 6, 8);

        let chunks = chunker.split_oversized_chunks(
            vec![parent, child_a, child_b],
            ChunkSplitConfig::new(5, 8),
            |chunk| Some(chunk.content.lines().count()),
        );

        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].context.symbol_name, "a");
        assert_eq!(
            chunks[0].context.parent_symbol_name.as_deref(),
            Some("impl Foo")
        );
        assert_eq!(chunks[1].context.symbol_name, "b");
        assert_eq!(
            chunks[1].context.parent_symbol_name.as_deref(),
            Some("impl Foo")
        );
    }

    #[test]
    fn test_split_oversized_leaf_by_lines() {
        let chunker = Chunker::with_overlap(0.0);
        let leaf = synthetic_chunk("large_fn", "function", 10, 18);

        let chunks = chunker.split_oversized_chunks(
            vec![leaf],
            ChunkSplitConfig::new(3, 5),
            |chunk| Some(chunk.content.lines().count()),
        );

        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].context.line_start, 10);
        assert_eq!(chunks[0].context.line_end, 12);
        assert_eq!(chunks[0].context.split_part, Some(1));
        assert_eq!(chunks[0].context.split_total, Some(3));
        assert_eq!(chunks[2].context.line_start, 16);
        assert_eq!(chunks[2].context.line_end, 18);
    }

    #[test]
    fn test_splitter_keeps_small_chunks_unchanged() {
        let chunker = Chunker::with_overlap(0.0);
        let leaf = synthetic_chunk("small_fn", "function", 1, 2);
        let id = leaf.id;

        let chunks = chunker.split_oversized_chunks(
            vec![leaf],
            ChunkSplitConfig::new(5, 8),
            |chunk| Some(chunk.content.lines().count()),
        );

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].id, id);
        assert_eq!(chunks[0].context.split_part, None);
    }
}

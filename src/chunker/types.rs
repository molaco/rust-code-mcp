//! Data types for code chunks: identifiers, context, and split configuration.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

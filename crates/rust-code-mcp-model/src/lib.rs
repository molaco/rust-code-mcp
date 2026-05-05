//! Shared domain model types for rust-code-mcp.

#![warn(unreachable_pub, dead_code)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use uuid::Uuid;

/// Embedding dimension for all-MiniLM-L6-v2.
pub const EMBEDDING_DIM: usize = 384;

/// An embedding vector.
pub type Embedding = Vec<f32>;

/// A unique identifier for a code chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChunkId(Uuid);

impl ChunkId {
    /// Create a new random chunk ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Convert to string representation.
    pub fn to_string(&self) -> String {
        self.0.to_string()
    }

    /// Parse from string representation.
    pub fn from_string(s: &str) -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Default for ChunkId {
    fn default() -> Self {
        Self::new()
    }
}

/// Context information for a code chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkContext {
    /// File path.
    pub file_path: PathBuf,
    /// Module path, for example `["crate", "parser", "mod"]`.
    pub module_path: Vec<String>,
    /// Symbol that this chunk represents.
    pub symbol_name: String,
    /// Kind of symbol.
    pub symbol_kind: String,
    /// Documentation string.
    pub docstring: Option<String>,
    /// Import statements in the file.
    pub imports: Vec<String>,
    /// Functions this symbol calls.
    pub outgoing_calls: Vec<String>,
    /// First source line for this chunk.
    pub line_start: usize,
    /// Last source line for this chunk.
    pub line_end: usize,
}

/// A code chunk with content and context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeChunk {
    /// Unique identifier.
    pub id: ChunkId,
    /// The code content.
    pub content: String,
    /// Rich context for this chunk.
    pub context: ChunkContext,
    /// Overlap from previous chunk, for continuity.
    pub overlap_prev: Option<String>,
    /// Overlap to next chunk, for continuity.
    pub overlap_next: Option<String>,
}

impl CodeChunk {
    /// Format chunk for embedding using contextual retrieval.
    ///
    /// This follows Anthropic's contextual retrieval pattern, which reduces
    /// retrieval errors by adding context to each chunk.
    pub fn format_for_embedding(&self) -> String {
        let mut parts = Vec::new();

        // File and location context.
        parts.push(format!("// File: {}", self.context.file_path.display()));
        parts.push(format!(
            "// Location: lines {}-{}",
            self.context.line_start, self.context.line_end
        ));

        // Module context.
        if !self.context.module_path.is_empty() {
            parts.push(format!(
                "// Module: {}",
                self.context.module_path.join("::")
            ));
        }

        // Symbol context.
        parts.push(format!(
            "// Symbol: {} ({})",
            self.context.symbol_name, self.context.symbol_kind
        ));

        // Documentation if available.
        if let Some(ref doc) = self.context.docstring {
            parts.push(format!("// Purpose: {}", doc));
        }

        // Import context (first 5 imports).
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

        // Call context (first 5 calls).
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

        // Add separator.
        parts.push(String::new());

        // Add the actual code content.
        parts.push(self.content.clone());

        parts.join("\n")
    }
}

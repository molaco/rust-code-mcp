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

/// How many imports, and how many outgoing calls, a chunk keeps: the ones
/// its embedded text names (`format_for_embedding`).
///
/// The chunker keeps no more than these, because nothing reads the rest: the
/// stored chunk is read back for search results (content, docstring, place)
/// and to rebuild the BM25 index (content, symbol, docstring), never for its
/// imports. Every chunk of a file carried the file's whole import list —
/// 35.5 % of the stored chunk JSON of a 0.93M-chunk store (2026-10-29). With
/// the cut a stored chunk still formats to exactly the text it was embedded
/// from.
pub const EMBEDDED_CONTEXT_ITEMS: usize = 5;

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
    /// The file's first imports (at most `EMBEDDED_CONTEXT_ITEMS`)
    pub imports: Vec<String>,
    /// Functions this symbol calls (the first `EMBEDDED_CONTEXT_ITEMS`)
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
    // No overlap text with the neighbouring chunks: it was computed, stored
    // (5.4 % of the stored JSON) and never read — not by the embedded text,
    // not by search. Rows written before carry `overlap_prev`/`overlap_next`;
    // serde skips them on read.
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

        // Import context. The chunker keeps no more than these; the `take`
        // keeps the text the same for chunks stored before it cut.
        if !self.context.imports.is_empty() {
            let imports_str = self
                .context
                .imports
                .iter()
                .take(EMBEDDED_CONTEXT_ITEMS)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("// Imports: {}", imports_str));
        }

        // Call context, likewise.
        if !self.context.outgoing_calls.is_empty() {
            let calls_str = self
                .context
                .outgoing_calls
                .iter()
                .take(EMBEDDED_CONTEXT_ITEMS)
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
    /// Whether only the first part of a line-split symbol carries its
    /// docstring, imports and calls; later parts keep the name, place and
    /// part number.
    ///
    /// Off: every part repeats them, and they count against every part's
    /// budget. On a 2199-line file a part's embedded text was 1647 bytes of
    /// which 210 were code and 958 docstring (2026-10-29), so a long
    /// documented function was cut into slivers of a few lines; across a
    /// 0.95M-chunk store the docstrings of later parts were 736 MiB, half of
    /// the stored chunk JSON. Changing it changes the embedded text, so a
    /// store built one way is not the other's (it is part of the identity a
    /// store is built under).
    pub context_in_first_part_only: bool,
}

impl ChunkSplitConfig {
    pub fn new(target_tokens: usize, hard_max_tokens: usize) -> Self {
        let target_tokens = target_tokens.max(1);
        Self {
            target_tokens,
            hard_max_tokens: hard_max_tokens.max(target_tokens),
            context_in_first_part_only: false,
        }
    }

    /// The same budget with `context_in_first_part_only` set to `on`.
    pub fn with_context_in_first_part_only(self, on: bool) -> Self {
        Self {
            context_in_first_part_only: on,
            ..self
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

    /// A row written before the slim form — the file's whole import list,
    /// the overlap fields — still reads, and formats to the same text it was
    /// embedded from.
    #[test]
    fn a_chunk_stored_before_the_cut_reads_to_its_embedded_text() {
        let imports: Vec<String> = (0..12).map(|i| format!("crate::m{i}::T{i}")).collect();
        let calls: Vec<String> = (0..9).map(|i| format!("call_{i}")).collect();
        let old_row = serde_json::json!({
            "id": ChunkId::new(),
            "content": "fn f() {\n    call_0();\n}",
            "context": {
                "file_path": "src/lib.rs",
                "module_path": ["crate"],
                "symbol_name": "f",
                "symbol_kind": "function",
                "docstring": "Does f",
                "imports": imports,
                "outgoing_calls": calls,
                "line_start": 3,
                "line_end": 5,
            },
            "overlap_prev": "fn e() {}",
            "overlap_next": null,
        })
        .to_string();
        let old: CodeChunk = serde_json::from_str(&old_row).unwrap();

        let mut slim = old.clone();
        slim.context.imports.truncate(EMBEDDED_CONTEXT_ITEMS);
        slim.context.outgoing_calls.truncate(EMBEDDED_CONTEXT_ITEMS);
        assert_eq!(slim.format_for_embedding(), old.format_for_embedding());
        assert!(old.format_for_embedding().contains("crate::m4::T4"));
        assert!(!old.format_for_embedding().contains("crate::m5::T5"));

        let written = serde_json::to_string(&slim).unwrap();
        assert!(!written.contains("overlap"), "{written}");
        assert!(!written.contains("crate::m5::T5"), "{written}");
    }

    /// The other direction: a daemon of an older binary shares the store
    /// with a newer one while it has clients, and must read the rows the
    /// newer one writes. Its chunk had the overlap fields as plain `Option`s
    /// — a missing one reads as `None`.
    #[test]
    fn a_daemon_of_the_older_shape_reads_a_slim_row() {
        #[derive(Deserialize)]
        #[allow(dead_code)]
        struct ChunkBeforeTheCut {
            id: ChunkId,
            content: String,
            context: ChunkContext,
            overlap_prev: Option<String>,
            overlap_next: Option<String>,
        }
        let chunk = CodeChunk {
            id: ChunkId::new(),
            content: "fn g() {}".to_string(),
            context: ChunkContext {
                file_path: PathBuf::from("src/lib.rs"),
                module_path: vec!["crate".to_string()],
                symbol_name: "g".to_string(),
                symbol_kind: "function".to_string(),
                docstring: None,
                imports: vec!["std::io".to_string()],
                outgoing_calls: vec![],
                parent_symbol_name: None,
                split_part: None,
                split_total: None,
                line_start: 1,
                line_end: 1,
            },
        };
        let row = serde_json::to_string(&chunk).unwrap();
        let old: ChunkBeforeTheCut = serde_json::from_str(&row).unwrap();
        assert_eq!(old.id, chunk.id);
        assert!(old.overlap_prev.is_none() && old.overlap_next.is_none());
    }
}

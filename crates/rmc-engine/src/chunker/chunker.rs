//! Core `Chunker` engine: file/symbol chunking with overlap and module-path extraction.

use std::path::Path;

use crate::parser::{ParseResult, Symbol};

use super::types::{ChunkContext, ChunkId, CodeChunk, EMBEDDED_CONTEXT_ITEMS};

/// Code chunker that splits by symbols and adds context
pub struct Chunker;

impl Chunker {
    /// Create a new chunker
    pub fn new() -> Self {
        Self
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

        // The file's first imports, the ones the embedded text names (see
        // `EMBEDDED_CONTEXT_ITEMS`); every chunk of the file carries them.
        let import_strings: Vec<String> = parse_result
            .imports
            .iter()
            .take(EMBEDDED_CONTEXT_ITEMS)
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
                .take(EMBEDDED_CONTEXT_ITEMS)
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

            chunks.push(CodeChunk {
                id: ChunkId::new(),
                content: code,
                context,
            });
        }

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

    /// Chunk-count probe, not a test: what the chunker makes of one real
    /// file (`RMC_CHUNK_PROBE_FILE`), before the oversize split. Prints the
    /// count per symbol kind, how many chunks lie inside another chunk's
    /// line span, how many share their exact span with another, and the
    /// bytes of code against the file's own size.
    #[test]
    #[ignore]
    fn probe_chunks_of_a_real_file() {
        let path = PathBuf::from(std::env::var("RMC_CHUNK_PROBE_FILE").unwrap());
        let source = std::fs::read_to_string(&path).unwrap();
        let parsed = RustParser::new()
            .unwrap()
            .parse_source_complete(&source)
            .unwrap();
        let chunks = Chunker::new().chunk_file(&path, &source, &parsed).unwrap();

        let mut by_kind = std::collections::BTreeMap::<&str, usize>::new();
        for chunk in &chunks {
            *by_kind
                .entry(chunk.context.symbol_kind.as_str())
                .or_default() += 1;
        }
        let spans: Vec<(usize, usize)> = chunks
            .iter()
            .map(|c| (c.context.line_start, c.context.line_end))
            .collect();
        let nested = spans
            .iter()
            .enumerate()
            .filter(|(i, (s, e))| {
                spans
                    .iter()
                    .enumerate()
                    .any(|(j, (s2, e2))| j != *i && s2 <= s && e <= e2 && (s2, e2) != (s, e))
            })
            .count();
        let mut sorted = spans.clone();
        sorted.sort();
        let same_span = sorted.windows(2).filter(|w| w[0] == w[1]).count();
        let code_bytes: usize = chunks.iter().map(|c| c.content.len()).sum();
        let json_bytes: usize = chunks
            .iter()
            .map(|c| serde_json::to_string(c).unwrap().len())
            .sum();
        eprintln!(
            "{}: {} lines, {} bytes -> {} chunks; nested in another chunk: {}; same span as another: {}; \
             code {} bytes ({:.2}x the file), chunk JSON {} bytes",
            path.display(),
            source.lines().count(),
            source.len(),
            chunks.len(),
            nested,
            same_span,
            code_bytes,
            code_bytes as f64 / source.len() as f64,
            json_bytes,
        );
        eprintln!("by kind: {by_kind:?}");
        // The oversize split as bge-small runs it (384 target / 512 hard
        // tokens), with tokens estimated as a quarter of the formatted
        // text's bytes — the real counter needs the model's tokenizer; this
        // is for the order of magnitude.
        let split = Chunker::new().split_oversized_chunks(
            chunks.clone(),
            crate::chunker::ChunkSplitConfig::new(384, 512),
            |chunk| Some(chunk.format_for_embedding().len() / 4),
        );
        let content: usize = split.iter().map(|c| c.content.len()).sum();
        let formatted: usize = split.iter().map(|c| c.format_for_embedding().len()).sum();
        let docstring: usize = split
            .iter()
            .map(|c| c.context.docstring.as_ref().map_or(0, String::len))
            .sum();
        eprintln!(
            "after the oversize split (≈ tokens): {} chunks; per chunk: content {} B, embedded text {} B, \
             of it docstring {} B",
            split.len(),
            content / split.len(),
            formatted / split.len(),
            docstring / split.len(),
        );
        let mut small: Vec<_> = chunks.iter().filter(|c| c.content.len() < 80).collect();
        small.truncate(8);
        for chunk in small {
            eprintln!(
                "  small {} {} L{}-{}: {:?}",
                chunk.context.symbol_kind,
                chunk.context.symbol_name,
                chunk.context.line_start,
                chunk.context.line_end,
                chunk.content
            );
        }
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

    /// A chunk keeps the imports and calls its embedded text names, in the
    /// file's order, and no more.
    #[test]
    fn a_chunk_keeps_the_imports_and_calls_it_embeds() {
        let source = r#"
use std::a::A;
use std::b::B;
use std::c::C;
use std::d::D;
use std::e::E;
use std::f::F;
use std::g::G;

fn caller() {
    c1(); c2(); c3(); c4(); c5(); c6(); c7();
}
        "#;
        let parse_result = RustParser::new()
            .unwrap()
            .parse_source_complete(source)
            .unwrap();
        let chunks = Chunker::new()
            .chunk_file(Path::new("src/lib.rs"), source, &parse_result)
            .unwrap();
        let caller = chunks
            .iter()
            .find(|c| c.context.symbol_name == "caller")
            .unwrap();

        assert_eq!(
            caller.context.imports,
            ["std::a::A", "std::b::B", "std::c::C", "std::d::D", "std::e::E"]
        );
        assert_eq!(caller.context.outgoing_calls.len(), EMBEDDED_CONTEXT_ITEMS);
        assert!(caller.format_for_embedding().contains("std::e::E"));
    }
}

//! Oversized-chunk and token-based splitting logic.
//!
//! Provides [`Chunker::split_oversized_chunks`] plus free helpers used to
//! decide which container chunks to elide and how to subdivide oversized
//! leaf chunks by line.

use super::chunker::Chunker;
use super::types::{ChunkId, ChunkSplitConfig, CodeChunk};

impl Chunker {
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

        output
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

        // Whether the part about to be cut drops the symbol's context: only a
        // part after the first, and only under `context_in_first_part_only`.
        let bare = |parts: &Vec<CodeChunk>| config.context_in_first_part_only && !parts.is_empty();

        for (line_idx, line) in lines.iter().enumerate() {
            let mut candidate_lines = current_lines.clone();
            candidate_lines.push(*line);
            let candidate = make_part_chunk(
                &chunk,
                part_start,
                line_idx + 1,
                &candidate_lines.join("\n"),
                bare(&parts),
            );
            let candidate_tokens = token_count_or_estimate(&candidate, token_count);

            if !current_lines.is_empty() && candidate_tokens > config.target_tokens {
                let content = current_lines.join("\n");
                let part = make_part_chunk(&chunk, part_start, line_idx, &content, bare(&parts));
                parts.push(part);
                part_start = line_idx;
                current_lines.clear();
            }

            current_lines.push(*line);
        }

        if !current_lines.is_empty() {
            let content = current_lines.join("\n");
            let part = make_part_chunk(&chunk, part_start, lines.len(), &content, bare(&parts));
            parts.push(part);
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

/// A part of `source` holding `content`. A `bare` part keeps the symbol's
/// name, place and part number but not its docstring, imports and calls —
/// those stay with the first part.
fn make_part_chunk(
    source: &CodeChunk,
    start_line_offset: usize,
    end_line_offset: usize,
    content: &str,
    bare: bool,
) -> CodeChunk {
    let mut part = source.clone();
    part.id = ChunkId::new();
    part.content = content.to_string();
    part.context.line_start = source.context.line_start + start_line_offset;
    part.context.line_end = source.context.line_start + end_line_offset.saturating_sub(1);
    if bare {
        part.context.docstring = None;
        part.context.imports.clear();
        part.context.outgoing_calls.clear();
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunker::types::{ChunkContext, ChunkId};
    use std::path::PathBuf;

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
        }
    }

    #[test]
    fn test_split_oversized_container_uses_child_chunks() {
        let chunker = Chunker::new();
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
        let chunker = Chunker::new();
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
        let chunker = Chunker::new();
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

    /// With `context_in_first_part_only` the docstring, imports and calls
    /// ride the first part only, so they stop eating every later part's
    /// budget: a documented function is cut into fewer, larger parts, and
    /// every line of it is still in exactly one part.
    #[test]
    fn context_in_the_first_part_only_leaves_the_budget_to_code() {
        let mut leaf = synthetic_chunk("documented_fn", "function", 1, 40);
        leaf.context.docstring = Some("explains ".repeat(40));
        leaf.context.imports = vec!["std::io::Read".to_string()];
        leaf.context.outgoing_calls = vec!["line_1".to_string()];
        // Lines of formatted text, with the docstring's own weight: a line
        // per 40 bytes of it.
        let tokens = |chunk: &CodeChunk| {
            let doc = chunk.context.docstring.as_ref().map_or(0, |d| d.len() / 40);
            Some(chunk.format_for_embedding().lines().count() + doc)
        };
        let budget = ChunkSplitConfig::new(20, 24);

        let repeated = Chunker::new().split_oversized_chunks(vec![leaf.clone()], budget, tokens);
        let first_only = Chunker::new().split_oversized_chunks(
            vec![leaf],
            budget.with_context_in_first_part_only(true),
            tokens,
        );

        assert!(
            first_only.len() < repeated.len(),
            "{} parts, repeating the context {}",
            first_only.len(),
            repeated.len()
        );
        assert!(repeated.iter().all(|p| p.context.docstring.is_some()));
        assert!(first_only[0].context.docstring.is_some());
        assert_eq!(first_only[0].context.imports, ["std::io::Read"]);
        for part in &first_only[1..] {
            assert!(part.context.docstring.is_none());
            assert!(part.context.imports.is_empty() && part.context.outgoing_calls.is_empty());
            assert_eq!(part.context.symbol_name, "documented_fn");
        }
        let lines: Vec<usize> = first_only
            .iter()
            .flat_map(|p| p.context.line_start..=p.context.line_end)
            .collect();
        assert_eq!(lines, (1..=40).collect::<Vec<_>>());
    }
}

# chunker — Detailed Logic

## Module: chunker

### `ChunkId::new() -> Self`
**Call graph:** Uuid::new_v4
**Steps:**
1. Generate a fresh v4 UUID via `Uuid::new_v4`.
2. Wrap it in a `ChunkId` tuple struct and return.

### `ChunkId::to_string(&self) -> String`
**Call graph:** Uuid::to_string
**Steps:**
1. Delegate to the inner `Uuid`'s `to_string` to produce the canonical hyphenated form.

### `ChunkId::from_string(s: &str) -> Result<Self, Box<dyn std::error::Error>>`
**Call graph:** Uuid::parse_str
**Steps:**
1. Call `Uuid::parse_str(s)` and propagate any parse error via `?`.
2. Wrap the parsed `Uuid` in `ChunkId` and return as `Ok`.

### `impl Default for ChunkId :: default() -> Self`
**Call graph:** ChunkId::new
**Steps:**
1. Forward to `ChunkId::new` to produce a fresh random ID.

### `CodeChunk::format_for_embedding(&self) -> String`
**Call graph:** Vec::push, format!, PathBuf::display, Vec::join, Iterator::take, Iterator::cloned, Iterator::collect, str::is_empty
**Steps:**
1. Initialize an empty `Vec<String>` named `parts` to accumulate header lines.
2. Push a `// File: ...` line built from `context.file_path.display()`.
3. Push a `// Location: lines {start}-{end}` line using `line_start` and `line_end`.
4. If `module_path` is non-empty, push a `// Module: ...` line joining the path with `::`.
5. Push a `// Symbol: {name} ({kind})` line summarizing the symbol identity.
6. If `docstring` is `Some`, push a `// Purpose: {doc}` line.
7. If `imports` is non-empty, take up to the first 5, clone them, join with `, `, and push `// Imports: ...`.
8. If `outgoing_calls` is non-empty, take up to the first 5, clone them, join with `, `, and push `// Calls: ...`.
9. Push an empty string as a blank-line separator between header and body.
10. Push the actual `content` of the chunk.
11. Return `parts.join("\n")` to produce the full embedding-ready string.

### `Chunker::new() -> Self`
**Call graph:** (none)
**Steps:**
1. Construct a `Chunker` with `overlap_percentage` set to `0.2` (20 percent default).

### `Chunker::with_overlap(overlap_percentage: f64) -> Self`
**Call graph:** f64::clamp
**Steps:**
1. Clamp the supplied percentage into the range `[0.0, 0.5]`.
2. Construct a `Chunker` with the clamped value.

### `Chunker::chunk_file(&self, file_path: &Path, source: &str, parse_result: &ParseResult) -> Result<Vec<CodeChunk>, Box<dyn std::error::Error>>`
**Call graph:** Chunker::extract_module_path, Iterator::map, Iterator::collect, Chunker::extract_symbol_code, CallGraph::get_callees, String::from, Path::to_path_buf, SymbolKind::as_str, str::to_string, Clone::clone, ChunkId::new, Vec::push, Chunker::add_overlap
**Steps:**
1. Initialize an empty `chunks` vector to accumulate produced `CodeChunk`s.
2. Compute the module path for the file via `extract_module_path`.
3. Build `import_strings` by mapping each `parse_result.imports` entry to its `path` clone.
4. Iterate over every `Symbol` in `parse_result.symbols`.
5. For each symbol, call `extract_symbol_code(source, symbol)` to get the source slice (propagate errors with `?`).
6. Look up outgoing calls via `parse_result.call_graph.get_callees(&symbol.name)` and convert each to an owned `String`.
7. Build a `ChunkContext` using the file path, cloned module path, symbol metadata (name/kind/docstring), shared imports, computed outgoing calls, and the symbol's line range.
8. Construct a `CodeChunk` with a fresh `ChunkId::new()`, the extracted `code`, the context, and `None` overlaps.
9. Push the chunk into `chunks`.
10. After the loop, call `add_overlap(&mut chunks)` to populate `overlap_prev` / `overlap_next` between adjacent chunks.
11. Return `Ok(chunks)`.

### `Chunker::extract_symbol_code(&self, source: &str, symbol: &Symbol) -> Result<String, Box<dyn std::error::Error>>` (private, load-bearing)
**Call graph:** str::lines, Iterator::collect, usize::saturating_sub, usize::min, slice::join
**Steps:**
1. Split `source` into a `Vec<&str>` of lines.
2. Convert the symbol's 1-indexed `start_line` to a 0-indexed `start` using `saturating_sub(1)`.
3. Set `end` to the symbol's `end_line`.
4. If `start >= lines.len()`, return an empty `String` (out-of-range guard).
5. Clamp `end` down to `lines.len()` so the slice never overruns.
6. Slice `lines[start..end]` to capture exactly the symbol's lines.
7. Join the slice with `\n` and return as `Ok(String)`.

### `Chunker::extract_module_path(&self, file_path: &Path) -> Vec<String>` (private, load-bearing)
**Call graph:** Path::components, Component::as_os_str, OsStr::to_str, Vec::push, str::strip_suffix, Path::file_stem
**Steps:**
1. Initialize an empty `parts` vector and a `found_src` flag set to `false`.
2. Walk each path component, decoding it to UTF-8 via `as_os_str().to_str()`.
3. When the component named `"src"` is encountered, flip `found_src` to `true`, push `"crate"` into `parts`, and continue.
4. After `src` has been seen, for each subsequent component strip a trailing `.rs` extension, then push the cleaned name unless it equals `"mod"` (which is collapsed into its parent).
5. If no path components produced anything (`parts` empty), fall back to using the file's stem from `file_stem()`.
6. Return `parts`.

### `Chunker::add_overlap(&self, chunks: &mut [CodeChunk])` (private, load-bearing)
**Call graph:** Chunker::calculate_overlap
**Steps:**
1. Iterate `i` from `0` through `chunks.len() - 1`.
2. If `i > 0`, compute `overlap_prev` for chunk `i` by calling `calculate_overlap(&chunks[i-1].content, false)` (taking the first lines of the previous chunk).
3. If `i < chunks.len() - 1`, compute `overlap_next` for chunk `i` by calling `calculate_overlap(&chunks[i].content, true)` (taking the last lines of the current chunk).
4. Assign each computed `Option<String>` into the corresponding chunk field.

### `Chunker::calculate_overlap(&self, content: &str, from_end: bool) -> Option<String>` (private, load-bearing)
**Call graph:** str::lines, Iterator::collect, f64::ceil, usize::saturating_sub, usize::min, slice::join
**Steps:**
1. Split `content` into a `Vec<&str>` of lines.
2. Compute `overlap_lines = ceil(lines.len() * overlap_percentage)` cast back to `usize`.
3. Return `None` if either `overlap_lines == 0` or `lines` is empty.
4. If `from_end` is `true`, slice `lines[len - overlap_lines ..]` (using `saturating_sub` to stay in bounds) to capture trailing context.
5. Otherwise slice `lines[..min(overlap_lines, len)]` to capture leading context.
6. Join the chosen slice with `\n` and return wrapped in `Some`.

### `impl Default for Chunker :: default() -> Self`
**Call graph:** Chunker::new
**Steps:**
1. Forward to `Chunker::new` to construct a chunker with the default 20 percent overlap.

# semantic — Detailed Logic

## Module: mod

### `SEMANTIC: LazyLock<Mutex<SemanticService>>`
**Call graph:** SemanticService::new
**Steps:**
1. Lazily initializes a global `Mutex<SemanticService>` on first access via `LazyLock`.
2. Wraps a freshly constructed `SemanticService` (created with `SemanticService::new()`) in a `Mutex` since `AnalysisHost` is not `Sync`.
3. Provides a single shared semantic service across the process for `symbol_search`, `find_references_by_name`, and `rename_by_name` callers.

### `pub use position::Location` / `pub use rename::{RenameEdit, RenameFileMove, RenamePreview}`
**Call graph:** —
**Steps:**
1. Re-exports `Location` from `position` so consumers can construct/format semantic locations without importing the private submodule.
2. Re-exports the rename preview surface (`RenameEdit`, `RenameFileMove`, `RenamePreview`) from `rename` so MCP tool handlers can describe a planned rename without touching disk.

### `ProjectContext` (private struct)
**Call graph:** —
**Steps:**
1. Bundles the `AnalysisHost` and `Vfs` returned by `loader::load_project` for a single canonical project root.
2. Stored as the value type in `SemanticService::projects`, keyed by the canonical project path.

### `SemanticService::new() -> Self`
**Call graph:** HashMap::new
**Steps:**
1. Constructs a new `SemanticService` with an empty `HashMap` of cached `ProjectContext` entries keyed by canonical project paths.

### `SemanticService::get_or_load(&mut self, project_path: &Path) -> Result<()>` (private)
**Call graph:** Path::canonicalize -> HashMap::contains_key -> tracing::info -> loader::load_project -> HashMap::insert
**Steps:**
1. Canonicalizes the input project path so cache lookups are independent of relative paths.
2. Checks if `self.projects` already has a `ProjectContext` for this canonical path; if so, returns `Ok(())` immediately.
3. Logs an informational trace announcing that an IDE is being loaded for the project.
4. Calls `loader::load_project` to construct an `AnalysisHost` plus a `Vfs` for the workspace.
5. Inserts the resulting `(host, vfs)` into the `projects` map under the canonical key, wrapped in `ProjectContext`.
6. Logs a follow-up trace confirming successful IDE load.

### `SemanticService::symbol_search(&mut self, project_path: &Path, symbol_name: &str, limit: usize) -> Result<Vec<Location>>`
**Call graph:** SemanticService::get_or_load -> Path::canonicalize -> HashMap::get -> position::symbol_search
**Steps:**
1. Ensures the project's IDE state is loaded by calling `get_or_load`.
2. Canonicalizes the project path again to retrieve the cached `ProjectContext`.
3. Looks up the `ProjectContext` in `self.projects`, returning an error if it is somehow missing after load.
4. Delegates to `position::symbol_search` with the host, VFS, symbol name, and result limit.

### `SemanticService::find_references_by_name(&mut self, project_path: &Path, symbol_name: &str) -> Result<Vec<Location>>`
**Call graph:** SemanticService::get_or_load -> Path::canonicalize -> HashMap::get -> position::find_references_by_name
**Steps:**
1. Calls `get_or_load` to lazily initialize or reuse cached IDE state for the project.
2. Canonicalizes the path and fetches the cached `ProjectContext`, erroring if not present.
3. Forwards the request to `position::find_references_by_name`, which searches by symbol name then resolves references for each match.

### `SemanticService::rename_by_name(&mut self, project_path: &Path, symbol_name: &str, new_name: &str) -> Result<RenamePreview>`
**Call graph:** SemanticService::get_or_load -> Path::canonicalize -> HashMap::get -> rename::rename_by_name
**Steps:**
1. Calls `get_or_load` to ensure the project's IDE state is cached for this canonical path.
2. Re-canonicalizes the path and retrieves the matching `ProjectContext`, erroring if not present.
3. Delegates to `rename::rename_by_name` with the host, VFS, original name, and replacement name; returns the `RenamePreview` without touching the filesystem.

## Module: loader

### `load_project(path: &Path) -> Result<(AnalysisHost, Vfs)>`
**Call graph:** CargoConfig::default -> num_cpus::get_physical -> ra_ap_load_cargo::load_workspace_at -> anyhow::Context::context -> AnalysisHost::with_database
**Steps:**
1. Builds a `CargoConfig` with `sysroot = None` and `no_deps = true` to skip dependency analysis (~120ms loads); other fields fall through to `Default::default()`.
2. Builds a `LoadCargoConfig` that disables `OUT_DIR` and the proc-macro server (`ProcMacroServerChoice::None`), enables `prefill_caches`, sets `num_worker_threads = num_cpus::get_physical()`, and uses one proc-macro process.
3. Calls `load_workspace_at` with the path, configs, and a no-op progress callback to load the workspace database and VFS.
4. Wraps any load error with the context message "Failed to load workspace".
5. Constructs an `AnalysisHost` from the returned database via `AnalysisHost::with_database`.
6. Returns the `(AnalysisHost, Vfs)` tuple, dropping the third tuple element from `load_workspace_at`.

## Module: position

### `Location` (struct, public fields)
**Call graph:** —
**Steps:**
1. Plain data struct holding `file_path: PathBuf`, 1-based `line: u32`, 1-based `column: u32`, and `name: String`.
2. Derives `Debug` and `Clone` for diagnostics and propagation across handler boundaries.

### `impl std::fmt::Display for Location`
**Call graph:** write! -> PathBuf::display
**Steps:**
1. Formats the location as `"{file_path}:{line}:{column} ({name})"` using `write!` and `Path::display` for portable rendering.

### `path_to_file_id(vfs: &Vfs, file_path: &Path) -> Result<ra_ap_vfs::FileId>` (private)
**Call graph:** Path::canonicalize -> anyhow::Context::context -> VfsPath::new_real_path -> Vfs::file_id
**Steps:**
1. Canonicalizes the file path, attaching the context "Failed to canonicalize path" on failure.
2. Builds a `VfsPath::new_real_path` from the canonicalized path's lossy UTF-8 string form.
3. Looks up the `FileId` in the VFS, taking only the id from the returned tuple.
4. Returns an `anyhow::anyhow!` error mentioning the original file path if the VFS does not know the file.

### `to_offset(analysis: &Analysis, file_id: FileId, line: u32, column: u32) -> Result<TextSize>` (private)
**Call graph:** Analysis::file_line_index -> anyhow::Context::context -> u32::saturating_sub -> LineIndex::offset
**Steps:**
1. Fetches the `LineIndex` for the file, contextualizing failure as "Failed to get line index".
2. Converts the 1-based input `(line, column)` to the 0-based `LineCol` rust-analyzer expects via `saturating_sub(1)`.
3. Calls `LineIndex::offset` to translate the line/column into a `TextSize` byte offset.
4. Returns an error if the position falls outside the file using `anyhow!`.

### `nav_target_to_location(vfs: &Vfs, analysis: &Analysis, target: &NavigationTarget) -> Result<Location>` (private)
**Call graph:** Vfs::file_path -> VfsPath::as_path -> Path::to_path_buf -> Analysis::file_line_index -> Option::unwrap_or -> TextRange::start -> LineIndex::line_col -> NavigationTarget::name::to_string
**Steps:**
1. Resolves the `NavigationTarget`'s `file_id` to a `VfsPath`, then converts it to a `PathBuf`, erroring with "Not a real path" if it is virtual.
2. Retrieves the `LineIndex` for the target file.
3. Picks the `focus_range` if available, otherwise the `full_range`, then takes its starting `TextSize`.
4. Maps the offset to `LineCol` via `LineIndex::line_col`.
5. Constructs a `Location` with 1-based line/column (adding 1 to each component) and the target's `name` as a `String`.

### `goto_definition(host: &AnalysisHost, vfs: &Vfs, file_path: &Path, line: u32, column: u32) -> Result<Vec<Location>>`
**Call graph:** AnalysisHost::analysis -> path_to_file_id -> to_offset -> RaFixtureConfig::default -> Analysis::goto_definition -> anyhow::Context::context -> nav_target_to_location
**Steps:**
1. Acquires an `Analysis` snapshot from the host.
2. Resolves the file path to a VFS `FileId` via `path_to_file_id`.
3. Converts the 1-based `(line, column)` to a `TextSize` offset via `to_offset`.
4. Builds a `FilePosition` and a default `GotoDefinitionConfig` (with a default `RaFixtureConfig`).
5. Calls `analysis.goto_definition`, attaching the context "goto_definition query failed".
6. If a `RangeInfo` is returned, maps each `NavigationTarget` in `info` to a `Location` via `nav_target_to_location`, collecting the results.
7. If no result is returned, returns an empty `Vec`.

### `find_references(host: &AnalysisHost, vfs: &Vfs, file_path: &Path, line: u32, column: u32) -> Result<Vec<Location>>`
**Call graph:** AnalysisHost::analysis -> path_to_file_id -> to_offset -> RaFixtureConfig::default -> Analysis::find_all_refs -> anyhow::Context::context -> nav_target_to_location -> ra_ap_vfs::FileId::from_raw -> Vfs::file_path -> VfsPath::as_path -> Analysis::file_line_index -> LineIndex::line_col -> Vec::push
**Steps:**
1. Takes an analysis snapshot, resolves the file id, and computes the byte offset from the 1-based line/column.
2. Builds a `FilePosition` and a `FindAllRefsConfig` that includes imports and tests with no scope restriction.
3. Calls `analysis.find_all_refs` with the context "find_all_refs query failed".
4. Iterates each returned `ReferenceSearchResult`; if it carries a `declaration`, converts its nav target to a `Location` via `nav_target_to_location` and pushes it.
5. For each `(ref_file_id, refs)` entry in `references`, converts the ide-db `FileId` to a vfs `FileId` via `from_raw(index())`, resolves the path through the VFS, and obtains the file's `LineIndex`.
6. For every `(range, _category)` reference, pushes a `Location` with 1-based line/column and the literal name `"reference"`.
7. Returns the accumulated `Vec<Location>`.

### `symbol_search(host: &AnalysisHost, vfs: &Vfs, symbol_name: &str, limit: usize) -> Result<Vec<Location>>`
**Call graph:** AnalysisHost::analysis -> Query::new -> Analysis::symbol_search -> anyhow::Context::context -> nav_target_to_location
**Steps:**
1. Takes an analysis snapshot from the host.
2. Builds a `Query::new(symbol_name.to_string())` and runs `analysis.symbol_search` with the requested `limit`, contextualizing failure as "symbol_search query failed".
3. Maps each returned `NavigationTarget` to a `Location` via `nav_target_to_location` and collects them.

### `find_references_by_name(host: &AnalysisHost, vfs: &Vfs, symbol_name: &str) -> Result<Vec<Location>>`
**Call graph:** AnalysisHost::analysis -> Query::new -> Analysis::symbol_search -> anyhow::Context::context -> Option::unwrap_or -> TextRange::start -> Analysis::find_all_refs -> nav_target_to_location -> Vfs::file_path -> VfsPath::as_path -> Analysis::file_line_index -> LineIndex::line_col -> Vec::push -> Vec::sort_by -> Vec::dedup_by
**Steps:**
1. Acquires an `Analysis` snapshot and runs `analysis.symbol_search` with limit 50 to find all symbols matching the name.
2. For each matched symbol, computes a `FilePosition` from its `file_id` and the start of `focus_range` (or `full_range` fallback).
3. Builds a `FindAllRefsConfig` (imports and tests included, no scope) and runs `analysis.find_all_refs` for that position.
4. For each returned `ReferenceSearchResult`, converts the optional `declaration` to a `Location` via `nav_target_to_location` and pushes it.
5. Walks `references` per-file; resolves the file path via the VFS and grabs the file's `LineIndex`.
6. For every `(range, _category)`, pushes a `Location` with the file path, 1-based line/column, and the literal name `"reference"`.
7. Sorts the accumulated locations by `(file_path, line, column)` and deduplicates adjacent equal triples via `dedup_by`.
8. Returns the deduplicated `Vec<Location>`.

## Module: rename

### `RenameEdit` (struct, public fields)
**Call graph:** —
**Steps:**
1. Plain data struct describing a single in-place text edit: `file_path: PathBuf`, 1-based `start_line` / `start_column` / `end_line` / `end_column`, and `new_text: String` (the replacement, empty for pure deletions).
2. Derives `Debug` and `Clone` so handlers can return previews to MCP clients.

### `impl std::fmt::Display for RenameEdit`
**Call graph:** write! -> PathBuf::display
**Steps:**
1. Formats the edit as `"{file_path}:{start_line}:{start_column}-{end_line}:{end_column} → {new_text:?}"`, debug-quoting the replacement so newlines/whitespace are visible.

### `RenameFileMove` (struct, public fields)
**Call graph:** —
**Steps:**
1. Plain data struct describing a filesystem change emitted by rust-analyzer alongside textual edits: `from: PathBuf` (empty for `CreateFile`), `to_anchor: PathBuf` (the existing sibling file rust-analyzer used as a layout anchor), and `to_path: String` (the destination path string from the IDE edit).
2. Derives `Debug` and `Clone`.

### `impl std::fmt::Display for RenameFileMove`
**Call graph:** write! -> PathBuf::display
**Steps:**
1. Formats the move as `"move: {from} → (anchor: {to_anchor}) {to_path}"`, surfacing the anchor so callers can resolve the relative destination.

### `RenamePreview` (struct, public fields)
**Call graph:** —
**Steps:**
1. Aggregate result of a rename query: `edits: Vec<RenameEdit>` (textual replacements) and `file_moves: Vec<RenameFileMove>` (creates / moves / dir moves).
2. Derives `Debug`, `Clone`, and `Default` so callers can build empty previews and clone for serialization.

### `rename_by_name(host: &AnalysisHost, vfs: &Vfs, symbol_name: &str, new_name: &str) -> Result<RenamePreview>`
**Call graph:** AnalysisHost::analysis -> Query::new -> Analysis::symbol_search -> anyhow::Context::context -> str::as_str -> Iterator::filter -> Iterator::collect -> anyhow::bail! -> VfsPath::as_path -> Path::to_string -> Vec::join -> TextRange::start -> Option::unwrap_or -> Analysis::rename -> anyhow::anyhow! -> source_change_to_preview
**Steps:**
1. Acquires an `Analysis` snapshot and runs `analysis.symbol_search` with limit 50 to enumerate every symbol matching the requested name, contextualizing failures with "symbol_search query failed".
2. Bails with `"No symbol found matching '{symbol_name}'"` when the search returns an empty list.
3. Filters the candidates down to those whose `name.as_str()` equals `symbol_name` exactly, avoiding substring/fuzzy matches that would otherwise silently rename unrelated items.
4. Branches on the exact-match list:
   - **0 matches**: bails with `"No exact match for '{symbol_name}'. Found {N} fuzzy candidates."` so the caller can refine the query.
   - **1 match**: keeps that `NavigationTarget` as the rename target.
   - **>1 matches**: collects each candidate's VFS path (falling back to `"<virtual>"` for non-real paths) into `"  - {path} ({name})"` lines and bails with an "Ambiguous symbol" error listing every location — refusing the rename is deliberate because rust-analyzer would otherwise operate on only one of them.
5. Computes a `FilePosition` for the chosen target by taking the start of `focus_range` (or `full_range` fallback) on the target's `file_id`.
6. Builds a `RenameConfig` with `prefer_no_std = false`, `prefer_prelude = true`, `prefer_absolute = false`, and `show_conflicts = true` so rust-analyzer surfaces conflicting edits rather than silently dropping them.
7. Calls `analysis.rename(position, new_name, &config)`:
   - Cancellation is converted to `Err` via `.context("rename query cancelled")`.
   - The inner `Result<SourceChange, RenameError>` is converted via `map_err` into `"rust-analyzer rename refused: {e}"`, propagating rust-analyzer's textual refusal (e.g. invalid identifier, rename across crate boundary).
8. Delegates to `source_change_to_preview` to materialize the `SourceChange` into a `RenamePreview` without writing to disk.

### `source_change_to_preview(vfs: &Vfs, analysis: &Analysis, change: SourceChange) -> Result<RenamePreview>` (private)
**Call graph:** RenamePreview::default -> Vfs::file_path -> VfsPath::as_path -> Path::to_path_buf -> Analysis::file_line_index -> anyhow::Context::context -> TextEdit::iter -> LineIndex::line_col -> Indel::insert::clone -> Vec::push -> VfsPath::to_path_buf -> PathBuf::new -> Vec::sort_by -> Ord::cmp
**Steps:**
1. Initializes an empty `RenamePreview` via `RenamePreview::default()`.
2. Walks `change.source_file_edits` — a map of `FileId -> (TextEdit, Option<SnippetEdit>)`:
   - Resolves each `FileId` to a real `PathBuf`, erroring with `"Edit refers to non-real path"` if rust-analyzer produced a virtual file edit.
   - Fetches the file's `LineIndex` with context `"Failed to get line index for edit"`.
   - Iterates each `Indel` (insert/delete pair) in the `TextEdit`, mapping `indel.delete.start()` / `indel.delete.end()` byte offsets through `line_index.line_col` to 0-based line/column, then bumps both to 1-based when constructing the `RenameEdit`.
   - Clones `indel.insert` as the `new_text` so the preview is self-contained.
3. Walks `change.file_system_edits`, branching on the variant:
   - **`FileSystemEdit::CreateFile { dst, .. }`**: resolves `dst.anchor` to a real path (empty `PathBuf` if virtual) and pushes a `RenameFileMove` with empty `from`, the anchor, and `dst.path` as `to_path`.
   - **`FileSystemEdit::MoveFile { src, dst }`**: resolves both `src` and `dst.anchor` to real paths (empty `PathBuf` on virtual) and pushes a `RenameFileMove` with the source path, anchor, and destination string.
   - **`FileSystemEdit::MoveDir { src, src_id: _, dst }`**: ignores the directory id, uses `src.path.as_str()` for `from`, resolves `dst.anchor` to a real path, and uses `dst.path` for `to_path`.
4. Sorts `preview.edits` by `(file_path, start_line, start_column)` using a chained `Ord::cmp` / `then` comparator so the output is deterministic across runs.
5. Returns the populated `RenamePreview`; the caller decides whether and how to apply the edits to disk.

# semantic — Abstract Logic

## Module: mod
**Purpose:** Provides a process-wide semantic service that lazily loads rust-analyzer IDE state per project and exposes symbol-level queries.

1. **Hold a single shared semantic service for the process** -> `SEMANTIC` (LazyLock<Mutex<SemanticService>>)
2. **Construct an empty service with a per-project IDE cache** -> `SemanticService::new()`
3. **Lazily load and cache rust-analyzer IDE state for a project path** -> `SemanticService::get_or_load()`
4. **Search for symbols by name within a project** -> `SemanticService::symbol_search()`
5. **Resolve all references for a named symbol within a project** -> `SemanticService::find_references_by_name()`

## Module: loader
**Purpose:** Bootstraps a rust-analyzer `AnalysisHost` and `Vfs` for a Cargo workspace with dependency-free, fast-load configuration.

1. **Load a workspace into an analysis host plus VFS using no-deps Cargo config** -> `load_project()`

## Module: position
**Purpose:** Translates between file paths, 1-based line/column coordinates, and rust-analyzer navigation targets to power goto-definition, find-references, and symbol search.

1. **Represent a resolved code location as a portable data record** -> `Location` (struct), `impl Display for Location`
2. **Convert a real file path into a VFS file id** -> `path_to_file_id()`
3. **Convert a 1-based line/column to a rust-analyzer byte offset** -> `to_offset()`
4. **Convert a rust-analyzer navigation target back into a `Location`** -> `nav_target_to_location()`
5. **Resolve the definition(s) at a given file position** -> `goto_definition()`
6. **Find all references to the symbol at a given file position** -> `find_references()`
7. **Search the workspace for symbols matching a name, bounded by a limit** -> `symbol_search()`
8. **Find every reference to any symbol matching a name, deduplicated and sorted** -> `find_references_by_name()`

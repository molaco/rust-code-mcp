# rust-analyzer IDE Integration for rust-code-mcp

## Executive Summary

This document outlines the integration of rust-analyzer's `ide` crate into rust-code-mcp to replace the current syntax-only analysis with full semantic code intelligence.

**Recommendation: Option A - High-level `ra_ap_ide` crate**

This approach provides the best balance of effort vs. capability, using rust-analyzer's battle-tested IDE APIs.

## Current State

### Current Implementation

The current analysis tools in `src/tools/analysis_tools.rs` use `ra_ap_syntax` for syntax-only parsing:

| Tool | Current Implementation | Limitation |
|------|----------------------|------------|
| `find_definition` | Matches symbol by name string | Returns ANY symbol named `foo`, ignoring scope/imports |
| `find_references` | Name matching in call graph | No semantic understanding of references |
| `get_call_graph` | Syntax-based call extraction | Can't resolve method calls through traits |
| `get_dependencies` | Import path extraction | No resolution of what imports resolve to |

### Problem

Without semantic understanding:
- `find_definition("parse")` returns every function named `parse` in the codebase
- References to a method don't include trait implementations
- Call graphs miss dynamically-dispatched calls

## Architecture Options

### Option A: High-level `ra_ap_ide` crate (Recommended)

```
┌─────────────────────────────────────────────────────────────┐
│                     rust-code-mcp                            │
├─────────────────────────────────────────────────────────────┤
│  SemanticAnalyzer                                            │
│  ├── host: AnalysisHost                                      │
│  ├── vfs: Vfs                                                │
│  └── project_root: PathBuf                                   │
├─────────────────────────────────────────────────────────────┤
│  MCP Tool Implementations                                    │
│  ├── find_definition(file, line, col) OR (symbol, dir)      │
│  ├── find_references(file, line, col)                        │
│  ├── get_call_graph(file, line, col)                        │
│  ├── hover(file, line, col)                                  │
│  └── symbol_search(query)                                    │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                     ra_ap_ide                                │
│  Analysis::goto_definition, find_all_refs, call_hierarchy   │
└─────────────────────────────────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                  ra_ap_load_cargo                            │
│  load_workspace_at, load_workspace                           │
└─────────────────────────────────────────────────────────────┘
```

**Pros:**
- Clean, well-documented API
- Battle-tested (used by the rust-analyzer LSP server)
- Handles edge cases (macros, doc comments, format strings, trait impls)
- Incremental updates supported via `AnalysisHost::apply_change`

**Cons:**
- Heavier dependency
- Less control over low-level details
- Initial project loading takes time (seconds for large projects)

### Option B: Low-level `ra_ap_hir` crates

Direct access to `RootDatabase`, `ModuleDefId`, `Semantics` as shown in `rust-codemaps/builder.rs`.

**Pros:**
- Full control
- Can build custom data structures
- Potentially more efficient for batch operations

**Cons:**
- Significantly more code to write
- Must handle many edge cases manually
- No incremental updates without implementing yourself

### Option C: Hybrid

Use `ra_ap_ide` for point queries, `ra_ap_hir` for batch indexing.

**Pros:**
- Best of both worlds

**Cons:**
- Added complexity
- Must keep two approaches in sync

## Recommended Approach: Option A

The `ra_ap_ide` crate is designed exactly for this use case and handles all the complexity internally.

## Key Types and APIs

### Project Loading

```rust
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;

// Load a Cargo project
fn load_project(path: &Path) -> anyhow::Result<(AnalysisHost, Vfs)> {
    let cargo_config = CargoConfig {
        sysroot: None,  // or Some(RustLibSource::Discover) for stdlib
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: true,
    };

    let (db, vfs, _) = load_workspace_at(
        path,
        &cargo_config,
        &load_config,
        &|_| {},
    )?;

    let host = AnalysisHost::with_database(db);
    Ok((host, vfs))
}
```

### FileId ↔ Path Mapping

```rust
use ra_ap_vfs::{Vfs, VfsPath, FileId};

// Path → FileId
fn path_to_file_id(vfs: &Vfs, path: &Path) -> Option<FileId> {
    let vfs_path = VfsPath::from(path.to_path_buf());
    vfs.file_id(&vfs_path).map(|(id, _)| id)
}

// FileId → Path
fn file_id_to_path(vfs: &Vfs, file_id: FileId) -> PathBuf {
    vfs.file_path(file_id).as_path().unwrap().to_path_buf()
}
```

### Position Mapping

```rust
use ra_ap_ide::{Analysis, FilePosition, LineCol, TextSize};

// (line, col) → byte offset
fn to_offset(analysis: &Analysis, file_id: FileId, line: u32, col: u32) -> Option<TextSize> {
    let line_index = analysis.file_line_index(file_id).ok()?;
    // LineCol is 0-based, MCP uses 1-based
    let line_col = LineCol { line: line - 1, col: col - 1 };
    line_index.offset(line_col)
}

// byte offset → (line, col)
fn to_line_col(analysis: &Analysis, file_id: FileId, offset: TextSize) -> Option<(u32, u32)> {
    let line_index = analysis.file_line_index(file_id).ok()?;
    let lc = line_index.try_line_col(offset)?;
    Some((lc.line + 1, lc.col + 1))  // Convert to 1-based
}
```

### IDE Operations

```rust
use ra_ap_ide::{Analysis, FilePosition, GotoDefinitionConfig, NavigationTarget};

impl SemanticAnalyzer {
    fn goto_definition(&self, file: &Path, line: u32, col: u32) -> Vec<NavigationTarget> {
        let file_id = self.path_to_file_id(file)?;
        let offset = self.to_offset(file_id, line, col)?;
        let position = FilePosition { file_id, offset };
        let config = GotoDefinitionConfig::default();

        let analysis = self.host.analysis();
        analysis.goto_definition(position, &config)
            .ok()
            .flatten()
            .map(|ri| ri.info)
            .unwrap_or_default()
    }

    fn find_references(&self, file: &Path, line: u32, col: u32) -> Vec<ReferenceSearchResult> {
        let position = self.to_position(file, line, col)?;
        let config = FindAllRefsConfig::default();

        self.host.analysis()
            .find_all_refs(position, &config)
            .ok()
            .flatten()
            .unwrap_or_default()
    }

    fn call_hierarchy(&self, file: &Path, line: u32, col: u32) -> CallHierarchy {
        let position = self.to_position(file, line, col)?;
        let config = CallHierarchyConfig::default();
        let analysis = self.host.analysis();

        CallHierarchy {
            incoming: analysis.incoming_calls(&config, position).ok().flatten(),
            outgoing: analysis.outgoing_calls(&config, position).ok().flatten(),
        }
    }

    fn hover(&self, file: &Path, line: u32, col: u32) -> Option<HoverResult> {
        let file_id = self.path_to_file_id(file)?;
        let offset = self.to_offset(file_id, line, col)?;
        let range = FileRange { file_id, range: TextRange::new(offset, offset) };
        let config = HoverConfig::default();

        self.host.analysis().hover(&config, range).ok().flatten()
    }

    fn symbol_search(&self, query: &str, limit: usize) -> Vec<NavigationTarget> {
        let query = Query::new(query.to_string());
        self.host.analysis()
            .symbol_search(query, limit)
            .ok()
            .unwrap_or_default()
    }
}
```

## Tool Mapping

| MCP Tool | rust-analyzer API | Notes |
|----------|------------------|-------|
| `find_definition` | `Analysis::goto_definition` | Returns `NavigationTarget` with file/range |
| `find_references` | `Analysis::find_all_refs` | Returns grouped by file |
| `get_call_graph` | `Analysis::incoming_calls` + `outgoing_calls` | Per-function call hierarchy |
| `get_dependencies` | - | Keep syntax-based (import paths) |
| `analyze_complexity` | - | Keep syntax-based (LOC, metrics) |
| NEW: `hover` | `Analysis::hover` | Type info, docs |
| NEW: `symbol_search` | `Analysis::symbol_search` | Fuzzy symbol lookup |
| NEW: `goto_type_definition` | `Analysis::goto_type_definition` | Navigate to type |
| NEW: `goto_implementation` | `Analysis::goto_implementation` | Find trait impls |

## Implementation Plan

### Phase 1: Core Infrastructure

1. **Add dependencies to Cargo.toml:**
   ```toml
   ra_ap_ide = "0.0.295"
   ra_ap_ide_db = "0.0.295"
   ra_ap_load_cargo = "0.0.295"
   ra_ap_project_model = "0.0.295"
   ra_ap_vfs = "0.0.295"
   ra_ap_paths = "0.0.295"
   ```

2. **Create `src/semantic/mod.rs`:**
   - `SemanticAnalyzer` struct holding `AnalysisHost` and `Vfs`
   - Project loading logic
   - FileId/Position mapping utilities

3. **Create project cache:**
   - Store loaded projects in a `HashMap<PathBuf, SemanticAnalyzer>`
   - Lazy loading on first query
   - Consider memory limits / LRU eviction

### Phase 2: Migrate Tools

4. **Update `find_definition`:**
   - Accept either (symbol_name, directory) OR (file, line, col)
   - Use `Analysis::goto_definition` for semantic resolution
   - Fall back to syntax search if no semantic match

5. **Update `find_references`:**
   - Accept (file, line, col) for position-based lookup
   - Use `Analysis::find_all_refs`
   - Return results grouped by file

6. **Update `get_call_graph`:**
   - Accept (file, line, col) to identify function
   - Use `incoming_calls` and `outgoing_calls`
   - Build caller/callee graph

### Phase 3: New Tools

7. **Add `hover` tool:**
   - Returns type information and documentation
   - Useful for understanding symbol without navigation

8. **Add `symbol_search` tool:**
   - Fuzzy symbol search across workspace
   - Useful for finding symbols without exact name

9. **Add `goto_implementation` tool:**
   - Find implementations of traits/interfaces

### Phase 4: Performance & Polish

10. **Caching strategy:**
    - Keep AnalysisHost in memory (MCP server is long-running)
    - Use `prefill_caches: true` for faster first queries
    - Consider file watching for incremental updates

11. **Error handling:**
    - Graceful degradation if semantic analysis fails
    - Fall back to syntax-based tools

12. **Testing:**
    - Test with various project sizes
    - Measure startup time and query latency
    - Memory usage monitoring

## Performance Considerations

### Startup Time

| Project Size | Load Time (approx) |
|-------------|-------------------|
| Small (< 10 crates) | 1-3 seconds |
| Medium (10-50 crates) | 3-10 seconds |
| Large (50+ crates) | 10-30 seconds |

**Mitigation:**
- Lazy loading (load on first query)
- Background loading
- Cache warm state

### Memory Usage

| Project Size | Memory (approx) |
|-------------|-----------------|
| Small | 100-300 MB |
| Medium | 300-800 MB |
| Large | 800 MB - 2 GB |

**Mitigation:**
- LRU cache for multiple projects
- Unload unused projects
- Consider `RA_LRU_CAP` environment variable

### Query Latency

| Operation | Typical Latency |
|-----------|----------------|
| goto_definition | 1-50 ms |
| find_all_refs | 10-200 ms |
| call_hierarchy | 50-500 ms |
| hover | 1-20 ms |
| symbol_search | 50-300 ms |

## Dependencies to Add

```toml
# In Cargo.toml
# Note: Use matching versions for all ra_ap_* crates

# Core IDE functionality
ra_ap_ide = "0.0.295"

# Project loading
ra_ap_load_cargo = "0.0.295"
ra_ap_project_model = "0.0.295"

# Virtual file system
ra_ap_vfs = "0.0.295"
ra_ap_vfs_notify = "0.0.295"  # For file watching (optional)

# Path handling
ra_ap_paths = "0.0.295"
```

## Files to Modify

1. `Cargo.toml` - Add dependencies
2. `src/lib.rs` - Add `semantic` module
3. `src/semantic/mod.rs` - New: SemanticAnalyzer
4. `src/semantic/position.rs` - New: Position mapping utilities
5. `src/tools/analysis_tools.rs` - Migrate to semantic APIs
6. `src/server.rs` - Project cache management

## Proof of Concept

See `examples/ide_poc.rs` for a working demonstration of the integration pattern.

## References

- [rust-analyzer source](https://github.com/rust-lang/rust-analyzer)
- [ra_ap_* crates on crates.io](https://crates.io/search?q=ra_ap)
- [rust-codemaps builder.rs](../../../rust-codemaps/src/builder.rs) - Low-level example

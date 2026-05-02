# Handoff Prompt: rust-analyzer IDE Integration for rust-code-mcp (Session 3)

## Project Context

- **Repository**: `/home/molaco/Documents/rust-code-mcp-final`
- **Goal**: Add semantic code navigation (goto definition, find references) using rust-analyzer's `ra_ap_ide` crate
- **Rust Edition**: 2024 (nightly 1.94.0)

---

## What Was Accomplished in Sessions 1-2

### Session 1: Benchmarking & Proof of Concept
- Benchmarked syntax-only vs IDE loading approaches
- Created example files demonstrating IDE APIs
- Added IDE dependencies to Cargo.toml (behind `ide` feature flag)

### Session 2: Architecture Analysis & Decision Making
- Explored current codebase architecture
- Compared `no_deps=true` vs `no_deps=false` loading
- Tested local type navigation capabilities
- Compared syntax-only vs IDE functionality
- Finalized architecture recommendation

---

## Key Decisions Made

### 1. Use `no_deps=true` (NOT full deps)

```rust
let cargo_config = CargoConfig {
    sysroot: None,      // Don't load stdlib
    no_deps: true,      // Only local project crates
    ..Default::default()
};
```

**Why:**
- Load time: ~120ms vs ~12s (100x faster)
- Memory: ~10-20MB vs ~400-500MB
- Sufficient for navigating LOCAL code (which is 99% of use cases)
- Users don't need to navigate INTO Vec's source code

**Limitation:** Can't navigate to dependency source code (acceptable trade-off)

### 2. Keep Syntax Parsing for Indexing

The current `RustParser` (`ra_ap_syntax`) stays for:
- Chunking code during indexing
- Fast parallel processing with Rayon
- Symbol extraction for embeddings

**Why:** Syntax parsing is fast (~0.1ms/file), parallel-friendly, and sufficient for chunking.

### 3. Lazy Load IDE on First Semantic Query

- Don't load IDE during indexing
- Load when user first calls `find_definition` or `find_references`
- Cache the loaded project for subsequent queries

**Why:** Users who only use search don't pay IDE loading cost.

### 4. Position-Based API (file, line, col)

**Old API returns ALL matches:**
```
find_definition("Config") → [utils::Config, network::Config]  // ambiguous
```

**New API returns THE match:**
```
find_definition(file, line, col) → utils::Config  // precise
```

**No fallbacks** - position-based API only (no `#[cfg(not(feature = "ide"))]` variants).

---

## Benchmark Results (from testing)

| Configuration               | Load Time | Memory | Crates |
|-----------------------------|-----------|--------|--------|
| no_deps=true, no sysroot    | 84ms      | ~10MB  | 22     |
| no_deps=true, with sysroot  | 387ms     | ~20MB  | 27     |
| no_deps=false, with sysroot | 11.7s     | ~400MB | 649    |

First query after load: Instant (with `prefill_caches=true`)

---

## Verified Working Code

### Loading a Project

```rust
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;

fn load_project(path: &Path) -> Result<(AnalysisHost, Vfs)> {
    let cargo_config = CargoConfig {
        sysroot: None,
        no_deps: true,
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: true,  // Important: pre-compute type info
    };

    let (db, vfs, _) = load_workspace_at(path, &cargo_config, &load_config, &|_| {})?;
    let host = AnalysisHost::with_database(db);

    Ok((host, vfs))
}
```

### Goto Definition

```rust
fn goto_definition(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,    // 1-based
    column: u32,  // 1-based
) -> Result<Vec<NavigationTarget>> {
    let analysis = host.analysis();

    // Path → FileId
    let vfs_path = ra_ap_vfs::VfsPath::new_real_path(file_path.to_path_buf());
    let file_id = vfs.file_id(&vfs_path)
        .ok_or("file not in vfs")?
        .0;  // Returns (FileId, FileExcluded)

    // Line/Col → Offset (LineCol is 0-based internally)
    let line_index = analysis.file_line_index(file_id)?;
    let offset = line_index.offset(ra_ap_ide::LineCol {
        line: line - 1,
        col: column - 1,
    }).ok_or("invalid position")?;

    // Query
    let position = ra_ap_ide::FilePosition { file_id, offset };
    let config = ra_ap_ide::GotoDefinitionConfig { minicore: Default::default() };

    let result = analysis.goto_definition(position, &config)?;

    Ok(result.map(|r| r.info).unwrap_or_default())
}
```

### Converting NavigationTarget to Location

```rust
struct Location {
    file_path: PathBuf,
    line: u32,
    column: u32,
    name: String,
}

fn nav_target_to_location(vfs: &Vfs, analysis: &Analysis, target: &NavigationTarget) -> Result<Location> {
    let file_path = vfs.file_path(target.file_id)
        .as_path()
        .ok_or("not a real path")?
        .to_path_buf();

    let line_index = analysis.file_line_index(target.file_id)?;
    let offset = target.focus_range.unwrap_or(target.full_range).start();
    let line_col = line_index.line_col(offset);

    Ok(Location {
        file_path,
        line: line_col.line + 1,    // Convert to 1-based
        column: line_col.col + 1,   // Convert to 1-based
        name: target.name.to_string(),
    })
}
```

---

## Current Codebase Structure

```
src/
├── lib.rs                      # Add: pub mod semantic;
├── parser/
│   └── mod.rs                  # Current syntax parser (KEEP)
├── indexing/
│   ├── unified.rs              # Indexing pipeline (KEEP AS-IS)
│   └── indexer_core.rs         # Uses RustParser (KEEP AS-IS)
├── tools/
│   ├── mod.rs                  # Tool exports
│   ├── analysis_tools.rs       # MODIFY: use SemanticService
│   ├── index_tool.rs           # KEEP AS-IS
│   └── search_tool.rs          # KEEP AS-IS
├── mcp/
│   └── mod.rs                  # MCP server (add SemanticService state)
└── semantic/                   # NEW MODULE TO CREATE
    ├── mod.rs
    ├── loader.rs
    └── position.rs
```

---

## Files to Create

### 1. `src/semantic/mod.rs`

```rust
//! Semantic code analysis using rust-analyzer
//!
//! Provides precise goto-definition and find-references using ra_ap_ide.
//! Loaded lazily on first semantic query.

mod loader;
mod position;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;
use anyhow::Result;

pub use position::Location;

/// Global semantic service instance
pub static SEMANTIC: LazyLock<SemanticService> = LazyLock::new(SemanticService::new);

/// Cached project context
struct ProjectContext {
    host: AnalysisHost,
    vfs: Vfs,
}

/// Service for semantic code queries
pub struct SemanticService {
    projects: RwLock<HashMap<PathBuf, ProjectContext>>,
}

impl SemanticService {
    pub fn new() -> Self {
        Self {
            projects: RwLock::new(HashMap::new()),
        }
    }

    /// Get or load project (lazy loading)
    fn get_or_load(&self, project_path: &Path) -> Result<()> {
        let canonical = project_path.canonicalize()?;

        // Check if already loaded (read lock)
        {
            let projects = self.projects.read().unwrap();
            if projects.contains_key(&canonical) {
                return Ok(());
            }
        }

        // Load project (write lock)
        {
            let mut projects = self.projects.write().unwrap();
            // Double-check after acquiring write lock
            if projects.contains_key(&canonical) {
                return Ok(());
            }

            tracing::info!("Loading IDE for project: {}", canonical.display());
            let (host, vfs) = loader::load_project(&canonical)?;
            projects.insert(canonical, ProjectContext { host, vfs });
            tracing::info!("IDE loaded successfully");
        }

        Ok(())
    }

    /// Goto definition at position
    pub fn goto_definition(
        &self,
        project_path: &Path,
        file_path: &Path,
        line: u32,
        column: u32,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let projects = self.projects.read().unwrap();
        let canonical = project_path.canonicalize()?;
        let ctx = projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::goto_definition(&ctx.host, &ctx.vfs, file_path, line, column)
    }

    /// Find all references at position
    pub fn find_references(
        &self,
        project_path: &Path,
        file_path: &Path,
        line: u32,
        column: u32,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let projects = self.projects.read().unwrap();
        let canonical = project_path.canonicalize()?;
        let ctx = projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::find_references(&ctx.host, &ctx.vfs, file_path, line, column)
    }

    /// Reload project (call when files change significantly)
    pub fn reload(&self, project_path: &Path) -> Result<()> {
        let canonical = project_path.canonicalize()?;

        tracing::info!("Reloading IDE for project: {}", canonical.display());
        let (host, vfs) = loader::load_project(&canonical)?;

        let mut projects = self.projects.write().unwrap();
        projects.insert(canonical, ProjectContext { host, vfs });
        tracing::info!("IDE reloaded successfully");

        Ok(())
    }

    /// Invalidate cached project
    pub fn invalidate(&self, project_path: &Path) {
        if let Ok(canonical) = project_path.canonicalize() {
            let mut projects = self.projects.write().unwrap();
            projects.remove(&canonical);
        }
    }
}
```

### 2. `src/semantic/loader.rs`

```rust
//! Project loading with rust-analyzer

use std::path::Path;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;
use anyhow::{Result, Context};

/// Load a Cargo project for semantic analysis
///
/// Uses no_deps=true for fast loading (~120ms).
/// Only local project code is analyzed.
pub fn load_project(path: &Path) -> Result<(AnalysisHost, Vfs)> {
    let cargo_config = CargoConfig {
        sysroot: None,
        no_deps: true,
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: true,
    };

    let (db, vfs, _) = load_workspace_at(path, &cargo_config, &load_config, &|_| {})
        .context("Failed to load workspace")?;

    let host = AnalysisHost::with_database(db);

    Ok((host, vfs))
}
```

### 3. `src/semantic/position.rs`

```rust
//! Position and coordinate utilities

use std::path::{Path, PathBuf};
use ra_ap_ide::{AnalysisHost, FilePosition, LineCol, NavigationTarget, TextSize};
use ra_ap_vfs::{Vfs, VfsPath};
use anyhow::{Result, Context};

/// A source code location
#[derive(Debug, Clone)]
pub struct Location {
    pub file_path: PathBuf,
    pub line: u32,      // 1-based
    pub column: u32,    // 1-based
    pub name: String,
}

impl std::fmt::Display for Location {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{} ({})", self.file_path.display(), self.line, self.column, self.name)
    }
}

/// Convert file path to FileId
fn path_to_file_id(vfs: &Vfs, file_path: &Path) -> Result<ra_ap_vfs::FileId> {
    let abs_path = file_path.canonicalize()
        .context("Failed to canonicalize path")?;
    let vfs_path = VfsPath::new_real_path(abs_path);

    vfs.file_id(&vfs_path)
        .map(|(id, _)| id)
        .ok_or_else(|| anyhow::anyhow!("File not found in VFS: {}", file_path.display()))
}

/// Convert line/column to byte offset
fn to_offset(
    analysis: &ra_ap_ide::Analysis,
    file_id: ra_ap_vfs::FileId,
    line: u32,
    column: u32,
) -> Result<TextSize> {
    let line_index = analysis.file_line_index(file_id)
        .context("Failed to get line index")?;

    // LineCol is 0-based, input is 1-based
    let line_col = LineCol {
        line: line.saturating_sub(1),
        col: column.saturating_sub(1),
    };

    line_index.offset(line_col)
        .ok_or_else(|| anyhow::anyhow!("Invalid position: line {}, col {}", line, column))
}

/// Convert NavigationTarget to Location
fn nav_target_to_location(
    vfs: &Vfs,
    analysis: &ra_ap_ide::Analysis,
    target: &NavigationTarget,
) -> Result<Location> {
    let vfs_path = vfs.file_path(target.file_id);
    let file_path = vfs_path.as_path()
        .ok_or_else(|| anyhow::anyhow!("Not a real path"))?
        .to_path_buf();

    let line_index = analysis.file_line_index(target.file_id)?;
    let offset = target.focus_range.unwrap_or(target.full_range).start();
    let line_col = line_index.line_col(offset);

    Ok(Location {
        file_path,
        line: line_col.line + 1,
        column: line_col.col + 1,
        name: target.name.to_string(),
    })
}

/// Goto definition at position
pub fn goto_definition(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<Vec<Location>> {
    let analysis = host.analysis();
    let file_id = path_to_file_id(vfs, file_path)?;
    let offset = to_offset(&analysis, file_id, line, column)?;

    let position = FilePosition { file_id, offset };
    let config = ra_ap_ide::GotoDefinitionConfig { minicore: Default::default() };

    let result = analysis.goto_definition(position, &config)
        .context("goto_definition query failed")?;

    match result {
        Some(nav_info) => {
            nav_info.info
                .iter()
                .map(|target| nav_target_to_location(vfs, &analysis, target))
                .collect()
        }
        None => Ok(vec![]),
    }
}

/// Find all references at position
pub fn find_references(
    host: &AnalysisHost,
    vfs: &Vfs,
    file_path: &Path,
    line: u32,
    column: u32,
) -> Result<Vec<Location>> {
    let analysis = host.analysis();
    let file_id = path_to_file_id(vfs, file_path)?;
    let offset = to_offset(&analysis, file_id, line, column)?;

    let position = FilePosition { file_id, offset };
    let config = ra_ap_ide::FindAllRefsConfig { minicore: Default::default() };

    let result = analysis.find_all_refs(position, &config)
        .context("find_all_refs query failed")?;

    match result {
        Some(search_results) => {
            let mut locations = Vec::new();

            // Add declaration
            if let Some(decl) = &search_results.declaration {
                if let Some(nav) = &decl.nav {
                    locations.push(nav_target_to_location(vfs, &analysis, nav)?);
                }
            }

            // Add references
            for (file_id, refs) in &search_results.references {
                let vfs_path = vfs.file_path(*file_id);
                let file_path = vfs_path.as_path()
                    .ok_or_else(|| anyhow::anyhow!("Not a real path"))?
                    .to_path_buf();

                let line_index = analysis.file_line_index(*file_id)?;

                for (range, _category) in refs {
                    let line_col = line_index.line_col(range.start());
                    locations.push(Location {
                        file_path: file_path.clone(),
                        line: line_col.line + 1,
                        column: line_col.col + 1,
                        name: "reference".to_string(),
                    });
                }
            }

            Ok(locations)
        }
        None => Ok(vec![]),
    }
}
```

---

## Files to Modify

### 1. `src/lib.rs`

Add:
```rust
#[cfg(feature = "ide")]
pub mod semantic;
```

### 2. `src/tools/search_tool.rs`

Update parameter structs:

```rust
#[cfg(feature = "ide")]
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindDefinitionParams {
    #[schemars(description = "Absolute path to the source file")]
    pub file_path: String,
    #[schemars(description = "Line number (1-based)")]
    pub line: u32,
    #[schemars(description = "Column number (1-based)")]
    pub column: u32,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
}

#[cfg(feature = "ide")]
#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct FindReferencesParams {
    #[schemars(description = "Absolute path to the source file")]
    pub file_path: String,
    #[schemars(description = "Line number (1-based)")]
    pub line: u32,
    #[schemars(description = "Column number (1-based)")]
    pub column: u32,
    #[schemars(description = "Project root directory containing Cargo.toml")]
    pub directory: String,
}
```

### 3. `src/tools/analysis_tools.rs`

Replace `find_definition` and `find_references` implementations:

```rust
#[cfg(feature = "ide")]
use crate::semantic::SEMANTIC;

/// Find the definition of a symbol at a specific position
#[cfg(feature = "ide")]
pub async fn find_definition(
    file_path: &str,
    line: u32,
    column: u32,
    directory: &str,
) -> Result<CallToolResult, McpError> {
    use std::path::Path;

    let project_path = Path::new(directory);
    let file = Path::new(file_path);

    let locations = SEMANTIC
        .goto_definition(project_path, file, line, column)
        .map_err(|e| McpError::internal_error(format!("Goto definition failed: {}", e), None))?;

    if locations.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(
            "No definition found at this position"
        )]))
    } else {
        let result = locations
            .iter()
            .map(|loc| loc.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Found {} definition(s):\n{}",
            locations.len(),
            result
        ))]))
    }
}

/// Find all references to the symbol at a specific position
#[cfg(feature = "ide")]
pub async fn find_references(
    file_path: &str,
    line: u32,
    column: u32,
    directory: &str,
) -> Result<CallToolResult, McpError> {
    use std::path::Path;

    let project_path = Path::new(directory);
    let file = Path::new(file_path);

    let locations = SEMANTIC
        .find_references(project_path, file, line, column)
        .map_err(|e| McpError::internal_error(format!("Find references failed: {}", e), None))?;

    if locations.is_empty() {
        Ok(CallToolResult::success(vec![Content::text(
            "No references found at this position"
        )]))
    } else {
        let result = locations
            .iter()
            .map(|loc| loc.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        Ok(CallToolResult::success(vec![Content::text(format!(
            "Found {} reference(s):\n{}",
            locations.len(),
            result
        ))]))
    }
}
```

### 4. `src/tools/search_tool_router.rs`

Update the tool handlers:

```rust
#[cfg(feature = "ide")]
#[tool(description = "Find where a Rust symbol is defined at a specific position")]
async fn find_definition(
    &self,
    Parameters(FindDefinitionParams {
        file_path,
        line,
        column,
        directory,
    }): Parameters<FindDefinitionParams>,
) -> Result<CallToolResult, McpError> {
    crate::tools::analysis_tools::find_definition(&file_path, line, column, &directory).await
}

#[cfg(feature = "ide")]
#[tool(description = "Find all references to the symbol at a specific position")]
async fn find_references(
    &self,
    Parameters(FindReferencesParams {
        file_path,
        line,
        column,
        directory,
    }): Parameters<FindReferencesParams>,
) -> Result<CallToolResult, McpError> {
    crate::tools::analysis_tools::find_references(&file_path, line, column, &directory).await
}
```

---

## Dependencies

Already in `Cargo.toml`:

```toml
[features]
ide = ["ra_ap_ide", "ra_ap_ide_db", "ra_ap_load-cargo", "ra_ap_project_model", "ra_ap_vfs", "ra_ap_paths", "ra_ap_hir"]

# All at version 0.0.313
ra_ap_ide = { version = "0.0.313", optional = true }
ra_ap_ide_db = { version = "0.0.313", optional = true }
"ra_ap_load-cargo" = { version = "0.0.313", optional = true }
ra_ap_project_model = { version = "0.0.313", optional = true }
ra_ap_vfs = { version = "0.0.313", optional = true }
ra_ap_paths = { version = "0.0.313", optional = true }
ra_ap_hir = { version = "0.0.313", optional = true }
```

**No additional dependencies needed** - uses `std::sync::{LazyLock, RwLock}`.

---

## API Quirks Discovered

### 1. GotoDefinitionConfig requires minicore

```rust
let config = ra_ap_ide::GotoDefinitionConfig {
    minicore: Default::default(),  // Required field
};
```

### 2. FindAllRefsConfig also requires minicore

```rust
let config = ra_ap_ide::FindAllRefsConfig {
    minicore: Default::default(),
};
```

### 3. VfsPath construction and file_id

```rust
// Correct way:
let vfs_path = VfsPath::new_real_path(path.to_path_buf());

// file_id returns (FileId, FileExcluded):
let file_id = vfs.file_id(&vfs_path).map(|(id, _)| id);
```

### 4. file_path panics if FileId is invalid

```rust
// UNSAFE - panics if file_id doesn't exist:
let path = vfs.file_path(file_id);

// SAFE - check first:
if vfs.exists(file_id) {
    let path = vfs.file_path(file_id);
}
```

### 5. LineCol is 0-based, MCP tools use 1-based

Always convert:
```rust
// Input (1-based) → rust-analyzer (0-based)
let line_col = LineCol {
    line: line - 1,
    col: column - 1,
};

// rust-analyzer (0-based) → Output (1-based)
let line = line_col.line + 1;
let column = line_col.col + 1;
```

---

## VFS Update / Error Recovery

When files change, call `reload()` to refresh the semantic model:

```rust
// In your file watcher or SyncManager integration:
if rust_file_changed {
    SEMANTIC.reload(project_path)?;
}
```

The `reload()` method:
1. Reloads the entire project (~120ms)
2. Replaces the cached `ProjectContext`
3. Subsequent queries use fresh data

For fine-grained updates (future optimization):
```rust
// This would require mutable VFS access:
vfs.set_file_contents(path, Some(new_content.into_bytes()));
let changes = vfs.take_changes();
// Build Change and apply to host
```

---

## Example Files (for reference/testing)

| File                            | Purpose                             |
|---------------------------------|-------------------------------------|
| `examples/ide_load_benchmark.rs`  | Benchmarks load times               |
| `examples/ide_functional_test.rs` | Tests IDE queries work              |
| `examples/ide_deps_comparison.rs` | Compares no_deps vs with_deps       |
| `examples/ide_local_types.rs`     | Tests local type navigation         |
| `examples/syntax_vs_ide.rs`       | Compares syntax vs IDE capabilities |

Run any of them with:
```bash
cargo run --example <name> --features ide --release
```

---

## Testing Commands

```bash
# Build with IDE feature
cargo build --features ide --release

# Run existing examples
cargo run --example ide_local_types --features ide --release

# Test the MCP server
cargo run --features ide --release

# Run tests
cargo test --features ide
```

---

## Implementation Checklist

- [ ] Create `src/semantic/mod.rs`
- [ ] Create `src/semantic/loader.rs`
- [ ] Create `src/semantic/position.rs`
- [ ] Add `pub mod semantic;` to `src/lib.rs`
- [ ] Update `FindDefinitionParams` schema in `search_tool.rs`
- [ ] Update `FindReferencesParams` schema in `search_tool.rs`
- [ ] Implement new `find_definition` function in `analysis_tools.rs`
- [ ] Implement new `find_references` function in `analysis_tools.rs`
- [ ] Update tool handlers in `search_tool_router.rs`
- [ ] Test with `cargo run --example ide_local_types --features ide --release`
- [ ] Test MCP tools manually
- [ ] Update tool documentation

---

## What NOT to Change

- `src/parser/` - Keep syntax parsing for indexing
- `src/indexing/` - Keep current indexing pipeline
- `src/tools/index_tool.rs` - Keep as-is
- `src/tools/search_tool.rs` - Keep search functionality as-is
- `analyze_complexity` function - Keep syntax-based
- `get_call_graph` function - Keep syntax-based
- `get_dependencies` function - Keep syntax-based

---

## Success Criteria

1. `find_definition(file, line, col)` returns THE definition (not all matches)
2. `find_references(file, line, col)` returns all usages
3. Local types resolve correctly
4. First query takes ~120ms (IDE load)
5. Subsequent queries are instant (<10ms)
6. Dependency types return "not found" (expected with `no_deps=true`)

---

## Key Changes from Original Plan

| Item | Original | Updated |
|------|----------|---------|
| Lazy static | `once_cell::sync::Lazy` | `std::sync::LazyLock` (std, no dep) |
| Concurrency | `Mutex` | `std::sync::RwLock` |
| Fallback API | `#[cfg(not(feature = "ide"))]` | None - position-based only |
| VFS recovery | Not specified | `reload()` method added |
| file_id return | Unclear | `Option<(FileId, FileExcluded)>` |

---

Good luck with Session 3!

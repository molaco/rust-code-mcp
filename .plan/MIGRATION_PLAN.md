# Tree-Sitter to Rust-Analyzer Migration Plan

## Executive Summary

Replace tree-sitter with `ra_ap_syntax` for parsing, enabling:
- **24x faster parsing** (parallel with Rayon)
- **Cleaner API** (typed AST vs string matching)
- **Future semantic analysis** capability via `ra_ap_ide`

---

## Phase 1: Research Findings Summary

### 1.1 Files Requiring Modification

| File | Dependency Type | Changes Required |
|------|----------------|------------------|
| `src/parser/mod.rs` | **CORE** - Direct tree-sitter | Complete rewrite |
| `src/parser/call_graph.rs` | Direct tree traversal | Complete rewrite |
| `src/parser/imports.rs` | Direct tree traversal | Complete rewrite |
| `src/parser/type_references.rs` | Direct tree traversal | Complete rewrite |
| `src/indexing/indexer_core.rs` | Uses RustParser | No changes (API preserved) |
| `src/chunker/mod.rs` | Uses ParseResult | No changes (API preserved) |
| `src/tools/analysis_tools.rs` | Uses RustParser | No changes (API preserved) |
| `Cargo.toml` | Dependencies | Add ra_ap_syntax, remove tree-sitter |

### 1.2 Public API to Preserve

```rust
// These signatures MUST remain identical:
pub struct RustParser;
impl RustParser {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>>;
    pub fn parse_file(&mut self, path: &Path) -> Result<Vec<Symbol>, Box<dyn std::error::Error>>;
    pub fn parse_source(&mut self, source: &str) -> Result<Vec<Symbol>, Box<dyn std::error::Error>>;
    pub fn parse_file_complete(&mut self, path: &Path) -> Result<ParseResult, Box<dyn std::error::Error>>;
    pub fn parse_source_complete(&mut self, source: &str) -> Result<ParseResult, Box<dyn std::error::Error>>;
}

pub struct ParseResult {
    pub symbols: Vec<Symbol>,
    pub call_graph: CallGraph,
    pub imports: Vec<Import>,
    pub type_references: Vec<TypeReference>,
}

pub struct Symbol { pub kind: SymbolKind, pub name: String, pub range: Range, pub docstring: Option<String>, pub visibility: Visibility }
pub enum SymbolKind { Function { is_async, is_unsafe, is_const }, Struct, Enum, Trait, Impl { trait_name, type_name }, Module, Const, Static, TypeAlias }
pub enum Visibility { Public, Crate, Restricted(String), Private }
pub struct Range { pub start_line: usize, pub end_line: usize, pub start_byte: usize, pub end_byte: usize }

pub struct CallGraph;  // Same methods: new, build, add_call, get_callees, get_callers, all_functions, edge_count, has_call
pub struct Import { pub path: String, pub is_glob: bool, pub items: Vec<String> }
pub struct TypeReference { pub type_name: String, pub usage_context: TypeUsageContext, pub line: usize }
pub enum TypeUsageContext { FunctionParameter{}, FunctionReturn{}, StructField{}, ImplBlock{}, LetBinding, GenericArgument }
```

### 1.3 Consumer Dependencies

| Consumer | ParseResult Fields Used |
|----------|------------------------|
| `Chunker.chunk_file()` | symbols, imports, call_graph |
| `find_definition()` | symbols (via parse_file) |
| `find_references()` | call_graph, type_references |
| `get_call_graph()` | call_graph |
| `get_dependencies()` | imports |
| `analyze_complexity()` | symbols, call_graph |

### 1.4 Thread Safety Requirements

- Each parallel task creates fresh `RustParser` instance
- Parser is NOT shared across threads
- `ra_ap_syntax::SourceFile::parse()` is stateless and thread-safe

---

## Phase 2: Implementation Steps

### Step 1: Update Dependencies (Cargo.toml)

```toml
# Remove:
tree-sitter = "0.20"
tree-sitter-rust = "0.20"

# Add:
ra_ap_syntax = "0.0.295"
```

### Step 2: Create New Parser Implementation

**File: `src/parser/ra_parser.rs`** (new file)

```rust
use ra_ap_syntax::{
    ast::{self, HasDocComments, HasGenericArgs, HasModuleItem, HasName, HasVisibility},
    AstNode, AstToken, Edition, SourceFile, SyntaxKind,
};
use std::path::Path;
use std::fs;

// Re-use existing data structures from mod.rs
use super::{Symbol, SymbolKind, Visibility, Range, ParseResult};
use super::call_graph::CallGraph;
use super::imports::Import;
use super::type_references::{TypeReference, TypeUsageContext};

pub struct RustParser;  // Stateless - no internal parser needed

impl RustParser {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self)
    }

    pub fn parse_file(&mut self, path: &Path) -> Result<Vec<Symbol>, Box<dyn std::error::Error>> {
        let source = fs::read_to_string(path)?;
        self.parse_source(&source)
    }

    pub fn parse_source(&mut self, source: &str) -> Result<Vec<Symbol>, Box<dyn std::error::Error>> {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();
        let mut symbols = Vec::new();
        extract_symbols_recursive(&file, source, &mut symbols);
        Ok(symbols)
    }

    pub fn parse_file_complete(&mut self, path: &Path) -> Result<ParseResult, Box<dyn std::error::Error>> {
        let source = fs::read_to_string(path)?;
        self.parse_source_complete(&source)
    }

    pub fn parse_source_complete(&mut self, source: &str) -> Result<ParseResult, Box<dyn std::error::Error>> {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();

        let mut symbols = Vec::new();
        extract_symbols_recursive(&file, source, &mut symbols);

        let call_graph = build_call_graph(&file, source);
        let imports = extract_imports(&file);
        let type_references = extract_type_references(&file, source);

        Ok(ParseResult {
            symbols,
            call_graph,
            imports,
            type_references,
        })
    }
}

// Helper functions for extraction (implemented in tests/ra_full_replacement_test.rs)
fn extract_symbols_recursive(file: &SourceFile, source: &str, symbols: &mut Vec<Symbol>) { ... }
fn build_call_graph(file: &SourceFile, source: &str) -> CallGraph { ... }
fn extract_imports(file: &SourceFile) -> Vec<Import> { ... }
fn extract_type_references(file: &SourceFile, source: &str) -> Vec<TypeReference> { ... }
```

### Step 3: Update Module Structure

**File: `src/parser/mod.rs`**

```rust
// Option A: Feature flag approach
#[cfg(feature = "ra_syntax")]
mod ra_parser;
#[cfg(feature = "ra_syntax")]
pub use ra_parser::RustParser;

#[cfg(not(feature = "ra_syntax"))]
mod ts_parser;  // Rename current implementation
#[cfg(not(feature = "ra_syntax"))]
pub use ts_parser::RustParser;

// Keep these unchanged:
pub mod call_graph;
pub mod imports;
pub mod type_references;

// Re-exports unchanged
pub use call_graph::CallGraph;
pub use imports::{extract_imports, get_external_dependencies, Import};
pub use type_references::{TypeReference, TypeReferenceTracker, TypeUsageContext};
```

### Step 4: Migrate Sub-Modules

**call_graph.rs changes:**
- Remove: `use tree_sitter::{Node, Tree};`
- Change `build(tree: &Tree, source: &str)` to `build(file: &SourceFile, source: &str)`
- Use `SyntaxKind::CALL_EXPR` and `SyntaxKind::METHOD_CALL_EXPR` instead of string matching

**imports.rs changes:**
- Remove: `use tree_sitter::{Node, Tree};`
- Change `extract_imports(tree: &Tree, source: &str)` to `extract_imports(file: &SourceFile)`
- Use `ast::Use` and `ast::UseTree` instead of node walking

**type_references.rs changes:**
- Remove: `use tree_sitter::{Node, Tree};`
- Change `build(tree: &Tree, source: &str)` to `build(file: &SourceFile, source: &str)`
- Use typed AST nodes for extraction

### Step 5: Update Tests

1. Keep all existing tests (they test the API, not implementation)
2. Add feature flag to run tests with both implementations
3. Create comparison test that runs both and validates identical output

```rust
#[test]
fn test_api_compatibility() {
    let source = "pub fn hello() {}";

    #[cfg(feature = "ra_syntax")]
    let mut parser = ra_parser::RustParser::new().unwrap();
    #[cfg(not(feature = "ra_syntax"))]
    let mut parser = ts_parser::RustParser::new().unwrap();

    let result = parser.parse_source(source).unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].name, "hello");
}
```

### Step 6: Validate and Remove Tree-Sitter

1. Run full test suite with `--features ra_syntax`
2. Run comparison tests to verify identical output
3. Remove tree-sitter code and feature flag
4. Update Cargo.toml to remove tree-sitter dependencies

---

## Phase 3: Implementation Order

```
Week 1: Foundation
├── Step 1: Update Cargo.toml (add ra_ap_syntax as optional)
├── Step 2: Create ra_parser.rs with basic structure
└── Step 3: Implement extract_symbols_recursive()

Week 2: Core Features
├── Step 4: Implement build_call_graph()
├── Step 5: Implement extract_imports()
└── Step 6: Implement extract_type_references()

Week 3: Integration
├── Step 7: Update mod.rs with feature flag
├── Step 8: Run all existing tests
└── Step 9: Fix any API mismatches

Week 4: Validation & Cleanup
├── Step 10: Create side-by-side comparison tests
├── Step 11: Performance benchmarks
└── Step 12: Remove tree-sitter, make ra_syntax default
```

---

## Phase 4: Validation Checklist

### API Compatibility
- [ ] `RustParser::new()` returns same type
- [ ] `parse_source()` returns identical `Vec<Symbol>`
- [ ] `parse_source_complete()` returns identical `ParseResult`
- [ ] All `SymbolKind` variants populated correctly
- [ ] `Range` has correct 1-indexed lines
- [ ] `CallGraph` finds same edges
- [ ] `Import` paths match exactly
- [ ] `TypeReference` contexts match

### Test Coverage
- [ ] All 28 existing parser tests pass
- [ ] All integration tests pass
- [ ] Chunker produces identical output
- [ ] MCP tools return same results

### Performance
- [ ] Sequential parsing ≥ 3x faster
- [ ] Parallel parsing ≥ 20x faster
- [ ] Memory usage similar or lower

---

## Files Changed Summary

| Action | File |
|--------|------|
| **CREATE** | `src/parser/ra_parser.rs` |
| **MODIFY** | `src/parser/mod.rs` |
| **MODIFY** | `src/parser/call_graph.rs` |
| **MODIFY** | `src/parser/imports.rs` |
| **MODIFY** | `src/parser/type_references.rs` |
| **MODIFY** | `Cargo.toml` |
| **DELETE** | (tree-sitter code after validation) |

---

## Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| API mismatch | Medium | High | Extensive comparison tests |
| Performance regression | Low | Medium | Benchmarks before merge |
| Edge case differences | Medium | Low | Existing tests + new edge case tests |
| ra_ap_syntax API changes | Medium | Medium | Pin exact version |

---

## Success Criteria

1. All 28 existing parser tests pass
2. All integration tests pass
3. Parallel parsing benchmark shows ≥10x improvement
4. No changes required in consumer code (indexer, chunker, tools)
5. Real codebase parsing produces identical results

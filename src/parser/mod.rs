//! Rust code parsing with rust-analyzer (ra_ap_syntax)
//!
//! Extracts symbols (functions, structs, traits, etc.) from Rust source files
//! and builds a call graph for understanding code relationships.

pub mod call_graph;
pub mod imports;
pub mod type_references;

use std::fs;
use std::path::Path;

use ra_ap_syntax::{
    ast::{self, HasDocComments, HasModuleItem, HasName, HasVisibility},
    AstNode, AstToken, Edition, SourceFile,
};

pub use call_graph::CallGraph;
pub use imports::{extract_imports, extract_imports_from_ast, get_external_dependencies, Import};
pub use type_references::{TypeReference, TypeReferenceTracker, TypeUsageContext};

/// A symbol extracted from Rust code
#[derive(Debug, Clone, PartialEq)]
pub struct Symbol {
    /// The kind of symbol
    pub kind: SymbolKind,
    /// Symbol name
    pub name: String,
    /// Line range in source file
    pub range: Range,
    /// Documentation comment
    pub docstring: Option<String>,
    /// Visibility (pub, pub(crate), private)
    pub visibility: Visibility,
}

/// Types of symbols that can be extracted
#[derive(Debug, Clone, PartialEq)]
pub enum SymbolKind {
    /// Function with modifiers
    Function {
        is_async: bool,
        is_unsafe: bool,
        is_const: bool,
    },
    /// Struct definition
    Struct,
    /// Enum definition
    Enum,
    /// Trait definition
    Trait,
    /// Implementation block
    Impl {
        /// Trait being implemented (None for inherent impls)
        trait_name: Option<String>,
        /// Type being implemented for
        type_name: String,
    },
    /// Module
    Module,
    /// Constant
    Const,
    /// Static variable
    Static,
    /// Type alias
    TypeAlias,
}

impl SymbolKind {
    pub fn as_str(&self) -> &str {
        match self {
            SymbolKind::Function { .. } => "function",
            SymbolKind::Struct => "struct",
            SymbolKind::Enum => "enum",
            SymbolKind::Trait => "trait",
            SymbolKind::Impl { .. } => "impl",
            SymbolKind::Module => "module",
            SymbolKind::Const => "const",
            SymbolKind::Static => "static",
            SymbolKind::TypeAlias => "type",
        }
    }
}

/// Visibility of a symbol
#[derive(Debug, Clone, PartialEq)]
pub enum Visibility {
    /// Public (pub)
    Public,
    /// Crate-local (pub(crate))
    Crate,
    /// Module-restricted (pub(in path))
    Restricted(String),
    /// Private (no pub keyword)
    Private,
}

/// Line range in source file (1-indexed)
#[derive(Debug, Clone, PartialEq)]
pub struct Range {
    pub start_line: usize,
    pub end_line: usize,
    pub start_byte: usize,
    pub end_byte: usize,
}

/// Complete parse result including symbols, call graph, imports, and type references
#[derive(Debug, Clone)]
pub struct ParseResult {
    /// Extracted symbols
    pub symbols: Vec<Symbol>,
    /// Call graph
    pub call_graph: CallGraph,
    /// Import statements
    pub imports: Vec<Import>,
    /// Type references
    pub type_references: Vec<TypeReference>,
}

/// Rust source code parser using rust-analyzer's syntax crate
pub struct RustParser {
    /// Edition of Rust to parse (affects syntax rules)
    edition: Edition,
}

impl RustParser {
    /// Create a new Rust parser
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(Self {
            edition: Edition::Edition2021,
        })
    }

    /// Create a parser for a specific Rust edition
    pub fn with_edition(edition: Edition) -> Self {
        Self { edition }
    }

    /// Parse a Rust source file and extract symbols
    pub fn parse_file(&mut self, path: &Path) -> Result<Vec<Symbol>, Box<dyn std::error::Error>> {
        let source = fs::read_to_string(path)?;
        self.parse_source(&source)
    }

    /// Parse Rust source code and extract symbols
    pub fn parse_source(&mut self, source: &str) -> Result<Vec<Symbol>, Box<dyn std::error::Error>> {
        let parse = SourceFile::parse(source, self.edition);
        let file = parse.tree();

        let mut symbols = Vec::new();
        extract_symbols_recursive(&file, source, &mut symbols);

        Ok(symbols)
    }

    /// Parse a file and extract everything: symbols, call graph, and imports
    pub fn parse_file_complete(
        &mut self,
        path: &Path,
    ) -> Result<ParseResult, Box<dyn std::error::Error>> {
        let source = fs::read_to_string(path)?;
        self.parse_source_complete(&source)
    }

    /// Parse source code and extract everything: symbols, call graph, imports, and type references
    ///
    /// This method parses the source only once and reuses the AST for all extractions,
    /// providing ~4x better performance than parsing separately for each component.
    pub fn parse_source_complete(
        &mut self,
        source: &str,
    ) -> Result<ParseResult, Box<dyn std::error::Error>> {
        // Parse once, reuse for all extractions
        let parse = SourceFile::parse(source, self.edition);
        let file = parse.tree();

        // Extract symbols from AST
        let mut symbols = Vec::new();
        extract_symbols_recursive(&file, source, &mut symbols);

        // Build call graph from AST (no re-parsing)
        let call_graph = CallGraph::build_from_ast(&file);

        // Extract imports from AST (no re-parsing)
        let imports = extract_imports_from_ast(&file);

        // Extract type references from AST (no re-parsing)
        let type_references = TypeReferenceTracker::build_from_ast(&file, source);

        Ok(ParseResult {
            symbols,
            call_graph,
            imports,
            type_references,
        })
    }
}

// ============================================================================
// HELPER FUNCTIONS
// ============================================================================

/// Calculate line number (1-indexed) from byte offset
fn line_of_offset(source: &str, offset: usize) -> usize {
    source[..offset.min(source.len())]
        .chars()
        .filter(|&c| c == '\n')
        .count()
        + 1
}

/// Extract visibility from an AST node
fn extract_visibility(vis: Option<ast::Visibility>) -> Visibility {
    match vis {
        None => Visibility::Private,
        Some(v) => {
            let text = v.syntax().text().to_string();
            if text == "pub" {
                Visibility::Public
            } else if text.starts_with("pub(crate)") {
                Visibility::Crate
            } else if text.starts_with("pub(") {
                Visibility::Restricted(text)
            } else {
                Visibility::Public
            }
        }
    }
}

/// Extract docstring from a node that has doc comments
fn extract_docstring<N: HasDocComments>(node: &N) -> Option<String> {
    let docs: Vec<String> = node
        .doc_comments()
        .map(|c| {
            let text = c.text();
            // Strip /// or //! prefix
            let stripped: &str = text
                .strip_prefix("///")
                .or_else(|| text.strip_prefix("//!"))
                .unwrap_or(text);
            stripped.trim().to_string()
        })
        .collect();

    if docs.is_empty() {
        None
    } else {
        Some(docs.join("\n"))
    }
}

/// Convert AST node to Range
fn node_to_range(node: &dyn AstNode, source: &str) -> Range {
    let range = node.syntax().text_range();
    let start_byte: usize = range.start().into();
    let end_byte: usize = range.end().into();

    Range {
        start_line: line_of_offset(source, start_byte),
        end_line: line_of_offset(source, end_byte),
        start_byte,
        end_byte,
    }
}

// ============================================================================
// SYMBOL EXTRACTION
// ============================================================================

/// Recursively extract symbols from a source file
fn extract_symbols_recursive(file: &SourceFile, source: &str, symbols: &mut Vec<Symbol>) {
    for item in file.items() {
        extract_item_symbols(&item, source, symbols);
    }
}

/// Extract symbols from a single AST item
fn extract_item_symbols(item: &ast::Item, source: &str, symbols: &mut Vec<Symbol>) {
    match item {
        ast::Item::Fn(f) => {
            if let Some(name) = f.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::Function {
                        is_async: f.async_token().is_some(),
                        is_unsafe: f.unsafe_token().is_some(),
                        is_const: f.const_token().is_some(),
                    },
                    range: node_to_range(f, source),
                    visibility: extract_visibility(f.visibility()),
                    docstring: extract_docstring(f),
                });
            }
        }
        ast::Item::Struct(s) => {
            if let Some(name) = s.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::Struct,
                    range: node_to_range(s, source),
                    visibility: extract_visibility(s.visibility()),
                    docstring: extract_docstring(s),
                });
            }
        }
        ast::Item::Enum(e) => {
            if let Some(name) = e.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::Enum,
                    range: node_to_range(e, source),
                    visibility: extract_visibility(e.visibility()),
                    docstring: extract_docstring(e),
                });
            }
        }
        ast::Item::Trait(t) => {
            if let Some(name) = t.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::Trait,
                    range: node_to_range(t, source),
                    visibility: extract_visibility(t.visibility()),
                    docstring: extract_docstring(t),
                });
            }
        }
        ast::Item::Impl(i) => {
            let type_name = i
                .self_ty()
                .map(|t| t.syntax().text().to_string())
                .unwrap_or_default();
            let trait_name = i.trait_().map(|t| t.syntax().text().to_string());

            symbols.push(Symbol {
                name: format!(
                    "impl {}{}",
                    trait_name
                        .as_ref()
                        .map(|t| format!("{} for ", t))
                        .unwrap_or_default(),
                    type_name.clone()
                ),
                kind: SymbolKind::Impl {
                    trait_name,
                    type_name,
                },
                range: node_to_range(i, source),
                visibility: Visibility::Private,
                docstring: None,
            });

            // Extract methods inside impl block
            if let Some(assoc_items) = i.assoc_item_list() {
                for assoc in assoc_items.assoc_items() {
                    if let ast::AssocItem::Fn(f) = assoc {
                        if let Some(name) = f.name() {
                            symbols.push(Symbol {
                                name: name.text().to_string(),
                                kind: SymbolKind::Function {
                                    is_async: f.async_token().is_some(),
                                    is_unsafe: f.unsafe_token().is_some(),
                                    is_const: f.const_token().is_some(),
                                },
                                range: node_to_range(&f, source),
                                visibility: extract_visibility(f.visibility()),
                                docstring: extract_docstring(&f),
                            });
                        }
                    }
                }
            }
        }
        ast::Item::Module(m) => {
            if let Some(name) = m.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::Module,
                    range: node_to_range(m, source),
                    visibility: extract_visibility(m.visibility()),
                    docstring: extract_docstring(m),
                });

                // Extract items inside module
                if let Some(item_list) = m.item_list() {
                    for inner_item in item_list.items() {
                        extract_item_symbols(&inner_item, source, symbols);
                    }
                }
            }
        }
        ast::Item::Const(c) => {
            if let Some(name) = c.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::Const,
                    range: node_to_range(c, source),
                    visibility: extract_visibility(c.visibility()),
                    docstring: extract_docstring(c),
                });
            }
        }
        ast::Item::Static(s) => {
            if let Some(name) = s.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::Static,
                    range: node_to_range(s, source),
                    visibility: extract_visibility(s.visibility()),
                    docstring: extract_docstring(s),
                });
            }
        }
        ast::Item::TypeAlias(t) => {
            if let Some(name) = t.name() {
                symbols.push(Symbol {
                    name: name.text().to_string(),
                    kind: SymbolKind::TypeAlias,
                    range: node_to_range(t, source),
                    visibility: extract_visibility(t.visibility()),
                    docstring: extract_docstring(t),
                });
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parser_creation() {
        let parser = RustParser::new();
        assert!(parser.is_ok());
    }

    #[test]
    fn test_parse_simple_function() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
            fn hello_world() {
                println!("Hello, world!");
            }
        "#;

        let symbols = parser.parse_source(source).unwrap();
        assert_eq!(symbols.len(), 1);

        let symbol = &symbols[0];
        assert_eq!(symbol.name, "hello_world");
        assert!(matches!(
            symbol.kind,
            SymbolKind::Function {
                is_async: false,
                is_unsafe: false,
                is_const: false
            }
        ));
        assert_eq!(symbol.visibility, Visibility::Private);
    }

    #[test]
    fn test_parse_async_function() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
            pub async fn fetch_data() -> Result<String, Error> {
                Ok("data".to_string())
            }
        "#;

        let symbols = parser.parse_source(source).unwrap();
        assert_eq!(symbols.len(), 1);

        let symbol = &symbols[0];
        assert_eq!(symbol.name, "fetch_data");
        assert!(matches!(
            symbol.kind,
            SymbolKind::Function {
                is_async: true,
                is_unsafe: false,
                is_const: false
            }
        ));
        assert_eq!(symbol.visibility, Visibility::Public);
    }

    #[test]
    fn test_parse_struct() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
            pub struct User {
                name: String,
                age: u32,
            }
        "#;

        let symbols = parser.parse_source(source).unwrap();
        assert_eq!(symbols.len(), 1);

        let symbol = &symbols[0];
        assert_eq!(symbol.name, "User");
        assert!(matches!(symbol.kind, SymbolKind::Struct));
        assert_eq!(symbol.visibility, Visibility::Public);
    }

    #[test]
    fn test_parse_with_docstring() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
/// A test function
/// that does something
fn test() {}
        "#;

        let symbols = parser.parse_source(source).unwrap();
        assert_eq!(symbols.len(), 1);

        let symbol = &symbols[0];
        assert_eq!(symbol.name, "test");
        assert!(symbol.docstring.is_some());
        assert!(symbol.docstring.as_ref().unwrap().contains("test function"));
    }

    #[test]
    fn test_parse_impl_block() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
            impl MyStruct {
                fn new() -> Self {
                    Self {}
                }
            }
        "#;

        let symbols = parser.parse_source(source).unwrap();
        // Should find: impl block + new function
        assert!(symbols.len() >= 2);

        // Find the impl symbol
        let impl_symbol = symbols.iter().find(|s| matches!(s.kind, SymbolKind::Impl { .. }));
        assert!(impl_symbol.is_some());
    }

    #[test]
    fn test_parse_trait_impl() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
            impl Display for MyType {
                fn fmt(&self, f: &mut Formatter) -> Result {
                    write!(f, "MyType")
                }
            }
        "#;

        let symbols = parser.parse_source(source).unwrap();

        // Find the impl symbol
        let impl_symbol = symbols.iter().find(|s| {
            matches!(s.kind, SymbolKind::Impl { trait_name: Some(_), .. })
        });
        assert!(impl_symbol.is_some());
    }

    #[test]
    fn test_parse_complete() {
        let mut parser = RustParser::new().unwrap();
        let source = r#"
            use std::collections::HashMap;
            use crate::parser::Symbol;

            /// A test struct
            pub struct MyStruct {
                data: HashMap<String, i32>,
            }

            impl MyStruct {
                pub fn new() -> Self {
                    Self {
                        data: HashMap::new(),
                    }
                }

                pub fn process(&self) {
                    self.helper();
                }

                fn helper(&self) {
                    println!("processing");
                }
            }

            pub fn main() {
                let s = MyStruct::new();
                s.process();
            }
        "#;

        let result = parser.parse_source_complete(source).unwrap();

        // Check symbols
        assert!(result.symbols.len() >= 5, "Should find struct, impl, and methods");
        let struct_symbol = result.symbols.iter().find(|s| s.name == "MyStruct");
        assert!(struct_symbol.is_some());
        assert!(matches!(struct_symbol.unwrap().kind, SymbolKind::Struct));

        // Check call graph
        assert!(result.call_graph.has_call("process", "helper"));
        assert!(result.call_graph.has_call("main", "new"));
        assert!(result.call_graph.has_call("main", "process"));

        // Check imports
        assert!(result.imports.len() >= 2);
        let has_hashmap = result.imports.iter().any(|i| i.path.contains("HashMap"));
        assert!(has_hashmap, "Should find HashMap import");
    }
}

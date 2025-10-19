//! Rust code parsing with tree-sitter
//!
//! Extracts symbols (functions, structs, traits, etc.) from Rust source files
//! and builds a call graph for understanding code relationships.

pub mod call_graph;
pub mod imports;
pub mod type_references;

use std::fs;
use std::path::Path;
use tree_sitter::{Node, Parser, Tree};

pub use call_graph::CallGraph;
pub use imports::{extract_imports, get_external_dependencies, Import};
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

/// Rust source code parser
pub struct RustParser {
    parser: Parser,
}

impl RustParser {
    /// Create a new Rust parser
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let mut parser = Parser::new();
        parser.set_language(tree_sitter_rust::language().into())?;
        Ok(Self { parser })
    }

    /// Parse a Rust source file and extract symbols
    pub fn parse_file(&mut self, path: &Path) -> Result<Vec<Symbol>, Box<dyn std::error::Error>> {
        let source = fs::read_to_string(path)?;
        self.parse_source(&source)
    }

    /// Parse Rust source code and extract symbols
    pub fn parse_source(&mut self, source: &str) -> Result<Vec<Symbol>, Box<dyn std::error::Error>> {
        let tree = self
            .parser
            .parse(source, None)
            .ok_or("Failed to parse source code")?;

        let mut symbols = Vec::new();
        self.extract_symbols(&tree, source, &mut symbols);
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
    pub fn parse_source_complete(
        &mut self,
        source: &str,
    ) -> Result<ParseResult, Box<dyn std::error::Error>> {
        let tree = self
            .parser
            .parse(source, None)
            .ok_or("Failed to parse source code")?;

        let mut symbols = Vec::new();
        self.extract_symbols(&tree, source, &mut symbols);

        let call_graph = CallGraph::build(&tree, source);
        let imports = extract_imports(&tree, source);
        let type_references = TypeReferenceTracker::build(&tree, source);

        Ok(ParseResult {
            symbols,
            call_graph,
            imports,
            type_references,
        })
    }

    /// Extract symbols from a parse tree
    fn extract_symbols(&self, tree: &Tree, source: &str, symbols: &mut Vec<Symbol>) {
        let root = tree.root_node();
        self.traverse_node(root, source, symbols, None);
    }

    /// Recursively traverse AST nodes and extract symbols
    fn traverse_node(
        &self,
        node: Node,
        source: &str,
        symbols: &mut Vec<Symbol>,
        parent_docstring: Option<String>,
    ) {
        // Check for doc comments before this node
        let docstring = self.extract_docstring_before(node, source).or(parent_docstring);

        // Extract symbol based on node kind
        match node.kind() {
            "function_item" => {
                if let Some(symbol) = self.extract_function(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "struct_item" => {
                if let Some(symbol) = self.extract_struct(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "enum_item" => {
                if let Some(symbol) = self.extract_enum(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "trait_item" => {
                if let Some(symbol) = self.extract_trait(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "impl_item" => {
                if let Some(symbol) = self.extract_impl(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "mod_item" => {
                if let Some(symbol) = self.extract_module(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "const_item" => {
                if let Some(symbol) = self.extract_const(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "static_item" => {
                if let Some(symbol) = self.extract_static(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            "type_item" => {
                if let Some(symbol) = self.extract_type_alias(node, source, docstring.clone()) {
                    symbols.push(symbol);
                }
            }
            _ => {}
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            self.traverse_node(child, source, symbols, None);
        }
    }

    /// Extract function symbol
    fn extract_function(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        // Check for async, unsafe, const modifiers by looking for the keyword tokens
        let mut is_async = false;
        let mut is_unsafe = false;
        let mut is_const = false;

        for child in node.children(&mut node.walk()) {
            let kind = child.kind();
            match kind {
                "async" => is_async = true,
                "unsafe" => is_unsafe = true,
                "const" => is_const = true,
                _ => {
                    // Check if this is a text node containing the keyword
                    if let Some(text) = self.get_node_text(child, source) {
                        if text == "async" {
                            is_async = true;
                        } else if text == "unsafe" {
                            is_unsafe = true;
                        } else if text == "const" {
                            is_const = true;
                        }
                    }
                }
            }
        }

        Some(Symbol {
            kind: SymbolKind::Function {
                is_async,
                is_unsafe,
                is_const,
            },
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract struct symbol
    fn extract_struct(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "type_identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        Some(Symbol {
            kind: SymbolKind::Struct,
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract enum symbol
    fn extract_enum(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "type_identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        Some(Symbol {
            kind: SymbolKind::Enum,
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract trait symbol
    fn extract_trait(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "type_identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        Some(Symbol {
            kind: SymbolKind::Trait,
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract impl block symbol
    fn extract_impl(&self, node: Node, source: &str, docstring: Option<String>) -> Option<Symbol> {
        // Find all type_identifiers - first is trait (if trait impl), last is type
        let type_identifiers: Vec<_> = node
            .children(&mut node.walk())
            .filter(|c| c.kind() == "type_identifier")
            .collect();

        if type_identifiers.is_empty() {
            return None;
        }

        // Determine if this is a trait impl by looking for "for" keyword
        let has_for_keyword = node
            .children(&mut node.walk())
            .any(|c| c.kind() == "for" || (c.kind() == "for" && self.get_node_text(c, source).map_or(false, |t| t == "for")));

        let (trait_name, type_name) = if type_identifiers.len() >= 2 || has_for_keyword {
            // Trait impl: first identifier is trait, last is type
            let trait_text = self.get_node_text(type_identifiers[0], source)?;
            let type_text = self.get_node_text(*type_identifiers.last()?, source)?;
            (Some(trait_text), type_text)
        } else {
            // Inherent impl: only one type identifier
            (None, self.get_node_text(type_identifiers[0], source)?)
        };

        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        // Use type name as the symbol name
        let name = if let Some(ref trait_name) = trait_name {
            format!("{} for {}", trait_name, type_name)
        } else {
            type_name.clone()
        };

        Some(Symbol {
            kind: SymbolKind::Impl {
                trait_name,
                type_name,
            },
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract module symbol
    fn extract_module(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        Some(Symbol {
            kind: SymbolKind::Module,
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract const symbol
    fn extract_const(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        Some(Symbol {
            kind: SymbolKind::Const,
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract static symbol
    fn extract_static(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        Some(Symbol {
            kind: SymbolKind::Static,
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract type alias symbol
    fn extract_type_alias(
        &self,
        node: Node,
        source: &str,
        docstring: Option<String>,
    ) -> Option<Symbol> {
        let name = self.get_node_text(self.find_child_by_kind(node, "type_identifier")?, source)?;
        let visibility = self.extract_visibility(node, source);
        let range = self.node_to_range(node);

        Some(Symbol {
            kind: SymbolKind::TypeAlias,
            name,
            range,
            docstring,
            visibility,
        })
    }

    /// Extract visibility modifier from a node
    fn extract_visibility(&self, node: Node, source: &str) -> Visibility {
        let vis_node = node
            .children(&mut node.walk())
            .find(|c| c.kind() == "visibility_modifier");

        match vis_node {
            None => Visibility::Private,
            Some(vis) => {
                let text = self.get_node_text(vis, source).unwrap_or_default();
                if text == "pub" {
                    Visibility::Public
                } else if text.starts_with("pub(crate)") {
                    Visibility::Crate
                } else if text.starts_with("pub(") {
                    // pub(in path)
                    Visibility::Restricted(text)
                } else {
                    Visibility::Private
                }
            }
        }
    }

    /// Extract docstring (/// or //!) before a node
    fn extract_docstring_before(&self, node: Node, source: &str) -> Option<String> {
        let mut prev = node.prev_sibling()?;

        // Keep going back while we find comments
        let mut doc_lines = Vec::new();
        loop {
            if prev.kind() == "line_comment" {
                let comment = self.get_node_text(prev, source)?;
                // Check if it's a doc comment (/// or //!)
                if comment.starts_with("///") || comment.starts_with("//!") {
                    let content = comment.trim_start_matches("///")
                        .trim_start_matches("//!")
                        .trim();
                    doc_lines.insert(0, content.to_string());
                } else {
                    // Non-doc comment, stop
                    break;
                }
            } else if prev.kind() != "attribute_item" {
                // Hit a non-comment, non-attribute node
                break;
            }

            // Move to previous sibling
            if let Some(p) = prev.prev_sibling() {
                prev = p;
            } else {
                break;
            }
        }

        if doc_lines.is_empty() {
            None
        } else {
            Some(doc_lines.join("\n"))
        }
    }

    /// Helper: Find first child node with given kind
    fn find_child_by_kind<'a>(&self, node: Node<'a>, kind: &str) -> Option<Node<'a>> {
        node.children(&mut node.walk()).find(|c| c.kind() == kind)
    }

    /// Helper: Get text content of a node
    fn get_node_text(&self, node: Node, source: &str) -> Option<String> {
        node.utf8_text(source.as_bytes()).ok().map(String::from)
    }

    /// Helper: Convert tree-sitter node to Range
    fn node_to_range(&self, node: Node) -> Range {
        let start = node.start_position();
        let end = node.end_position();

        Range {
            start_line: start.row + 1, // Convert to 1-indexed
            end_line: end.row + 1,
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
        }
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
    fn test_parse_real_file() {
        use std::path::Path;

        let mut parser = RustParser::new().unwrap();
        let path = Path::new("/home/molaco/Documents/rust-code-mcp/src/metadata_cache.rs");

        // Only run if file exists
        if !path.exists() {
            return;
        }

        let symbols = parser.parse_file(&path).unwrap();

        // Should find at least: FileMetadata struct, MetadataCache struct, and various methods
        assert!(symbols.len() > 5, "Expected multiple symbols, found {}", symbols.len());

        // Should find FileMetadata struct
        let file_metadata = symbols.iter().find(|s| s.name == "FileMetadata");
        assert!(file_metadata.is_some(), "Should find FileMetadata struct");
        assert!(matches!(file_metadata.unwrap().kind, SymbolKind::Struct));

        // Should find MetadataCache struct
        let metadata_cache = symbols.iter().find(|s| s.name == "MetadataCache");
        assert!(metadata_cache.is_some(), "Should find MetadataCache struct");

        // Should find impl blocks
        let impl_count = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Impl { .. })).count();
        assert!(impl_count >= 2, "Should find at least 2 impl blocks");

        // Should find methods like 'new' and 'has_changed'
        let has_new = symbols.iter().any(|s| s.name == "new");
        let has_changed = symbols.iter().any(|s| s.name == "has_changed");
        assert!(has_new, "Should find 'new' method");
        assert!(has_changed, "Should find 'has_changed' method");
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

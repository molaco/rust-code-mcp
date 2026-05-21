//! Parse-result data types: symbols, kinds, ranges, and visibility.

use super::call_graph::CallGraph;
use super::imports::Import;
use super::type_references::TypeReference;

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

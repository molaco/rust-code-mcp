//! Rust code parsing with rust-analyzer (ra_ap_syntax)
//!
//! An alternative parser implementation using rust-analyzer's syntax crate
//! for more accurate Rust parsing compared to tree-sitter.

use std::fs;
use std::path::Path;

use ra_ap_syntax::{
    ast::{self, HasDocComments, HasGenericArgs, HasModuleItem, HasName, HasVisibility},
    AstNode, AstToken, Edition, SourceFile, SyntaxKind,
};

use super::{
    CallGraph, Import, ParseResult, Range, Symbol, SymbolKind, TypeReference, TypeUsageContext,
    Visibility,
};

/// Rust source code parser using rust-analyzer's syntax crate
pub struct RaParser {
    /// Edition of Rust to parse (affects syntax rules)
    edition: Edition,
}

impl RaParser {
    /// Create a new rust-analyzer based parser
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
    pub fn parse_source_complete(
        &mut self,
        source: &str,
    ) -> Result<ParseResult, Box<dyn std::error::Error>> {
        let parse = SourceFile::parse(source, self.edition);
        let file = parse.tree();

        // Extract symbols
        let mut symbols = Vec::new();
        extract_symbols_recursive(&file, source, &mut symbols);

        // Build call graph
        let call_graph = build_call_graph(source, self.edition);

        // Extract imports
        let imports = extract_imports(source, self.edition);

        // Extract type references
        let type_references = extract_type_references(source, self.edition);

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

// ============================================================================
// CALL GRAPH BUILDING
// ============================================================================

/// Build a call graph from source code
fn build_call_graph(source: &str, edition: Edition) -> CallGraph {
    let parse = SourceFile::parse(source, edition);
    let file = parse.tree();
    let mut graph = CallGraph::new();

    for item in file.items() {
        match &item {
            ast::Item::Fn(f) => {
                if let Some(name) = f.name() {
                    let caller = name.text().to_string();
                    if let Some(body) = f.body() {
                        extract_calls_from_expr(body.syntax(), &caller, &mut graph);
                    }
                }
            }
            ast::Item::Impl(i) => {
                if let Some(assoc_items) = i.assoc_item_list() {
                    for assoc in assoc_items.assoc_items() {
                        if let ast::AssocItem::Fn(f) = assoc {
                            if let Some(name) = f.name() {
                                let caller = name.text().to_string();
                                if let Some(body) = f.body() {
                                    extract_calls_from_expr(body.syntax(), &caller, &mut graph);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    graph
}

/// Extract function calls from an expression
fn extract_calls_from_expr(
    node: &ra_ap_syntax::SyntaxNode,
    caller: &str,
    graph: &mut CallGraph,
) {
    for descendant in node.descendants() {
        if descendant.kind() == SyntaxKind::CALL_EXPR {
            if let Some(call) = ast::CallExpr::cast(descendant.clone()) {
                if let Some(callee_name) = extract_call_target(&call) {
                    graph.add_call(caller.to_string(), callee_name);
                }
            }
        }
        if descendant.kind() == SyntaxKind::METHOD_CALL_EXPR {
            if let Some(method_call) = ast::MethodCallExpr::cast(descendant.clone()) {
                if let Some(name) = method_call.name_ref() {
                    graph.add_call(caller.to_string(), name.text().to_string());
                }
            }
        }
    }
}

/// Extract the target function name from a call expression
fn extract_call_target(call: &ast::CallExpr) -> Option<String> {
    let expr = call.expr()?;

    match expr {
        ast::Expr::PathExpr(path_expr) => {
            let path = path_expr.path()?;
            // Get the last segment (function name)
            path.segments()
                .last()
                .and_then(|seg| seg.name_ref())
                .map(|name| name.text().to_string())
        }
        _ => None,
    }
}

// ============================================================================
// IMPORT EXTRACTION
// ============================================================================

/// Extract imports from source code
fn extract_imports(source: &str, edition: Edition) -> Vec<Import> {
    let parse = SourceFile::parse(source, edition);
    let file = parse.tree();
    let mut imports = Vec::new();

    for item in file.items() {
        if let ast::Item::Use(use_item) = item {
            extract_use_tree(use_item.use_tree(), &mut imports, String::new());
        }
    }

    imports
}

/// Extract imports from a use tree recursively
fn extract_use_tree(use_tree: Option<ast::UseTree>, imports: &mut Vec<Import>, prefix: String) {
    let Some(tree) = use_tree else { return };

    let path = tree
        .path()
        .map(|p| {
            let path_str = p.syntax().text().to_string();
            if prefix.is_empty() {
                path_str
            } else {
                format!("{}::{}", prefix, path_str)
            }
        })
        .unwrap_or(prefix.clone());

    // Check for glob (*)
    if tree.star_token().is_some() {
        imports.push(Import {
            path: path.clone(),
            is_glob: true,
            items: vec![],
        });
        return;
    }

    // Check for use list {a, b, c}
    if let Some(use_tree_list) = tree.use_tree_list() {
        for subtree in use_tree_list.use_trees() {
            extract_use_tree(Some(subtree), imports, path.clone());
        }
        return;
    }

    // Check for rename (as)
    let rename = tree
        .rename()
        .and_then(|r| r.name().map(|n| n.text().to_string()));

    // Simple import
    if !path.is_empty() {
        imports.push(Import {
            path,
            is_glob: false,
            items: rename.map(|r| vec![r]).unwrap_or_default(),
        });
    }
}

// ============================================================================
// TYPE REFERENCE EXTRACTION
// ============================================================================

/// Extract type references from source code
fn extract_type_references(source: &str, edition: Edition) -> Vec<TypeReference> {
    let parse = SourceFile::parse(source, edition);
    let file = parse.tree();
    let mut refs = Vec::new();

    for item in file.items() {
        match &item {
            ast::Item::Fn(f) => {
                let fn_name = f
                    .name()
                    .map(|n| n.text().to_string())
                    .unwrap_or_default();

                // Parameters
                if let Some(params) = f.param_list() {
                    for param in params.params() {
                        if let Some(ty) = param.ty() {
                            extract_types_from_type(
                                &ty,
                                source,
                                &mut refs,
                                TypeUsageContext::FunctionParameter {
                                    function_name: fn_name.clone(),
                                },
                            );
                        }
                    }
                }

                // Return type
                if let Some(ret) = f.ret_type() {
                    if let Some(ty) = ret.ty() {
                        extract_types_from_type(
                            &ty,
                            source,
                            &mut refs,
                            TypeUsageContext::FunctionReturn {
                                function_name: fn_name.clone(),
                            },
                        );
                    }
                }
            }
            ast::Item::Struct(s) => {
                let struct_name = s
                    .name()
                    .map(|n| n.text().to_string())
                    .unwrap_or_default();

                if let Some(field_list) = s.field_list() {
                    match field_list {
                        ast::FieldList::RecordFieldList(record) => {
                            for field in record.fields() {
                                let field_name = field
                                    .name()
                                    .map(|n| n.text().to_string())
                                    .unwrap_or_default();
                                if let Some(ty) = field.ty() {
                                    extract_types_from_type(
                                        &ty,
                                        source,
                                        &mut refs,
                                        TypeUsageContext::StructField {
                                            struct_name: struct_name.clone(),
                                            field_name,
                                        },
                                    );
                                }
                            }
                        }
                        ast::FieldList::TupleFieldList(tuple) => {
                            for (i, field) in tuple.fields().enumerate() {
                                if let Some(ty) = field.ty() {
                                    extract_types_from_type(
                                        &ty,
                                        source,
                                        &mut refs,
                                        TypeUsageContext::StructField {
                                            struct_name: struct_name.clone(),
                                            field_name: format!("{}", i),
                                        },
                                    );
                                }
                            }
                        }
                    }
                }
            }
            ast::Item::Impl(i) => {
                let type_name = i.self_ty().map(|t| t.syntax().text().to_string());
                let trait_name = i.trait_().map(|t| t.syntax().text().to_string());

                if let Some(ref tn) = type_name {
                    refs.push(TypeReference {
                        type_name: tn.clone(),
                        usage_context: TypeUsageContext::ImplBlock {
                            trait_name: trait_name.clone(),
                        },
                        line: line_of_offset(source, i.syntax().text_range().start().into()),
                    });
                }
            }
            _ => {}
        }
    }

    refs
}

/// Extract types from a type AST node
fn extract_types_from_type(
    ty: &ast::Type,
    source: &str,
    refs: &mut Vec<TypeReference>,
    context: TypeUsageContext,
) {
    match ty {
        ast::Type::PathType(path_ty) => {
            if let Some(path) = path_ty.path() {
                if let Some(segment) = path.segments().last() {
                    if let Some(name) = segment.name_ref() {
                        refs.push(TypeReference {
                            type_name: name.text().to_string(),
                            usage_context: context.clone(),
                            line: line_of_offset(source, name.syntax().text_range().start().into()),
                        });
                    }
                    // Handle generics like Vec<T>
                    if let Some(generic_args) = segment.generic_arg_list() {
                        for arg in generic_args.generic_args() {
                            if let ast::GenericArg::TypeArg(ref type_arg) = arg {
                                if let Some(inner_ty) = type_arg.ty() {
                                    extract_types_from_type(
                                        &inner_ty,
                                        source,
                                        refs,
                                        TypeUsageContext::GenericArgument,
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
        ast::Type::RefType(ref_ty) => {
            if let Some(inner) = ref_ty.ty() {
                extract_types_from_type(&inner, source, refs, context);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ra_parser_creation() {
        let parser = RaParser::new();
        assert!(parser.is_ok());
    }

    #[test]
    fn test_ra_parse_simple_source() {
        let mut parser = RaParser::new().unwrap();
        let source = r#"
            fn hello_world() {
                println!("Hello, world!");
            }
        "#;

        let result = parser.parse_source(source);
        assert!(result.is_ok());
        let symbols = result.unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "hello_world");
    }

    #[test]
    fn test_ra_parse_complete() {
        let mut parser = RaParser::new().unwrap();
        let source = r#"
            use std::collections::HashMap;

            pub struct MyStruct {
                data: HashMap<String, i32>,
            }

            impl MyStruct {
                pub fn new() -> Self {
                    Self { data: HashMap::new() }
                }
            }
        "#;

        let result = parser.parse_source_complete(source);
        assert!(result.is_ok());
        let parse_result = result.unwrap();

        // Check symbols
        assert!(parse_result.symbols.len() >= 3); // struct, impl, new method

        // Check imports
        assert!(!parse_result.imports.is_empty());
    }

    #[test]
    fn test_symbol_extraction_all_types() {
        let source = r#"
/// A greeting function
pub fn hello() {}

pub async fn async_hello() {}
pub unsafe fn unsafe_hello() {}
pub const fn const_hello() {}

pub struct User { name: String }
pub enum Status { Active, Inactive }
pub trait Greet { fn greet(&self); }

impl User {
    pub fn new() -> Self { todo!() }
}

impl Greet for User {
    fn greet(&self) {}
}

pub mod inner {
    pub fn nested() {}
}

pub const MAX: i32 = 100;
pub static COUNTER: i32 = 0;
pub type UserId = u64;
"#;

        let mut parser = RaParser::new().unwrap();
        let symbols = parser.parse_source(source).unwrap();

        // Count by type
        let funcs = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Function { .. }))
            .count();
        let structs = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Struct))
            .count();
        let enums = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Enum))
            .count();
        let traits = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Trait))
            .count();
        let impls = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Impl { .. }))
            .count();
        let modules = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Module))
            .count();
        let consts = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Const))
            .count();
        let statics = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::Static))
            .count();
        let type_aliases = symbols
            .iter()
            .filter(|s| matches!(s.kind, SymbolKind::TypeAlias))
            .count();

        assert!(funcs >= 7, "Should find at least 7 functions (including nested)");
        assert_eq!(structs, 1);
        assert_eq!(enums, 1);
        assert_eq!(traits, 1);
        assert_eq!(impls, 2);
        assert!(modules >= 1);
        assert_eq!(consts, 1);
        assert_eq!(statics, 1);
        assert_eq!(type_aliases, 1);

        // Check docstring extraction
        let hello = symbols.iter().find(|s| s.name == "hello").unwrap();
        assert!(hello.docstring.is_some(), "Should extract docstring");

        // Check async function detection
        let async_fn = symbols.iter().find(|s| s.name == "async_hello").unwrap();
        assert!(
            matches!(async_fn.kind, SymbolKind::Function { is_async: true, .. }),
            "Should detect async function"
        );

        // Check unsafe function detection
        let unsafe_fn = symbols.iter().find(|s| s.name == "unsafe_hello").unwrap();
        assert!(
            matches!(unsafe_fn.kind, SymbolKind::Function { is_unsafe: true, .. }),
            "Should detect unsafe function"
        );

        // Check const function detection
        let const_fn = symbols.iter().find(|s| s.name == "const_hello").unwrap();
        assert!(
            matches!(const_fn.kind, SymbolKind::Function { is_const: true, .. }),
            "Should detect const function"
        );
    }

    #[test]
    fn test_call_graph() {
        let source = r#"
fn main() {
    foo();
    bar();
}

fn foo() {
    helper();
}

fn bar() {
    helper();
}

fn helper() {}
"#;

        let mut parser = RaParser::new().unwrap();
        let result = parser.parse_source_complete(source).unwrap();
        let graph = result.call_graph;

        assert!(graph.has_call("main", "foo"), "main should call foo");
        assert!(graph.has_call("main", "bar"), "main should call bar");
        assert!(graph.has_call("foo", "helper"), "foo should call helper");
        assert!(graph.has_call("bar", "helper"), "bar should call helper");
        assert!(
            !graph.has_call("main", "helper"),
            "main should NOT directly call helper"
        );
    }

    #[test]
    fn test_method_calls() {
        let source = r#"
fn process() {
    let s = String::new();
    s.push_str("test");
    s.len();
}
"#;

        let mut parser = RaParser::new().unwrap();
        let result = parser.parse_source_complete(source).unwrap();
        let graph = result.call_graph;

        assert!(graph.has_call("process", "new"), "should detect String::new()");
        assert!(
            graph.has_call("process", "push_str"),
            "should detect .push_str()"
        );
        assert!(graph.has_call("process", "len"), "should detect .len()");
    }

    #[test]
    fn test_import_extraction() {
        let source = r#"
use std::collections::HashMap;
use std::io::{Read, Write};
use std::fs::*;
use serde::Serialize;
use crate::parser::Symbol;
"#;

        let mut parser = RaParser::new().unwrap();
        let result = parser.parse_source_complete(source).unwrap();
        let imports = result.imports;

        assert!(
            imports.iter().any(|i| i.path.contains("HashMap")),
            "Should find HashMap import"
        );
        assert!(
            imports.iter().any(|i| i.path.contains("Read")),
            "Should find Read import"
        );
        assert!(
            imports.iter().any(|i| i.path.contains("Write")),
            "Should find Write import"
        );
        assert!(
            imports.iter().any(|i| i.is_glob && i.path.contains("fs")),
            "Should find glob import"
        );
        assert!(
            imports.iter().any(|i| i.path.contains("Serialize")),
            "Should find serde import"
        );
    }

    #[test]
    fn test_type_references() {
        let source = r#"
struct Container {
    parser: RustParser,
    items: Vec<String>,
}

fn process(parser: RustParser) -> Container {
    todo!()
}

impl Container {
    fn new() -> Self { todo!() }
}
"#;

        let mut parser = RaParser::new().unwrap();
        let result = parser.parse_source_complete(source).unwrap();
        let refs = result.type_references;

        let parser_refs: Vec<_> = refs.iter().filter(|r| r.type_name == "RustParser").collect();
        assert!(
            parser_refs.len() >= 2,
            "Should find RustParser in struct field and parameter"
        );

        let has_struct_field = parser_refs
            .iter()
            .any(|r| matches!(&r.usage_context, TypeUsageContext::StructField { .. }));
        let has_param = parser_refs
            .iter()
            .any(|r| matches!(&r.usage_context, TypeUsageContext::FunctionParameter { .. }));
        assert!(has_struct_field, "Should find struct field reference");
        assert!(has_param, "Should find function parameter reference");

        // Check generic type
        let string_refs: Vec<_> = refs.iter().filter(|r| r.type_name == "String").collect();
        let has_generic = string_refs
            .iter()
            .any(|r| matches!(&r.usage_context, TypeUsageContext::GenericArgument));
        assert!(
            has_generic,
            "Should find String as generic argument in Vec<String>"
        );
    }

    #[test]
    fn test_complete_parse() {
        let source = r#"
use std::collections::HashMap;

/// A user struct
pub struct User {
    name: String,
}

impl User {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    pub fn greet(&self) {
        println!("Hello!");
    }
}

fn main() {
    let user = User::new("Alice".to_string());
    user.greet();
}
"#;

        let mut parser = RaParser::new().unwrap();
        let result = parser.parse_source_complete(source).unwrap();

        assert!(!result.symbols.is_empty(), "Should have symbols");
        assert!(!result.imports.is_empty(), "Should have imports");
        assert!(!result.type_references.is_empty(), "Should have type references");
        assert!(
            result.call_graph.edge_count() > 0,
            "Should have call graph edges"
        );
    }

    #[test]
    fn test_visibility_extraction() {
        let source = r#"
pub fn public_fn() {}
pub(crate) fn crate_fn() {}
pub(super) fn super_fn() {}
fn private_fn() {}
"#;

        let mut parser = RaParser::new().unwrap();
        let symbols = parser.parse_source(source).unwrap();

        let public_fn = symbols.iter().find(|s| s.name == "public_fn").unwrap();
        assert_eq!(public_fn.visibility, Visibility::Public);

        let crate_fn = symbols.iter().find(|s| s.name == "crate_fn").unwrap();
        assert_eq!(crate_fn.visibility, Visibility::Crate);

        let super_fn = symbols.iter().find(|s| s.name == "super_fn").unwrap();
        assert!(matches!(super_fn.visibility, Visibility::Restricted(_)));

        let private_fn = symbols.iter().find(|s| s.name == "private_fn").unwrap();
        assert_eq!(private_fn.visibility, Visibility::Private);
    }

    #[test]
    fn test_range_calculation() {
        let source = "fn test() {}\n";

        let mut parser = RaParser::new().unwrap();
        let symbols = parser.parse_source(source).unwrap();

        assert_eq!(symbols.len(), 1);
        let symbol = &symbols[0];
        assert_eq!(symbol.range.start_line, 1);
        assert_eq!(symbol.range.end_line, 1);
        assert_eq!(symbol.range.start_byte, 0);
        assert_eq!(symbol.range.end_byte, 12); // "fn test() {}"
    }

    #[test]
    fn test_nested_module_symbols() {
        let source = r#"
pub mod outer {
    pub fn outer_fn() {}

    pub mod inner {
        pub fn inner_fn() {}
    }
}
"#;

        let mut parser = RaParser::new().unwrap();
        let symbols = parser.parse_source(source).unwrap();

        // Should find: outer module, outer_fn, inner module, inner_fn
        assert!(symbols.len() >= 4);

        let outer_mod = symbols.iter().find(|s| s.name == "outer").unwrap();
        assert!(matches!(outer_mod.kind, SymbolKind::Module));

        let outer_fn = symbols.iter().find(|s| s.name == "outer_fn").unwrap();
        assert!(matches!(outer_fn.kind, SymbolKind::Function { .. }));

        let inner_mod = symbols.iter().find(|s| s.name == "inner").unwrap();
        assert!(matches!(inner_mod.kind, SymbolKind::Module));

        let inner_fn = symbols.iter().find(|s| s.name == "inner_fn").unwrap();
        assert!(matches!(inner_fn.kind, SymbolKind::Function { .. }));
    }

    #[test]
    fn test_impl_trait_for_type() {
        let source = r#"
pub trait Display {
    fn fmt(&self);
}

pub struct MyType;

impl Display for MyType {
    fn fmt(&self) {}
}
"#;

        let mut parser = RaParser::new().unwrap();
        let symbols = parser.parse_source(source).unwrap();

        // Find the trait impl
        let impl_symbol = symbols
            .iter()
            .find(|s| {
                matches!(
                    &s.kind,
                    SymbolKind::Impl {
                        trait_name: Some(_),
                        ..
                    }
                )
            })
            .unwrap();

        if let SymbolKind::Impl {
            trait_name,
            type_name,
        } = &impl_symbol.kind
        {
            assert_eq!(trait_name.as_ref().unwrap(), "Display");
            assert_eq!(type_name, "MyType");
        }
    }
}

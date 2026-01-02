//! Comprehensive test: Can ra_ap_syntax replace ALL tree-sitter functionality?
//!
//! Tests:
//! 1. Symbol extraction (functions, structs, enums, traits, impls, etc.)
//! 2. Call graph building
//! 3. Import extraction
//! 4. Type reference tracking
//! 5. Docstring extraction
//! 6. Nested symbol extraction (methods inside impl blocks)
//!
//! Run with: cargo test --test ra_full_replacement_test --features ra_syntax_test -- --nocapture

#[cfg(feature = "ra_syntax_test")]
mod full_replacement_tests {
    use ra_ap_syntax::{
        ast::{self, HasDocComments, HasGenericArgs, HasModuleItem, HasName, HasVisibility},
        AstNode, AstToken, Edition, SourceFile, SyntaxKind,
    };
    use std::collections::{HashMap, HashSet};

    // ============================================================================
    // DATA STRUCTURES (matching current tree-sitter implementation)
    // ============================================================================

    #[derive(Debug, Clone)]
    pub struct Symbol {
        pub name: String,
        pub kind: SymbolKind,
        pub start_line: usize,
        pub end_line: usize,
        pub visibility: Visibility,
        pub docstring: Option<String>,
    }

    #[derive(Debug, Clone)]
    pub enum SymbolKind {
        Function { is_async: bool, is_unsafe: bool, is_const: bool },
        Struct,
        Enum,
        Trait,
        Impl { trait_name: Option<String>, type_name: String },
        Module,
        Const,
        Static,
        TypeAlias,
    }

    #[derive(Debug, Clone)]
    pub enum Visibility {
        Public,
        Crate,
        Restricted(String),
        Private,
    }

    #[derive(Debug, Clone, Default)]
    pub struct CallGraph {
        edges: HashMap<String, HashSet<String>>,
    }

    #[derive(Debug, Clone)]
    pub struct Import {
        pub path: String,
        pub is_glob: bool,
        pub items: Vec<String>,
    }

    #[derive(Debug, Clone)]
    pub struct TypeReference {
        pub type_name: String,
        pub usage_context: TypeUsageContext,
        pub line: usize,
    }

    #[derive(Debug, Clone)]
    pub enum TypeUsageContext {
        FunctionParameter { function_name: String },
        FunctionReturn { function_name: String },
        StructField { struct_name: String, field_name: String },
        ImplBlock { trait_name: Option<String> },
        LetBinding,
        GenericArgument,
    }

    // ============================================================================
    // COMPLETE ParseResult (what tree-sitter returns)
    // ============================================================================

    #[derive(Debug, Clone)]
    pub struct ParseResult {
        pub symbols: Vec<Symbol>,
        pub call_graph: CallGraph,
        pub imports: Vec<Import>,
        pub type_references: Vec<TypeReference>,
    }

    // ============================================================================
    // HELPER FUNCTIONS
    // ============================================================================

    fn line_of_offset(source: &str, offset: usize) -> usize {
        source[..offset.min(source.len())].chars().filter(|&c| c == '\n').count() + 1
    }

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

    fn extract_docstring<N: HasDocComments>(node: &N) -> Option<String> {
        let docs: Vec<String> = node.doc_comments()
            .map(|c| {
                let text = c.text();
                // Strip /// or //! prefix
                let stripped: &str = text.strip_prefix("///")
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

    // ============================================================================
    // 1. SYMBOL EXTRACTION
    // ============================================================================

    fn extract_symbols(source: &str) -> Vec<Symbol> {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();
        let mut symbols = Vec::new();

        extract_symbols_recursive(&file, source, &mut symbols);
        symbols
    }

    fn extract_symbols_recursive(file: &SourceFile, source: &str, symbols: &mut Vec<Symbol>) {
        for item in file.items() {
            extract_item_symbols(&item, source, symbols);
        }
    }

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
                        start_line: line_of_offset(source, f.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, f.syntax().text_range().end().into()),
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
                        start_line: line_of_offset(source, s.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, s.syntax().text_range().end().into()),
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
                        start_line: line_of_offset(source, e.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, e.syntax().text_range().end().into()),
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
                        start_line: line_of_offset(source, t.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, t.syntax().text_range().end().into()),
                        visibility: extract_visibility(t.visibility()),
                        docstring: extract_docstring(t),
                    });
                }
            }
            ast::Item::Impl(i) => {
                let type_name = i.self_ty()
                    .map(|t| t.syntax().text().to_string())
                    .unwrap_or_default();
                let trait_name = i.trait_()
                    .map(|t| t.syntax().text().to_string());

                symbols.push(Symbol {
                    name: format!("impl {}{}",
                        trait_name.as_ref().map(|t| format!("{} for ", t)).unwrap_or_default(),
                        type_name.clone()
                    ),
                    kind: SymbolKind::Impl { trait_name, type_name },
                    start_line: line_of_offset(source, i.syntax().text_range().start().into()),
                    end_line: line_of_offset(source, i.syntax().text_range().end().into()),
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
                                    start_line: line_of_offset(source, f.syntax().text_range().start().into()),
                                    end_line: line_of_offset(source, f.syntax().text_range().end().into()),
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
                        start_line: line_of_offset(source, m.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, m.syntax().text_range().end().into()),
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
                        start_line: line_of_offset(source, c.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, c.syntax().text_range().end().into()),
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
                        start_line: line_of_offset(source, s.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, s.syntax().text_range().end().into()),
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
                        start_line: line_of_offset(source, t.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, t.syntax().text_range().end().into()),
                        visibility: extract_visibility(t.visibility()),
                        docstring: extract_docstring(t),
                    });
                }
            }
            _ => {}
        }
    }

    // ============================================================================
    // 2. CALL GRAPH BUILDING
    // ============================================================================

    impl CallGraph {
        fn new() -> Self {
            Self { edges: HashMap::new() }
        }

        fn add_call(&mut self, caller: String, callee: String) {
            self.edges.entry(caller).or_default().insert(callee);
        }

        fn get_callees(&self, caller: &str) -> Vec<&str> {
            self.edges.get(caller)
                .map(|s| s.iter().map(|s| s.as_str()).collect())
                .unwrap_or_default()
        }

        fn has_call(&self, caller: &str, callee: &str) -> bool {
            self.edges.get(caller).map(|s| s.contains(callee)).unwrap_or(false)
        }

        fn edge_count(&self) -> usize {
            self.edges.values().map(|s| s.len()).sum()
        }
    }

    fn build_call_graph(source: &str) -> CallGraph {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();
        let mut graph = CallGraph::new();

        for item in file.items() {
            if let ast::Item::Fn(ref f) = item {
                if let Some(name) = f.name() {
                    let caller = name.text().to_string();
                    if let Some(body) = f.body() {
                        extract_calls_from_expr(body.syntax(), &caller, &mut graph);
                    }
                }
            }
            // Also handle impl blocks
            if let ast::Item::Impl(ref i) = item {
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
        }

        graph
    }

    fn extract_calls_from_expr(node: &ra_ap_syntax::SyntaxNode, caller: &str, graph: &mut CallGraph) {
        // Look for call expressions
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

    fn extract_call_target(call: &ast::CallExpr) -> Option<String> {
        let expr = call.expr()?;

        match expr {
            ast::Expr::PathExpr(path_expr) => {
                let path = path_expr.path()?;
                // Get the last segment (function name)
                path.segments().last()
                    .and_then(|seg| seg.name_ref())
                    .map(|name| name.text().to_string())
            }
            _ => None
        }
    }

    // ============================================================================
    // 3. IMPORT EXTRACTION
    // ============================================================================

    fn extract_imports(source: &str) -> Vec<Import> {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();
        let mut imports = Vec::new();

        for item in file.items() {
            if let ast::Item::Use(use_item) = item {
                extract_use_tree(use_item.use_tree(), &mut imports, String::new());
            }
        }

        imports
    }

    fn extract_use_tree(use_tree: Option<ast::UseTree>, imports: &mut Vec<Import>, prefix: String) {
        let Some(tree) = use_tree else { return };

        let path = tree.path().map(|p| {
            let path_str = p.syntax().text().to_string();
            if prefix.is_empty() {
                path_str
            } else {
                format!("{}::{}", prefix, path_str)
            }
        }).unwrap_or(prefix.clone());

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
        let rename = tree.rename().map(|r| r.name().map(|n| n.text().to_string())).flatten();

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
    // 4. TYPE REFERENCE TRACKING
    // ============================================================================

    fn extract_type_references(source: &str) -> Vec<TypeReference> {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();
        let mut refs = Vec::new();

        for item in file.items() {
            match &item {
                ast::Item::Fn(f) => {
                    let fn_name = f.name().map(|n| n.text().to_string()).unwrap_or_default();

                    // Parameters
                    if let Some(params) = f.param_list() {
                        for param in params.params() {
                            if let Some(ty) = param.ty() {
                                extract_types_from_type(&ty, source, &mut refs,
                                    TypeUsageContext::FunctionParameter { function_name: fn_name.clone() });
                            }
                        }
                    }

                    // Return type
                    if let Some(ret) = f.ret_type() {
                        if let Some(ty) = ret.ty() {
                            extract_types_from_type(&ty, source, &mut refs,
                                TypeUsageContext::FunctionReturn { function_name: fn_name.clone() });
                        }
                    }
                }
                ast::Item::Struct(s) => {
                    let struct_name = s.name().map(|n| n.text().to_string()).unwrap_or_default();

                    if let Some(field_list) = s.field_list() {
                        match field_list {
                            ast::FieldList::RecordFieldList(record) => {
                                for field in record.fields() {
                                    let field_name = field.name().map(|n| n.text().to_string()).unwrap_or_default();
                                    if let Some(ty) = field.ty() {
                                        extract_types_from_type(&ty, source, &mut refs,
                                            TypeUsageContext::StructField {
                                                struct_name: struct_name.clone(),
                                                field_name
                                            });
                                    }
                                }
                            }
                            ast::FieldList::TupleFieldList(tuple) => {
                                for (i, field) in tuple.fields().enumerate() {
                                    if let Some(ty) = field.ty() {
                                        extract_types_from_type(&ty, source, &mut refs,
                                            TypeUsageContext::StructField {
                                                struct_name: struct_name.clone(),
                                                field_name: format!("{}", i)
                                            });
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
                            usage_context: TypeUsageContext::ImplBlock { trait_name: trait_name.clone() },
                            line: line_of_offset(source, i.syntax().text_range().start().into()),
                        });
                    }
                }
                _ => {}
            }
        }

        refs
    }

    fn extract_types_from_type(ty: &ast::Type, source: &str, refs: &mut Vec<TypeReference>, context: TypeUsageContext) {
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
                                        extract_types_from_type(&inner_ty, source, refs, TypeUsageContext::GenericArgument);
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

    // ============================================================================
    // COMPLETE PARSE (like tree-sitter's parse_source_complete)
    // ============================================================================

    fn parse_complete(source: &str) -> ParseResult {
        ParseResult {
            symbols: extract_symbols(source),
            call_graph: build_call_graph(source),
            imports: extract_imports(source),
            type_references: extract_type_references(source),
        }
    }

    // ============================================================================
    // TESTS
    // ============================================================================

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

        let symbols = extract_symbols(source);

        println!("\n=== Symbol Extraction Test ===");
        for s in &symbols {
            println!("  {} ({:?}) - {:?}", s.name,
                match &s.kind {
                    SymbolKind::Function { .. } => "function",
                    SymbolKind::Struct => "struct",
                    SymbolKind::Enum => "enum",
                    SymbolKind::Trait => "trait",
                    SymbolKind::Impl { .. } => "impl",
                    SymbolKind::Module => "module",
                    SymbolKind::Const => "const",
                    SymbolKind::Static => "static",
                    SymbolKind::TypeAlias => "type_alias",
                },
                s.docstring.as_ref().map(|d| d.chars().take(20).collect::<String>())
            );
        }

        // Count by type
        let funcs = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Function { .. })).count();
        let structs = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Struct)).count();
        let enums = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Enum)).count();
        let traits = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Trait)).count();
        let impls = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Impl { .. })).count();
        let modules = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Module)).count();
        let consts = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Const)).count();
        let statics = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::Static)).count();
        let type_aliases = symbols.iter().filter(|s| matches!(s.kind, SymbolKind::TypeAlias)).count();

        println!("\n  Functions: {}", funcs);
        println!("  Structs: {}", structs);
        println!("  Enums: {}", enums);
        println!("  Traits: {}", traits);
        println!("  Impls: {}", impls);
        println!("  Modules: {}", modules);
        println!("  Consts: {}", consts);
        println!("  Statics: {}", statics);
        println!("  Type aliases: {}", type_aliases);

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

        println!("\n✓ All symbol types extracted correctly!");
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

        let graph = build_call_graph(source);

        println!("\n=== Call Graph Test ===");
        println!("  main calls: {:?}", graph.get_callees("main"));
        println!("  foo calls: {:?}", graph.get_callees("foo"));
        println!("  bar calls: {:?}", graph.get_callees("bar"));
        println!("  Total edges: {}", graph.edge_count());

        assert!(graph.has_call("main", "foo"), "main should call foo");
        assert!(graph.has_call("main", "bar"), "main should call bar");
        assert!(graph.has_call("foo", "helper"), "foo should call helper");
        assert!(graph.has_call("bar", "helper"), "bar should call helper");
        assert!(!graph.has_call("main", "helper"), "main should NOT directly call helper");

        println!("\n✓ Call graph built correctly!");
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

        let graph = build_call_graph(source);

        println!("\n=== Method Call Test ===");
        println!("  process calls: {:?}", graph.get_callees("process"));

        assert!(graph.has_call("process", "new"), "should detect String::new()");
        assert!(graph.has_call("process", "push_str"), "should detect .push_str()");
        assert!(graph.has_call("process", "len"), "should detect .len()");

        println!("\n✓ Method calls detected correctly!");
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

        let imports = extract_imports(source);

        println!("\n=== Import Extraction Test ===");
        for imp in &imports {
            println!("  {} (glob: {}, items: {:?})", imp.path, imp.is_glob, imp.items);
        }

        assert!(imports.iter().any(|i| i.path.contains("HashMap")), "Should find HashMap import");
        assert!(imports.iter().any(|i| i.path.contains("Read")), "Should find Read import");
        assert!(imports.iter().any(|i| i.path.contains("Write")), "Should find Write import");
        assert!(imports.iter().any(|i| i.is_glob && i.path.contains("fs")), "Should find glob import");
        assert!(imports.iter().any(|i| i.path.contains("Serialize")), "Should find serde import");

        println!("\n✓ Imports extracted correctly!");
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

        let refs = extract_type_references(source);

        println!("\n=== Type Reference Test ===");
        for r in &refs {
            println!("  {} - {:?} (line {})", r.type_name, r.usage_context, r.line);
        }

        let parser_refs: Vec<_> = refs.iter().filter(|r| r.type_name == "RustParser").collect();
        assert!(parser_refs.len() >= 2, "Should find RustParser in struct field and parameter");

        let has_struct_field = parser_refs.iter().any(|r| matches!(&r.usage_context, TypeUsageContext::StructField { .. }));
        let has_param = parser_refs.iter().any(|r| matches!(&r.usage_context, TypeUsageContext::FunctionParameter { .. }));
        assert!(has_struct_field, "Should find struct field reference");
        assert!(has_param, "Should find function parameter reference");

        // Check generic type
        let string_refs: Vec<_> = refs.iter().filter(|r| r.type_name == "String").collect();
        let has_generic = string_refs.iter().any(|r| matches!(&r.usage_context, TypeUsageContext::GenericArgument));
        assert!(has_generic, "Should find String as generic argument in Vec<String>");

        println!("\n✓ Type references extracted correctly!");
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

        let result = parse_complete(source);

        println!("\n=== Complete Parse Test ===");
        println!("  Symbols: {}", result.symbols.len());
        println!("  Imports: {}", result.imports.len());
        println!("  Type refs: {}", result.type_references.len());
        println!("  Call edges: {}", result.call_graph.edge_count());

        assert!(!result.symbols.is_empty(), "Should have symbols");
        assert!(!result.imports.is_empty(), "Should have imports");
        assert!(!result.type_references.is_empty(), "Should have type references");
        assert!(result.call_graph.edge_count() > 0, "Should have call graph edges");

        println!("\n✓ Complete parse works - all features functional!");
    }

    #[test]
    fn test_parse_real_codebase_file() {
        use std::time::Instant;

        let path = std::path::Path::new("src/parser/mod.rs");
        if !path.exists() {
            println!("Skipping - file not found");
            return;
        }

        let source = std::fs::read_to_string(path).unwrap();

        let start = Instant::now();
        let result = parse_complete(&source);
        let elapsed = start.elapsed();

        println!("\n=== Real File Parse Test: src/parser/mod.rs ===");
        println!("  Parse time: {:?}", elapsed);
        println!("  Symbols: {}", result.symbols.len());
        println!("  Imports: {}", result.imports.len());
        println!("  Type refs: {}", result.type_references.len());
        println!("  Call edges: {}", result.call_graph.edge_count());

        assert!(!result.symbols.is_empty());

        println!("\n✓ Real file parsed successfully!");
    }
}

#[cfg(not(feature = "ra_syntax_test"))]
#[test]
fn test_feature_not_enabled() {
    println!("\nRun with: cargo test --test ra_full_replacement_test --features ra_syntax_test -- --nocapture\n");
}

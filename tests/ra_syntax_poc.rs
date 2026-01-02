//! Proof-of-concept test for ra_ap_syntax as a tree-sitter replacement
//! Run with: cargo test --test ra_syntax_poc --features ra_syntax_test -- --nocapture

#[cfg(feature = "ra_syntax_test")]
mod ra_syntax_tests {
    use ra_ap_syntax::{
        ast::{self, HasModuleItem, HasName, HasVisibility},
        AstNode, Edition, SourceFile,
    };

    #[derive(Debug, Clone)]
    pub struct Symbol {
        pub name: String,
        pub kind: SymbolKind,
        pub start_line: usize,
        pub end_line: usize,
        pub visibility: String,
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

    fn extract_visibility(vis: Option<ast::Visibility>) -> String {
        match vis {
            None => "private".to_string(),
            Some(v) => {
                let text = v.syntax().text().to_string();
                if text == "pub" {
                    "public".to_string()
                } else if text.starts_with("pub(crate)") {
                    "crate".to_string()
                } else if text.starts_with("pub(") {
                    format!("restricted({})", text)
                } else {
                    "public".to_string()
                }
            }
        }
    }

    fn line_of_offset(source: &str, offset: usize) -> usize {
        source[..offset.min(source.len())].chars().filter(|&c| c == '\n').count() + 1
    }

    pub fn extract_symbols(source: &str) -> Vec<Symbol> {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();
        let mut symbols = Vec::new();

        for item in file.items() {
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
                            type_name
                        ),
                        kind: SymbolKind::Impl { trait_name, type_name },
                        start_line: line_of_offset(source, i.syntax().text_range().start().into()),
                        end_line: line_of_offset(source, i.syntax().text_range().end().into()),
                        visibility: "private".to_string(),
                    });
                }
                ast::Item::Module(m) => {
                    if let Some(name) = m.name() {
                        symbols.push(Symbol {
                            name: name.text().to_string(),
                            kind: SymbolKind::Module,
                            start_line: line_of_offset(source, m.syntax().text_range().start().into()),
                            end_line: line_of_offset(source, m.syntax().text_range().end().into()),
                            visibility: extract_visibility(m.visibility()),
                        });
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
                        });
                    }
                }
                _ => {}
            }
        }

        symbols
    }

    #[test]
    fn test_basic_parsing() {
        let source = r#"
/// A greeting function
pub fn hello(name: &str) -> String {
    format!("Hello, {}!", name)
}

pub struct User {
    pub name: String,
    pub age: u32,
}

impl User {
    pub fn new(name: String) -> Self {
        Self { name, age: 0 }
    }
}

pub enum Status {
    Active,
    Inactive,
}

pub trait Greet {
    fn greet(&self) -> String;
}

impl Greet for User {
    fn greet(&self) -> String {
        format!("Hello, I'm {}", self.name)
    }
}
"#;

        let symbols = extract_symbols(source);

        println!("\n=== Extracted Symbols ===");
        for s in &symbols {
            println!("{:?}", s);
        }

        assert!(symbols.iter().any(|s| s.name == "hello"), "Should find 'hello' function");
        assert!(symbols.iter().any(|s| s.name == "User"), "Should find 'User' struct");
        assert!(symbols.iter().any(|s| s.name == "Status"), "Should find 'Status' enum");
        assert!(symbols.iter().any(|s| s.name == "Greet"), "Should find 'Greet' trait");
        assert!(symbols.iter().any(|s| matches!(&s.kind, SymbolKind::Impl { type_name, .. } if type_name == "User")),
            "Should find impl for User");

        println!("\n✓ All basic symbol types extracted correctly!");
    }

    #[test]
    fn test_async_unsafe_const() {
        let source = r#"
pub async fn async_func() {}
pub unsafe fn unsafe_func() {}
pub const fn const_func() {}
pub async unsafe fn async_unsafe_func() {}
"#;

        let symbols = extract_symbols(source);

        println!("\n=== Function Modifiers ===");
        for s in &symbols {
            println!("{:?}", s);
        }

        let async_fn = symbols.iter().find(|s| s.name == "async_func").unwrap();
        assert!(matches!(&async_fn.kind, SymbolKind::Function { is_async: true, is_unsafe: false, is_const: false }));

        let unsafe_fn = symbols.iter().find(|s| s.name == "unsafe_func").unwrap();
        assert!(matches!(&unsafe_fn.kind, SymbolKind::Function { is_async: false, is_unsafe: true, is_const: false }));

        let const_fn = symbols.iter().find(|s| s.name == "const_func").unwrap();
        assert!(matches!(&const_fn.kind, SymbolKind::Function { is_async: false, is_unsafe: false, is_const: true }));

        let async_unsafe = symbols.iter().find(|s| s.name == "async_unsafe_func").unwrap();
        assert!(matches!(&async_unsafe.kind, SymbolKind::Function { is_async: true, is_unsafe: true, is_const: false }));

        println!("\n✓ All function modifiers detected correctly!");
    }

    #[test]
    fn test_parallel_parsing() {
        use std::time::Instant;
        use rayon::prelude::*;

        // Generate 100 sample source files
        let sources: Vec<String> = (0..100)
            .map(|i| format!(r#"
                pub fn function_{i}(x: i32) -> i32 {{ x + {i} }}
                pub struct Struct{i} {{ field: i32 }}
                pub enum Enum{i} {{ A, B, C }}
                impl Struct{i} {{
                    pub fn new() -> Self {{ Self {{ field: {i} }} }}
                }}
            "#))
            .collect();

        // Sequential parsing
        let start = Instant::now();
        let _sequential: Vec<_> = sources.iter()
            .map(|s| extract_symbols(s))
            .collect();
        let sequential_time = start.elapsed();

        // Parallel parsing with rayon
        let start = Instant::now();
        let _parallel: Vec<_> = sources.par_iter()
            .map(|s| extract_symbols(s))
            .collect();
        let parallel_time = start.elapsed();

        println!("\n=== Parallel Parsing Benchmark ===");
        println!("Files parsed: 100");
        println!("Sequential time: {:?}", sequential_time);
        println!("Parallel time:   {:?}", parallel_time);
        println!("Speedup: {:.2}x", sequential_time.as_secs_f64() / parallel_time.as_secs_f64());

        // Parallel should be faster on multi-core systems
        println!("\n✓ Parallel parsing works!");
    }

    #[test]
    fn test_parse_real_file() {
        // Test parsing a real file from this codebase
        let path = std::path::Path::new("src/parser/mod.rs");
        if path.exists() {
            let source = std::fs::read_to_string(path).unwrap();
            let start = std::time::Instant::now();
            let symbols = extract_symbols(&source);
            let elapsed = start.elapsed();

            println!("\n=== Parsing Real File: src/parser/mod.rs ===");
            println!("Parse time: {:?}", elapsed);
            println!("Symbols found: {}", symbols.len());
            println!("\nFirst 10 symbols:");
            for s in symbols.iter().take(10) {
                println!("  - {} ({:?})", s.name, std::mem::discriminant(&s.kind));
            }

            assert!(!symbols.is_empty(), "Should find symbols in real file");
            println!("\n✓ Successfully parsed real codebase file!");
        } else {
            println!("Skipping real file test - src/parser/mod.rs not found");
        }
    }

    #[test]
    fn test_error_recovery() {
        // Test that ra_ap_syntax handles broken code gracefully
        let broken_source = r#"
pub fn valid_function() {}

pub fn broken_function( {  // Missing parameter and closing paren
    let x =
}

pub fn another_valid() -> i32 { 42 }
"#;

        let symbols = extract_symbols(broken_source);

        println!("\n=== Error Recovery Test ===");
        println!("Parsing broken source code...");
        for s in &symbols {
            println!("  Found: {} (line {})", s.name, s.start_line);
        }

        // Should still find valid_function and another_valid
        assert!(symbols.iter().any(|s| s.name == "valid_function"),
            "Should find valid_function despite broken code");
        assert!(symbols.iter().any(|s| s.name == "another_valid"),
            "Should find another_valid despite broken code");

        println!("\n✓ Error recovery works - valid symbols extracted from broken code!");
    }
}

// Fallback test when feature is not enabled
#[cfg(not(feature = "ra_syntax_test"))]
#[test]
fn test_feature_not_enabled() {
    println!("\n========================================");
    println!("To run ra_ap_syntax tests, run:");
    println!();
    println!("  cargo test --test ra_syntax_poc --features ra_syntax_test -- --nocapture");
    println!();
    println!("========================================\n");
}

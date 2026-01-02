//! Comparison test: tree-sitter vs ra_ap_syntax
//! Run with: cargo test --test parser_comparison --features ra_syntax_test -- --nocapture

#[cfg(feature = "ra_syntax_test")]
mod comparison_tests {
    use std::path::Path;
    use std::time::Instant;

    // Import current tree-sitter based parser
    use file_search_mcp::parser::RustParser;

    // ra_ap_syntax implementation
    use ra_ap_syntax::{
        ast::{self, HasModuleItem, HasName, HasVisibility},
        AstNode, Edition, SourceFile,
    };

    fn ra_extract_symbols(source: &str) -> Vec<(String, &'static str)> {
        let parse = SourceFile::parse(source, Edition::Edition2021);
        let file = parse.tree();
        let mut symbols = Vec::new();

        for item in file.items() {
            match item {
                ast::Item::Fn(f) => {
                    if let Some(name) = f.name() {
                        symbols.push((name.text().to_string(), "function"));
                    }
                }
                ast::Item::Struct(s) => {
                    if let Some(name) = s.name() {
                        symbols.push((name.text().to_string(), "struct"));
                    }
                }
                ast::Item::Enum(e) => {
                    if let Some(name) = e.name() {
                        symbols.push((name.text().to_string(), "enum"));
                    }
                }
                ast::Item::Trait(t) => {
                    if let Some(name) = t.name() {
                        symbols.push((name.text().to_string(), "trait"));
                    }
                }
                ast::Item::Impl(i) => {
                    let type_name = i.self_ty()
                        .map(|t| t.syntax().text().to_string())
                        .unwrap_or_default();
                    symbols.push((format!("impl {}", type_name), "impl"));
                }
                ast::Item::Module(m) => {
                    if let Some(name) = m.name() {
                        symbols.push((name.text().to_string(), "module"));
                    }
                }
                ast::Item::Const(c) => {
                    if let Some(name) = c.name() {
                        symbols.push((name.text().to_string(), "const"));
                    }
                }
                ast::Item::Static(s) => {
                    if let Some(name) = s.name() {
                        symbols.push((name.text().to_string(), "static"));
                    }
                }
                ast::Item::TypeAlias(t) => {
                    if let Some(name) = t.name() {
                        symbols.push((name.text().to_string(), "type_alias"));
                    }
                }
                _ => {}
            }
        }
        symbols
    }

    fn ts_extract_symbols(source: &str) -> Vec<(String, &'static str)> {
        let mut parser = RustParser::new().unwrap();
        let symbols = parser.parse_source(source).unwrap_or_default();

        symbols.into_iter().map(|s| {
            let kind = match s.kind {
                file_search_mcp::parser::SymbolKind::Function { .. } => "function",
                file_search_mcp::parser::SymbolKind::Struct => "struct",
                file_search_mcp::parser::SymbolKind::Enum => "enum",
                file_search_mcp::parser::SymbolKind::Trait => "trait",
                file_search_mcp::parser::SymbolKind::Impl { .. } => "impl",
                file_search_mcp::parser::SymbolKind::Module => "module",
                file_search_mcp::parser::SymbolKind::Const => "const",
                file_search_mcp::parser::SymbolKind::Static => "static",
                file_search_mcp::parser::SymbolKind::TypeAlias => "type_alias",
            };
            (s.name, kind)
        }).collect()
    }

    #[test]
    fn test_output_comparison() {
        let source = r#"
pub fn hello() {}
pub async fn async_hello() {}
pub struct User { name: String }
pub enum Status { Active, Inactive }
pub trait Greet { fn greet(&self); }
impl User { fn new() -> Self { todo!() } }
impl Greet for User { fn greet(&self) {} }
pub const MAX: i32 = 100;
pub static COUNTER: i32 = 0;
pub type UserId = u64;
mod inner { pub fn nested() {} }
"#;

        let ra_symbols = ra_extract_symbols(source);
        let ts_symbols = ts_extract_symbols(source);

        println!("\n=== Symbol Comparison ===");
        println!("\nra_ap_syntax found {} symbols:", ra_symbols.len());
        for (name, kind) in &ra_symbols {
            println!("  {} ({})", name, kind);
        }

        println!("\ntree-sitter found {} symbols:", ts_symbols.len());
        for (name, kind) in &ts_symbols {
            println!("  {} ({})", name, kind);
        }

        // Compare top-level counts
        let ra_funcs = ra_symbols.iter().filter(|(_, k)| *k == "function").count();
        let ts_funcs = ts_symbols.iter().filter(|(_, k)| *k == "function").count();

        println!("\n=== Summary ===");
        println!("Functions: ra={}, ts={}", ra_funcs, ts_funcs);
        println!("Structs: ra={}, ts={}",
            ra_symbols.iter().filter(|(_, k)| *k == "struct").count(),
            ts_symbols.iter().filter(|(_, k)| *k == "struct").count()
        );
        println!("Enums: ra={}, ts={}",
            ra_symbols.iter().filter(|(_, k)| *k == "enum").count(),
            ts_symbols.iter().filter(|(_, k)| *k == "enum").count()
        );

        // Note: tree-sitter extracts nested functions too, ra_ap_syntax only top-level
        println!("\n⚠ Note: tree-sitter extracts nested items, ra_ap_syntax (this POC) only top-level");
    }

    #[test]
    fn test_performance_comparison() {
        // Find real Rust files to test on
        let test_files: Vec<_> = walkdir::WalkDir::new("src")
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().map(|ext| ext == "rs").unwrap_or(false))
            .map(|e| e.path().to_path_buf())
            .collect();

        if test_files.is_empty() {
            println!("No Rust files found for benchmark");
            return;
        }

        println!("\n=== Performance Comparison ===");
        println!("Testing on {} files from src/\n", test_files.len());

        // Read all files first
        let sources: Vec<_> = test_files.iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .collect();

        let total_lines: usize = sources.iter().map(|s| s.lines().count()).sum();
        let total_bytes: usize = sources.iter().map(|s| s.len()).sum();

        println!("Total lines: {}", total_lines);
        println!("Total bytes: {}", total_bytes);

        // Benchmark tree-sitter (sequential)
        let start = Instant::now();
        let mut ts_total_symbols = 0;
        for source in &sources {
            let symbols = ts_extract_symbols(source);
            ts_total_symbols += symbols.len();
        }
        let ts_time = start.elapsed();

        // Benchmark ra_ap_syntax (sequential)
        let start = Instant::now();
        let mut ra_total_symbols = 0;
        for source in &sources {
            let symbols = ra_extract_symbols(source);
            ra_total_symbols += symbols.len();
        }
        let ra_seq_time = start.elapsed();

        // Benchmark ra_ap_syntax (parallel)
        use rayon::prelude::*;
        let start = Instant::now();
        let ra_par_symbols: usize = sources.par_iter()
            .map(|s| ra_extract_symbols(s).len())
            .sum();
        let ra_par_time = start.elapsed();

        println!("\n--- Sequential Performance ---");
        println!("tree-sitter:   {:?} ({} symbols)", ts_time, ts_total_symbols);
        println!("ra_ap_syntax:  {:?} ({} symbols)", ra_seq_time, ra_total_symbols);
        println!("Ratio (ts/ra): {:.2}x", ts_time.as_secs_f64() / ra_seq_time.as_secs_f64());

        println!("\n--- Parallel Performance (ra_ap_syntax) ---");
        println!("Sequential: {:?}", ra_seq_time);
        println!("Parallel:   {:?}", ra_par_time);
        println!("Speedup:    {:.2}x", ra_seq_time.as_secs_f64() / ra_par_time.as_secs_f64());

        println!("\n--- Throughput ---");
        println!("tree-sitter:      {:.0} lines/sec", total_lines as f64 / ts_time.as_secs_f64());
        println!("ra_ap_syntax seq: {:.0} lines/sec", total_lines as f64 / ra_seq_time.as_secs_f64());
        println!("ra_ap_syntax par: {:.0} lines/sec", total_lines as f64 / ra_par_time.as_secs_f64());

        // Note about symbol count difference
        if ts_total_symbols != ra_total_symbols {
            println!("\n⚠ Symbol count differs:");
            println!("  tree-sitter finds nested symbols (inside impl blocks, modules)");
            println!("  This POC only extracts top-level items");
            println!("  Full implementation would recurse into item bodies");
        }
    }

    #[test]
    fn test_nested_symbol_extraction() {
        // Test extracting symbols inside impl blocks (like tree-sitter does)
        let source = r#"
pub struct User {
    name: String,
}

impl User {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    pub fn greet(&self) -> String {
        format!("Hello, {}", self.name)
    }
}
"#;

        // Extended ra_ap_syntax extraction that recurses into impl blocks
        fn ra_extract_all_symbols(source: &str) -> Vec<(String, &'static str, Option<String>)> {
            let parse = SourceFile::parse(source, Edition::Edition2021);
            let file = parse.tree();
            let mut symbols = Vec::new();

            for item in file.items() {
                match &item {
                    ast::Item::Struct(s) => {
                        if let Some(name) = s.name() {
                            symbols.push((name.text().to_string(), "struct", None));
                        }
                    }
                    ast::Item::Impl(i) => {
                        let type_name = i.self_ty()
                            .map(|t| t.syntax().text().to_string());

                        // Recurse into impl body
                        if let Some(assoc_items) = i.assoc_item_list() {
                            for assoc in assoc_items.assoc_items() {
                                if let ast::AssocItem::Fn(f) = assoc {
                                    if let Some(name) = f.name() {
                                        symbols.push((
                                            name.text().to_string(),
                                            "function",
                                            type_name.clone()
                                        ));
                                    }
                                }
                            }
                        }

                        symbols.push((
                            format!("impl {}", type_name.as_deref().unwrap_or("?")),
                            "impl",
                            None
                        ));
                    }
                    _ => {}
                }
            }
            symbols
        }

        let symbols = ra_extract_all_symbols(source);

        println!("\n=== Nested Symbol Extraction ===");
        for (name, kind, parent) in &symbols {
            if let Some(p) = parent {
                println!("  {}::{} ({})", p, name, kind);
            } else {
                println!("  {} ({})", name, kind);
            }
        }

        assert!(symbols.iter().any(|(n, k, _)| n == "new" && *k == "function"),
            "Should find 'new' function inside impl");
        assert!(symbols.iter().any(|(n, k, _)| n == "greet" && *k == "function"),
            "Should find 'greet' function inside impl");

        println!("\n✓ Nested symbol extraction works!");
    }
}

#[cfg(not(feature = "ra_syntax_test"))]
#[test]
fn test_feature_not_enabled() {
    println!("\nRun with: cargo test --test parser_comparison --features ra_syntax_test -- --nocapture\n");
}

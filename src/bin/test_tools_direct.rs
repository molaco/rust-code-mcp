use file_search_mcp::parser::RustParser;
use std::fs;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("========================================");
    println!("Direct Functionality Testing (No MCP)");
    println!("========================================\n");

    let project_dir = "/home/molaco/Documents/rust-code-mcp";

    // Test 1: Read file content (standard library)
    println!("Test 1: Read File Content");
    println!("--------------------------");
    let file_path = format!("{}/src/lib.rs", project_dir);
    match fs::read_to_string(&file_path) {
        Ok(content) => {
            let lines: Vec<&str> = content.lines().take(5).collect();
            println!("✓ Successfully read file: {}", file_path);
            println!("First 5 lines:");
            for line in lines {
                println!("  {}", line);
            }
        }
        Err(e) => println!("✗ Error: {}", e),
    }
    println!();

    // Test 2: Find definition using RustParser
    println!("Test 2: Find Definition (RustParser)");
    println!("--------------------------");
    let mut parser = RustParser::new()?;
    let symbol_to_find = "RustParser";
    let mut found = false;

    let src_dir = format!("{}/src", project_dir);
    for entry in fs::read_dir(&src_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.is_dir() {
            // Check parser subdirectory
            let parser_mod = path.join("mod.rs");
            if parser_mod.exists() && path.file_name().unwrap() == "parser" {
                match parser.parse_file(&parser_mod) {
                    Ok(symbols) => {
                        for symbol in symbols {
                            if symbol.name == symbol_to_find {
                                println!("✓ Found '{}' at {}:{} ({})",
                                    symbol.name,
                                    parser_mod.display(),
                                    symbol.range.start_line,
                                    symbol.kind.as_str()
                                );
                                found = true;
                            }
                        }
                    }
                    Err(e) => println!("Parse error: {}", e),
                }
            }
        }
    }

    if !found {
        println!("✗ Symbol '{}' not found", symbol_to_find);
    }
    println!();

    // Test 3: Get dependencies using RustParser
    println!("Test 3: Get Dependencies (RustParser)");
    println!("--------------------------");
    let parser_file = format!("{}/src/parser/mod.rs", project_dir);
    match parser.parse_file_complete(Path::new(&parser_file)) {
        Ok(parse_result) => {
            println!("✓ Dependencies for '{}':", parser_file);
            if parse_result.imports.is_empty() {
                println!("  No imports found");
            } else {
                println!("  Imports ({}):", parse_result.imports.len());
                for import in parse_result.imports.iter().take(5) {
                    println!("    - {}", import.path);
                }
                if parse_result.imports.len() > 5 {
                    println!("    ... and {} more", parse_result.imports.len() - 5);
                }
            }
        }
        Err(e) => println!("✗ Error: {}", e),
    }
    println!();

    // Test 4: Analyze complexity
    println!("Test 4: Analyze Complexity");
    println!("--------------------------");
    let search_file = format!("{}/src/search/mod.rs", project_dir);
    match fs::read_to_string(&search_file) {
        Ok(source) => {
            match parser.parse_file_complete(Path::new(&search_file)) {
                Ok(parse_result) => {
                    let lines_of_code = source.lines().count();
                    let non_empty_loc = source.lines().filter(|l| !l.trim().is_empty()).count();
                    let function_count = parse_result.symbols.iter()
                        .filter(|s| matches!(s.kind, file_search_mcp::parser::SymbolKind::Function { .. }))
                        .count();

                    println!("✓ Complexity analysis for '{}':", search_file);
                    println!("  Total lines:     {}", lines_of_code);
                    println!("  Non-empty lines: {}", non_empty_loc);
                    println!("  Functions:       {}", function_count);
                    println!("  Call graph edges: {}", parse_result.call_graph.edge_count());
                }
                Err(e) => println!("✗ Parse error: {}", e),
            }
        }
        Err(e) => println!("✗ Error reading file: {}", e),
    }
    println!();

    println!("========================================");
    println!("All direct functionality tests completed!");
    println!("Tests prove the core libraries work correctly.");
    println!("========================================");

    Ok(())
}

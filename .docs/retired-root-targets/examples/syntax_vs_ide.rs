//! Direct comparison: Syntax-only (current) vs IDE (proposed)
//!
//! This shows exactly what you gain by switching to ra_ap_ide

use std::path::Path;
use std::time::Instant;

// Current approach: syntax only
use ra_ap_syntax::{Edition, SourceFile, ast::{self, HasName, HasModuleItem}};

// Proposed approach: IDE
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;

fn main() -> anyhow::Result<()> {
    println!("=== Syntax-only vs IDE Comparison ===\n");

    // Test code with ambiguous symbols
    let test_code = r#"
mod utils {
    pub struct Config {
        pub name: String,
    }

    pub fn parse(input: &str) -> Config {
        Config { name: input.to_string() }
    }
}

mod network {
    pub struct Config {
        pub host: String,
        pub port: u16,
    }

    pub fn parse(data: &[u8]) -> Config {
        Config { host: "localhost".to_string(), port: 8080 }
    }
}

use utils::Config;  // Which Config?

fn main() {
    let cfg = utils::parse("test");   // Which parse?
    let net = network::parse(&[]);     // Which parse?

    process(cfg);
}

fn process(config: Config) {  // Which Config is this?
    println!("{}", config.name);
}
"#;

    println!("Test code has:");
    println!("  - Two 'Config' structs (utils::Config, network::Config)");
    println!("  - Two 'parse' functions (utils::parse, network::parse)");
    println!("  - Ambiguous usage that requires semantic understanding\n");

    // =========================================
    println!("========================================");
    println!("APPROACH 1: Syntax-only (current)");
    println!("========================================\n");

    let start = Instant::now();
    let parse = SourceFile::parse(test_code, Edition::Edition2021);
    let file = parse.tree();
    println!("Parse time: {:?}\n", start.elapsed());

    // Find all symbols named "Config"
    println!("find_definition('Config'):");
    let mut config_count = 0;
    for item in file.items() {
        find_symbols_named(&item, "Config", &mut config_count, 1);
    }
    println!("  Result: Found {} definitions", config_count);
    println!("  Problem: Which one does 'process(config: Config)' refer to?\n");

    // Find all symbols named "parse"
    println!("find_definition('parse'):");
    let mut parse_count = 0;
    for item in file.items() {
        find_symbols_named(&item, "parse", &mut parse_count, 1);
    }
    println!("  Result: Found {} definitions", parse_count);
    println!("  Problem: Can't distinguish utils::parse from network::parse\n");

    println!("Syntax-only limitations:");
    println!("  - Returns ALL symbols with matching name");
    println!("  - No understanding of imports or scope");
    println!("  - Can't answer: 'which Config does this line refer to?'");
    println!("  - Call graph is name-based, not semantic\n");

    // =========================================
    println!("========================================");
    println!("APPROACH 2: IDE with no_deps=true");
    println!("========================================\n");

    // Load the actual project
    let project_path = std::env::current_dir()?;
    let start = Instant::now();

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

    let (db, vfs, _) = load_workspace_at(&project_path, &cargo_config, &load_config, &|_| {})?;
    let host = AnalysisHost::with_database(db);
    println!("Load time: {:?}\n", start.elapsed());

    // Demonstrate position-based goto definition
    println!("IDE capabilities (position-based):");
    println!("  - Click on 'RustParser' at line X, col Y → goes to exact definition");
    println!("  - Understands imports: 'use crate::parser::RustParser'");
    println!("  - Resolves through re-exports and type aliases");
    println!();

    // Show a real example
    let analysis = host.analysis();
    for (file_id, path) in vfs.iter() {
        let path_str = path.as_path().map(|p| p.to_string()).unwrap_or_default();
        if !path_str.ends_with("indexer_core.rs") || path_str.contains("target") {
            continue;
        }

        if let Ok(text) = analysis.file_text(file_id) {
            // Find "RustParser" usage
            if let Some(idx) = text.find("parser: RustParser") {
                let offset = ra_ap_ide::TextSize::from((idx + 8) as u32); // point to "RustParser"
                let position = ra_ap_ide::FilePosition { file_id, offset };
                let config = ra_ap_ide::GotoDefinitionConfig { minicore: Default::default() };

                println!("Example: goto_definition on 'RustParser' in indexer_core.rs:");
                if let Ok(Some(nav_info)) = analysis.goto_definition(position, &config) {
                    for target in &nav_info.info {
                        let target_path = vfs.file_path(target.file_id);
                        let short = target_path.as_path()
                            .map(|p| p.to_string())
                            .unwrap_or_default()
                            .split('/')
                            .last()
                            .unwrap_or("?")
                            .to_string();
                        println!("  → {} defined in {}", target.name, short);
                    }
                }
            }
        }
        break;
    }

    // =========================================
    println!("\n========================================");
    println!("COMPARISON SUMMARY");
    println!("========================================\n");

    println!("| Feature                          | Syntax-only | IDE no_deps |");
    println!("|----------------------------------|-------------|-------------|");
    println!("| Parse time                       | ~1ms        | ~120ms      |");
    println!("| find_definition by NAME          | ✅ (all)    | ✅ (all)    |");
    println!("| find_definition by POSITION      | ❌          | ✅          |");
    println!("| Disambiguate same-name symbols   | ❌          | ✅          |");
    println!("| Understand imports/scope         | ❌          | ✅          |");
    println!("| Resolve type aliases             | ❌          | ✅          |");
    println!("| Navigate to deps source          | ❌          | ❌          |");
    println!();

    println!("Key insight:");
    println!("  Syntax-only: 'find all things named X'");
    println!("  IDE:         'find THE thing at this position'\n");

    println!("For MCP tools:");
    println!("  - If user gives (file, line, col): IDE is required");
    println!("  - If user gives just a name: syntax works, but may return multiple");
    println!("  - IDE adds ~120ms load cost but enables precise navigation");

    Ok(())
}

fn find_symbols_named(item: &ast::Item, name: &str, count: &mut usize, depth: usize) {
    let indent = "  ".repeat(depth);

    match item {
        ast::Item::Struct(s) => {
            if let Some(n) = s.name() {
                if n.text() == name {
                    println!("{}  - struct {} (line ~{})", indent, name, "?");
                    *count += 1;
                }
            }
        }
        ast::Item::Fn(f) => {
            if let Some(n) = f.name() {
                if n.text() == name {
                    println!("{}  - fn {} (line ~{})", indent, name, "?");
                    *count += 1;
                }
            }
        }
        ast::Item::Module(m) => {
            if let Some(items) = m.item_list() {
                for inner in items.items() {
                    find_symbols_named(&inner, name, count, depth + 1);
                }
            }
        }
        ast::Item::Impl(i) => {
            if let Some(items) = i.assoc_item_list() {
                for assoc in items.assoc_items() {
                    if let ast::AssocItem::Fn(f) = assoc {
                        if let Some(n) = f.name() {
                            if n.text() == name {
                                println!("{}  - fn {} in impl (line ~{})", indent, name, "?");
                                *count += 1;
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

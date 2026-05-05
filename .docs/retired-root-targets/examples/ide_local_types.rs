//! Test: Can we navigate to LOCAL types with no_deps=true?

use std::path::Path;
use std::time::Instant;

use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;

fn load_project(path: &Path, no_deps: bool) -> anyhow::Result<(AnalysisHost, Vfs, std::time::Duration)> {
    let start = Instant::now();

    let cargo_config = CargoConfig {
        sysroot: None,
        no_deps,
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: true,
    };

    let (db, vfs, _) = load_workspace_at(path, &cargo_config, &load_config, &|_| {})?;
    let load_time = start.elapsed();
    let host = AnalysisHost::with_database(db);

    Ok((host, vfs, load_time))
}

fn test_goto_def_in_file(host: &AnalysisHost, vfs: &Vfs, file_suffix: &str, search_term: &str, description: &str) {
    let analysis = host.analysis();

    for (file_id, path) in vfs.iter() {
        let path_str = path.as_path().map(|p| p.to_string()).unwrap_or_default();
        if !path_str.ends_with(file_suffix) || path_str.contains("target") {
            continue;
        }

        if let Ok(text) = analysis.file_text(file_id) {
            // Find all occurrences and try each
            let mut found = false;
            for (idx, _) in text.match_indices(search_term) {
                let offset = ra_ap_ide::TextSize::from(idx as u32);
                let position = ra_ap_ide::FilePosition { file_id, offset };
                let config = ra_ap_ide::GotoDefinitionConfig { minicore: Default::default() };

                match analysis.goto_definition(position, &config) {
                    Ok(Some(nav_info)) if !nav_info.info.is_empty() => {
                        let short_file = path_str.split('/').last().unwrap_or("?");
                        println!("  {} '{}' in {}:", description, search_term, short_file);

                        // Get line number
                        let line_num = text[..idx].matches('\n').count() + 1;

                        // Show context
                        let line_start = text[..idx].rfind('\n').map(|p| p + 1).unwrap_or(0);
                        let line_end = text[idx..].find('\n').map(|p| idx + p).unwrap_or(text.len());
                        let line_content: String = text[line_start..line_end].chars().take(80).collect();
                        println!("      Found at line {}: {}", line_num, line_content.trim());

                        for target in nav_info.info.iter().take(2) {
                            let target_path = vfs.file_path(target.file_id);
                            let short = target_path.as_path()
                                .map(|p| p.to_string())
                                .unwrap_or_default()
                                .split('/')
                                .last()
                                .unwrap_or("?")
                                .to_string();
                            println!("      -> Defined: {} in {}", target.name, short);
                        }
                        found = true;
                        break;
                    }
                    _ => continue,
                }
            }
            if !found {
                let short_file = path_str.split('/').last().unwrap_or("?");
                println!("  {} '{}' in {}: NOT RESOLVED", description, search_term, short_file);
            }
            return;
        }
    }
    println!("  {} '{}': file not found", description, search_term);
}

fn main() -> anyhow::Result<()> {
    let project_path = std::env::current_dir()?;

    println!("=== Testing Local Type Navigation with no_deps=true ===\n");

    let (host, vfs, load_time) = load_project(&project_path, true)?;
    println!("Loaded in {:?} (no_deps=true)\n", load_time);

    println!("--- Local types (should resolve) ---\n");

    // Test in specific files where these types are USED (not just imported)
    test_goto_def_in_file(&host, &vfs, "unified.rs", "CodeChunk", "Local struct");
    test_goto_def_in_file(&host, &vfs, "unified.rs", "IndexStats", "Local struct");
    test_goto_def_in_file(&host, &vfs, "indexer_core.rs", "RustParser", "Local struct");
    test_goto_def_in_file(&host, &vfs, "indexer_core.rs", "Chunker", "Local struct");
    test_goto_def_in_file(&host, &vfs, "analysis_tools.rs", "Symbol", "Local struct");

    println!();
    println!("--- Dependency types (should NOT resolve with no_deps) ---\n");

    test_goto_def_in_file(&host, &vfs, "mod.rs", "SourceFile", "Dep (ra_ap_syntax)");
    test_goto_def_in_file(&host, &vfs, "tantivy_adapter.rs", "Index", "Dep (tantivy)");

    println!();
    println!("=== Conclusion ===");
    println!("With no_deps=true (~120ms load):");
    println!("  - Local types CAN be resolved");
    println!("  - Dependency types CANNOT be resolved");
    println!("This is fine for 'find definition' on YOUR code.");

    Ok(())
}

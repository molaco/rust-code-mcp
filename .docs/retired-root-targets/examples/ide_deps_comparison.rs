//! Compare IDE capabilities: no_deps=true vs no_deps=false
//!
//! Run with:
//!   cargo run --example ide_deps_comparison --features ide --release -- /path/to/project

use std::path::Path;
use std::time::Instant;

use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::{CargoConfig, RustLibSource};
use ra_ap_ide::AnalysisHost;
use ra_ap_hir::Crate;
use ra_ap_vfs::Vfs;

struct LoadedProject {
    host: AnalysisHost,
    vfs: Vfs,
    load_time: std::time::Duration,
    crate_count: usize,
    file_count: usize,
}

fn load_project(path: &Path, no_deps: bool, prefill: bool, with_sysroot: bool) -> anyhow::Result<LoadedProject> {
    let start = Instant::now();

    let cargo_config = CargoConfig {
        // Discover sysroot automatically to load stdlib
        sysroot: if with_sysroot { Some(RustLibSource::Discover) } else { None },
        no_deps,
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: prefill,
    };

    let (db, vfs, _) = load_workspace_at(path, &cargo_config, &load_config, &|_| {})?;
    let load_time = start.elapsed();

    let host = AnalysisHost::with_database(db);
    let crate_count = Crate::all(host.raw_database()).len();
    let file_count = vfs.iter().count();

    Ok(LoadedProject {
        host,
        vfs,
        load_time,
        crate_count,
        file_count,
    })
}

fn test_symbol_search(project: &LoadedProject, query: &str) {
    let analysis = project.host.analysis();
    let q = ra_ap_ide::Query::new(query.to_string());

    print!("  symbol_search '{}': ", query);
    let start = Instant::now();

    match analysis.symbol_search(q, 50) {
        Ok(results) => {
            println!("{} result(s) ({:?})", results.len(), start.elapsed());

            // Count local vs dependency results
            let mut local_count = 0;
            let mut dep_count = 0;
            let mut std_count = 0;

            for result in &results {
                let path = project.vfs.file_path(result.file_id);
                let path_str = path.as_path().map(|p| p.to_string()).unwrap_or_default();
                if path_str.contains(".rustup") {
                    std_count += 1;
                } else if path_str.contains(".cargo/registry") {
                    dep_count += 1;
                } else {
                    local_count += 1;
                }
            }

            println!("      local: {}, deps: {}, stdlib: {}", local_count, dep_count, std_count);

            // Show first few results
            for result in results.iter().take(8) {
                let path = project.vfs.file_path(result.file_id);
                let path_str = path.as_path().map(|p| p.to_string()).unwrap_or_default();
                let marker = if path_str.contains(".rustup") {
                    " [STD]"
                } else if path_str.contains(".cargo/registry") {
                    " [DEP]"
                } else {
                    " [LOCAL]"
                };
                let short_path = path_str.split('/').last().unwrap_or(&path_str);
                println!("      {} ({:?}){} in {}", result.name, result.kind, marker, short_path);
            }
            if results.len() > 8 {
                println!("      ... and {} more", results.len() - 8);
            }
        }
        Err(e) => println!("error: {:?}", e),
    }
}

fn test_goto_def_at_search(project: &LoadedProject, search_term: &str) {
    let analysis = project.host.analysis();

    // Find a file containing the search term
    for (file_id, path) in project.vfs.iter() {
        let path_str = path.as_path().map(|p| p.to_string()).unwrap_or_default();
        // Skip deps, look in local files only
        if path_str.contains(".cargo/registry") || path_str.contains(".rustup") || path_str.contains("target") {
            continue;
        }
        if !path_str.ends_with(".rs") {
            continue;
        }

        // Get file text
        if let Ok(text) = analysis.file_text(file_id) {
            if let Some(pos) = text.find(search_term) {
                let offset = ra_ap_ide::TextSize::from(pos as u32);
                let position = ra_ap_ide::FilePosition { file_id, offset };

                // Create config with empty minicore
                let config = ra_ap_ide::GotoDefinitionConfig {
                    minicore: Default::default(),
                };

                print!("  goto_def '{}' in {}: ", search_term, path_str.split('/').last().unwrap_or("?"));
                let start = Instant::now();

                match analysis.goto_definition(position, &config) {
                    Ok(Some(nav_info)) => {
                        println!("{} target(s) ({:?})", nav_info.info.len(), start.elapsed());
                        for target in nav_info.info.iter().take(3) {
                            let target_path = project.vfs.file_path(target.file_id);
                            let target_str = target_path.as_path().map(|p| p.to_string()).unwrap_or_default();
                            let marker = if target_str.contains(".rustup") {
                                " [STD]"
                            } else if target_str.contains(".cargo/registry") {
                                " [DEP]"
                            } else {
                                " [LOCAL]"
                            };
                            let short_path = target_str.split('/').last().unwrap_or(&target_str);
                            println!("      -> {}{} in {}", target.name, marker, short_path);
                        }
                    }
                    Ok(None) => println!("no definition found ({:?})", start.elapsed()),
                    Err(e) => println!("error: {:?}", e),
                }
                return;
            }
        }
    }
    println!("  goto_def '{}': not found in local files", search_term);
}

fn run_tests(project: &LoadedProject) {
    println!("\n--- Symbol Search (workspace-wide) ---");
    println!("  (Can we find stdlib types?)");
    test_symbol_search(project, "Vec");
    test_symbol_search(project, "HashMap");
    test_symbol_search(project, "PathBuf");

    println!("\n--- Goto Definition ---");
    println!("  (Can we navigate to the definition of stdlib types used in code?)");
    test_goto_def_at_search(project, "Vec<");
    test_goto_def_at_search(project, "HashMap");
    test_goto_def_at_search(project, "PathBuf");
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let project_path = if args.len() > 1 {
        std::path::PathBuf::from(&args[1])
    } else {
        std::env::current_dir()?
    };

    if !project_path.join("Cargo.toml").exists() {
        eprintln!("Error: {} does not contain Cargo.toml", project_path.display());
        std::process::exit(1);
    }

    println!("=== IDE Capabilities Comparison ===");
    println!("Project: {}", project_path.display());
    println!();

    // ========================================
    println!("========================================");
    println!("CONFIGURATION 1: no_deps=true, no sysroot");
    println!("  (fastest, most limited)");
    println!("========================================");

    let project1 = load_project(&project_path, true, false, false)?;
    println!("Load time: {:?}", project1.load_time);
    println!("Crates loaded: {}", project1.crate_count);
    println!("Files in VFS: {}", project1.file_count);
    run_tests(&project1);
    drop(project1);

    println!("\n");

    // ========================================
    println!("========================================");
    println!("CONFIGURATION 2: no_deps=true, WITH sysroot");
    println!("  (fast, can see stdlib but not deps)");
    println!("========================================");

    let project2 = load_project(&project_path, true, false, true)?;
    println!("Load time: {:?}", project2.load_time);
    println!("Crates loaded: {}", project2.crate_count);
    println!("Files in VFS: {}", project2.file_count);
    run_tests(&project2);
    drop(project2);

    println!("\n");

    // ========================================
    println!("========================================");
    println!("CONFIGURATION 3: no_deps=false, WITH sysroot, prefill=true");
    println!("  (slowest, full semantic analysis)");
    println!("========================================");

    let project3 = load_project(&project_path, false, true, true)?;
    println!("Load time: {:?}", project3.load_time);
    println!("Crates loaded: {}", project3.crate_count);
    println!("Files in VFS: {}", project3.file_count);
    run_tests(&project3);

    // ========================================
    println!("\n========================================");
    println!("SUMMARY");
    println!("========================================");
    println!("Config 1 (no_deps, no sysroot): ~80ms, local symbols only");
    println!("Config 2 (no_deps, sysroot):    ~200ms, local + stdlib");
    println!("Config 3 (deps, sysroot):       ~10-30s, full semantic");

    Ok(())
}

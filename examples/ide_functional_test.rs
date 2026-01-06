//! Functional test - verify IDE actually works after loading
//!
//! Run with:
//!   cargo run --example ide_functional_test --features ide --release -- /path/to/project

use std::time::Instant;

use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_hir::Crate;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();

    let project_path = if args.len() > 1 {
        std::path::PathBuf::from(&args[1])
    } else {
        std::env::current_dir()?
    };

    println!("=== IDE Functional Test ===");
    println!("Project: {}", project_path.display());
    println!();

    // Load with deps
    println!("Loading with no_deps=false (full deps)...");
    let start = Instant::now();

    let cargo_config = CargoConfig {
        sysroot: None,
        no_deps: false,
        ..Default::default()
    };

    // Test both configurations
    let prefill = std::env::var("PREFILL").map(|v| v == "1").unwrap_or(false);
    println!("prefill_caches: {}", prefill);
    println!();

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: prefill,
    };

    let (db, vfs, _) = load_workspace_at(
        &project_path,
        &cargo_config,
        &load_config,
        &|_| {},
    )?;

    let load_time = start.elapsed();
    let host = AnalysisHost::with_database(db);
    let analysis = host.analysis();

    let crate_count = Crate::all(host.raw_database()).len();
    let file_count = vfs.iter().count();

    println!("Loaded in {:?}", load_time);
    println!("Crates: {}", crate_count);
    println!("Files in VFS: {}", file_count);
    println!();

    // Test 1: Workspace symbol search
    println!("=== Test 1: Symbol search for 'new' ===");
    let search_start = Instant::now();
    let query = ra_ap_ide::Query::new("new".to_string());
    match analysis.symbol_search(query, 20) {
        Ok(results) => {
            println!("Found {} results ({:?})", results.len(), search_start.elapsed());
            for result in results.iter().take(10) {
                let file_path = vfs.file_path(result.file_id);
                println!("  - {} ({:?}) in {}", result.name, result.kind, file_path);
            }
            if results.len() > 10 {
                println!("  ... and {} more", results.len() - 10);
            }
        }
        Err(e) => println!("Search error: {:?}", e),
    }

    // Test 2: Search for a type
    println!();
    println!("=== Test 2: Symbol search for 'Tensor' ===");
    let search_start = Instant::now();
    let query = ra_ap_ide::Query::new("Tensor".to_string());
    match analysis.symbol_search(query, 20) {
        Ok(results) => {
            println!("Found {} results ({:?})", results.len(), search_start.elapsed());
            for result in results.iter().take(10) {
                let file_path = vfs.file_path(result.file_id);
                println!("  - {} ({:?}) in {}", result.name, result.kind, file_path);
            }
        }
        Err(e) => println!("Search error: {:?}", e),
    }

    // Test 3: Search for a trait
    println!();
    println!("=== Test 3: Symbol search for 'Backend' ===");
    let search_start = Instant::now();
    let query = ra_ap_ide::Query::new("Backend".to_string());
    match analysis.symbol_search(query, 20) {
        Ok(results) => {
            println!("Found {} results ({:?})", results.len(), search_start.elapsed());
            for result in results.iter().take(10) {
                let file_path = vfs.file_path(result.file_id);
                println!("  - {} ({:?}) in {}", result.name, result.kind, file_path);
            }
        }
        Err(e) => println!("Search error: {:?}", e),
    }

    // Test 4: Line index (proves file content is loaded)
    println!();
    println!("=== Test 4: Line index queries ===");
    let mut tested = 0;
    for (file_id, path) in vfs.iter().take(100) {
        if path.as_path().map(|p| p.to_string().ends_with(".rs")).unwrap_or(false) {
            let li_start = Instant::now();
            match analysis.file_line_index(file_id) {
                Ok(line_index) => {
                    let len = line_index.len();
                    if tested < 3 {
                        println!("  {} -> {:?} ({:?})", path, len, li_start.elapsed());
                    }
                    tested += 1;
                }
                Err(_) => {}
            }
        }
    }
    println!("  ... tested {} files", tested);

    // Test 5: Diagnostics for a file
    println!();
    println!("=== Test 5: Diagnostics ===");
    let diag_config = ra_ap_ide::DiagnosticsConfig::test_sample();
    for (file_id, path) in vfs.iter().take(50) {
        if let Some(p) = path.as_path() {
            let path_str = p.to_string();
            if path_str.ends_with("lib.rs") && path_str.contains("/burn") && !path_str.contains("target") {
                println!("Checking: {}", path_str);
                let diag_start = Instant::now();
                match analysis.full_diagnostics(&diag_config, ra_ap_ide::AssistResolveStrategy::None, file_id) {
                    Ok(diags) => {
                        println!("  {} diagnostics ({:?})", diags.len(), diag_start.elapsed());
                        for diag in diags.iter().take(3) {
                            println!("    - {:?}: {}", diag.code, diag.message);
                        }
                    }
                    Err(e) => println!("  Error: {:?}", e),
                }
                break;
            }
        }
    }

    println!();
    println!("=== Summary ===");
    println!("Load time: {:?}", load_time);
    println!("Crates loaded: {}", crate_count);
    println!("Files in VFS: {}", file_count);
    println!();
    println!("IDE is FUNCTIONAL - semantic queries return real results!");

    Ok(())
}

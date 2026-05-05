//! Benchmark for IDE approach (ra_ap_load_cargo + ra_ap_ide)
//!
//! Run with:
//!   cargo run --example ide_load_benchmark --features ide -- /path/to/project
//!
//! Compare with syntax-only:
//!   cargo run --example load_benchmark -- /path/to/project

use std::path::Path;
use std::time::Instant;
use std::fs;

use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_hir::Crate;

// Note: Cargo normalizes hyphenated crate names to underscores for imports
// ra_ap_load-cargo -> ra_ap_load_cargo
// ra_ap_project-model -> ra_ap_project_model

fn get_memory_mb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        if let Ok(status) = fs::read_to_string("/proc/self/status") {
            for line in status.lines() {
                if line.starts_with("VmRSS:") {
                    let parts: Vec<&str> = line.split_whitespace().collect();
                    if parts.len() >= 2 {
                        if let Ok(kb) = parts[1].parse::<f64>() {
                            return Some(kb / 1024.0);
                        }
                    }
                }
            }
        }
        None
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

fn count_project_stats(project_path: &Path) -> (usize, usize) {
    let mut files = 0;
    let mut lines = 0;

    fn visit(dir: &Path, files: &mut usize, lines: &mut usize) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().map(|n| n == "target").unwrap_or(false) {
                        continue;
                    }
                    visit(&path, files, lines);
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    *files += 1;
                    if let Ok(content) = fs::read_to_string(&path) {
                        *lines += content.lines().count();
                    }
                }
            }
        }
    }

    visit(project_path, &mut files, &mut lines);
    (files, lines)
}

fn benchmark_syntax_only(project_path: &Path) -> (std::time::Duration, usize) {
    use ra_ap_syntax::{Edition, SourceFile, ast::HasModuleItem};

    let start = Instant::now();
    let mut symbols = 0;

    fn visit(dir: &Path, symbols: &mut usize) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().map(|n| n == "target").unwrap_or(false) {
                        continue;
                    }
                    visit(&path, symbols);
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    if let Ok(source) = fs::read_to_string(&path) {
                        let parse = SourceFile::parse(&source, Edition::Edition2021);
                        *symbols += parse.tree().items().count();
                    }
                }
            }
        }
    }

    visit(project_path, &mut symbols);
    (start.elapsed(), symbols)
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
        eprintln!("Usage: {} [path-to-cargo-project]", args[0]);
        std::process::exit(1);
    }

    println!("=== IDE Load Time Benchmark ===");
    println!("Project: {}", project_path.display());
    println!();

    let (total_files, total_lines) = count_project_stats(&project_path);
    println!("Project stats: {} .rs files, {} lines of code", total_files, total_lines);
    println!();

    // Syntax-only benchmark
    println!("1. Syntax-only benchmark (ra_ap_syntax)...");
    let mem_before_syntax = get_memory_mb();
    let (syntax_time, syntax_symbols) = benchmark_syntax_only(&project_path);
    let mem_after_syntax = get_memory_mb();
    println!("   Time: {:?}", syntax_time);
    println!("   Symbols: {}", syntax_symbols);
    if let (Some(before), Some(after)) = (mem_before_syntax, mem_after_syntax) {
        println!("   Memory delta: {:.1} MB", after - before);
    }
    println!();

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,  // Skip build script outputs
        with_proc_macro_server: ProcMacroServerChoice::None,  // Skip proc macros
        prefill_caches: false,  // Don't prefill for initial benchmark
    };

    // ============ NO DEPS (local project only) ============
    println!("2. IDE benchmark - NO DEPS (local project only)...");

    let mem_before_no_deps = get_memory_mb();
    let start_no_deps = Instant::now();

    let cargo_config_no_deps = CargoConfig {
        sysroot: None,
        no_deps: true,  // Only local project!
        ..Default::default()
    };

    let load_result_no_deps = load_workspace_at(
        &project_path,
        &cargo_config_no_deps,
        &load_config,
        &|_| {},
    );

    let load_time_no_deps = start_no_deps.elapsed();
    let mem_after_no_deps = get_memory_mb();

    let (crates_no_deps, files_no_deps) = match &load_result_no_deps {
        Ok((db, vfs, _)) => {
            let host = AnalysisHost::with_database(db.clone());
            let all_crates = Crate::all(host.raw_database());
            (all_crates.len(), vfs.iter().count())
        }
        Err(_) => (0, 0),
    };

    println!("   Load time: {:?}", load_time_no_deps);
    println!("   Crates: {}, Files: {}", crates_no_deps, files_no_deps);
    if let (Some(before), Some(after)) = (mem_before_no_deps, mem_after_no_deps) {
        println!("   Memory: {:.1} MB", after - before);
    }
    println!();

    // ============ WITH DEPS ============
    println!("3. IDE benchmark - WITH DEPS (all dependencies)...");

    let mem_before_deps = get_memory_mb();
    let start_deps = Instant::now();

    let cargo_config_deps = CargoConfig {
        sysroot: None,
        no_deps: false,  // Load all dependencies
        ..Default::default()
    };

    let load_result_deps = load_workspace_at(
        &project_path,
        &cargo_config_deps,
        &load_config,
        &|_| {},
    );

    let load_time_deps = start_deps.elapsed();
    let mem_after_deps = get_memory_mb();

    let (crates_deps, files_deps, host_for_query) = match load_result_deps {
        Ok((db, vfs, _)) => {
            let host = AnalysisHost::with_database(db);
            let all_crates = Crate::all(host.raw_database());
            let count = (all_crates.len(), vfs.iter().count());
            (count.0, count.1, Some((host, vfs)))
        }
        Err(_) => (0, 0, None),
    };

    println!("   Load time: {:?}", load_time_deps);
    println!("   Crates: {}, Files: {}", crates_deps, files_deps);
    if let (Some(before), Some(after)) = (mem_before_deps, mem_after_deps) {
        println!("   Memory: {:.1} MB", after - before);
    }
    println!();

    // ============ QUERY PERFORMANCE ============
    if let Some((host, vfs)) = host_for_query {
        println!("4. Query performance (with deps loaded)...");
        let analysis = host.analysis();
        if let Some((file_id, _)) = vfs.iter().next() {
            let query_start = Instant::now();
            let _ = analysis.file_line_index(file_id);
            println!("   file_line_index: {:?}", query_start.elapsed());
        }
        println!();
    }

    // ============ SUMMARY ============
    println!("=== BENCHMARK SUMMARY ===");
    println!();
    println!("| Configuration      | Load Time       | Memory   | Crates | Files  |");
    println!("|--------------------|-----------------|----------|--------|--------|");
    println!("| Syntax-only        | {:>15?} | {:>6.1} MB | {:>6} | {:>6} |",
             syntax_time,
             mem_after_no_deps.unwrap_or(0.0) - mem_before_no_deps.unwrap_or(0.0),
             "-", total_files);
    println!("| IDE (no_deps=true) | {:>15?} | {:>6.1} MB | {:>6} | {:>6} |",
             load_time_no_deps,
             mem_after_no_deps.unwrap_or(0.0) - mem_before_no_deps.unwrap_or(0.0),
             crates_no_deps, files_no_deps);
    println!("| IDE (with deps)    | {:>15?} | {:>6.1} MB | {:>6} | {:>6} |",
             load_time_deps,
             mem_after_deps.unwrap_or(0.0) - mem_before_deps.unwrap_or(0.0),
             crates_deps, files_deps);
    println!();

    let ratio_no_deps = load_time_no_deps.as_secs_f64() / syntax_time.as_secs_f64();
    let ratio_deps = load_time_deps.as_secs_f64() / syntax_time.as_secs_f64();

    println!("Slowdown vs syntax-only:");
    println!("  - no_deps=true:  {:.1}x slower", ratio_no_deps);
    println!("  - with deps:     {:.1}x slower", ratio_deps);
    println!();
    println!("RECOMMENDATION: Use no_deps=true for fast loading (~{:.0}ms)",
             load_time_no_deps.as_secs_f64() * 1000.0);
    println!("  - Still provides full semantic analysis for YOUR code");
    println!("  - Can't resolve symbols into external crates (shows as unresolved)");
    println!("  - Best for: find_definition, find_references within project");

    Ok(())
}

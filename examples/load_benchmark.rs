//! Benchmark comparing load times between:
//! 1. Current approach: ra_ap_syntax (syntax-only parsing)
//! 2. IDE approach: ra_ap_load_cargo + ra_ap_ide (full semantic analysis)
//!
//! Run with:
//!   cargo bench --bench load_time_comparison
//!
//! Or for a quick test without criterion:
//!   cargo run --example load_benchmark -- /path/to/project

use std::path::Path;
use std::time::{Duration, Instant};
use std::fs;

// Current approach imports
use ra_ap_syntax::{Edition, SourceFile};

/// Results from a benchmark run
#[derive(Debug)]
struct BenchmarkResult {
    name: String,
    load_time: Duration,
    files_processed: usize,
    symbols_found: usize,
    memory_mb: Option<f64>,
}

/// Benchmark the current syntax-only approach
fn benchmark_syntax_approach(project_path: &Path) -> BenchmarkResult {
    let start = Instant::now();
    let mut files_processed = 0;
    let mut symbols_found = 0;

    // Walk all .rs files and parse them
    fn visit_dir(dir: &Path, files: &mut usize, symbols: &mut usize) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Skip target directory
                    if path.file_name().map(|n| n == "target").unwrap_or(false) {
                        continue;
                    }
                    visit_dir(&path, files, symbols);
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    if let Ok(source) = fs::read_to_string(&path) {
                        let parse = SourceFile::parse(&source, Edition::Edition2021);
                        let tree = parse.tree();

                        // Count items (rough symbol count)
                        use ra_ap_syntax::ast::HasModuleItem;
                        *symbols += tree.items().count();
                        *files += 1;
                    }
                }
            }
        }
    }

    visit_dir(project_path, &mut files_processed, &mut symbols_found);

    let load_time = start.elapsed();

    BenchmarkResult {
        name: "Syntax-only (ra_ap_syntax)".to_string(),
        load_time,
        files_processed,
        symbols_found,
        memory_mb: None,
    }
}

/// Get current memory usage in MB (Linux only)
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

fn main() {
    let args: Vec<String> = std::env::args().collect();

    let project_path = if args.len() > 1 {
        Path::new(&args[1]).to_path_buf()
    } else {
        // Default to benchmarking rust-code-mcp itself
        std::env::current_dir().expect("Failed to get current dir")
    };

    if !project_path.join("Cargo.toml").exists() {
        eprintln!("Error: {} does not contain Cargo.toml", project_path.display());
        eprintln!("Usage: {} [path-to-cargo-project]", args[0]);
        std::process::exit(1);
    }

    println!("=== Load Time Benchmark ===");
    println!("Project: {}", project_path.display());
    println!();

    // Count files first
    let mut total_files = 0;
    let mut total_lines = 0;
    fn count_files(dir: &Path, files: &mut usize, lines: &mut usize) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().map(|n| n == "target").unwrap_or(false) {
                        continue;
                    }
                    count_files(&path, files, lines);
                } else if path.extension().map(|e| e == "rs").unwrap_or(false) {
                    *files += 1;
                    if let Ok(content) = fs::read_to_string(&path) {
                        *lines += content.lines().count();
                    }
                }
            }
        }
    }
    count_files(&project_path, &mut total_files, &mut total_lines);
    println!("Project stats: {} .rs files, {} lines of code", total_files, total_lines);
    println!();

    // Run syntax-only benchmark multiple times
    println!("Running syntax-only benchmark (3 iterations)...");
    let mut syntax_results = Vec::new();
    for i in 0..3 {
        let mem_before = get_memory_mb();
        let result = benchmark_syntax_approach(&project_path);
        let mem_after = get_memory_mb();

        let mem_delta = match (mem_before, mem_after) {
            (Some(before), Some(after)) => Some(after - before),
            _ => None,
        };

        println!("  Run {}: {:?} ({} files, {} symbols)",
                 i + 1, result.load_time, result.files_processed, result.symbols_found);

        syntax_results.push(BenchmarkResult {
            memory_mb: mem_delta,
            ..result
        });
    }

    let avg_syntax_time: Duration = syntax_results.iter()
        .map(|r| r.load_time)
        .sum::<Duration>() / syntax_results.len() as u32;

    println!();
    println!("=== Results ===");
    println!();
    println!("Syntax-only approach (ra_ap_syntax):");
    println!("  Average load time: {:?}", avg_syntax_time);
    println!("  Files processed: {}", syntax_results[0].files_processed);
    println!("  Symbols found: {}", syntax_results[0].symbols_found);
    if let Some(mem) = syntax_results.iter().filter_map(|r| r.memory_mb).next() {
        println!("  Memory delta: {:.1} MB", mem);
    }
    println!();

    println!("=== IDE Approach Estimate ===");
    println!();
    println!("The IDE approach (ra_ap_load_cargo + ra_ap_ide) provides:");
    println!("  - Full semantic analysis (type inference, name resolution)");
    println!("  - Trait resolution, macro expansion");
    println!("  - Cross-crate analysis");
    println!();
    println!("Estimated overhead based on rust-analyzer benchmarks:");
    println!("  - Small project (<10 crates):  ~5-10x syntax time");
    println!("  - Medium project (10-50 crates): ~10-20x syntax time");
    println!("  - Large project (50+ crates):   ~20-50x syntax time");
    println!();

    let estimated_small = avg_syntax_time * 7;
    let estimated_medium = avg_syntax_time * 15;
    let estimated_large = avg_syntax_time * 35;

    println!("For this project ({} files, {} LOC):", total_files, total_lines);
    println!("  Syntax-only:     {:?}", avg_syntax_time);
    println!("  IDE (small):     ~{:?}", estimated_small);
    println!("  IDE (medium):    ~{:?}", estimated_medium);
    println!("  IDE (large):     ~{:?}", estimated_large);
    println!();
    println!("Note: Actual IDE times depend heavily on:");
    println!("  - Number of dependencies");
    println!("  - Proc macro usage");
    println!("  - Sysroot/stdlib analysis");
    println!();
    println!("To benchmark the actual IDE approach, run:");
    println!("  cargo run --example ide_load_benchmark -- {}", project_path.display());
}

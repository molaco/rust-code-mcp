//! Phase-0 spike: time `Definition::usages(sema).all()` on a sample of items
//! from a workspace, to estimate the cost of an eager usage-extraction pass.
//!
//! Usage:
//!   cargo run --release --example spike_usages -- <workspace> [sample_size]
//!
//! Defaults: workspace=/home/molaco/Documents/coding-agent, sample_size=10.

use std::path::Path;
use std::time::Instant;

use ra_ap_hir::{ModuleDef, Semantics, attach_db};
use ra_ap_ide_db::defs::Definition;

use file_search_mcp::graph::loader;

fn main() {
    let workspace = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/molaco/Documents/coding-agent".to_string());
    let sample_size: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10);

    let workspace_path = Path::new(&workspace);
    eprintln!("workspace: {}", workspace_path.display());

    let t_load = Instant::now();
    let loaded = loader::load(workspace_path).expect("load");
    eprintln!("load:   {:>8.2?} ({} local crates)", t_load.elapsed(), loaded.local_crates.len());

    let db = &loaded.db;

    attach_db(db, || {
        let sema = Semantics::new(db);

        // Walk every local crate, collecting all reachable items. If sample_size==0
        // process them all; otherwise take a deterministic stride to spread coverage.
        let mut all: Vec<(String, Definition)> = Vec::new();
        for &krate in &loaded.local_crates {
            let crate_name = krate
                .display_name(db)
                .map(|n| n.canonical_name().as_str().to_string())
                .unwrap_or_else(|| "?".into());
            let mut module_queue = vec![krate.root_module(db)];
            while let Some(module) = module_queue.pop() {
                for child in module.children(db) {
                    module_queue.push(child);
                }
                for module_def in module.declarations(db) {
                    let (name, def) = match module_def {
                        ModuleDef::Function(f) => (f.name(db).as_str().to_string(), Definition::Function(f)),
                        ModuleDef::Adt(a) => (a.name(db).as_str().to_string(), Definition::Adt(a)),
                        ModuleDef::Trait(t) => (t.name(db).as_str().to_string(), Definition::Trait(t)),
                        ModuleDef::Const(c) => (
                            c.name(db).map(|n| n.as_str().to_string()).unwrap_or_default(),
                            Definition::Const(c),
                        ),
                        ModuleDef::Static(s) => (s.name(db).as_str().to_string(), Definition::Static(s)),
                        ModuleDef::TypeAlias(t) => {
                            (t.name(db).as_str().to_string(), Definition::TypeAlias(t))
                        }
                        _ => continue,
                    };
                    all.push((format!("{crate_name}::{name}"), def));
                }
            }
        }
        eprintln!("local items reachable from root modules: {}", all.len());

        let candidates: Vec<(String, Definition)> = if sample_size == 0 || sample_size >= all.len() {
            all
        } else {
            let stride = (all.len() / sample_size).max(1);
            all.into_iter().step_by(stride).take(sample_size).collect()
        };

        eprintln!(
            "\nMeasuring Definition::usages for {} items...\n",
            candidates.len()
        );

        let mut total_refs = 0usize;
        let total_t = Instant::now();
        for (name, def) in &candidates {
            let t = Instant::now();
            let result = def.usages(&sema).all();
            let elapsed = t.elapsed();
            let refs: usize = result.references.values().map(|v| v.len()).sum();
            total_refs += refs;
            eprintln!("  {:50} {:>5} refs   {:>9.2?}", truncate(name, 50), refs, elapsed);
        }
        let total = total_t.elapsed();
        eprintln!();
        eprintln!(
            "total: {} refs across {} items in {:.2?}  (avg {:.2?}/item)",
            total_refs,
            candidates.len(),
            total,
            total / candidates.len() as u32
        );
    });
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max - 1])
    }
}

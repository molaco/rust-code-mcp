//! Debug: list local crates loaded by `graph::loader::load` against burn,
//! and diff against the workspace's expected member set.
//!
//! Usage: cargo run --release --example debug_burn_loader [-- <workspace>]
//! Default workspace: /home/molaco/Documents/burn

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use file_search_mcp::graph::{LoadedWorkspace, load};

fn main() {
    let workspace = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "/home/molaco/Documents/burn".to_string());
    let workspace_path = Path::new(&workspace);

    eprintln!("loading workspace: {}", workspace_path.display());
    let started = std::time::Instant::now();
    let loaded: LoadedWorkspace = match load(workspace_path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("ERR loader: {e:#}");
            std::process::exit(1);
        }
    };
    eprintln!("loaded in {:?}", started.elapsed());

    // 1. Loader's local_crates set, normalized to canonical_name (underscored).
    let mut loaded_names: BTreeSet<String> = BTreeSet::new();
    for k in &loaded.local_crates {
        let name = k
            .display_name(&loaded.db)
            .map(|n| n.canonical_name().as_str().to_string())
            .unwrap_or_else(|| "<no display_name>".to_string());
        loaded_names.insert(name);
    }
    println!("=== loader.local_crates ({}) ===", loaded_names.len());
    for n in &loaded_names {
        println!("  {n}");
    }

    // 2. Expected member crate names: walk workspace members per Cargo.toml.
    let expected_names = match expected_member_crates(workspace_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("ERR expected: {e:#}");
            std::process::exit(1);
        }
    };
    println!();
    println!("=== expected workspace members ({}) ===", expected_names.len());
    for n in &expected_names {
        println!("  {n}");
    }

    // 3. Diffs.
    let missing_from_loader: BTreeSet<&String> =
        expected_names.difference(&loaded_names).collect();
    let extra_in_loader: BTreeSet<&String> = loaded_names.difference(&expected_names).collect();

    println!();
    println!(
        "=== expected NOT in loader.local_crates ({}) ===",
        missing_from_loader.len()
    );
    for n in &missing_from_loader {
        println!("  {n}");
    }

    println!();
    println!(
        "=== loader.local_crates NOT in expected ({}) ===",
        extra_in_loader.len()
    );
    for n in &extra_in_loader {
        println!("  {n}");
    }
}

/// Discover workspace member package names by:
///   1. parsing root Cargo.toml [workspace].members (with literal entries +
///      glob patterns like "crates/*", "examples/*"),
///   2. expanding globs over the filesystem,
///   3. parsing each member directory's Cargo.toml and reading [package].name,
///   4. normalizing hyphens to underscores (RA's canonical_name convention).
fn expected_member_crates(workspace_root: &Path) -> anyhow::Result<BTreeSet<String>> {
    let root_toml = workspace_root.join("Cargo.toml");
    let bytes = fs::read_to_string(&root_toml)?;

    // Naive but workable: scan the [workspace] members = [...] block.
    let members_lines = extract_array_block(&bytes, "members")
        .ok_or_else(|| anyhow::anyhow!("no [workspace].members in {}", root_toml.display()))?;
    let excludes_lines = extract_array_block(&bytes, "exclude").unwrap_or_default();

    let member_patterns: Vec<String> = parse_string_list(&members_lines);
    let exclude_patterns: Vec<String> = parse_string_list(&excludes_lines);

    let mut member_dirs: HashSet<PathBuf> = HashSet::new();
    for pat in &member_patterns {
        for dir in expand_pattern(workspace_root, pat) {
            member_dirs.insert(dir);
        }
    }
    for pat in &exclude_patterns {
        for dir in expand_pattern(workspace_root, pat) {
            member_dirs.remove(&dir);
        }
    }

    let mut names: BTreeSet<String> = BTreeSet::new();
    for dir in &member_dirs {
        let cargo = dir.join("Cargo.toml");
        if !cargo.exists() {
            // Probably a directory we matched but isn't a crate (e.g. notebook).
            continue;
        }
        let txt = match fs::read_to_string(&cargo) {
            Ok(t) => t,
            Err(_) => continue,
        };
        if let Some(name) = extract_package_name(&txt) {
            names.insert(name.replace('-', "_"));
        }
    }
    Ok(names)
}

fn extract_array_block(toml_text: &str, key: &str) -> Option<String> {
    // Find `key = [` and capture until the matching `]`.
    // Only inspect the `[workspace]` table (top of file by convention).
    let needle = format!("{key} = [");
    let start = toml_text.find(&needle)?;
    let after = &toml_text[start + needle.len()..];
    let end = after.find(']')?;
    Some(after[..end].to_string())
}

fn parse_string_list(block: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in block.split(',') {
        let s = raw.trim();
        // Strip surrounding quotes.
        if let Some(stripped) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
            out.push(stripped.to_string());
        } else if let Some(stripped) = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
            out.push(stripped.to_string());
        }
    }
    out
}

fn expand_pattern(workspace_root: &Path, pattern: &str) -> Vec<PathBuf> {
    let full = workspace_root.join(pattern);
    let s = full.to_string_lossy().to_string();
    if s.contains('*') || s.contains('?') {
        match glob::glob(&s) {
            Ok(iter) => iter
                .filter_map(|r| r.ok())
                .filter(|p| p.is_dir())
                .collect(),
            Err(_) => Vec::new(),
        }
    } else if full.is_dir() {
        vec![full]
    } else {
        Vec::new()
    }
}

fn extract_package_name(toml_text: &str) -> Option<String> {
    // Look for `[package]` then `name = "..."`.
    let pkg_idx = toml_text.find("[package]")?;
    let after = &toml_text[pkg_idx..];
    // Bound search at next [section] header.
    let bound = after[1..]
        .find("\n[")
        .map(|i| i + 1)
        .unwrap_or(after.len());
    let block = &after[..bound];
    for line in block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let v = rest.trim();
                if let Some(stripped) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    return Some(stripped.to_string());
                }
            }
        }
    }
    None
}

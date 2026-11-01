//! Freshness gate for the hand-labelled relevance dataset.
//!
//! A relevance dataset rots silently: files get renamed or merged, the labels
//! keep naming paths that no longer exist, every profile scores zero on those
//! queries, and the table still prints — the drop reads as "search got worse"
//! rather than "the ruler broke". The previous symbol-name dataset rotted
//! exactly this way. These tests are cheap (no model, no index, no network) and
//! run in the ordinary suite so the ruler is checked before it is used.

use rmc_engine::search::RelevanceCase;
use serde::Deserialize;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct RelevanceDataset {
    cases: Vec<RelevanceCase>,
}

/// Repository root, from this crate's manifest directory (`crates/rust-code-mcp`).
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn load() -> (RelevanceDataset, PathBuf) {
    let root = repo_root();
    let path = root.join("eval/relevance_queries.json");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    let dataset: RelevanceDataset = serde_json::from_str(&source)
        .unwrap_or_else(|e| panic!("cannot parse {}: {e}", path.display()));
    (dataset, root)
}

#[test]
fn every_labelled_file_still_exists() {
    let (dataset, root) = load();

    let missing: Vec<String> = dataset
        .cases
        .iter()
        .flat_map(|case| {
            case.expected_files.iter().map(move |file| (case, file))
        })
        .filter(|(_, file)| !root.join(&file.0).is_file())
        .map(|(case, file)| format!("`{}` -> {}", case.query, file.0))
        .collect();

    assert!(
        missing.is_empty(),
        "{} labelled path(s) no longer exist; the dataset is stale, not the search:\n{}",
        missing.len(),
        missing.join("\n")
    );
}

#[test]
fn every_case_is_scoreable() {
    let (dataset, _) = load();

    for case in &dataset.cases {
        case.validate()
            .unwrap_or_else(|e| panic!("unscoreable case in the dataset: {e}"));
    }
}

#[test]
fn queries_are_unique() {
    // A duplicated query would silently double its weight in every mean.
    let (dataset, _) = load();

    let mut seen = HashSet::new();
    for case in &dataset.cases {
        assert!(
            seen.insert(case.query.to_lowercase()),
            "duplicate query in the dataset: `{}`",
            case.query
        );
    }
}

#[test]
fn dataset_is_large_enough_to_compare_profiles() {
    // 30 is the floor named in the plan: below it a one-query difference moves
    // a mean by more than the gap between two profiles usually is.
    let (dataset, _) = load();

    assert!(
        dataset.cases.len() >= 30,
        "only {} labelled queries; a profile comparison on this many is noise",
        dataset.cases.len()
    );
}

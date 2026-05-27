//! Parallel-traversal helpers for the unified indexing pipeline.
//!
//! Companion to `unified.rs`. Hosts pure traversal and parallel-parse
//! helpers extracted from `UnifiedIndexer` so the orchestration file
//! stays focused on coordination. These helpers do not touch
//! `UnifiedIndexer` state directly — they operate on borrowed inputs and
//! return owned results that the caller folds back into the indexer.

use crate::indexing::error_collection::{categorize_error, ErrorCollector, ErrorDetail};
use crate::indexing::indexer_core::{IndexerCore, ProcessedFile};
use crate::indexing::unified::IndexStats;
use anyhow::Result;
use rayon::prelude::*;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Walk `dir_path` and return all reachable `*.rs` files, skipping common
/// VCS / build / generated directories (`target`, `vendor`, `.git`, `.jj`,
/// `.direnv`, `.skeleton`).
///
/// Pure traversal: does not touch `UnifiedIndexer` state. The caller passes
/// in `stats` so we can populate `total_files` in one place.
pub(super) fn collect_rust_files(
    dir_path: &Path,
    stats: &mut IndexStats,
) -> Result<Vec<PathBuf>> {
    let mut rust_files = Vec::new();
    let mut walk_errors = 0;

    let walker = WalkDir::new(dir_path)
        .into_iter()
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !(entry.file_type().is_dir()
                && matches!(
                    name.as_ref(),
                    "target" | "vendor" | ".git" | ".jj" | ".direnv" | ".skeleton"
                ))
        });

    for entry in walker {
        match entry {
            Ok(e)
                if e.file_type().is_file()
                    && e.path().extension() == Some(std::ffi::OsStr::new("rs")) =>
            {
                rust_files.push(e.path().to_path_buf());
            }
            Ok(_) => {}
            Err(err) => {
                let path = err.path().unwrap_or_else(|| Path::new("<unknown>"));
                tracing::warn!("Failed to access {}: {}", path.display(), err);
                walk_errors += 1;
            }
        }
    }

    if walk_errors > 0 {
        tracing::warn!(
            "Encountered {} errors during directory walk, continuing with accessible files",
            walk_errors
        );
    }

    stats.total_files = rust_files.len();
    Ok(rust_files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn collect_rust_files_skips_generated_skeleton_tree() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        let src = root.join("src");
        let skeleton = root.join(".skeleton/src");
        fs::create_dir_all(&src).expect("create src");
        fs::create_dir_all(&skeleton).expect("create skeleton");
        fs::write(src.join("lib.rs"), "pub fn real() {}\n").expect("write real source");
        fs::write(skeleton.join("lib.rs"), "pub fn generated() {}\n")
            .expect("write generated source");

        let mut stats = IndexStats::default();
        let files = collect_rust_files(root, &mut stats).expect("collect rust files");

        assert_eq!(stats.total_files, 1);
        assert_eq!(files, vec![src.join("lib.rs")]);
    }
}

/// PHASE 1 of `index_directory_parallel`: parse and chunk a batch of files
/// in parallel using rayon.
///
/// Returns the successfully processed files plus an `ErrorCollector`
/// holding categorized errors for files that failed. Pure CPU-bound work;
/// no embedding generation or store mutation here.
pub(super) fn parallel_parse_batch(
    core: &IndexerCore,
    file_batch: &[PathBuf],
) -> (Vec<ProcessedFile>, ErrorCollector) {
    let error_collector = ErrorCollector::new();
    let error_collector_clone = error_collector.clone();

    let processed: Vec<ProcessedFile> = file_batch
        .par_iter()
        .filter_map(|file_path| {
            match core.process_file_sync(file_path) {
                Ok(processed) => {
                    tracing::debug!("Parsed: {}", file_path.display());
                    Some(processed)
                }
                Err(e) => {
                    error_collector_clone.record(ErrorDetail {
                        file_path: file_path.clone(),
                        category: categorize_error(&e),
                        message: e.to_string(),
                    });
                    None
                }
            }
        })
        .collect();

    (processed, error_collector)
}

/// Drain `error_collector` into `stats.skipped_files`, logging each entry
/// at the appropriate level for its category.
pub(super) fn process_batch_errors(
    error_collector: &ErrorCollector,
    stats: &mut IndexStats,
) {
    for error in error_collector.get_errors() {
        match error.category {
            crate::indexing::error_collection::ErrorCategory::Permanent => {
                tracing::debug!("Skipped {}: {}", error.file_path.display(), error.message);
                stats.skipped_files += 1;
            }
            crate::indexing::error_collection::ErrorCategory::Transient => {
                tracing::warn!("Failed {}: {}", error.file_path.display(), error.message);
                stats.skipped_files += 1;
            }
        }
    }
}

//! Single source of truth for "which `*.rs` files belong to a project".
//!
//! The walk used to live inside `unified_parallel::collect_rust_files`, where
//! it was reachable only by the indexer. A daemon that keeps a rust-analyzer
//! context loaded across sessions needs the same answer for a different
//! question — "did anything in this project change while it sat idle?" — and
//! two independent walkers would mean the two sides disagree about what a
//! project file is, which shows up as a context that is either refreshed for no
//! reason or not refreshed when it should be.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Directories never descended into: build output, vendored copies, VCS
/// metadata and generated skeleton trees. Sources under these are either not
/// ours or not authoritative.
pub(crate) const SKIPPED_DIRS: [&str; 6] =
    ["target", "vendor", ".git", ".jj", ".direnv", ".skeleton"];

/// True when a directory with this name must not be descended into.
pub(crate) fn is_skipped_dir_name(name: &str) -> bool {
    SKIPPED_DIRS.contains(&name)
}

/// Walk `root` and return every reachable `*.rs` file outside
/// [`SKIPPED_DIRS`], plus the number of entries that could not be read.
///
/// Unreadable entries are counted rather than fatal: a single permission error
/// must not make the whole tree unindexable. The caller decides whether to
/// warn — both call sites do.
pub fn collect_project_rust_files(root: &Path) -> (Vec<PathBuf>, usize) {
    let mut rust_files = Vec::new();
    let mut walk_errors = 0;

    let walker = WalkDir::new(root).into_iter().filter_entry(|entry| {
        !(entry.file_type().is_dir() && is_skipped_dir_name(&entry.file_name().to_string_lossy()))
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

    (rust_files, walk_errors)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn skips_build_and_generated_trees() {
        let temp_dir = TempDir::new().expect("temp dir");
        let root = temp_dir.path();
        for dir in ["src", "target/debug", "vendor/foo/src", ".skeleton/src"] {
            fs::create_dir_all(root.join(dir)).expect("create dir");
            fs::write(root.join(dir).join("lib.rs"), "pub fn f() {}\n").expect("write");
        }

        let (files, errors) = collect_project_rust_files(root);

        assert_eq!(errors, 0);
        assert_eq!(files.len(), 1, "only src/lib.rs is a project file: {files:?}");
        assert!(files[0].ends_with("src/lib.rs"));
    }
}

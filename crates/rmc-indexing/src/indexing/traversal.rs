//! Single source of truth for "which `*.rs` files belong to a project".
//!
//! Before this module there were TWO independent walkers with different
//! answers: the indexer skipped build/VCS/generated directories
//! (`unified_parallel::collect_rust_files`), while the Merkle tree hashed
//! literally every `*.rs` under the root (`FileSystemMerkle::from_directory`).
//! On a large workspace that gap was 621 files out of 3067 — a fifth of the tree
//! that the change detector tracked and the indexer never touched.
//!
//! The gap is not just wasted hashing: it makes the two sides
//! incomparable, so "is the index complete?" cannot be answered by
//! putting Merkle's file set next to the indexed one. Coverage checking
//! (see `monitoring::health`) needs both sides to mean the same thing,
//! hence one walker used by both.
//!
//! The same argument governs nested project roots (see [`NESTED_ROOT_MARKER`]):
//! the boundary is drawn INSIDE the shared walker precisely so that a subtree
//! leaves the index, the change detector and the staleness check at once.
//! Drawing it at one call site only would recreate the very gap this module
//! was written to close.

use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Directories never descended into: build output, vendored copies, VCS
/// metadata and generated skeleton trees. Sources under these are either
/// not ours or not authoritative.
pub(crate) const SKIPPED_DIRS: [&str; 6] =
    ["target", "vendor", ".git", ".jj", ".direnv", ".skeleton"];

/// Marker file that declares the directory containing it a project of its own.
///
/// This is a boundary, not an exclusion. The subtree is not dropped from view —
/// it becomes separately indexable, because everything the server stores is
/// already keyed by the directory it was asked about: `index_codebase` on that
/// path builds its own metadata cache, Tantivy index, vector collection and
/// hypergraph, and `clear_cache` on the same path removes exactly those and
/// nothing of the parent's. What was missing was only the other half — the
/// enclosing project walking straight through the boundary and swallowing the
/// subtree into ITS index.
///
/// The declaration lives in the subtree rather than in a list held by the
/// parent so that it travels with the directory: move or copy the subtree and
/// it is still its own project, with nothing to keep in sync. That matters most
/// for the case this exists to serve — a subtree on its way to becoming a
/// separate repository, where the parent's list would be the last thing anyone
/// remembers to update.
///
/// Unlike [`SKIPPED_DIRS`], which encodes what is never source anywhere, this
/// is a per-project statement about a place: vendored forks under `patches/`,
/// a storage engine being extracted, a retired subtree kept for reference.
pub const NESTED_ROOT_MARKER: &str = ".rmcroot";

/// True when a directory with this name must not be descended into.
pub(crate) fn is_skipped_dir_name(name: &str) -> bool {
    SKIPPED_DIRS.contains(&name)
}

/// True when `dir` declares itself a project of its own via
/// [`NESTED_ROOT_MARKER`].
///
/// A plain existence check: an unreadable or otherwise odd marker still counts
/// as present, because the failure that matters here is the opposite one —
/// silently indexing a subtree that asked to be separate.
pub fn is_nested_project_root(dir: &Path) -> bool {
    dir.join(NESTED_ROOT_MARKER).exists()
}

/// Walk `root` and return every reachable `*.rs` file outside
/// [`SKIPPED_DIRS`] and any nested project root (see [`NESTED_ROOT_MARKER`]),
/// plus the number of entries that could not be read.
///
/// Unreadable entries are counted rather than fatal: a single permission
/// error must not make the whole tree unindexable. The caller decides
/// whether to warn — both call sites do.
pub fn collect_project_rust_files(root: &Path) -> (Vec<PathBuf>, usize) {
    let mut rust_files = Vec::new();
    let mut walk_errors = 0;
    // Filled by the closure below; readable once the walker that borrows it is
    // dropped, which is why the report comes after the loop.
    let mut nested_roots: Vec<PathBuf> = Vec::new();

    let walker = WalkDir::new(root).into_iter().filter_entry(|entry| {
        if !entry.file_type().is_dir() {
            return true;
        }
        if is_skipped_dir_name(&entry.file_name().to_string_lossy()) {
            return false;
        }
        // `root` is the project being walked: it may well carry the marker
        // itself (that is exactly how a nested project gets indexed on its
        // own), and honouring it here would make such a project unindexable.
        // The boundary applies to what lies BELOW the root.
        if entry.path() == root {
            return true;
        }
        if is_nested_project_root(entry.path()) {
            nested_roots.push(entry.path().to_path_buf());
            return false;
        }
        true
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

    if !nested_roots.is_empty() {
        // Reported at info, not debug: a boundary is invisible by nature — the
        // files it holds back simply are not in the result to be noticed — so
        // the run that applied it is the one place it can be observed at all.
        nested_roots.sort();
        tracing::info!(
            "{}: {} nested project root(s) left to their own index ({} in each): {}",
            root.display(),
            nested_roots.len(),
            NESTED_ROOT_MARKER,
            nested_roots
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
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
        assert_eq!(
            files.len(),
            1,
            "only src/lib.rs is a project file: {files:?}"
        );
        assert!(files[0].ends_with("src/lib.rs"));
    }

    /// Lay out a small tree and return its root: one source per directory in
    /// `dirs`, each named `lib.rs`.
    fn tree_with(dirs: &[&str]) -> TempDir {
        let temp_dir = TempDir::new().expect("temp dir");
        for dir in dirs {
            let path = temp_dir.path().join(dir);
            fs::create_dir_all(&path).expect("create dir");
            fs::write(path.join("lib.rs"), "pub fn f() {}\n").expect("write");
        }
        temp_dir
    }

    /// Relative `dir/lib.rs` paths of everything the walker returned, sorted.
    fn collected(root: &Path) -> Vec<String> {
        let (files, errors) = collect_project_rust_files(root);
        assert_eq!(errors, 0);
        let mut relative: Vec<String> = files
            .iter()
            .map(|path| {
                path.strip_prefix(root)
                    .expect("walked file is under root")
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect();
        relative.sort();
        relative
    }

    /// Declare `dir` (relative to the tree root) a project of its own.
    fn mark_nested_root(root: &Path, dir: &str) {
        fs::write(root.join(dir).join(NESTED_ROOT_MARKER), "").expect("write marker");
    }

    #[test]
    fn without_a_marker_the_whole_tree_belongs_to_the_root_project() {
        let tree = tree_with(&["src", "patches/eframe"]);

        assert_eq!(
            collected(tree.path()),
            vec!["patches/eframe/lib.rs", "src/lib.rs"]
        );
    }

    #[test]
    fn a_marked_subtree_is_left_out_of_the_parent_walk() {
        let tree = tree_with(&["src", "patches/eframe", "patches/ron"]);
        mark_nested_root(tree.path(), "patches");

        assert_eq!(collected(tree.path()), vec!["src/lib.rs"]);
    }

    /// The other half of the boundary, and the one that makes it a split
    /// rather than an exclusion: walked as a root in its own right, the marked
    /// subtree yields its own files. Both indexes exist; neither contains the
    /// other's sources.
    #[test]
    fn a_marked_subtree_walks_fully_when_it_is_itself_the_root() {
        let tree = tree_with(&["src", "patches/eframe", "patches/ron"]);
        mark_nested_root(tree.path(), "patches");

        assert_eq!(
            collected(&tree.path().join("patches")),
            vec!["eframe/lib.rs", "ron/lib.rs"]
        );
    }

    /// The marker is about a place, not a name: marking `patches` must not
    /// also silence `crates/foo/patches`, which never asked to be separate.
    #[test]
    fn the_boundary_is_the_marked_directory_not_its_name() {
        let tree = tree_with(&["patches/eframe", "crates/foo/patches"]);
        mark_nested_root(tree.path(), "patches");

        assert_eq!(collected(tree.path()), vec!["crates/foo/patches/lib.rs"]);
    }

    #[test]
    fn a_marked_subtree_may_sit_deep_in_the_tree() {
        let tree = tree_with(&["src", "crates/legacy/old", "crates/live"]);
        mark_nested_root(tree.path(), "crates/legacy");

        assert_eq!(
            collected(tree.path()),
            vec!["crates/live/lib.rs", "src/lib.rs"]
        );
    }

    /// Nested roots nest: a project that is itself separated from its parent
    /// still stops at boundaries declared below it.
    #[test]
    fn boundaries_below_a_nested_root_still_apply() {
        let tree = tree_with(&["engine/src", "engine/patches/fork"]);
        mark_nested_root(tree.path(), "engine");
        mark_nested_root(tree.path(), "engine/patches");

        assert_eq!(collected(&tree.path().join("engine")), vec!["src/lib.rs"]);
    }

    /// The marker cannot resurrect what [`SKIPPED_DIRS`] drops, and does not
    /// need to: the two rules are independent, so a marker inside `target` is a
    /// harmless no-op rather than a conflict.
    #[test]
    fn the_marker_and_skipped_dirs_are_independent() {
        let tree = tree_with(&["src", "target/debug"]);
        mark_nested_root(tree.path(), "target");

        assert_eq!(collected(tree.path()), vec!["src/lib.rs"]);
    }
}

//! Guard the declared workspace slice from hashing through publication.
//!
//! Content hashes name reusable snapshots. File identity/change stamps guard
//! one build attempt, including edit-and-revert (ABA), which hashes alone miss.
//! Directory stamps also catch a temporary source appearing during Cargo/RA
//! loading and disappearing before the final scan. Outputs under target/.git
//! are excluded by the same policy as the fingerprint.
use std::{collections::BTreeMap, fs, path::{Path, PathBuf}, time::SystemTime};

use anyhow::{Context, Result};
use walkdir::WalkDir;

use super::storage::{BuildInputs, compute_build_inputs, is_fingerprinted, is_source_walk_excluded};

#[derive(Debug, thiserror::Error)]
#[error("workspace inputs changed during graph build at {0}; no snapshot was published")]
pub(super) struct InputChanged(pub PathBuf);

#[derive(Debug, PartialEq, Eq)]
struct Stamp {
    len: u64,
    modified: SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

fn stamps(root: &Path, graph_output: Option<&Path>) -> Result<BTreeMap<PathBuf, Stamp>> {
    let mut out = BTreeMap::new();
    for entry in WalkDir::new(root).follow_links(false).into_iter()
        .filter_entry(|e| !is_source_walk_excluded(e.path()) && graph_output.is_none_or(|output| !e.path().starts_with(output)))
    {
        let entry = entry.with_context(|| format!("walk graph inputs at {}", root.display()))?;
        let path = entry.path();
        let rel = path.strip_prefix(root).expect("walk entry under root").to_string_lossy().replace('\\', "/");
        if !entry.file_type().is_dir() && !is_fingerprinted(&rel) { continue; }
        let meta = fs::metadata(path).map_err(|_| InputChanged(path.to_owned()))?;
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
        };
        out.insert(path.to_owned(), Stamp {
            len: meta.len(), modified: meta.modified()?,
            #[cfg(unix)]
            identity,
        });
    }
    Ok(out)
}

pub(super) struct InputSlice {
    pub inputs: BuildInputs,
    stamps: BTreeMap<PathBuf, Stamp>,
    graph_output: Option<PathBuf>,
}

impl InputSlice {
    #[cfg(test)]
    fn capture(root: &Path) -> Result<Self> { Self::for_build(root, None) }

    pub fn for_build(root: &Path, graph_output: Option<PathBuf>) -> Result<Self> {
        anyhow::ensure!(graph_output.as_deref() != Some(root), "graph output cannot replace the source root");
        let slice = Self { stamps: stamps(root, graph_output.as_deref())?, inputs: compute_build_inputs(root)?, graph_output };
        slice.validate(root)?;
        Ok(slice)
    }

    pub fn validate(&self, root: &Path) -> Result<()> {
        let current = stamps(root, self.graph_output.as_deref())?;
        if let Some(path) = self.stamps.keys().chain(current.keys())
            .find(|p| self.stamps.get(*p) != current.get(*p))
        {
            return Err(InputChanged(path.clone()).into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guards_edits_reverts_replacements_and_temporary_sources() {
        for change in ["edit", "aba", "replace", "temporary", "manifest", "config"] {
            let root = tempfile::tempdir().unwrap();
            let file = root.path().join("lib.rs");
            fs::write(&file, "pub fn old() {}\n").unwrap();
            fs::create_dir(root.path().join(".cargo")).unwrap();
            let slice = InputSlice::capture(root.path()).unwrap();
            slice.validate(root.path()).unwrap();
            match change {
                "edit" => fs::write(&file, "pub fn new() {}\n").unwrap(),
                "aba" => {
                    fs::write(&file, "pub fn new() {}\n").unwrap();
                    fs::write(&file, "pub fn old() {}\n").unwrap();
                    assert_eq!(slice.inputs, compute_build_inputs(root.path()).unwrap());
                }
                "replace" => { fs::remove_file(&file).unwrap(); fs::write(&file, "pub fn old() {}\n").unwrap(); }
                "temporary" => {
                    fs::write(root.path().join("ghost.rs"), "pub fn ghost() {}\n").unwrap();
                    fs::remove_file(root.path().join("ghost.rs")).unwrap();
                }
                "manifest" => fs::write(root.path().join("Cargo.toml"), "[workspace]\n").unwrap(),
                "config" => fs::write(root.path().join(".cargo/config.toml"), "[build]\n").unwrap(),
                _ => unreachable!(),
            }
            assert!(slice.validate(root.path()).unwrap_err().is::<InputChanged>(), "{change}");
        }
    }

    #[test]
    fn ignores_build_outputs_but_reports_walk_errors() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("target")).unwrap();
        let slice = InputSlice::capture(root.path()).unwrap();
        fs::write(root.path().join("target/out.rs"), "generated").unwrap();
        slice.validate(root.path()).unwrap();
        assert!(InputSlice::capture(&root.path().join("missing")).is_err());
        let output = root.path().join("graphs");
        fs::create_dir(&output).unwrap();
        let slice = InputSlice::for_build(root.path(), Some(output.clone())).unwrap();
        fs::create_dir(output.join("new-snapshot")).unwrap();
        fs::write(output.join("new-snapshot/data.mdb"), "graph bytes").unwrap();
        slice.validate(root.path()).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_source_contents_participate_in_the_fingerprint() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = outside.path().join("original.rs");
        fs::write(&source, "pub fn old() {}\n").unwrap();
        std::os::unix::fs::symlink(&source, root.path().join("linked.rs")).unwrap();
        let first = InputSlice::capture(root.path()).unwrap();
        fs::write(&source, "pub fn new() {}\n").unwrap();
        assert_ne!(first.inputs, compute_build_inputs(root.path()).unwrap());
        assert!(first.validate(root.path()).unwrap_err().is::<InputChanged>());
    }
}

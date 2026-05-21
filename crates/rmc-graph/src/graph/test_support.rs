//! Shared test fixtures for the `graph` module family.
//!
//! Moved from `queries.rs::tests::shared_snapshot` in PR 11 so the four
//! sibling test modules (attributes, signatures, unsafe_audit, statics)
//! can keep importing it after `queries.rs` is reduced to a facade. Tests
//! living inside `queries.rs::tests` also call back here.

#![cfg(test)]

use std::path::Path;
use std::sync::OnceLock;

use super::snapshot::{BuildOptions, OpenedSnapshot, build_and_persist, open_current};
use super::storage::{GraphEnvOptions, GraphPaths};

// Build the snapshot once and share across all tests in this module.
// Saves ~3s/test in release (~25s in debug). The TempDir is held inside
// the static so the heed env stays valid for the process lifetime.
struct SharedSnap {
    _td: tempfile::TempDir,
    snap: OpenedSnapshot,
}

pub(crate) fn shared_snapshot() -> &'static OpenedSnapshot {
    static CACHE: OnceLock<SharedSnap> = OnceLock::new();
    &CACHE
        .get_or_init(|| {
            let td = tempfile::tempdir().unwrap();
            let opts = BuildOptions {
                data_dir_override: Some(td.path().to_path_buf()),
                ..Default::default()
            };
            let result =
                build_and_persist(Path::new(env!("CARGO_MANIFEST_DIR")), opts).unwrap();
            let paths = GraphPaths::for_workspace_in(td.path(), &result.workspace_root);
            let snap = open_current(&paths, GraphEnvOptions::default())
                .unwrap()
                .unwrap();
            SharedSnap { _td: td, snap }
        })
        .snap
}

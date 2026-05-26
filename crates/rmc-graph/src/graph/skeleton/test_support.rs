//! Synthetic fixtures for skeleton collect/render tests.
//!
//! These tests need a real persisted graph, but they should not load the
//! `rmc-graph` crate itself. This module materializes a tiny standalone Cargo
//! package in a tempdir and runs `build_and_persist` against that root.

#![cfg(test)]

use std::sync::OnceLock;

use crate::graph::snapshot::{BuildOptions, OpenedSnapshot, build_and_persist, open_current};
use crate::graph::storage::{GraphEnvOptions, GraphPaths};

pub(super) const FALLBACK_FUNCTION: &str = "synthetic_skeleton_crate::public_function";
pub(super) const FALLBACK_STATIC: &str = "synthetic_skeleton_crate::GLOBAL_COUNT";

struct SharedSnap {
    _workspace_td: tempfile::TempDir,
    _data_td: tempfile::TempDir,
    snap: OpenedSnapshot,
}

const FIXTURE_CARGO_TOML: &str = r#"
[package]
name = "synthetic_skeleton_crate"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[workspace]
"#;

const FIXTURE_LIB_RS: &str = r#"
#![allow(dead_code)]

/// Adds the private helper result to the input.
#[inline]
pub fn public_function(input: usize) -> usize {
    input + private_helper()
}

fn private_helper() -> usize {
    1
}

pub(crate) fn crate_visible_function() -> usize {
    public_function(1)
}

pub const ANSWER: usize = 40 + 2;

/// Static used by missing-source fallback tests.
#[allow(non_upper_case_globals)]
pub(crate) static mut GLOBAL_COUNT: usize = 0;

/// Host for inherent impl facade rendering.
#[derive(Debug, Clone)]
pub struct Host {
    value: usize,
}

impl Host {
    pub fn new(value: usize) -> Self {
        Self { value }
    }

    pub(crate) fn value(&self) -> usize {
        self.value
    }

    fn hidden(&self) -> usize {
        self.value + 1
    }

    pub const LIMIT: usize = ANSWER;
}

pub trait Behavior {
    type Output;
    const DEFAULT: usize = 10;

    fn required(&self) -> Self::Output;

    fn provided(&self) -> usize {
        Self::DEFAULT
    }
}

impl Behavior for Host {
    type Output = usize;
    const DEFAULT: usize = 11;

    fn required(&self) -> Self::Output {
        self.value()
    }
}

pub mod nested;

#[cfg(test)]
pub fn cfg_test_helper() {
    assert_eq!(public_function(1), 2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exercises_test_attr() {
        assert_eq!(public_function(1), 2);
    }
}
"#;

const FIXTURE_NESTED_RS: &str = r#"
#![allow(dead_code)]

/// Nested type with an outer attribute.
#[repr(transparent)]
pub struct Nested(pub usize);

impl Nested {
    pub fn doubled(&self) -> usize {
        self.0 * 2
    }
}

pub mod deeper {
    /// A nested function with a body.
    pub fn visible_nested() -> &'static str {
        "nested"
    }

    #[cfg(test)]
    pub fn cfg_test_nested() {
        assert_eq!(visible_nested(), "nested");
    }
}
"#;

pub(super) fn shared_snapshot() -> &'static OpenedSnapshot {
    static CACHE: OnceLock<SharedSnap> = OnceLock::new();
    &CACHE
        .get_or_init(|| {
            let workspace_td = tempfile::tempdir().expect("create workspace tempdir");
            let workspace_path = workspace_td.path();
            std::fs::write(
                workspace_path.join("Cargo.toml"),
                FIXTURE_CARGO_TOML.trim_start(),
            )
            .expect("write Cargo.toml");

            let src_dir = workspace_path.join("src");
            std::fs::create_dir_all(&src_dir).expect("create src dir");
            std::fs::write(src_dir.join("lib.rs"), FIXTURE_LIB_RS.trim_start())
                .expect("write lib.rs");
            std::fs::write(src_dir.join("nested.rs"), FIXTURE_NESTED_RS.trim_start())
                .expect("write nested.rs");

            let data_td = tempfile::tempdir().expect("create data tempdir");
            let opts = BuildOptions {
                data_dir_override: Some(data_td.path().to_path_buf()),
                ..Default::default()
            };
            let result = build_and_persist(workspace_path, opts)
                .expect("build_and_persist on synthetic skeleton fixture");
            let paths = GraphPaths::for_workspace_in(data_td.path(), &result.workspace_root);
            let snap = open_current(&paths, GraphEnvOptions::default())
                .expect("open_current succeeds")
                .expect("snapshot exists after build_and_persist");

            SharedSnap {
                _workspace_td: workspace_td,
                _data_td: data_td,
                snap,
            }
        })
        .snap
}

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
            // These cross-crate query/signature fixtures intentionally use
            // rmc-server and rmc-engine too. Declare the workspace root rather
            // than relying on a member request silently including siblings.
            let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().and_then(Path::parent)
                .expect("rmc-graph is under crates/ in the workspace");
            let result = build_and_persist(root, opts).unwrap();
            let paths = GraphPaths::for_workspace_in(td.path(), &result.workspace_root);
            let snap = open_current(&paths, GraphEnvOptions::default())
                .unwrap()
                .unwrap();
            SharedSnap { _td: td, snap }
        })
        .snap
}

/// Write a two-crate workspace into `root`: `alpha` (a library with a module,
/// a struct with inherent methods and an associated const, a trait and its
/// impl, an enum, a `static`) and `beta`, which depends on `alpha` by path and
/// uses all of it from inside functions. Both import `std`'s `HashMap`, only
/// `beta` imports `BTreeSet` and `mem::swap`: external stubs with two owning
/// crates and with one, and a stub that carries a signature.
///
/// Small enough to load in seconds, and shaped so that every snapshot table
/// gets rows — cross-crate usages, usages inside function bodies, use-bindings,
/// signatures, static metadata, stub owners. No registry dependencies, so it
/// loads offline (`std` comes from the sysroot).
pub(crate) fn write_two_crate_fixture(root: &Path) {
    const FILES: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            r#"[workspace]
members = ["alpha", "beta"]
resolver = "2"
"#,
        ),
        (
            "alpha/Cargo.toml",
            r#"[package]
name = "alpha"
version = "0.1.0"
edition = "2021"
"#,
        ),
        (
            "alpha/src/lib.rs",
            r#"pub mod shapes;

use std::collections::HashMap;

pub static ORIGIN_NAME: &str = "origin";

pub fn tally(xs: &[i32]) -> HashMap<i32, usize> {
    let mut out = HashMap::new();
    for &x in xs {
        *out.entry(x).or_insert(0) += 1;
    }
    out
}

pub trait Area {
    fn area(&self) -> f64;
}

pub fn double(x: i32) -> i32 {
    x * 2
}

pub fn quadruple(x: i32) -> i32 {
    double(double(x))
}
"#,
        ),
        (
            "alpha/src/shapes.rs",
            r#"use crate::Area;

pub struct Square {
    pub side: f64,
}

impl Square {
    pub const SIDES: u32 = 4;

    pub fn new(side: f64) -> Self {
        Square { side }
    }
}

impl Area for Square {
    fn area(&self) -> f64 {
        self.side * self.side
    }
}

pub enum Kind {
    Square,
    Round,
}
"#,
        ),
        (
            "beta/Cargo.toml",
            r#"[package]
name = "beta"
version = "0.1.0"
edition = "2021"

[dependencies]
alpha = { path = "../alpha" }
"#,
        ),
        (
            "beta/src/lib.rs",
            r#"use alpha::shapes::{Kind, Square};
use alpha::Area;
use std::collections::{BTreeSet, HashMap};
use std::mem::swap;

pub fn distinct(xs: &[i32]) -> BTreeSet<i32> {
    xs.iter().copied().collect()
}

pub fn ordered(mut a: i32, mut b: i32) -> (i32, i32) {
    if a > b {
        swap(&mut a, &mut b);
    }
    (a, b)
}

pub fn tallied(xs: &[i32]) -> HashMap<i32, usize> {
    alpha::tally(xs)
}

pub fn total(sides: &[f64]) -> f64 {
    sides.iter().map(|&s| Square::new(s).area()).sum()
}

pub fn kind_of(n: u32) -> Kind {
    if n == Square::SIDES { Kind::Square } else { Kind::Round }
}

pub fn twice(x: i32) -> i32 {
    alpha::double(x) + alpha::ORIGIN_NAME.len() as i32
}
"#,
        ),
    ];
    for (path, text) in FILES {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

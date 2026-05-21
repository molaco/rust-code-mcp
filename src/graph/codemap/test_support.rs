//! Shared test fixtures for the `codemap` family.
//!
//! Two fixtures are used across the family's `#[cfg(test)] mod tests` blocks:
//!
//! - [`hand_built_codemap`] builds a tiny in-memory `Codemap` literal (two
//!   functions in a single module + a `Calls` edge). Used by the render
//!   tests in `render.rs` which exercise `render_mermaid` / `render_outline`
//!   against a known-shape input.
//! - [`shared_fixture`] builds a one-shot `OpenedSnapshot` over a synthetic
//!   workspace via `build_and_persist` + `open_current`. Cached in a
//!   `OnceLock` so the build cost is paid at most once per test binary.
//!   Used by the fixture-dependent tests in `build.rs`, `render.rs`, and
//!   `seeds.rs`.

use std::sync::OnceLock;

use crate::graph::codemap::model::{Codemap, CodemapEdge, CodemapNode, CodemapStats, EdgeKind};
use crate::graph::ids::NodeId;
use crate::graph::model::{ItemKind, NodeKind};
use crate::graph::ModuleTreeNode;
use crate::graph::snapshot::OpenedSnapshot;

/// Build a hex-filled NodeId from a single byte, e.g. `nid(0xAA)`
/// → `NodeId([0xAA; 32])`. Sufficient for renderer tests where we
/// just need stable, distinguishable IDs.
fn nid(byte: u8) -> NodeId {
    NodeId([byte; 32])
}

#[track_caller]
fn make_node(
    id: NodeId,
    qualified_name: &str,
    kind: NodeKind,
    item_kind: Option<ItemKind>,
    is_seed: bool,
) -> CodemapNode {
    CodemapNode {
        id,
        qualified_name: qualified_name.to_string(),
        kind,
        item_kind,
        file: Some("src/lib.rs".to_string()),
        span: Some((0, 16)),
        line: Some(1),
        relevance: if is_seed { 1.0 } else { 0.2 },
        is_seed,
        snippet: None,
    }
}

/// Build a tiny hand-rolled `Codemap` with two functions in the
/// same module and a single `Calls` edge between them. Used by
/// the renderer smoke tests.
pub(super) fn hand_built_codemap() -> Codemap {
    let caller_id = nid(0xAA);
    let callee_id = nid(0xBB);
    Codemap {
        prompt: String::new(),
        snapshot_id: "test_snapshot".to_string(),
        generated_at_unix: 0,
        seeds: vec![caller_id],
        nodes: vec![
            make_node(
                caller_id,
                "demo_crate::caller",
                NodeKind::Item,
                Some(ItemKind::Function),
                true,
            ),
            make_node(
                callee_id,
                "demo_crate::callee",
                NodeKind::Item,
                Some(ItemKind::Function),
                false,
            ),
        ],
        edges: vec![CodemapEdge {
            from: caller_id,
            to: callee_id,
            kind: EdgeKind::Calls,
            weight: 1,
        }],
        hierarchy: ModuleTreeNode {
            qualified_name: "demo_crate".to_string(),
            display_name: "demo_crate".to_string(),
            kind: "Crate".to_string(),
            item_kind: None,
            visibility: None,
            children: vec![
                ModuleTreeNode {
                    qualified_name: "demo_crate::callee".to_string(),
                    display_name: "callee".to_string(),
                    kind: "Item".to_string(),
                    item_kind: Some("Function".to_string()),
                    visibility: None,
                    children: vec![],
                },
                ModuleTreeNode {
                    qualified_name: "demo_crate::caller".to_string(),
                    display_name: "caller".to_string(),
                    kind: "Item".to_string(),
                    item_kind: Some("Function".to_string()),
                    visibility: None,
                    children: vec![],
                },
            ],
        },
        stats: CodemapStats {
            seed_count: 1,
            node_count: 2,
            edge_count: 1,
            embedded_nodes: 0,
            embeddings_computed: 0,
            total_ms: 0,
        },
        diagnostics: vec![],
    }
}

pub(super) struct FixtureSnap {
    pub _workspace_td: tempfile::TempDir,
    pub _data_td: tempfile::TempDir,
    pub snap: OpenedSnapshot,
}

// The empty `[workspace]` table makes this manifest a self-contained
// workspace root. Without it, `cargo metadata` walks up the directory
// tree looking for an enclosing `[workspace]` and can latch onto an
// unrelated `Cargo.toml` (e.g. a stray `/tmp/Cargo.toml`), causing the
// RA load to fail with "no targets specified in the manifest".
const FIXTURE_CARGO_TOML: &str = r#"
[package]
name = "synthetic_codemap_crate"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[workspace]
"#;

// Notably outer() and inner() both exist; the line-range lookup for
// inner()'s body should resolve to inner, not outer. The top-level
// caller()/callee() pair gives the raw-ID adapter tests a clean
// pair of qualified names to look up.
const FIXTURE_LIB_RS: &str = r#"
pub fn outer() {
    fn inner() {
        let _x = 1;
    }
    inner();
}

pub fn other() {
    let _y = 2;
}

pub fn callee() {}

pub fn caller() {
    callee();
}
"#;

/// Build a minimal one-shot `OpenedSnapshot` over a synthetic workspace
/// so we can exercise `line_to_byte` and `enclosing_item_for_line_range`
/// against a real snapshot handle. The fixture is cached across tests
/// in this module via a `OnceLock`.
pub(super) fn shared_fixture() -> &'static FixtureSnap {
    use crate::graph::snapshot::{BuildOptions, build_and_persist, open_current};
    use crate::graph::storage::{GraphEnvOptions, GraphPaths};

    static CACHE: OnceLock<FixtureSnap> = OnceLock::new();
    CACHE.get_or_init(|| {
        let workspace_td = tempfile::tempdir().expect("create workspace tempdir");
        let workspace_path = workspace_td.path();
        std::fs::write(
            workspace_path.join("Cargo.toml"),
            FIXTURE_CARGO_TOML.trim_start(),
        )
        .expect("write Cargo.toml");
        std::fs::create_dir_all(workspace_path.join("src")).expect("create src dir");
        std::fs::write(
            workspace_path.join("src").join("lib.rs"),
            FIXTURE_LIB_RS.trim_start(),
        )
        .expect("write lib.rs");

        let data_td = tempfile::tempdir().expect("create data tempdir");
        let opts = BuildOptions {
            data_dir_override: Some(data_td.path().to_path_buf()),
            ..Default::default()
        };
        let result = build_and_persist(workspace_path, opts)
            .expect("build_and_persist on synthetic fixture");

        let paths = GraphPaths::for_workspace_in(data_td.path(), &result.workspace_root);
        let snap = open_current(&paths, GraphEnvOptions::default())
            .expect("open_current succeeds")
            .expect("snapshot exists after build_and_persist");

        FixtureSnap {
            _workspace_td: workspace_td,
            _data_td: data_td,
            snap,
        }
    })
}

//! Task-conditioned codemap response types and query-time helpers.
//!
//! The serializable shape returned by the `build_codemap` MCP tool lives
//! at the top of this file. Below the types are query-time helpers used
//! by the algorithm (Phase 5): a span-resolution helper that turns a
//! workspace-relative file + line range into an enclosing Item NodeId,
//! and a small path-normalization helper.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::graph::ids::NodeId;
use crate::graph::model::{ItemKind, NodeKind};
use crate::graph::queries::ModuleTreeNode;
use crate::graph::snapshot::OpenedSnapshot;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Codemap {
    pub prompt: String,
    pub snapshot_id: String,
    pub generated_at_unix: u64,
    pub seeds: Vec<NodeId>,
    pub nodes: Vec<CodemapNode>,
    pub edges: Vec<CodemapEdge>,
    pub hierarchy: ModuleTreeNode,
    pub stats: CodemapStats,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapNode {
    pub id: NodeId,
    pub qualified_name: String,
    pub kind: NodeKind,
    pub item_kind: Option<ItemKind>,
    pub file: Option<String>,
    pub span: Option<(u32, u32)>,
    pub relevance: f32,
    pub is_seed: bool,
    pub snippet: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapEdge {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: EdgeKind,
    pub weight: u32,
}

/// Edge kind. Marked `#[non_exhaustive]` so future variants
/// (`Implements`, `Inherits`, …) are not semver-breaking — `EdgeKind`
/// is part of the MCP tool's serialized JSON output.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EdgeKind {
    Calls,
    Uses,
    Imports,
    Contains,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodemapStats {
    pub seed_count: usize,
    pub node_count: usize,
    pub edge_count: usize,
    pub embedded_nodes: usize,
    pub embeddings_computed: usize,
    pub total_ms: u64,
}

/// Caller-tunable knobs. The MCP tool layer translates JSON params into this.
#[derive(Debug, Clone)]
pub struct CodemapOptions {
    pub max_nodes: usize,
    pub depth: u8,
    pub top_k_seeds: usize,
    pub max_incoming_per_node: usize,
    pub embedding_policy: EmbeddingPolicy,
    pub include_snippets: bool,
}

impl Default for CodemapOptions {
    fn default() -> Self {
        Self {
            max_nodes: 80,
            depth: 3,
            top_k_seeds: 20,
            max_incoming_per_node: 8,
            embedding_policy: EmbeddingPolicy::NoRerank,
            include_snippets: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbeddingPolicy {
    NoRerank,
    UseCachedOnly,
    ComputeMissing,
}

/// Convert a 1-indexed inclusive line range into a byte range for `file`,
/// then find the smallest enclosing Item NodeId from the span index.
///
/// Returns `None` if (a) the file isn't in the snapshot, (b) the file
/// can't be read from disk, (c) the line range is out of range, or
/// (d) no Item span covers the byte range.
///
/// Line convention: 1-indexed, inclusive (per src/parser/mod.rs:100,205).
/// Conversion:
///   byte_start = line_to_byte[line_start - 1]
///   byte_end   = if line_end < line_count { line_to_byte[line_end] - 1 } else { last-line offset }
/// — the byte just before the next line's '\n'.
pub(crate) fn enclosing_item_for_line_range(
    snap: &OpenedSnapshot,
    workspace_relative_file: &str,
    line_start: u32,
    line_end: u32,
) -> Option<NodeId> {
    if line_start == 0 || line_end < line_start {
        return None;
    }
    let table = snap.line_to_byte(workspace_relative_file).ok()?;
    let line_count = table.len() as u32;
    if line_start > line_count {
        return None;
    }
    let byte_start = table[(line_start - 1) as usize];
    let byte_end = if line_end < line_count {
        table[line_end as usize].saturating_sub(1)
    } else {
        // EOF case: use the start-of-last-line offset. For "smallest
        // enclosing item" purposes a point overlap inside the last line
        // is sufficient.
        table[(line_count - 1) as usize]
    };
    let spans = snap.span_index().get(workspace_relative_file)?;

    // Linear scan from the front, breaking when start > byte_end. The
    // vec is sorted by start, so once we pass byte_end no further span
    // can begin before our range ends. Within the candidates we pick
    // the smallest (narrowest) that fully contains [byte_start, byte_end].
    let mut best: Option<(u32, u32, NodeId)> = None;
    for &(s, e, nid) in spans.iter() {
        if s > byte_end {
            break;
        }
        if s <= byte_start && e >= byte_end {
            match best {
                None => best = Some((s, e, nid)),
                Some((bs, be, _)) if (e - s) < (be - bs) => best = Some((s, e, nid)),
                _ => {}
            }
        }
    }
    best.map(|(_, _, nid)| nid)
}

/// Workspace-relative path normalization for query-time use.
///
/// The build-time `resolve_workspace_relative` in src/graph/usages.rs takes
/// `(&Vfs, FileId, &Path)`; we have no VFS at query time, so this one
/// operates on disk paths. Canonicalizes `path`, strips the canonicalized
/// `workspace_root` prefix, returns the relative path as a `String`
/// matching the format of `Node.file`.
pub(crate) fn canonicalize_and_strip(path: &Path, workspace_root: &Path) -> Option<String> {
    let abs = std::fs::canonicalize(path).ok()?;
    let ws = std::fs::canonicalize(workspace_root).ok()?;
    abs.strip_prefix(&ws)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Build a minimal one-shot `OpenedSnapshot` over a synthetic workspace
    /// so we can exercise `line_to_byte` and `enclosing_item_for_line_range`
    /// against a real snapshot handle. The fixture is cached across tests
    /// in this module via a `OnceLock`.
    fn shared_fixture() -> &'static FixtureSnap {
        use crate::graph::snapshot::{BuildOptions, build_and_persist, open_current};
        use crate::graph::storage::{GraphEnvOptions, GraphPaths};
        use std::sync::OnceLock;

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

    struct FixtureSnap {
        _workspace_td: tempfile::TempDir,
        _data_td: tempfile::TempDir,
        snap: OpenedSnapshot,
    }

    const FIXTURE_CARGO_TOML: &str = r#"
[package]
name = "synthetic_codemap_crate"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#;

    // Notably outer() and inner() both exist; the line-range lookup for
    // inner()'s body should resolve to inner, not outer.
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
"#;

    #[test]
    fn line_to_byte_correct_for_lf_file() {
        // We don't need a real snapshot for this test — just need the
        // line_to_byte function to read a real on-disk file. Build a
        // fixture so the snapshot's workspace_root is set, then write a
        // small file under it and read it back.
        let fixture = shared_fixture();
        let ws_root = PathBuf::from(&fixture.snap.manifest.workspace_root);

        // Write a small file at a known workspace-relative path.
        // Content: "a\nbb\nccc\nd\n" — byte offsets per line:
        //   line 1 ("a")   starts at 0
        //   line 2 ("bb")  starts at 2   (after "a\n")
        //   line 3 ("ccc") starts at 5   (after "a\nbb\n")
        //   line 4 ("d")   starts at 9   (after "a\nbb\nccc\n")
        //   trailing \n at byte 10 makes a line 5 starting at 11
        let rel = "src/_line_to_byte_test.rs";
        let abs = ws_root.join(rel);
        std::fs::write(&abs, b"a\nbb\nccc\nd\n").expect("write test file");

        let table = fixture
            .snap
            .line_to_byte(rel)
            .expect("line_to_byte returns offsets");
        assert_eq!(&*table, &[0u32, 2, 5, 9, 11]);

        // Second call should hit the cache and return the same Arc.
        let table2 = fixture
            .snap
            .line_to_byte(rel)
            .expect("line_to_byte returns cached offsets");
        assert!(std::sync::Arc::ptr_eq(&table, &table2));

        let _ = std::fs::remove_file(&abs);
    }

    #[test]
    fn enclosing_item_returns_none_for_unknown_file() {
        let fixture = shared_fixture();
        let got = enclosing_item_for_line_range(
            &fixture.snap,
            "does/not/exist.rs",
            1,
            1,
        );
        assert!(got.is_none(), "unknown file should yield None");
    }

    #[test]
    fn enclosing_item_returns_none_for_invalid_range() {
        let fixture = shared_fixture();
        let got = enclosing_item_for_line_range(
            &fixture.snap,
            "src/lib.rs",
            0,
            0,
        );
        assert!(got.is_none(), "line_start = 0 is invalid (1-indexed)");

        let got2 = enclosing_item_for_line_range(
            &fixture.snap,
            "src/lib.rs",
            5,
            2,
        );
        assert!(got2.is_none(), "end before start is invalid");
    }

    #[test]
    fn canonicalize_and_strip_normalizes() {
        let td = tempfile::tempdir().expect("tempdir");
        let nested = td.path().join("a");
        std::fs::create_dir_all(&nested).expect("create a/");
        let file = nested.join("b.rs");
        std::fs::write(&file, b"// hi").expect("write b.rs");

        let rel = canonicalize_and_strip(&file, td.path())
            .expect("canonicalize_and_strip succeeds");
        // On macOS canonicalize may add a /private prefix; we strip the
        // canonicalized workspace root from the canonicalized file path,
        // so the relative result should still be "a/b.rs" regardless.
        let expected = PathBuf::from("a").join("b.rs");
        assert_eq!(rel, expected.to_string_lossy());
    }
}

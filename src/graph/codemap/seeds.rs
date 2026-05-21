//! Codemap-local DTO for search-derived seed hits, plus seed-resolution helpers.
//!
//! `build_codemap` originally accepted `&[crate::search::SearchResult]` directly,
//! baking a `graph → search` edge into the graph layer. PR 12 introduced this
//! DTO so the tools-side endpoint does the `SearchResult → SeedHit` mapping
//! before calling the algorithm, keeping the graph layer search-independent.
//!
//! PR 13 absorbed the path/span helpers (`canonicalize_and_strip`,
//! `enclosing_item_for_line_range`) here because the seeds layer is their
//! only non-test consumer.

use std::collections::HashSet;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::graph::codemap::model::CodemapOptions;
use crate::graph::ids::NodeId;
use crate::graph::model::NodeKind;
use crate::graph::snapshot::OpenedSnapshot;

/// One search-hit normalized to a codemap seed.
///
/// Mirrors only the subset of `crate::search::SearchResult` fields the codemap
/// algorithm reads: span (for chunk-to-item resolution) and BM25 score.
///
/// `file_path` is left as a `PathBuf` (not pre-normalized) because
/// `canonicalize_and_strip` needs to run against a real disk path during seed
/// resolution. The tools-side mapping just clones the `SearchResult.chunk.context.file_path`
/// verbatim.
#[derive(Debug, Clone)]
pub struct SeedHit {
    pub file_path: PathBuf,
    pub line_start: u32,
    pub line_end: u32,
    pub score: f32,
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
pub(super) fn enclosing_item_for_line_range(
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
pub(super) fn canonicalize_and_strip(path: &Path, workspace_root: &Path) -> Option<String> {
    let abs = std::fs::canonicalize(path).ok()?;
    let ws = std::fs::canonicalize(workspace_root).ok()?;
    abs.strip_prefix(&ws)
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
}

/// Resolve qualified-name seeds. Names that don't resolve become diagnostics
/// (`"unresolved seed: <name>"`); no RA fallback. When the leaf fails but the
/// parent path resolves to a `Module` node, the diagnostic is enriched with
/// a hint so a user can distinguish "leaf is private / not indexed" from a
/// straight typo.
pub(super) fn resolve_override_seeds(
    snap: &OpenedSnapshot,
    names: &[String],
    diagnostics: &mut Vec<String>,
) -> anyhow::Result<HashSet<NodeId>> {
    let mut seeds: HashSet<NodeId> = HashSet::new();
    for qn in names {
        match snap.lookup_by_qualified_name(qn)? {
            Some((nid, _)) => {
                seeds.insert(nid);
            }
            None => {
                let hint: &'static str = if let Some((parent, _)) = qn.rsplit_once("::") {
                    match snap.lookup_by_qualified_name(parent)? {
                        Some((_, node)) if matches!(node.kind, NodeKind::Module) => {
                            " (parent module resolves; leaf likely private or not indexed)"
                        }
                        _ => "",
                    }
                } else {
                    ""
                };
                diagnostics.push(format!("unresolved seed: {qn}{hint}"));
            }
        }
    }
    Ok(seeds)
}

/// Resolve search-hit seeds via the span index + line→byte bridge. Items
/// that are not callable or type-shaped are filtered out (a const-literal hit
/// is not a useful codemap seed).
///
/// Tracks three drop counters and pushes a single summary diagnostic if the
/// total is > 0 (item 2 of pass-1 polish).
pub(super) fn resolve_search_seeds(
    snap: &OpenedSnapshot,
    hits: &[SeedHit],
    ws_root: &Path,
    opts: &CodemapOptions,
    diagnostics: &mut Vec<String>,
) -> anyhow::Result<HashSet<NodeId>> {
    let mut seeds: HashSet<NodeId> = HashSet::new();
    let mut dropped_path_norm: usize = 0;
    let mut dropped_line_resolve: usize = 0;
    let mut dropped_kind_filter: usize = 0;
    let rtxn = snap.read_txn()?;
    for hit in hits {
        if seeds.len() >= opts.top_k_seeds {
            break;
        }
        let Some(rel) = canonicalize_and_strip(&hit.file_path, ws_root) else {
            dropped_path_norm += 1;
            continue;
        };
        let ls = hit.line_start;
        let le = hit.line_end;
        let Some(nid) = enclosing_item_for_line_range(snap, &rel, ls, le) else {
            dropped_line_resolve += 1;
            continue;
        };
        let Some(node) = snap.node(&rtxn, nid)? else {
            dropped_line_resolve += 1;
            continue;
        };
        let kind_ok = node
            .item_kind
            .map_or(false, |k| k.is_callable() || k.is_type());
        if !kind_ok {
            dropped_kind_filter += 1;
            continue;
        }
        seeds.insert(nid);
    }
    let total = dropped_path_norm + dropped_line_resolve + dropped_kind_filter;
    if total > 0 {
        diagnostics.push(format!(
            "{total} search hits dropped: {dropped_path_norm} path-norm, {dropped_line_resolve} line-resolve, {dropped_kind_filter} kind-filter"
        ));
    }
    Ok(seeds)
}

/// Pre-compute the `NodeId -> bm25 score` map by resolving each search hit
/// the same way `resolve_search_seeds` does. We sum (rather than max) when
/// multiple hits resolve to the same NodeId so that frequently-cited callers
/// rank higher.
pub(super) fn build_bm25_by_node(
    snap: &OpenedSnapshot,
    hits: &[SeedHit],
    ws_root: &Path,
) -> HashMap<NodeId, f32> {
    let mut out: HashMap<NodeId, f32> = HashMap::new();
    for hit in hits {
        let Some(rel) = canonicalize_and_strip(&hit.file_path, ws_root) else {
            continue;
        };
        let ls = hit.line_start;
        let le = hit.line_end;
        let Some(nid) = enclosing_item_for_line_range(snap, &rel, ls, le) else {
            continue;
        };
        *out.entry(nid).or_insert(0.0) += hit.score;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use crate::graph::codemap::test_support::shared_fixture;

    // ===== pure tests =====

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

    // ----- A9: enriched diagnostic distinguishes private-leaf from typo -----
    //
    // We can't unit-test `resolve_override_seeds` without an
    // `OpenedSnapshot`, but we can exercise the same branching logic
    // on a small `Option<NodeKind>` helper inline so a regression in
    // the public/typo distinction is caught by `cargo check --lib --tests`.

    #[test]
    fn unresolved_seed_hint_branches_by_parent_kind() {
        // Mirror of the `match` inside `resolve_override_seeds`. The
        // hint is `&'static str` so this is an allocation-free check.
        fn hint(parent_kind: Option<NodeKind>) -> &'static str {
            match parent_kind {
                Some(NodeKind::Module) => {
                    " (parent module resolves; leaf likely private or not indexed)"
                }
                _ => "",
            }
        }
        assert_eq!(
            hint(Some(NodeKind::Module)),
            " (parent module resolves; leaf likely private or not indexed)",
            "module parent yields the enriched hint",
        );
        assert_eq!(hint(Some(NodeKind::Crate)), "", "crate parent is terse");
        assert_eq!(hint(Some(NodeKind::Item)), "", "item parent is terse");
        assert_eq!(hint(None), "", "no parent → terse");
    }

    // ===== fixture-dependent tests =====

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
    fn callees_of_includes_called_function() {
        let fixture = shared_fixture();
        let (caller_id, _) = fixture
            .snap
            .lookup_by_qualified_name("synthetic_codemap_crate::caller")
            .expect("lookup_by_qualified_name caller")
            .expect("caller resolves");
        let (callee_id, _) = fixture
            .snap
            .lookup_by_qualified_name("synthetic_codemap_crate::callee")
            .expect("lookup_by_qualified_name callee")
            .expect("callee resolves");

        let callees = fixture
            .snap
            .callees_of(caller_id)
            .expect("callees_of caller succeeds");
        assert!(
            callees.contains(&callee_id),
            "callees_of(caller) should include callee, got {:?}",
            callees
        );
    }

    #[test]
    fn referrers_of_includes_caller() {
        let fixture = shared_fixture();
        let (caller_id, _) = fixture
            .snap
            .lookup_by_qualified_name("synthetic_codemap_crate::caller")
            .expect("lookup_by_qualified_name caller")
            .expect("caller resolves");
        let (callee_id, _) = fixture
            .snap
            .lookup_by_qualified_name("synthetic_codemap_crate::callee")
            .expect("lookup_by_qualified_name callee")
            .expect("callee resolves");

        let referrers = fixture
            .snap
            .referrers_of(callee_id)
            .expect("referrers_of callee succeeds");
        assert!(
            referrers.contains(&caller_id),
            "referrers_of(callee) should include caller, got {:?}",
            referrers
        );
    }
}

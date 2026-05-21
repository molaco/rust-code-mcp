//! Codemap-local DTO for search-derived seed hits, plus seed-resolution helpers.
//!
//! `build_codemap` originally accepted `&[crate::search::SearchResult]` directly,
//! baking a `graph → search` edge into the graph layer. PR 12 introduced this
//! DTO so the tools-side endpoint does the `SearchResult → SeedHit` mapping
//! before calling the algorithm, keeping the graph layer search-independent.

use std::collections::HashSet;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::graph::codemap::model::CodemapOptions;
use crate::graph::codemap::{canonicalize_and_strip, enclosing_item_for_line_range};
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

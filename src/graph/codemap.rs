//! Task-conditioned codemap response types and query-time helpers.
//!
//! The serializable shape returned by the `build_codemap` MCP tool lives
//! at the top of this file. Below the types are query-time helpers used
//! by the algorithm (Phase 5): a span-resolution helper that turns a
//! workspace-relative file + line range into an enclosing Item NodeId,
//! and a small path-normalization helper.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use heed::RoTxn;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

/// Bounded BFS over outgoing `callees_of` edges from `node`. Returns the
/// shortest distance to any `NodeId` in `seeds`, or `u32::MAX` if none is
/// reachable within `max_depth`. `0` means `node` itself is a seed.
///
/// Mirrors the frontier+visited pattern of `recursive_callers_count`
/// (src/graph/queries.rs:852+), but in the forward direction (callees,
/// not callers) and returning depth rather than count.
pub(crate) fn min_call_distance(
    snap: &OpenedSnapshot,
    node: NodeId,
    seeds: &HashSet<NodeId>,
    max_depth: u32,
) -> u32 {
    if seeds.contains(&node) {
        return 0;
    }
    let mut visited: HashSet<NodeId> = HashSet::new();
    visited.insert(node);
    let mut frontier: VecDeque<(NodeId, u32)> = VecDeque::new();
    frontier.push_back((node, 0));
    while let Some((cur, d)) = frontier.pop_front() {
        if d >= max_depth {
            continue;
        }
        let callees = match snap.callees_of(cur) {
            Ok(v) => v,
            Err(_) => continue,
        };
        for next in callees {
            if !visited.insert(next) {
                continue;
            }
            if seeds.contains(&next) {
                return d + 1;
            }
            frontier.push_back((next, d + 1));
        }
    }
    u32::MAX
}

// ---------------------------------------------------------------------------
// Phase 5 — `build_codemap` algorithm core.
// ---------------------------------------------------------------------------

/// Algorithm-core entry point for the codemap MCP tool.
///
/// Phase 6 will wire `HybridSearch::search` + parameter parsing into this
/// function from `src/tools/`; keeping `build_codemap` synchronous w.r.t.
/// search makes the algorithm easier to unit-test against an in-memory
/// snapshot.
///
/// Inputs:
/// - `prompt`: Some when the caller wants embedding-aware scoring; may be
///   None when `override_seeds` is supplied.
/// - `override_seeds`: qualified names resolved via
///   `OpenedSnapshot::lookup_by_qualified_name`. Names that fail to resolve
///   are surfaced in `Codemap.diagnostics` rather than erroring out.
/// - `hits`: search-result list when seeds come from `HybridSearch::search`.
///   The caller is expected to have already capped to `top_k_seeds * 3`;
///   we defensively truncate to the same bound.
pub(crate) async fn build_codemap(
    snap: &OpenedSnapshot,
    prompt: Option<&str>,
    override_seeds: Option<&[String]>,
    hits: Option<&[crate::search::SearchResult]>,
    opts: &CodemapOptions,
) -> anyhow::Result<Codemap> {
    let started = SystemTime::now();
    let mut diagnostics: Vec<String> = Vec::new();

    // ---------- 1. SEEDS ----------
    let ws_root = PathBuf::from(&snap.manifest.workspace_root);

    // Build a `NodeId -> bm25 score` map upfront so `rank_referrer` is O(1).
    // We only need this when the seed source is a search hit list; with
    // override_seeds there is no BM25 signal at all.
    let bm25_by_node: HashMap<NodeId, f32> = match hits {
        Some(hs) => build_bm25_by_node(snap, hs, &ws_root),
        None => HashMap::new(),
    };

    let seeds: HashSet<NodeId> = match override_seeds {
        Some(names) => resolve_override_seeds(snap, names, &mut diagnostics)?,
        None => {
            let hs = hits.unwrap_or(&[]);
            // Defensively cap; the caller is supposed to have done this.
            let limit = opts.top_k_seeds.saturating_mul(3);
            let slice: &[crate::search::SearchResult] = if hs.len() > limit {
                &hs[..limit]
            } else {
                hs
            };
            resolve_search_seeds(snap, slice, &ws_root, opts)?
        }
    };

    // ---------- 2. EXPAND (bounded BFS) ----------
    let mut retained: HashSet<NodeId> = seeds.clone();
    let mut frontier: HashSet<NodeId> = seeds.clone();
    // `(from, to, kind)` -> weight. EdgeKind is Copy.
    let mut edges: HashMap<(NodeId, NodeId, EdgeKind), u32> = HashMap::new();

    {
        let rtxn = snap.read_txn()?;
        for _ in 0..opts.depth {
            if frontier.is_empty() {
                break;
            }
            let mut next: HashSet<NodeId> = HashSet::new();
            // Deterministic iteration order over `frontier`.
            let mut ordered: Vec<NodeId> = frontier.iter().copied().collect();
            ordered.sort_by_key(|n| node_qualified_name(snap, &rtxn, *n));
            for n in ordered {
                // Outgoing edges.
                let callees = snap.callees_of(n).unwrap_or_default();
                let mut callees_sorted: Vec<NodeId> = callees;
                callees_sorted.sort_by_key(|t| node_qualified_name(snap, &rtxn, *t));
                for target_id in callees_sorted {
                    let target_kind = snap
                        .node(&rtxn, target_id)
                        .ok()
                        .flatten()
                        .and_then(|nd| nd.item_kind);
                    let kind = if target_kind.map_or(false, |k| k.is_callable()) {
                        EdgeKind::Calls
                    } else {
                        EdgeKind::Uses
                    };
                    *edges.entry((n, target_id, kind)).or_insert(0) += 1;
                    if retained.insert(target_id) {
                        next.insert(target_id);
                    }
                }

                // Incoming edges — branch on `n`'s item kind.
                let n_kind = snap
                    .node(&rtxn, n)
                    .ok()
                    .flatten()
                    .and_then(|nd| nd.item_kind);
                let record_kind = match n_kind {
                    Some(k) if k.is_callable() => Some(EdgeKind::Calls),
                    Some(k) if k.is_type() => Some(EdgeKind::Uses),
                    _ => None,
                };
                if let Some(record_kind) = record_kind {
                    let mut refs = snap.referrers_of(n).unwrap_or_default();
                    refs.sort_by_key(|r| rank_referrer(*r, &bm25_by_node, snap, &rtxn));
                    for r in refs.into_iter().take(opts.max_incoming_per_node) {
                        *edges.entry((r, n, record_kind)).or_insert(0) += 1;
                        if retained.insert(r) {
                            next.insert(r);
                        }
                    }
                }
            }
            frontier = next;
        }
        // rtxn dropped at end of scope.
    }

    // ---------- 3. SCORE ----------
    // PHASE A — sync, with rtxn.
    let max_bm25 = bm25_by_node
        .values()
        .copied()
        .fold(0.0_f32, f32::max);
    let mut bm25_norm: HashMap<NodeId, f32> = HashMap::with_capacity(retained.len());
    let mut graph_prox: HashMap<NodeId, f32> = HashMap::with_capacity(retained.len());
    let mut cached: HashMap<NodeId, Vec<f32>> = HashMap::new();
    let mut missing: Vec<NodeId> = Vec::new();
    {
        let rtxn = snap.read_txn()?;
        for &nid in &retained {
            let raw = bm25_by_node.get(&nid).copied().unwrap_or(0.0);
            let norm = if max_bm25 > 0.0 {
                (raw / max_bm25).clamp(0.0, 1.0)
            } else {
                0.0
            };
            bm25_norm.insert(nid, norm);

            let dist = min_call_distance(snap, nid, &seeds, opts.depth as u32);
            let prox = if dist == u32::MAX {
                0.0
            } else {
                1.0 / (1.0 + dist as f32)
            };
            graph_prox.insert(nid, prox);
        }

        if opts.embedding_policy != EmbeddingPolicy::NoRerank {
            // Order the retained set deterministically so `missing` is
            // stable across runs.
            let mut ordered: Vec<NodeId> = retained.iter().copied().collect();
            ordered.sort_by_key(|n| node_qualified_name(snap, &rtxn, *n));
            for nid in ordered {
                let rec = snap.dbs.embeddings_by_target.get(&rtxn, nid.as_bytes())?;
                let fresh = rec.as_ref().map(|r| {
                    r.embedder_version == crate::tools::graph_tools::EMBEDDER_VERSION
                });
                match (rec, fresh) {
                    (Some(rec), Some(true)) => {
                        cached.insert(nid, rec.vector);
                    }
                    _ => {
                        missing.push(nid);
                    }
                }
            }
        }
        // Drop rtxn before any await.
    }

    // PHASE B — async, no txn.
    let prompt_emb: Option<Vec<f32>> =
        if opts.embedding_policy == EmbeddingPolicy::NoRerank || prompt.unwrap_or("").is_empty() {
            None
        } else {
            let generator = crate::embeddings::EmbeddingGenerator::new()
                .map_err(|e| anyhow::anyhow!("EmbeddingGenerator init: {e}"))?;
            let v = generator
                .embed_async(prompt.unwrap().to_owned())
                .await
                .map_err(|e| anyhow::anyhow!("embed_async: {e}"))?;
            Some(v)
        };

    let embeddings_computed: usize =
        if opts.embedding_policy == EmbeddingPolicy::ComputeMissing && !missing.is_empty() {
            let resolved = crate::tools::graph_tools::ensure_embeddings_for(snap, &missing).await?;
            let added = resolved.len();
            for (nid, re) in resolved {
                cached.insert(nid, re.vector);
            }
            added
        } else {
            0
        };

    // PHASE C — sync, no txn needed for scoring.
    let mut relevance: HashMap<NodeId, f32> = HashMap::with_capacity(retained.len());
    for &nid in &retained {
        let bm = *bm25_norm.get(&nid).unwrap_or(&0.0);
        let gp = *graph_prox.get(&nid).unwrap_or(&0.0);
        let emb_sim = prompt_emb
            .as_ref()
            .and_then(|pe| cached.get(&nid).map(|nv| crate::tools::graph_tools::cosine(pe, nv)));
        let r = match emb_sim {
            Some(s) => 0.40 * s + 0.35 * bm + 0.25 * gp,
            None => 0.60 * bm + 0.40 * gp,
        };
        relevance.insert(nid, r);
    }

    // ---------- 4. PRUNE ----------
    let mut non_seed: Vec<NodeId> = retained
        .iter()
        .copied()
        .filter(|nid| !seeds.contains(nid))
        .collect();
    // Sort by (-relevance, qualified_name) for deterministic top-K.
    {
        let rtxn = snap.read_txn()?;
        non_seed.sort_by(|a, b| {
            let ra = relevance.get(a).copied().unwrap_or(0.0);
            let rb = relevance.get(b).copied().unwrap_or(0.0);
            rb.partial_cmp(&ra)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    node_qualified_name(snap, &rtxn, *a)
                        .cmp(&node_qualified_name(snap, &rtxn, *b))
                })
        });
    }
    let budget = opts.max_nodes.saturating_sub(seeds.len());
    let keep_non_seed: HashSet<NodeId> = non_seed.into_iter().take(budget).collect();
    let final_set: HashSet<NodeId> = seeds.iter().copied().chain(keep_non_seed).collect();

    // Drop edges whose endpoints aren't both retained.
    edges.retain(|(from, to, _), _| final_set.contains(from) && final_set.contains(to));

    // ---------- 5. PROJECT ----------
    let hierarchy = project_hierarchy(snap, &final_set)?;

    // ---------- 6. ASSEMBLE ----------
    let prompt_str = prompt.unwrap_or("").to_string();
    let snapshot_id = snap.manifest.graph_id.clone();
    let generated_at_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Sort seeds deterministically by qualified name.
    let mut sorted_seeds: Vec<NodeId> = seeds.iter().copied().collect();
    {
        let rtxn = snap.read_txn()?;
        sorted_seeds.sort_by_key(|n| node_qualified_name(snap, &rtxn, *n));
    }

    // Build CodemapNode entries — sorted by qualified_name.
    let mut nodes_out: Vec<CodemapNode> = Vec::with_capacity(final_set.len());
    {
        let rtxn = snap.read_txn()?;
        let mut ordered: Vec<NodeId> = final_set.iter().copied().collect();
        ordered.sort_by_key(|n| node_qualified_name(snap, &rtxn, *n));
        for nid in ordered {
            let Some(node) = snap.node(&rtxn, nid)? else {
                continue;
            };
            nodes_out.push(CodemapNode {
                id: nid,
                qualified_name: node.qualified_name.clone(),
                kind: node.kind,
                item_kind: node.item_kind,
                file: node.file.clone(),
                span: node.span,
                relevance: relevance.get(&nid).copied().unwrap_or(0.0),
                is_seed: seeds.contains(&nid),
                // Snippet extraction is deferred — see Phase 5 plan §"Snippet extraction (defer)".
                snippet: None,
            });
        }
    }

    // Build CodemapEdge list — sorted by (from_qn, to_qn).
    let mut edges_out: Vec<CodemapEdge> = edges
        .into_iter()
        .map(|((from, to, kind), weight)| CodemapEdge {
            from,
            to,
            kind,
            weight,
        })
        .collect();
    {
        let rtxn = snap.read_txn()?;
        edges_out.sort_by(|a, b| {
            let fa = node_qualified_name(snap, &rtxn, a.from);
            let fb = node_qualified_name(snap, &rtxn, b.from);
            fa.cmp(&fb).then_with(|| {
                let ta = node_qualified_name(snap, &rtxn, a.to);
                let tb = node_qualified_name(snap, &rtxn, b.to);
                ta.cmp(&tb)
            })
        });
    }

    let stats = CodemapStats {
        seed_count: seeds.len(),
        node_count: nodes_out.len(),
        edge_count: edges_out.len(),
        embedded_nodes: cached.len(),
        embeddings_computed,
        total_ms: started.elapsed().map(|d| d.as_millis() as u64).unwrap_or(0),
    };

    Ok(Codemap {
        prompt: prompt_str,
        snapshot_id,
        generated_at_unix,
        seeds: sorted_seeds,
        nodes: nodes_out,
        edges: edges_out,
        hierarchy,
        stats,
        diagnostics,
    })
}

/// Resolve qualified-name seeds. Names that don't resolve become diagnostics
/// (`"unresolved seed: <name>"`); no RA fallback.
fn resolve_override_seeds(
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
            None => diagnostics.push(format!("unresolved seed: {qn}")),
        }
    }
    Ok(seeds)
}

/// Resolve search-hit seeds via the span index + line→byte bridge. Items
/// that are not callable or type-shaped are filtered out (a const-literal hit
/// is not a useful codemap seed).
fn resolve_search_seeds(
    snap: &OpenedSnapshot,
    hits: &[crate::search::SearchResult],
    ws_root: &Path,
    opts: &CodemapOptions,
) -> anyhow::Result<HashSet<NodeId>> {
    let mut seeds: HashSet<NodeId> = HashSet::new();
    let rtxn = snap.read_txn()?;
    for hit in hits {
        if seeds.len() >= opts.top_k_seeds {
            break;
        }
        let ctx = &hit.chunk.context;
        let Some(rel) = canonicalize_and_strip(&ctx.file_path, ws_root) else {
            continue;
        };
        let ls = ctx.line_start as u32;
        let le = ctx.line_end as u32;
        let Some(nid) = enclosing_item_for_line_range(snap, &rel, ls, le) else {
            continue;
        };
        let Some(node) = snap.node(&rtxn, nid)? else {
            continue;
        };
        let Some(kind) = node.item_kind else {
            continue;
        };
        if !(kind.is_callable() || kind.is_type()) {
            continue;
        }
        seeds.insert(nid);
    }
    Ok(seeds)
}

/// Pre-compute the `NodeId -> bm25 score` map by resolving each search hit
/// the same way `resolve_search_seeds` does. We sum (rather than max) when
/// multiple hits resolve to the same NodeId so that frequently-cited callers
/// rank higher.
fn build_bm25_by_node(
    snap: &OpenedSnapshot,
    hits: &[crate::search::SearchResult],
    ws_root: &Path,
) -> HashMap<NodeId, f32> {
    let mut out: HashMap<NodeId, f32> = HashMap::new();
    for hit in hits {
        let ctx = &hit.chunk.context;
        let Some(rel) = canonicalize_and_strip(&ctx.file_path, ws_root) else {
            continue;
        };
        let ls = ctx.line_start as u32;
        let le = ctx.line_end as u32;
        let Some(nid) = enclosing_item_for_line_range(snap, &rel, ls, le) else {
            continue;
        };
        *out.entry(nid).or_insert(0.0) += hit.score;
    }
    out
}

/// Deterministic ranking key for `referrers_of` results. Primary: negative
/// BM25 score (so higher score sorts first when using ascending `sort_by_key`).
/// Tiebreak: qualified name ascending.
fn rank_referrer(
    nid: NodeId,
    bm25_by_node: &HashMap<NodeId, f32>,
    snap: &OpenedSnapshot,
    rtxn: &RoTxn<'_, heed::WithoutTls>,
) -> (i32, String) {
    let s = bm25_by_node.get(&nid).copied().unwrap_or(0.0);
    let qn = snap
        .node(rtxn, nid)
        .ok()
        .flatten()
        .map(|n| n.qualified_name)
        .unwrap_or_default();
    (-((s * 1000.0) as i32), qn)
}

/// Helper: fetch a node's qualified name (or `String::new()` if missing).
/// Used in many sort closures; pulling it out keeps them readable.
fn node_qualified_name(
    snap: &OpenedSnapshot,
    rtxn: &RoTxn<'_, heed::WithoutTls>,
    nid: NodeId,
) -> String {
    snap.node(rtxn, nid)
        .ok()
        .flatten()
        .map(|n| n.qualified_name)
        .unwrap_or_default()
}

/// Project a hierarchy `ModuleTreeNode` over the retained set.
///
/// Strategy: discover the distinct crate qualified names represented by
/// retained nodes (via `Node.crate_id`), pull each crate's full module tree
/// via `OpenedSnapshot::module_tree`, then filter each tree post-order so
/// only branches containing at least one retained node survive.
///
/// If only one crate is represented, return its filtered tree directly. If
/// multiple, wrap the per-crate trees under a synthetic `Workspace` root.
fn project_hierarchy(
    snap: &OpenedSnapshot,
    retained: &HashSet<NodeId>,
) -> anyhow::Result<ModuleTreeNode> {
    // Find distinct crate ids of retained nodes.
    let mut crate_ids: HashSet<NodeId> = HashSet::new();
    {
        let rtxn = snap.read_txn()?;
        for &nid in retained {
            if let Some(node) = snap.node(&rtxn, nid)? {
                if let Some(cid) = node.crate_id {
                    crate_ids.insert(cid);
                }
            }
        }
    }

    // Map each crate id to its qualified name, sorted for determinism.
    let mut crate_names: Vec<String> = Vec::new();
    {
        let rtxn = snap.read_txn()?;
        for cid in &crate_ids {
            if let Some(c) = snap.node(&rtxn, *cid)? {
                if c.kind == NodeKind::Crate {
                    crate_names.push(c.qualified_name);
                }
            }
        }
    }
    crate_names.sort();
    crate_names.dedup();

    // Retained qualified-name set for the filter predicate.
    let retained_qns: HashSet<String> = {
        let rtxn = snap.read_txn()?;
        retained
            .iter()
            .filter_map(|nid| snap.node(&rtxn, *nid).ok().flatten().map(|n| n.qualified_name))
            .collect()
    };

    let mut filtered_trees: Vec<ModuleTreeNode> = Vec::new();
    for name in &crate_names {
        let tree = snap.module_tree(name, None)?;
        if let Some(filtered) = filter_module_tree(tree, &retained_qns) {
            filtered_trees.push(filtered);
        }
    }

    if filtered_trees.len() == 1 {
        // Safe: len == 1.
        Ok(filtered_trees.into_iter().next().expect("len == 1"))
    } else {
        // Wrap in a synthetic Workspace root. ModuleTreeNode fields are
        // string-typed so we mint sensible labels rather than touching the
        // queries.rs struct.
        Ok(ModuleTreeNode {
            qualified_name: "<workspace>".to_string(),
            display_name: "workspace".to_string(),
            kind: "Workspace".to_string(),
            item_kind: None,
            visibility: None,
            children: filtered_trees,
        })
    }
}

/// Post-order filter on a `ModuleTreeNode`. Keeps a node iff its
/// `qualified_name` is in `retained_qns` OR any descendant is kept.
fn filter_module_tree(
    mut node: ModuleTreeNode,
    retained_qns: &HashSet<String>,
) -> Option<ModuleTreeNode> {
    let kept_children: Vec<ModuleTreeNode> = std::mem::take(&mut node.children)
        .into_iter()
        .filter_map(|c| filter_module_tree(c, retained_qns))
        .collect();
    let self_retained = retained_qns.contains(&node.qualified_name);
    if self_retained || !kept_children.is_empty() {
        node.children = kept_children;
        Some(node)
    } else {
        None
    }
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

    #[test]
    fn min_call_distance_zero_when_seed_is_self() {
        let fixture = shared_fixture();
        let (caller_id, _) = fixture
            .snap
            .lookup_by_qualified_name("synthetic_codemap_crate::caller")
            .expect("lookup_by_qualified_name caller")
            .expect("caller resolves");
        let mut seeds: HashSet<NodeId> = HashSet::new();
        seeds.insert(caller_id);
        let d = min_call_distance(&fixture.snap, caller_id, &seeds, 3);
        assert_eq!(d, 0, "node is itself a seed → distance 0");
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

    #[tokio::test]
    async fn build_codemap_override_seeds_resolves_deterministically() {
        let fixture = shared_fixture();
        let names = vec![
            "synthetic_codemap_crate::caller".to_string(),
            "synthetic_codemap_crate::callee".to_string(),
        ];
        let opts = CodemapOptions::default();
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts)
            .await
            .expect("build_codemap succeeds with override_seeds");

        // Both names resolve, so seeds.len() == 2 and diagnostics is empty.
        assert_eq!(cm.stats.seed_count, 2, "both seeds should resolve");
        assert_eq!(cm.diagnostics, Vec::<String>::new());

        // Seeds are sorted by qualified_name; the test asserts that
        // ordering so the snapshot is stable.
        let seed_qns: Vec<String> = cm
            .seeds
            .iter()
            .filter_map(|nid| {
                let rtxn = fixture.snap.read_txn().ok()?;
                fixture.snap.node(&rtxn, *nid).ok().flatten().map(|n| n.qualified_name)
            })
            .collect();
        assert_eq!(
            seed_qns,
            vec![
                "synthetic_codemap_crate::callee".to_string(),
                "synthetic_codemap_crate::caller".to_string(),
            ],
        );

        // Two seeds, prompt empty, snapshot_id from manifest non-empty.
        assert!(!cm.snapshot_id.is_empty());
        assert_eq!(cm.prompt, "");

        // The retained set must include both seeds.
        let retained_qns: HashSet<String> =
            cm.nodes.iter().map(|n| n.qualified_name.clone()).collect();
        assert!(retained_qns.contains("synthetic_codemap_crate::caller"));
        assert!(retained_qns.contains("synthetic_codemap_crate::callee"));
    }

    #[tokio::test]
    async fn build_codemap_depth_zero_returns_only_seeds() {
        let fixture = shared_fixture();
        let names = vec!["synthetic_codemap_crate::caller".to_string()];
        let opts = CodemapOptions {
            depth: 0,
            ..CodemapOptions::default()
        };
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts)
            .await
            .expect("build_codemap succeeds at depth=0");

        // With depth=0 the BFS body never executes, so only the seed is
        // retained — `caller` itself.
        assert_eq!(cm.stats.seed_count, 1);
        assert_eq!(cm.stats.node_count, 1, "depth=0 → seeds only");
        assert_eq!(cm.stats.edge_count, 0, "no edges at depth 0");
        assert_eq!(cm.nodes.len(), 1);
        assert!(cm.nodes[0].is_seed);
        assert_eq!(cm.nodes[0].qualified_name, "synthetic_codemap_crate::caller");
    }

    #[tokio::test]
    async fn build_codemap_unresolved_seed_records_diagnostic() {
        let fixture = shared_fixture();
        let names = vec![
            "synthetic_codemap_crate::caller".to_string(),
            "synthetic_codemap_crate::does_not_exist_xyzzy".to_string(),
        ];
        let opts = CodemapOptions::default();
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts)
            .await
            .expect("build_codemap succeeds despite unresolved name");

        // One name resolves, one doesn't — so we get 1 seed + 1 diagnostic.
        assert_eq!(cm.stats.seed_count, 1);
        assert_eq!(cm.diagnostics.len(), 1);
        assert_eq!(
            cm.diagnostics[0],
            "unresolved seed: synthetic_codemap_crate::does_not_exist_xyzzy",
        );
    }
}

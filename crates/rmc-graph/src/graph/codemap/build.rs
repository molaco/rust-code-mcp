//! `build_codemap` algorithm core + the pre-build freshness check.
//!
//! Split out of `mod.rs` in PR 13. The BFS expand/score/prune pipeline and
//! its small private helpers (rank_referrer, prune_to_budget,
//! node_qualified_name, line_of_byte, extract_snippet) all live here.
//!
//! `newest_source_mtime` is co-located because the tools-layer handler
//! invokes it as a pre-build diagnostic before calling `build_codemap`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use heed::RoTxn;

use crate::graph::codemap::hierarchy::project_hierarchy;
use crate::graph::codemap::model::{
    Codemap, CodemapEdge, CodemapNode, CodemapOptions, CodemapStats, EdgeKind, EmbeddingPolicy,
};
use crate::graph::codemap::seeds::{
    SeedHit, build_bm25_by_node, resolve_override_seeds, resolve_search_seeds,
};
use crate::graph::ids::NodeId;
use crate::graph::snapshot::OpenedSnapshot;

// `min_call_distance` previously walked outgoing callees from a candidate
// looking for a seed. That direction was wrong: `build_codemap`'s BFS
// expands from seeds outward (both directions), so the *correct* distance
// is the BFS depth at which the candidate was discovered. We track that
// directly during EXPAND in a `dist_from_seed: HashMap<NodeId, u32>` and
// read it in SCORE. No separate distance walk is needed.

// ---------------------------------------------------------------------------
// Phase 5 — `build_codemap` algorithm core.
// ---------------------------------------------------------------------------

/// Algorithm-core entry point for the codemap MCP tool.
///
/// The `tools::graph::codemap` adapter wires `HybridSearch::search` and
/// parameter parsing on the tools side and maps results into `SeedHit`
/// before calling this function; keeping `build_codemap` independent of
/// `crate::search` types makes the algorithm easier to unit-test against
/// an in-memory snapshot.
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
/// - `pre_diagnostics`: caller-supplied diagnostics to prepend onto the
///   returned `Codemap.diagnostics`. Used by the MCP handler to surface
///   pre-flight signals (e.g. snapshot staleness) without coupling the
///   algorithm core to filesystem I/O.
pub async fn build_codemap(
    snap: &OpenedSnapshot,
    prompt: Option<&str>,
    override_seeds: Option<&[String]>,
    hits: Option<&[SeedHit]>,
    opts: &CodemapOptions,
    pre_diagnostics: &[String],
) -> anyhow::Result<Codemap> {
    let started = SystemTime::now();
    let mut diagnostics: Vec<String> = pre_diagnostics.to_vec();

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
            let slice: &[SeedHit] = if hs.len() > limit {
                &hs[..limit]
            } else {
                hs
            };
            let resolved = resolve_search_seeds(snap, slice, &ws_root, opts, &mut diagnostics)?;
            // Item 1: if the caller supplied hits but none resolved, surface
            // a diagnostic so the caller can distinguish "no hits" from
            // "hits all dropped".
            if hits.is_some() && resolved.is_empty() {
                diagnostics.push("no search hits resolved to graph items".to_string());
            }
            resolved
        }
    };

    // ---------- 2. EXPAND (bounded BFS) ----------
    let mut retained: HashSet<NodeId> = seeds.clone();
    let mut frontier: HashSet<NodeId> = seeds.clone();
    // Distance from the nearest seed, in BFS-discovery order. Seeds are 0.
    // Populated as nodes enter `retained` so SCORE can read it without a
    // second graph walk.
    let mut dist_from_seed: HashMap<NodeId, u32> = HashMap::new();
    for &s in &seeds {
        dist_from_seed.insert(s, 0);
    }
    // `(from, to, kind)` -> weight. EdgeKind is Copy.
    let mut edges: HashMap<(NodeId, NodeId, EdgeKind), u32> = HashMap::new();

    {
        let rtxn = snap.read_txn()?;
        for d in 0..opts.depth {
            if frontier.is_empty() {
                break;
            }
            let next_dist = (d as u32) + 1;
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
                    edges.entry((n, target_id, kind)).or_insert(1);
                    if retained.insert(target_id) {
                        dist_from_seed.entry(target_id).or_insert(next_dist);
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
                        edges.entry((r, n, record_kind)).or_insert(1);
                        if retained.insert(r) {
                            dist_from_seed.entry(r).or_insert(next_dist);
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

            // BFS distance was recorded during EXPAND. Missing entries
            // (shouldn't happen for retained nodes) clamp to u32::MAX → 0.
            let dist = dist_from_seed.get(&nid).copied().unwrap_or(u32::MAX);
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
            // Compute the active embedder identity once (same default the
            // generator below picks up).
            //
            // TODO(step7+): when build_codemap learns to accept an
            // `&EmbeddingBackend` from its caller, replace this default
            // with the caller's choice so non-default Qwen3 variants
            // invalidate stale cache rows correctly.
            let active_version = rmc_engine::embeddings::EmbeddingBackend::default().identity();
            for nid in ordered {
                let rec = snap.dbs.embeddings_by_target.get(&rtxn, nid.as_bytes())?;
                let fresh = rec
                    .as_ref()
                    .map(|r| r.embedder_version == active_version);
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
            let generator = rmc_engine::embeddings::EmbeddingGenerator::new()
                .map_err(|e| anyhow::anyhow!("EmbeddingGenerator init: {e}"))?;
            // Query-side: the prompt is a retrieval query scored against
            // cached document vectors — apply the instruction prefix.
            let v = generator
                .embed_queries(vec![prompt.unwrap().to_owned()])
                .await
                .map_err(|e| anyhow::anyhow!("embed_queries: {e}"))?
                .into_iter()
                .next()
                .ok_or_else(|| anyhow::anyhow!("embed_queries returned no vector"))?;
            Some(v)
        };

    let embeddings_computed: usize =
        if opts.embedding_policy == EmbeddingPolicy::ComputeMissing && !missing.is_empty() {
            // Codemap is not profile-parameterized; embed with the default
            // backend, matching the prompt embedder constructed above.
            let resolved = crate::graph::ensure_embeddings_for(
                snap,
                &missing,
                &rmc_engine::embeddings::EmbeddingBackend::default(),
            )
            .await?;
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
            .and_then(|pe| cached.get(&nid).map(|nv| crate::graph::cosine(pe, nv)));
        let r = match emb_sim {
            Some(s) => 0.40 * s + 0.35 * bm + 0.25 * gp,
            None => 0.60 * bm + 0.40 * gp,
        };
        relevance.insert(nid, r);
    }

    // ---------- 4. PRUNE ----------
    // Build a qualified-name map for the tie-break key so `prune_to_budget`
    // can stay snapshot-free and unit-testable. Seeds always survive
    // regardless of budget — that invariant lives in `prune_to_budget`.
    let qualified_names: HashMap<NodeId, String> = {
        let rtxn = snap.read_txn()?;
        retained
            .iter()
            .copied()
            .map(|nid| (nid, node_qualified_name(snap, &rtxn, nid)))
            .collect()
    };
    let final_set = prune_to_budget(
        &seeds,
        &retained,
        &relevance,
        &qualified_names,
        opts.max_nodes,
    );

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

    // Build CodemapNode entries — sorted by qualified_name. Populates the
    // line number (item 3) and, when `opts.include_snippets`, the snippet
    // text (item 4). File reads are cached so each file is touched at most
    // once per call regardless of how many retained nodes live in it.
    let mut nodes_out: Vec<CodemapNode> = Vec::with_capacity(final_set.len());
    {
        let rtxn = snap.read_txn()?;
        let mut ordered: Vec<NodeId> = final_set.iter().copied().collect();
        ordered.sort_by_key(|n| node_qualified_name(snap, &rtxn, *n));
        let ws_root_path = PathBuf::from(&snap.manifest.workspace_root);
        let mut file_cache: HashMap<String, String> = HashMap::new();
        for nid in ordered {
            let Some(node) = snap.node(&rtxn, nid)? else {
                continue;
            };
            let line = match (&node.file, node.span) {
                (Some(file), Some((byte_start, _))) => line_of_byte(snap, file, byte_start),
                _ => None,
            };
            let snippet = if opts.include_snippets {
                match (&node.file, node.span) {
                    (Some(file), Some((byte_start, _))) => {
                        extract_snippet(&ws_root_path, file, byte_start, &mut file_cache)
                    }
                    _ => None,
                }
            } else {
                None
            };
            nodes_out.push(CodemapNode {
                id: nid,
                qualified_name: node.qualified_name.clone(),
                kind: node.kind,
                item_kind: node.item_kind,
                file: node.file.clone(),
                span: node.span,
                line,
                relevance: relevance.get(&nid).copied().unwrap_or(0.0),
                is_seed: seeds.contains(&nid),
                snippet,
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
/// Cap `retained` to `max_nodes`, preserving seeds unconditionally.
///
/// **Invariant**: every NodeId in `seeds` survives, even when
/// `seeds.len() >= max_nodes`. The `max_nodes` budget governs only the
/// number of *non-seed* nodes that may be kept; if the seed count alone
/// already meets or exceeds the budget, the non-seed budget saturates to
/// zero and the returned set equals `seeds ∩ retained`.
///
/// Non-seed nodes are ranked by (descending `relevance`, ascending
/// `qualified_names[nid]`) and the top `max_nodes - seeds.len()` are
/// kept. `qualified_names` is the snapshot-free lookup the caller built
/// from `node_qualified_name`; nodes missing from the map sort with an
/// empty string (which is fine — the snapshot would have returned the
/// same default).
fn prune_to_budget(
    seeds: &HashSet<NodeId>,
    retained: &HashSet<NodeId>,
    relevance: &HashMap<NodeId, f32>,
    qualified_names: &HashMap<NodeId, String>,
    max_nodes: usize,
) -> HashSet<NodeId> {
    let mut non_seed: Vec<NodeId> = retained
        .iter()
        .copied()
        .filter(|nid| !seeds.contains(nid))
        .collect();
    non_seed.sort_by(|a, b| {
        let ra = relevance.get(a).copied().unwrap_or(0.0);
        let rb = relevance.get(b).copied().unwrap_or(0.0);
        rb.partial_cmp(&ra)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                let aq = qualified_names.get(a).map(String::as_str).unwrap_or("");
                let bq = qualified_names.get(b).map(String::as_str).unwrap_or("");
                aq.cmp(bq)
            })
    });
    let budget = max_nodes.saturating_sub(seeds.len());
    let keep_non_seed: HashSet<NodeId> = non_seed.into_iter().take(budget).collect();
    // Seeds are always in `retained` by construction (EXPAND seeds them
    // before BFS), so we don't filter; the union below preserves all seeds.
    seeds.iter().copied().chain(keep_non_seed).collect()
}

pub(super) fn node_qualified_name(
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

/// Resolve a byte offset within `workspace_relative_file` to a 1-indexed
/// source-line number. Uses `OpenedSnapshot::line_to_byte`, then binary-
/// searches for the largest offset `<= byte_start`. Returns `None` if the
/// offset table can't be built (file unreadable, etc.).
fn line_of_byte(
    snap: &OpenedSnapshot,
    workspace_relative_file: &str,
    byte_start: u32,
) -> Option<u32> {
    let table = snap.line_to_byte(workspace_relative_file).ok()?;
    // Largest index `i` such that `table[i] <= byte_start`.
    // `partition_point` returns the first index where the predicate is false,
    // so subtract one to get the last matching index.
    let pp = table.partition_point(|&off| off <= byte_start);
    if pp == 0 {
        // No entry satisfied the predicate, which only happens if `table` is
        // empty (line_to_byte always inserts a 0 first, so this is a
        // pathological fallthrough).
        None
    } else {
        Some(pp as u32) // (idx + 1) where idx = pp - 1.
    }
}

/// Extract a short snippet for a node from its source file.
///
/// Reads at most **5 lines or 400 bytes (whichever comes first)** starting
/// at `byte_start` and strips trailing whitespace. `file_cache` is keyed on
/// the workspace-relative path so each file is read at most once per call.
/// Returns `None` if the file can't be read or the byte offset is outside
/// the file.
fn extract_snippet(
    ws_root: &Path,
    workspace_relative_file: &str,
    byte_start: u32,
    file_cache: &mut HashMap<String, String>,
) -> Option<String> {
    let key = workspace_relative_file.to_string();
    if !file_cache.contains_key(&key) {
        let abs = ws_root.join(workspace_relative_file);
        let s = std::fs::read_to_string(&abs).ok()?;
        file_cache.insert(key.clone(), s);
    }
    let content = file_cache.get(&key)?;
    let start = byte_start as usize;
    if start > content.len() || !content.is_char_boundary(start) {
        return None;
    }
    // Snap `start` back to the beginning of its containing line so doc-comment-
    // adjacent items don't emit snippets that begin mid-line (pass-2 #A1).
    let file_bytes = content.as_bytes();
    let line_start = file_bytes[..start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map(|i| i + 1)
        .unwrap_or(0);
    let start = line_start;
    let tail = &content[start..];
    // Cap at 5 lines or 400 bytes — whichever boundary is hit first.
    let byte_cap = tail.len().min(400);
    let mut end = byte_cap;
    let mut lines_seen: usize = 0;
    for (i, b) in tail.as_bytes().iter().take(byte_cap).enumerate() {
        if *b == b'\n' {
            lines_seen += 1;
            if lines_seen >= 5 {
                end = i + 1;
                break;
            }
        }
    }
    // Ensure we slice on a char boundary (we capped by byte index above; if
    // the cap landed mid-codepoint, walk back).
    while end > 0 && !content.is_char_boundary(start + end) {
        end -= 1;
    }
    let slice = tail.get(..end)?;
    let trimmed = slice.trim_end();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Walk `workspace_root` for every `.rs` file (excluding `target/` and
/// `.git/` — mirroring [`crate::graph::storage::compute_fingerprint`]'s
/// filter) and return the maximum file `mtime` in seconds since the UNIX
/// epoch.
///
/// Used by the `build_codemap` MCP handler to surface a "snapshot is
/// older than newest `.rs` file" diagnostic without forcing the user to
/// re-read the manifest by hand. Returns `None` if no `.rs` file is
/// reachable or every walk entry fails — in that case the caller should
/// skip the diagnostic rather than treat it as an error.
pub fn newest_source_mtime(workspace_root: &Path) -> Option<u64> {
    let mut newest: Option<u64> = None;
    for entry in walkdir::WalkDir::new(workspace_root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| {
            !e.path()
                .components()
                .any(|c| c.as_os_str() == "target" || c.as_os_str() == ".git")
        })
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let is_rs = entry
            .path()
            .extension()
            .and_then(|s| s.to_str())
            .map(|ext| ext == "rs")
            .unwrap_or(false);
        if !is_rs {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(dur) = modified.duration_since(UNIX_EPOCH) else {
            continue;
        };
        let secs = dur.as_secs();
        newest = Some(match newest {
            Some(prev) => prev.max(secs),
            None => secs,
        });
    }
    newest
}

#[cfg(test)]
mod tests {
    use super::*;

    // ===== pure tests =====

    #[test]
    fn prune_preserves_seeds_when_budget_below_seed_count() {
        // Build 10 seeds; budget is 3 (below seed count). All 10
        // seeds must survive because `prune_to_budget`'s invariant
        // is that seeds are unconditional regardless of budget. The
        // non-seed budget saturates to zero.
        let seeds: HashSet<NodeId> = (0u8..10).map(|b| NodeId([b; 32])).collect();
        // Add a few non-seed retained candidates that should NOT
        // make it through because the budget is exhausted by seeds.
        let extra_a = NodeId([0xC1; 32]);
        let extra_b = NodeId([0xC2; 32]);
        let mut retained: HashSet<NodeId> = seeds.clone();
        retained.insert(extra_a);
        retained.insert(extra_b);

        let mut relevance: HashMap<NodeId, f32> = HashMap::new();
        // Non-seeds get very high relevance just to confirm they
        // still lose to seeds when the budget is below seed count.
        relevance.insert(extra_a, 99.0);
        relevance.insert(extra_b, 99.0);
        for &s in &seeds {
            relevance.insert(s, 0.0);
        }
        let mut qualified_names: HashMap<NodeId, String> = HashMap::new();
        for (i, &s) in seeds.iter().enumerate() {
            qualified_names.insert(s, format!("seed_{i:02}"));
        }
        qualified_names.insert(extra_a, "extra_a".to_string());
        qualified_names.insert(extra_b, "extra_b".to_string());

        let max_nodes = 3;
        let kept = prune_to_budget(&seeds, &retained, &relevance, &qualified_names, max_nodes);

        // All 10 seeds survive — that's the property under test.
        assert_eq!(
            kept.len(),
            seeds.len(),
            "seeds must all survive; budget below seed count yields zero non-seed slots"
        );
        for s in &seeds {
            assert!(kept.contains(s), "seed {s:?} must be retained");
        }
        // Confirm neither extra was kept.
        assert!(
            !kept.contains(&extra_a) && !kept.contains(&extra_b),
            "non-seed extras must be dropped when budget is exhausted by seeds"
        );
    }

    #[test]
    fn edge_recording_dedups_repeats() {
        // After A5: recording the same (from, to, kind) edge multiple times
        // should yield weight = 1, not weight = N. This guards against
        // accidental += 1 reintroduction.
        let mut edges: HashMap<(NodeId, NodeId, EdgeKind), u32> = HashMap::new();
        let a = NodeId([0xAA; 32]);
        let b = NodeId([0xBB; 32]);
        edges.entry((a, b, EdgeKind::Calls)).or_insert(1);
        edges.entry((a, b, EdgeKind::Calls)).or_insert(1);
        edges.entry((a, b, EdgeKind::Calls)).or_insert(1);
        assert_eq!(
            edges[&(a, b, EdgeKind::Calls)],
            1,
            "set semantics required after A5"
        );
        assert_eq!(edges.len(), 1);
    }

    // ----- A8: newest_source_mtime pure I/O against a tempdir -----

    /// Stamp `path`'s mtime to `secs_since_epoch`. Uses
    /// `File::set_modified` which is stable since Rust 1.75; we are on
    /// edition 2024 so it is always available.
    fn stamp_mtime(path: &Path, secs_since_epoch: u64) {
        use std::fs::OpenOptions;
        use std::time::{Duration, SystemTime, UNIX_EPOCH};
        let f = OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for mtime set");
        let t = UNIX_EPOCH + Duration::from_secs(secs_since_epoch);
        f.set_modified(t).expect("set_modified");
        // Verify the platform actually honored the stamp. Some
        // filesystems clamp or round; if so, the assertions below would
        // be confusing — surface the skew here instead.
        let actual = std::fs::metadata(path)
            .and_then(|m| m.modified())
            .expect("metadata mtime")
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("post-epoch")
            .as_secs();
        assert_eq!(actual, secs_since_epoch, "platform did not honor mtime stamp");
    }

    #[test]
    fn newest_source_mtime_picks_max_across_rs_files() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let a = root.join("a.rs");
        let b = root.join("nested").join("b.rs");
        fs::create_dir_all(b.parent().unwrap()).expect("nested dir");
        fs::write(&a, "// a").expect("write a");
        fs::write(&b, "// b").expect("write b");

        // Pin mtimes deterministically: b is 60s newer than a.
        stamp_mtime(&a, 1_000_000);
        stamp_mtime(&b, 1_000_060);

        let got = newest_source_mtime(root).expect("at least one .rs file");
        assert_eq!(
            got, 1_000_060,
            "mtime should be the newest .rs file's mtime"
        );
    }

    #[test]
    fn newest_source_mtime_skips_target_and_git_dirs() {
        use std::fs;
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path();
        let kept = root.join("src").join("kept.rs");
        let skipped_target = root.join("target").join("skipped.rs");
        let skipped_git = root.join(".git").join("skipped.rs");
        for p in [&kept, &skipped_target, &skipped_git] {
            fs::create_dir_all(p.parent().unwrap()).expect("mkdir");
            fs::write(p, "// rs").expect("write");
        }

        // Make the excluded files much newer than kept; the result must
        // still reflect `kept`'s mtime, proving target/ and .git/ were
        // filtered out.
        stamp_mtime(&kept, 1_000_000);
        stamp_mtime(&skipped_target, 2_000_000);
        stamp_mtime(&skipped_git, 2_000_000);

        let got = newest_source_mtime(root).expect("at least one kept .rs file");
        assert_eq!(
            got, 1_000_000,
            "target/ and .git/ entries must not count"
        );
    }

    #[test]
    fn newest_source_mtime_returns_none_when_no_rs_files() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("readme.md"), "# nope").expect("write");
        assert!(newest_source_mtime(dir.path()).is_none());
    }

    // ===== fixture-dependent tests (build_codemap end-to-end) =====

    use crate::graph::codemap::model::CodemapOptions;
    use crate::graph::codemap::test_support::shared_fixture;

    #[tokio::test]
    async fn build_codemap_override_seeds_resolves_deterministically() {
        let fixture = shared_fixture();
        let names = vec![
            "synthetic_codemap_crate::caller".to_string(),
            "synthetic_codemap_crate::callee".to_string(),
        ];
        let opts = CodemapOptions::default();
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts, &[])
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
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts, &[])
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
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts, &[])
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

    // ----- BFS-distance regression tests (commit 71f88332) -----
    //
    // Both tests document that `dist_from_seed` is tracked during
    // EXPAND in BOTH directions: outgoing (callees) and incoming
    // (referrers). A regression where the BFS only updated distance
    // in one direction would zero out `graph_prox` for the other
    // direction and these would fail.

    #[tokio::test]
    async fn graph_prox_positive_for_direct_callee_of_seed() {
        // Seed = caller; direct callee is `callee`. BFS reaches
        // `callee` at distance 1, so graph_prox = 1/(1+1) = 0.5 and
        // (without embeddings) relevance = 0.40 * 0.5 = 0.20 > 0.
        let fixture = shared_fixture();
        let names = vec!["synthetic_codemap_crate::caller".to_string()];
        let opts = CodemapOptions::default();
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts, &[])
            .await
            .expect("build_codemap succeeds for graph-prox callee test");

        let callee = cm
            .nodes
            .iter()
            .find(|n| n.qualified_name == "synthetic_codemap_crate::callee")
            .expect("direct callee must be retained at depth>=1");
        assert!(
            callee.relevance > 0.0,
            "direct callee of seed must have graph_prox > 0 (relevance={}); \
             a regression in the BFS-distance fix would zero this out",
            callee.relevance
        );
    }

    #[tokio::test]
    async fn graph_prox_positive_for_direct_caller_of_seed() {
        // Seed = callee; direct caller is `caller`. BFS expands
        // incoming `referrers_of` for callable seeds, so `caller`
        // is reached at distance 1.
        let fixture = shared_fixture();
        let names = vec!["synthetic_codemap_crate::callee".to_string()];
        let opts = CodemapOptions::default();
        let cm = build_codemap(&fixture.snap, None, Some(&names), None, &opts, &[])
            .await
            .expect("build_codemap succeeds for graph-prox caller test");

        let caller = cm
            .nodes
            .iter()
            .find(|n| n.qualified_name == "synthetic_codemap_crate::caller")
            .expect("direct caller must be retained at depth>=1");
        assert!(
            caller.relevance > 0.0,
            "direct caller of seed must have graph_prox > 0 (relevance={}); \
             a regression in the BFS-distance fix would zero this out",
            caller.relevance
        );
    }

    // Phase 6 end-to-end validation against the live workspace snapshot is
    // intentionally NOT included as an automated test: opening the
    // rust-code-mcp workspace's own snapshot at test time would require
    // having previously built it (and would couple the test to a particular
    // dev environment). The renderer smoke tests above plus the existing
    // synthetic-fixture build_codemap tests give enough coverage of the
    // code paths Phase 6 added.
}

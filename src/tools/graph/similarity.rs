//! Similarity endpoint family.
//!
//! Vector-similarity queries backed by the persisted hypergraph: a per-Item
//! semantic nearest-neighbor lookup (`similar_to_item`) and a workspace-wide
//! pairwise/clustered duplicate-detection audit (`semantic_overlaps`). Both
//! resolve seed Items by qualified name, embed source bytes via the
//! configured backend (cached vectors live in LMDB), and run cosine
//! similarity over the result set. They share `resolve_graph_tool_backend`
//! for embedding-profile resolution and reach cluster/page helpers + the
//! shared `ItemRef` / `SimilarityCluster` shapes via
//! `crate::tools::graph::response::*`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rmcp::{ErrorData as McpError, model::CallToolResult};
use serde::Serialize;

use crate::graph::labels::item_kind_short_label as short_item_kind_label;
use crate::graph::{Node, NodeId, NodeKind};
use crate::tools::graph::response::*;
use crate::tools::params::{SemanticOverlapsParams, SimilarToItemParams};

/// v0.1 "semantic overlaps": resolve `target` to a hypergraph Item, read its
/// source bytes from (file, span), and run vector_only_search using those
/// bytes as the query. Drops the seed's own chunk (file-path-only match — see
/// limitation note) and applies optional `threshold` / `item_kind` filters.
///
/// Limitation: self-match detection is file-path-only. If the seed file
/// contains other items that match the seed's source semantically, those
/// will be returned. A finer span-overlap check is left for v0.2.
pub(crate) async fn similar_to_item(
    params: SimilarToItemParams,
) -> Result<CallToolResult, McpError> {
    // 1. Resolve seed Item from the hypergraph snapshot.
    let snap = open_workspace_snapshot(&params.directory)?;
    let (_seed_id, seed_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;

    let seed_file = seed_node.file.clone().ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "target item `{}` has no source location (synthetic / macro-generated?)",
                params.target
            ),
            None,
        )
    })?;
    let seed_span = seed_node.span.ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "target item `{}` has no source span (synthetic / macro-generated?)",
                params.target
            ),
            None,
        )
    })?;

    // 2. Read seed source bytes from disk.
    let abs_path = PathBuf::from(&params.directory).join(&seed_file);
    let content = std::fs::read_to_string(&abs_path).map_err(|e| {
        McpError::invalid_params(
            format!("failed to read seed file `{}`: {e}", abs_path.display()),
            None,
        )
    })?;
    let (start, end) = (seed_span.0 as usize, seed_span.1 as usize);
    let seed_source = content.get(start..end).ok_or_else(|| {
        McpError::invalid_params(
            format!(
                "seed span {}..{} is out of bounds or splits a UTF-8 character in `{}` (file len = {})",
                start,
                end,
                abs_path.display(),
                content.len()
            ),
            None,
        )
    })?.to_string();

    // 3. Run vector-only search against the index built with the
    //    requested embedding profile (the default profile when unset).
    let backend = resolve_graph_tool_backend(
        params.embedding_profile.as_deref(),
        &params.directory,
    )?;
    let paths = crate::mcp::project_paths::ProjectPaths::from_directory(
        Path::new(&params.directory),
        &backend,
    );
    let hybrid_search =
        crate::tools::endpoints::query::create_hybrid_search(&paths, None, backend).await?;

    let limit = params.limit.unwrap_or(10);
    let threshold = params.threshold.unwrap_or(0.0);
    let limit_plus_one = limit.saturating_add(1);

    let results = hybrid_search
        .vector_only_search(&seed_source, limit_plus_one)
        .await
        .map_err(|e| McpError::invalid_params(format!("vector search failed: {e}"), None))?;

    // 4. Filter results.
    let item_kind_filter = params.item_kind.as_ref().map(|s| s.to_lowercase());
    // Precompute the seed's line range once for the self-match overlap check.
    // The seed_file is workspace-relative (e.g. `crates/foo/src/lib.rs`) but
    // chunk file paths from the vector store are absolute, so we use
    // `Path::ends_with` for the same-file check (component-aware suffix match,
    // not byte equality) — this avoids the v0.1 false-negative where the seed
    // appeared as the top match because the relative-vs-absolute paths never
    // compared equal as strings.
    let seed_line_start = content[..start].matches('\n').count() + 1;
    let seed_line_end = content[..end].matches('\n').count() + 1;
    let seed_rel_path = Path::new(&seed_file);
    let mut matches: Vec<SimilarMatch> = Vec::new();
    for r in results {
        if r.chunk.context.file_path.ends_with(seed_rel_path) {
            // Drop only chunks whose line range overlaps the seed's byte span,
            // not every chunk in the same file.
            let result_line_start = r.chunk.context.line_start;
            let result_line_end = r.chunk.context.line_end;
            let overlaps = result_line_start <= seed_line_end
                && result_line_end >= seed_line_start;
            if overlaps {
                continue;
            }
        }
        if r.score < threshold {
            continue;
        }
        if let Some(ref want) = item_kind_filter {
            if r.chunk.context.symbol_kind.to_lowercase() != *want {
                continue;
            }
        }
        let preview = r
            .chunk
            .content
            .lines()
            .take(3)
            .collect::<Vec<_>>()
            .join("\n");
        matches.push(SimilarMatch {
            similarity: r.score,
            symbol_name: r.chunk.context.symbol_name,
            symbol_kind: r.chunk.context.symbol_kind,
            file: r.chunk.context.file_path.to_string_lossy().to_string(),
            line_start: r.chunk.context.line_start,
            line_end: r.chunk.context.line_end,
            preview,
        });
        if matches.len() >= limit {
            break;
        }
    }

    // 6. Build response.
    let resp = SimilarToItemResp {
        seed: SeedItemRef {
            qualified_name: seed_node.qualified_name,
            file: seed_file,
            span: seed_span,
            item_kind: seed_node.item_kind.map(|k| short_item_kind_label(k).to_string()),
        },
        limit,
        threshold,
        item_kind_filter: params.item_kind,
        match_count: matches.len(),
        matches,
    };
    json_result(&resp)
}

/// v1.1 "semantic_overlaps": workspace-wide duplicate-detection audit with
/// a per-Item embedding cache.
///
/// Algorithm (replaces v1.0's per-seed `vector_only_search` pipeline):
///   1. Enumerate seed Items (filter by crate / item_kind / file+span / tests).
///   2. For each seed: read source bytes, hash them (SHA-256 truncated to
///      16 bytes), look up `embeddings_by_target` — if hit AND content_hash
///      AND embedder_version match, reuse the cached vector; else mark for
///      embedding.
///   3. Batch-embed all cache misses via `EmbeddingGenerator::embed_documents`
///      in chunks of `EMBED_CHUNK`; persist each fresh vector to LMDB.
///   4. Identical-source short-circuit (v1.1c): items sharing a content_hash
///      get `score = 1.0` directly (skip cosine for that pair).
///   5. In-memory pairwise cosine over remaining (NodeId, vector) pairs.
///      O(N²) on embedder-dim vectors (default 1024 for Qwen3-0.6B) —
///      comfortable for a few thousand items.
///   6. Apply existing filters (cross_crate_only, skip_tests, threshold) and
///      dedupe symmetric edges via canonical (smaller-id-first) key.
///
/// Subsequent scans on unchanged code reuse cached vectors — only freshly
/// modified items pay the embedding cost. The cache lives in LMDB at the
/// `embeddings_by_target` sub-DB; `build_hypergraph --force_rebuild` clears
/// it (the new graph_id implies a fresh snapshot env).
pub(crate) async fn semantic_overlaps(
    params: SemanticOverlapsParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    let backend = resolve_graph_tool_backend(
        params.embedding_profile.as_deref(),
        &directory,
    )?;
    // Default cutoff is model-derived: cosine-similarity scales differ
    // between embedding models, and `ensure_embeddings_for` embeds with
    // `backend`, so the default threshold is sourced from the same model.
    let threshold = params
        .threshold
        .unwrap_or_else(|| backend.semantic_overlap_threshold());
    let limit = params.max_pairs.unwrap_or(50);
    let offset = params.offset.unwrap_or(0);
    let summary = params.summary.unwrap_or(false);
    let max_cluster_size = params.max_cluster_size.unwrap_or(15);
    let output_mode = params
        .output_mode
        .as_deref()
        .unwrap_or("clusters")
        .to_string();
    if output_mode != "pairs" && output_mode != "clusters" {
        return Err(McpError::invalid_params(
            format!(
                "output_mode must be \"pairs\" or \"clusters\"; got `{output_mode}`"
            ),
            None,
        ));
    }
    let skip_tests = params.skip_test_chunks.unwrap_or(true);
    let cross_crate_only = params.cross_crate_only.unwrap_or(false);
    let item_kind_filter_label = params.item_kind.clone();
    let crate_name = params.crate_name.clone();

    // 1. Open snapshot.
    let snap = open_workspace_snapshot(&directory)?;

    // 2. Resolve crate scope (if any).
    let crate_id_filter: Option<NodeId> = if let Some(qn) = &crate_name {
        let (id, node) = snap
            .lookup_by_qualified_name(qn)
            .map_err(internal_error("lookup_by_qualified_name"))?
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("no node found for qualified name `{qn}`"),
                    None,
                )
            })?;
        Some(match node.kind {
            NodeKind::Crate => id,
            NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
                McpError::invalid_params(
                    format!("`{qn}` resolves to a Module with no crate_id"),
                    None,
                )
            })?,
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "`{qn}` is a {other:?}, expected a Crate or its root Module"
                    ),
                    None,
                ));
            }
        })
    } else {
        None
    };

    let item_kind_enum = parse_item_kind_filter(item_kind_filter_label.as_deref())?;

    // 3. Enumerate seed Items: NodeKind::Item with optional crate/item_kind
    //    filters; require file+span (skip synthetic/macro-generated).
    let mut seeds: Vec<(NodeId, Node)> = Vec::new();
    {
        let rtxn = snap
            .env
            .read_txn()
            .map_err(|e| McpError::internal_error(format!("read_txn: {e}"), None))?;
        for entry in snap
            .dbs
            .nodes_by_id
            .iter(&rtxn)
            .map_err(|e| McpError::internal_error(format!("nodes_by_id.iter: {e}"), None))?
        {
            let (key, node) = entry
                .map_err(|e| McpError::internal_error(format!("nodes_by_id entry: {e}"), None))?;
            if node.kind != NodeKind::Item {
                continue;
            }
            if let Some(cid) = crate_id_filter {
                if node.crate_id != Some(cid) {
                    continue;
                }
            }
            if let Some(want_kind) = item_kind_enum {
                if node.item_kind != Some(want_kind) {
                    continue;
                }
            }
            if node.file.is_none() || node.span.is_none() {
                continue;
            }
            if skip_tests && node.qualified_name.contains("::tests::") {
                continue;
            }
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            seeds.push((NodeId(id), node));
        }
    }

    // 4. Resolve embeddings via the shared helper. It performs the read
    //    pass (hash + cache lookup), the async batched embed, and the
    //    write pass for fresh vectors. After the call, `embeddings`
    //    contains one entry per seed whose source was readable +
    //    non-empty + spannable.
    let seed_nids: Vec<NodeId> = seeds.iter().map(|(id, _)| *id).collect();
    let embeddings = crate::graph::ensure_embeddings_for(&snap, &seed_nids, &backend)
        .await
        .map_err(internal_error("ensure_embeddings_for"))?;

    // Per-seed context retained for the pairwise pass: id, node, hash,
    // vector. Items the helper skipped (unreadable file, empty source,
    // out-of-range span) are dropped here.
    struct SeedCtx {
        id: NodeId,
        node: Node,
        content_hash: [u8; 16],
        cached_vec: Option<Vec<f32>>,
    }

    let mut seeds_ctx: Vec<SeedCtx> = Vec::with_capacity(seeds.len());
    for (seed_id, seed_node) in seeds.drain(..) {
        if let Some(emb) = embeddings.get(&seed_id) {
            seeds_ctx.push(SeedCtx {
                id: seed_id,
                node: seed_node,
                content_hash: emb.content_hash,
                cached_vec: Some(emb.vector.clone()),
            });
        }
    }

    // 5. Edge accumulator. For symmetric dedup we use a canonical
    //    (smaller-id-first) key.
    type EdgeKey = (NodeId, NodeId);
    let mut edges: HashMap<EdgeKey, Vec<f32>> = HashMap::new();

    let canonical = |a: NodeId, b: NodeId| -> EdgeKey {
        if a.as_bytes() < b.as_bytes() {
            (a, b)
        } else {
            (b, a)
        }
    };

    // 6. v1.1c — identical-source short-circuit. Items sharing a content_hash
    //    get score=1.0 directly (subject to existing filters); skip the
    //    cosine pass for those pairs.
    let mut by_hash: HashMap<[u8; 16], Vec<usize>> = HashMap::new();
    for (i, ctx) in seeds_ctx.iter().enumerate() {
        if ctx.cached_vec.is_some() {
            by_hash.entry(ctx.content_hash).or_default().push(i);
        }
    }
    for indices in by_hash.values() {
        if indices.len() < 2 {
            continue;
        }
        for ai in 0..indices.len() {
            let a = &seeds_ctx[indices[ai]];
            for bi in (ai + 1)..indices.len() {
                let b = &seeds_ctx[indices[bi]];
                if cross_crate_only && a.node.crate_id == b.node.crate_id {
                    continue;
                }
                // skip_tests was already enforced during seed enumeration.
                let key = canonical(a.id, b.id);
                edges.entry(key).or_default().push(1.0);
            }
        }
    }

    // 7. In-memory pairwise cosine. O(N²) on embedder-dim vectors
    //    (default 1024 for Qwen3-0.6B). Identical-hash pairs are
    //    skipped here (already handled above with score=1.0).
    for i in 0..seeds_ctx.len() {
        let Some(va) = seeds_ctx[i].cached_vec.as_ref() else {
            continue;
        };
        for j in (i + 1)..seeds_ctx.len() {
            let Some(vb) = seeds_ctx[j].cached_vec.as_ref() else {
                continue;
            };
            let a = &seeds_ctx[i];
            let b = &seeds_ctx[j];
            if a.content_hash == b.content_hash {
                continue;
            }
            if cross_crate_only && a.node.crate_id == b.node.crate_id {
                continue;
            }
            let score = crate::graph::cosine(va, vb);
            if score < threshold {
                continue;
            }
            let key = canonical(a.id, b.id);
            edges.entry(key).or_default().push(score);
        }
    }

    // 8. Symmetric dedup: average the per-direction scores.
    let mut pairs: Vec<(NodeId, NodeId, f32)> = edges
        .into_iter()
        .map(|((a, b), scores)| {
            let avg = scores.iter().sum::<f32>() / scores.len() as f32;
            (a, b, avg)
        })
        .collect();
    pairs.sort_by(|x, y| y.2.partial_cmp(&x.2).unwrap_or(std::cmp::Ordering::Equal));
    let total_pair_count = pairs.len();

    // 9. Build response. v1.1 only ever produces edges between seeds, so
    //    the lookup table is the seeds themselves — no fallback `node_by_id`
    //    read needed.
    let seed_count = seeds_ctx.len();
    let seed_index: HashMap<NodeId, &Node> =
        seeds_ctx.iter().map(|c| (c.id, &c.node)).collect();
    let lookup_ref = |id: NodeId| -> Option<ItemRef> {
        seed_index
            .get(&id)
            .map(|node| node_to_item_ref(node, summary))
    };

    let scope = ScopeSummary {
        directory: directory.clone(),
        crate_name: crate_name.clone(),
        item_kind: item_kind_filter_label.clone(),
        seed_count,
    };

    let mut clusters = build_clusters(&pairs, usize::MAX, lookup_ref);
    if max_cluster_size > 0 {
        clusters.retain(|c| c.size <= max_cluster_size);
    }
    let total_cluster_count = clusters.len();

    if output_mode == "pairs" {
        let pair_refs: Vec<SimilarityPair> = pairs
            .into_iter()
            .skip(offset)
            .take(limit)
            .filter_map(|(a, b, s)| {
                Some(SimilarityPair {
                    a: lookup_ref(a)?,
                    b: lookup_ref(b)?,
                    similarity: s,
                })
            })
            .collect();
        return json_result(&SemanticOverlapsResp {
            scope,
            threshold,
            pair_count: total_pair_count,
            total_pair_count,
            total_cluster_count,
            offset,
            limit,
            summary,
            output_mode,
            pairs: Some(pair_refs),
            clusters: None,
        });
    }

    // Clusters mode (default).
    let clusters = page_clusters_by_member_limit(clusters, offset, limit);
    json_result(&SemanticOverlapsResp {
        scope,
        threshold,
        pair_count: total_pair_count,
        total_pair_count,
        total_cluster_count,
        offset,
        limit,
        summary,
        output_mode,
        pairs: None,
        clusters: Some(clusters),
    })
}

/// Resolve an optional `embedding_profile` argument into an
/// `EmbeddingBackend`, falling back to the default profile when unset.
///
/// Shared by the hypergraph-backed similarity tools (`similar_to_item`,
/// `semantic_overlaps`). A profile name is resolved against the registry
/// — built-ins plus any `embedding_profiles.toml` in `directory`.
fn resolve_graph_tool_backend(
    embedding_profile: Option<&str>,
    directory: &str,
) -> Result<crate::embeddings::EmbeddingBackend, McpError> {
    crate::mcp::project_paths::resolve_embedding_backend(
        embedding_profile,
        Path::new(directory),
    )
    .map_err(|msg| McpError::invalid_params(msg, None))
}

// ----- response shapes -----

#[derive(Debug, Serialize)]
pub(crate) struct SimilarToItemResp {
    pub(crate) seed: SeedItemRef,
    pub(crate) limit: usize,
    pub(crate) threshold: f32,
    pub(crate) item_kind_filter: Option<String>,
    pub(crate) match_count: usize,
    pub(crate) matches: Vec<SimilarMatch>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SeedItemRef {
    pub(crate) qualified_name: String,
    pub(crate) file: String,
    pub(crate) span: (u32, u32),
    /// Short label form (e.g. `"Fn"`, `"Struct"`); `None` when the seed Node
    /// has no `item_kind` (e.g. it's a Module).
    pub(crate) item_kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SimilarMatch {
    pub(crate) similarity: f32,
    pub(crate) symbol_name: String,
    pub(crate) symbol_kind: String,
    pub(crate) file: String,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    /// First 3 lines of `chunk.content` joined with `\n`.
    pub(crate) preview: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct SemanticOverlapsResp {
    pub(crate) scope: ScopeSummary,
    pub(crate) threshold: f32,
    /// Back-compatible alias for `total_pair_count`.
    pub(crate) pair_count: usize,
    pub(crate) total_pair_count: usize,
    pub(crate) total_cluster_count: usize,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) summary: bool,
    pub(crate) output_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pairs: Option<Vec<SimilarityPair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) clusters: Option<Vec<SimilarityCluster>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ScopeSummary {
    pub(crate) directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) crate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) item_kind: Option<String>,
    pub(crate) seed_count: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct SimilarityPair {
    pub(crate) a: ItemRef,
    pub(crate) b: ItemRef,
    pub(crate) similarity: f32,
}

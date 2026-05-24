//! Query methods on `OpenedSnapshot` - similarity family.
//!
//! Owns workspace-wide semantic overlap mechanics: graph item enumeration,
//! embedding-cache refresh, cosine scoring, and similarity DTO rendering.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use rmc_engine::embeddings::EmbeddingBackend;

use super::super::embedding_cache::ensure_embeddings_for;
use super::super::ids::NodeId;
use super::super::labels::item_kind_short_label;
use super::super::math::cosine;
use super::super::model::{ItemKind, Node, NodeKind};
use super::super::snapshot::{OpenedSnapshot, open_current};
use super::super::storage::{GraphEnvOptions, GraphPaths};
use super::model::{
    SemanticOverlapScope, SemanticOverlapsOutput, SimilarityCluster, SimilarityItem,
    SimilarityPair,
};

#[derive(Debug, Clone, Default)]
pub struct SemanticOverlapOptions {
    pub threshold: Option<f32>,
    pub max_pairs: Option<usize>,
    pub offset: Option<usize>,
    pub summary: Option<bool>,
    pub max_cluster_size: Option<usize>,
    pub output_mode: Option<String>,
    pub skip_test_chunks: Option<bool>,
    pub cross_crate_only: Option<bool>,
    pub item_kind: Option<String>,
    pub crate_name: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum GraphSimilarityError {
    #[error("failed to canonicalize {directory}: {source}")]
    InvalidDirectory {
        directory: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no snapshot at {directory}")]
    MissingSnapshot { directory: PathBuf },
    #[error("no node found for qualified name `{0}`")]
    UnknownCrateFilter(String),
    #[error("`{name}` is a {kind:?}, expected a Crate or its root Module")]
    InvalidCrateFilterKind { name: String, kind: NodeKind },
    #[error("`{name}` resolves to a Module with no crate_id")]
    ModuleMissingCrateId { name: String },
    #[error("unknown item_kind `{0}`; expected Function | Struct | Enum | Union | Trait | TypeAlias | Const | Static | AssocFunction | AssocConst | AssocType | Method | EnumVariant")]
    InvalidItemKindFilter(String),
    #[error("output_mode must be \"pairs\" or \"clusters\"; got `{0}`")]
    InvalidOutputMode(String),
}

pub async fn run_semantic_overlaps(
    directory: &Path,
    backend: &EmbeddingBackend,
    options: SemanticOverlapOptions,
) -> Result<SemanticOverlapsOutput> {
    let canonical = canonicalize_directory(directory)?;
    let snap = open_directory_snapshot(&canonical)?;
    let threshold = options
        .threshold
        .unwrap_or_else(|| backend.semantic_overlap_threshold());
    let limit = options.max_pairs.unwrap_or(50);
    let offset = options.offset.unwrap_or(0);
    let summary = options.summary.unwrap_or(false);
    let max_cluster_size = options.max_cluster_size.unwrap_or(15);
    let output_mode = options
        .output_mode
        .unwrap_or_else(|| "clusters".to_string());
    if output_mode != "pairs" && output_mode != "clusters" {
        return Err(GraphSimilarityError::InvalidOutputMode(output_mode).into());
    }
    let skip_tests = options.skip_test_chunks.unwrap_or(true);
    let cross_crate_only = options.cross_crate_only.unwrap_or(false);
    let item_kind_filter_label = options.item_kind;
    let crate_name = options.crate_name;

    let crate_id_filter = resolve_crate_filter(&snap, crate_name.as_deref())?;
    let item_kind_enum = parse_item_kind_filter(item_kind_filter_label.as_deref())?;
    let mut seeds = enumerate_similarity_seeds(
        &snap,
        crate_id_filter,
        item_kind_enum,
        skip_tests,
    )?;
    let seed_nids: Vec<NodeId> = seeds.iter().map(|(id, _)| *id).collect();
    let embeddings = ensure_embeddings_for(&snap, &seed_nids, backend).await?;

    struct SeedCtx {
        id: NodeId,
        node: Node,
        content_hash: [u8; 16],
        cached_vec: Vec<f32>,
    }

    let mut seeds_ctx = Vec::with_capacity(seeds.len());
    for (seed_id, seed_node) in seeds.drain(..) {
        if let Some(embedding) = embeddings.get(&seed_id) {
            seeds_ctx.push(SeedCtx {
                id: seed_id,
                node: seed_node,
                content_hash: embedding.content_hash,
                cached_vec: embedding.vector.clone(),
            });
        }
    }

    let mut edges: HashMap<(NodeId, NodeId), Vec<f32>> = HashMap::new();
    let canonical_edge = |a: NodeId, b: NodeId| -> (NodeId, NodeId) {
        if a.as_bytes() < b.as_bytes() {
            (a, b)
        } else {
            (b, a)
        }
    };

    let mut by_hash: HashMap<[u8; 16], Vec<usize>> = HashMap::new();
    for (i, ctx) in seeds_ctx.iter().enumerate() {
        by_hash.entry(ctx.content_hash).or_default().push(i);
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
                let key = canonical_edge(a.id, b.id);
                edges.entry(key).or_default().push(1.0);
            }
        }
    }

    for i in 0..seeds_ctx.len() {
        let va = &seeds_ctx[i].cached_vec;
        for j in (i + 1)..seeds_ctx.len() {
            let a = &seeds_ctx[i];
            let b = &seeds_ctx[j];
            if a.content_hash == b.content_hash {
                continue;
            }
            if cross_crate_only && a.node.crate_id == b.node.crate_id {
                continue;
            }
            let score = cosine(va, &b.cached_vec);
            if score < threshold {
                continue;
            }
            let key = canonical_edge(a.id, b.id);
            edges.entry(key).or_default().push(score);
        }
    }

    let mut pairs: Vec<(NodeId, NodeId, f32)> = edges
        .into_iter()
        .map(|((a, b), scores)| {
            let avg = scores.iter().sum::<f32>() / scores.len() as f32;
            (a, b, avg)
        })
        .collect();
    pairs.sort_by(|x, y| y.2.partial_cmp(&x.2).unwrap_or(std::cmp::Ordering::Equal));
    let total_pair_count = pairs.len();

    let seed_count = seeds_ctx.len();
    let seed_index: HashMap<NodeId, &Node> =
        seeds_ctx.iter().map(|ctx| (ctx.id, &ctx.node)).collect();
    let lookup_ref = |id: NodeId| -> Option<SimilarityItem> {
        seed_index
            .get(&id)
            .map(|node| node_to_similarity_item(node, summary))
    };

    let mut clusters = build_clusters(&pairs, usize::MAX, lookup_ref);
    if max_cluster_size > 0 {
        clusters.retain(|cluster| cluster.size <= max_cluster_size);
    }
    let total_cluster_count = clusters.len();

    let scope = SemanticOverlapScope {
        directory: directory.to_string_lossy().to_string(),
        crate_name,
        item_kind: item_kind_filter_label,
        seed_count,
    };

    if output_mode == "pairs" {
        let pair_refs: Vec<SimilarityPair> = pairs
            .into_iter()
            .skip(offset)
            .take(limit)
            .filter_map(|(a, b, similarity)| {
                Some(SimilarityPair {
                    a: lookup_ref(a)?,
                    b: lookup_ref(b)?,
                    similarity,
                })
            })
            .collect();
        return Ok(SemanticOverlapsOutput {
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

    let clusters = page_clusters_by_member_limit(clusters, offset, limit);
    Ok(SemanticOverlapsOutput {
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

fn canonicalize_directory(directory: &Path) -> Result<PathBuf> {
    directory
        .canonicalize()
        .map_err(|source| GraphSimilarityError::InvalidDirectory {
            directory: directory.to_path_buf(),
            source,
        }
        .into())
}

fn open_directory_snapshot(directory: &Path) -> Result<OpenedSnapshot> {
    let paths = GraphPaths::for_workspace(directory);
    match open_current(&paths, GraphEnvOptions::default())? {
        Some(snapshot) => Ok(snapshot),
        None => Err(GraphSimilarityError::MissingSnapshot {
            directory: directory.to_path_buf(),
        }
        .into()),
    }
}

fn resolve_crate_filter(snap: &OpenedSnapshot, crate_name: Option<&str>) -> Result<Option<NodeId>> {
    let Some(qn) = crate_name else {
        return Ok(None);
    };
    let (id, node) = snap
        .lookup_by_qualified_name(qn)?
        .ok_or_else(|| GraphSimilarityError::UnknownCrateFilter(qn.to_owned()))?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node
            .crate_id
            .or(node.parent_id)
            .ok_or_else(|| GraphSimilarityError::ModuleMissingCrateId {
                name: qn.to_owned(),
            })?,
        other => {
            return Err(GraphSimilarityError::InvalidCrateFilterKind {
                name: qn.to_owned(),
                kind: other,
            }
            .into());
        }
    };
    Ok(Some(crate_id))
}

fn parse_item_kind_filter(raw: Option<&str>) -> Result<Option<ItemKind>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    let kind = match raw.to_ascii_lowercase().as_str() {
        "function" | "fn" => ItemKind::Function,
        "struct" => ItemKind::Struct,
        "enum" => ItemKind::Enum,
        "union" => ItemKind::Union,
        "trait" => ItemKind::Trait,
        "typealias" | "type_alias" | "type" => ItemKind::TypeAlias,
        "const" => ItemKind::Const,
        "static" => ItemKind::Static,
        "assocfunction" | "assocfn" | "assoc_function" => ItemKind::AssocFunction,
        "assocconst" | "assoc_const" => ItemKind::AssocConst,
        "assoctype" | "assoc_type" => ItemKind::AssocType,
        "method" => ItemKind::Method,
        "enumvariant" | "enum_variant" | "variant" => ItemKind::EnumVariant,
        other => return Err(GraphSimilarityError::InvalidItemKindFilter(other.to_string()).into()),
    };
    Ok(Some(kind))
}

fn enumerate_similarity_seeds(
    snap: &OpenedSnapshot,
    crate_id_filter: Option<NodeId>,
    item_kind_filter: Option<ItemKind>,
    skip_tests: bool,
) -> Result<Vec<(NodeId, Node)>> {
    let rtxn = snap.env.read_txn()?;
    let mut seeds = Vec::new();
    for entry in snap.dbs.nodes_by_id.iter(&rtxn)? {
        let (key, node) = entry?;
        if node.kind != NodeKind::Item {
            continue;
        }
        if let Some(crate_id) = crate_id_filter {
            if node.crate_id != Some(crate_id) {
                continue;
            }
        }
        if let Some(want_kind) = item_kind_filter {
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
    Ok(seeds)
}

fn node_to_similarity_item(node: &Node, summary: bool) -> SimilarityItem {
    SimilarityItem {
        qualified_name: node.qualified_name.clone(),
        item_kind: node.item_kind.map(|kind| item_kind_short_label(kind).to_string()),
        file: if summary {
            None
        } else {
            Some(node.file.clone().unwrap_or_default())
        },
        span: if summary {
            None
        } else {
            Some(node.span.unwrap_or((0, 0)))
        },
    }
}

fn build_clusters<F>(
    edges: &[(NodeId, NodeId, f32)],
    max_members: usize,
    lookup: F,
) -> Vec<SimilarityCluster>
where
    F: Fn(NodeId) -> Option<SimilarityItem>,
{
    let mut nodes = Vec::new();
    let mut seen = HashMap::new();
    for (a, b, _) in edges {
        if !seen.contains_key(a) {
            seen.insert(*a, nodes.len());
            nodes.push(*a);
        }
        if !seen.contains_key(b) {
            seen.insert(*b, nodes.len());
            nodes.push(*b);
        }
    }
    let n = nodes.len();
    if n == 0 {
        return Vec::new();
    }

    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]];
            x = parent[x];
        }
        x
    }
    for (a, b, _) in edges {
        let ra = find(&mut parent, seen[a]);
        let rb = find(&mut parent, seen[b]);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }

    let mut clusters = Vec::new();
    for (_root, group) in groups {
        if group.len() < 2 {
            continue;
        }
        let group_set: std::collections::HashSet<usize> = group.iter().copied().collect();
        let mut group_scores = Vec::new();
        for (a, b, score) in edges {
            let ai = seen[a];
            let bi = seen[b];
            if group_set.contains(&ai) && group_set.contains(&bi) {
                group_scores.push(*score);
            }
        }
        if group_scores.is_empty() {
            continue;
        }
        let sum: f32 = group_scores.iter().sum();
        let avg_similarity = sum / group_scores.len() as f32;
        let mut min_similarity = group_scores[0];
        for score in &group_scores[1..] {
            if *score < min_similarity {
                min_similarity = *score;
            }
        }

        let size = group.len();
        let truncated = size > max_members;
        let take_n = max_members.min(size);
        let members = group
            .into_iter()
            .take(take_n)
            .filter_map(|i| lookup(nodes[i]))
            .collect();

        clusters.push(SimilarityCluster {
            members,
            avg_similarity,
            min_similarity,
            size,
            truncated,
        });
    }

    clusters.sort_by(|a, b| {
        b.avg_similarity
            .partial_cmp(&a.avg_similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| {
                b.min_similarity
                    .partial_cmp(&a.min_similarity)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    clusters
}

fn page_clusters_by_member_limit(
    clusters: Vec<SimilarityCluster>,
    offset: usize,
    member_limit: usize,
) -> Vec<SimilarityCluster> {
    let mut remaining = member_limit;
    let mut paged = Vec::new();
    for mut cluster in clusters.into_iter().skip(offset) {
        if remaining == 0 {
            break;
        }
        if cluster.members.len() > remaining {
            cluster.members.truncate(remaining);
            cluster.truncated = true;
            paged.push(cluster);
            break;
        }
        remaining -= cluster.members.len();
        paged.push(cluster);
    }
    paged
}

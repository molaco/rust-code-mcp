//! Cross-family helpers shared by the graph endpoint families.
//!
//! Pagination, snapshot opening, error mapping, JSON serialization, and a
//! handful of small parsing/range helpers that more than one endpoint
//! family needs. Each family's own module (`core`, future surface/audits/
//! similarity/codemap modules) re-imports these via `use
//! crate::tools::graph::response::*;`.

use std::collections::HashMap;
use std::path::Path;
use std::path::PathBuf;

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use serde::Serialize;

use crate::graph::labels::item_kind_short_label as short_item_kind_label;
use crate::graph::{
    BindingVisibility, GraphEnvOptions, GraphPaths, ItemKind, Node, NodeId, NodeKind,
    OpenedSnapshot, OverlapScope, open_current,
};
use crate::tools::params::ListPaginationParams;

pub(crate) const DEFAULT_LIST_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ListPage {
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) summary: bool,
}

#[derive(Debug, Serialize, Clone, Copy)]
pub(crate) struct ListMeta {
    pub(crate) total_match_count: usize,
    pub(crate) offset: usize,
    pub(crate) limit: usize,
    pub(crate) summary: bool,
    pub(crate) returned_match_count: usize,
}

pub(crate) fn list_page(params: &ListPaginationParams) -> ListPage {
    ListPage {
        offset: params.offset.unwrap_or(0),
        limit: params.limit.unwrap_or(DEFAULT_LIST_LIMIT),
        summary: params.summary.unwrap_or(false),
    }
}

pub(crate) fn page_list<T>(items: Vec<T>, page: ListPage) -> (ListMeta, Vec<T>) {
    let total_match_count = items.len();
    let paged: Vec<T> = items
        .into_iter()
        .skip(page.offset)
        .take(page.limit)
        .collect();
    let meta = ListMeta {
        total_match_count,
        offset: page.offset,
        limit: page.limit,
        summary: page.summary,
        returned_match_count: paged.len(),
    };
    (meta, paged)
}

pub(crate) fn clear_locations_for_summary<T>(
    items: &mut [T],
    summary: bool,
    mut clear: impl FnMut(&mut T),
) {
    if summary {
        for item in items {
            clear(item);
        }
    }
}

pub(crate) fn open_workspace_snapshot(directory: &str) -> Result<OpenedSnapshot, McpError> {
    let dir = PathBuf::from(directory);
    let canonical = dir.canonicalize().map_err(|e| {
        McpError::invalid_params(format!("failed to canonicalize {directory}: {e}"), None)
    })?;
    let paths = GraphPaths::for_workspace(&canonical);
    open_current(&paths, GraphEnvOptions::default())
        .map_err(internal_error("open_current"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no snapshot at {directory} — call build_hypergraph first"),
                None,
            )
        })
}

pub(crate) fn resolve_required_node(
    snap: &OpenedSnapshot,
    qualified_name: &str,
    expect_kind: NodeKind,
) -> Result<NodeId, McpError> {
    let (id, node) = snap
        .lookup_by_qualified_name(qualified_name)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{qualified_name}`"),
                None,
            )
        })?;
    if node.kind == expect_kind {
        return Ok(id);
    }
    // Transparent crate→root-module fallback: every Crate has a root Module
    // sharing its qualified_name, so when callers pass a crate name where a
    // module is expected (e.g., `consumer: "rust_code_mcp"`), promote the
    // lookup to that root module instead of failing.
    if expect_kind == NodeKind::Module && node.kind == NodeKind::Crate {
        if let Some(root_module_id) = snap
            .find_root_module_of(id)
            .map_err(internal_error("find_root_module_of"))?
        {
            return Ok(root_module_id);
        }
    }
    Err(McpError::invalid_params(
        format!(
            "`{qualified_name}` is a {:?}, expected {expect_kind:?}",
            node.kind
        ),
        None,
    ))
}

pub(crate) fn visibility_label(
    snap: &OpenedSnapshot,
    rtxn: &heed::RoTxn<'_, heed::WithoutTls>,
    vis: &BindingVisibility,
) -> String {
    match vis {
        BindingVisibility::Public => "pub".to_string(),
        BindingVisibility::Private => "private".to_string(),
        BindingVisibility::Crate(id) => match snap.node_by_id(rtxn, *id).ok().flatten() {
            Some(node) => format!("pub(crate={})", node.qualified_name),
            None => "pub(crate)".to_string(),
        },
        BindingVisibility::RestrictedTo(id) => match snap.node_by_id(rtxn, *id).ok().flatten() {
            Some(node) => format!("pub(in {})", node.qualified_name),
            None => "pub(in ?)".to_string(),
        },
    }
}

pub(crate) fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

pub(crate) fn internal_error(label: &'static str) -> impl Fn(anyhow::Error) -> McpError {
    move |e| McpError::internal_error(format!("{label}: {e:#}"), None)
}

/// Parse the user-supplied `item_kind` filter string into an `Option<ItemKind>`.
/// Case-insensitive. None → no filter. Unknown variants return an
/// `invalid_params` error.
pub(crate) fn parse_item_kind_filter(s: Option<&str>) -> Result<Option<ItemKind>, McpError> {
    let Some(raw) = s else {
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
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "unknown item_kind `{other}`; expected Function | Struct | Enum | Union | Trait | TypeAlias | Const | Static | AssocFunction | AssocConst | AssocType | Method | EnumVariant"
                ),
                None,
            ));
        }
    };
    Ok(Some(kind))
}

pub(crate) fn parse_overlap_scope(input: Option<&str>) -> Result<OverlapScope, McpError> {
    match input.unwrap_or("all") {
        "all" => Ok(OverlapScope::All),
        "local" => Ok(OverlapScope::Local),
        "local_no_vendor" => Ok(OverlapScope::LocalNoVendor),
        other => Err(McpError::invalid_params(
            format!("unknown scope `{other}`; expected all | local | local_no_vendor"),
            None,
        )),
    }
}

/// Pure helper: returns true iff `[a_start, a_end]` and `[b_start, b_end]`
/// overlap as inclusive line ranges. Extracted for unit testing.
///
/// As of v1.1 of `semantic_overlaps` no production caller uses this — kept
/// alive by tests and as a reachable helper for `resolve_chunk_to_item` so
/// future tools can re-introduce chunk → Item resolution.
#[allow(dead_code)]
pub(crate) fn line_range_overlaps(a_start: u32, a_end: u32, b_start: u32, b_end: u32) -> bool {
    a_start <= b_end && a_end >= b_start
}

/// Map a vector-store chunk's `(file, line_range)` to a hypergraph Item NodeId.
///
/// `chunk_file` is normally absolute (vector store stores absolute paths).
/// `snap` carries workspace-relative paths on each Node. We do a component-aware
/// suffix match via `chunk_file.ends_with(node.file)` (mirrors the v0.1
/// self-match fix in `similar_to_item`). `file_contents_cache` caches file
/// contents keyed by absolute path string so repeated chunk lookups in the same
/// file pay just one I/O.
///
/// Returns the first Item whose byte-span-derived line range overlaps
/// `[chunk_line_start, chunk_line_end]`. None if no Item matches.
///
/// As of v1.1, `semantic_overlaps` no longer routes through chunk → Item
/// resolution (it embeds Item source directly), so this helper has no
/// production caller. Retained for future tools that bridge the vector
/// store with the hypergraph.
#[allow(dead_code)]
pub(crate) fn resolve_chunk_to_item(
    snap: &OpenedSnapshot,
    chunk_file: &Path,
    chunk_line_start: u32,
    chunk_line_end: u32,
    file_contents_cache: &mut HashMap<String, String>,
) -> Option<(NodeId, Node)> {
    let rtxn = snap.env.read_txn().ok()?;
    for entry in snap.dbs.nodes_by_id.iter(&rtxn).ok()? {
        let (key, node) = entry.ok()?;
        if node.kind != NodeKind::Item {
            continue;
        }
        let Some(rel_file) = node.file.as_deref() else {
            continue;
        };
        let Some(span) = node.span else {
            continue;
        };
        let rel_path = Path::new(rel_file);
        if !chunk_file.ends_with(rel_path) {
            continue;
        }
        // Derive the workspace root from the absolute chunk_file so we can
        // resolve other Items in the same file from cached content.
        // We use chunk_file as the absolute key for the cache.
        let chunk_file_key = chunk_file.to_string_lossy().to_string();
        if !file_contents_cache.contains_key(&chunk_file_key) {
            match std::fs::read_to_string(chunk_file) {
                Ok(s) => {
                    file_contents_cache.insert(chunk_file_key.clone(), s);
                }
                Err(_) => continue,
            }
        }
        let content = file_contents_cache.get(&chunk_file_key)?;
        let (start, end) = (span.0 as usize, span.1 as usize);
        if start > content.len() || end > content.len() || start > end {
            continue;
        }
        let item_line_start = content[..start].matches('\n').count() as u32 + 1;
        let item_line_end = content[..end].matches('\n').count() as u32 + 1;
        if line_range_overlaps(item_line_start, item_line_end, chunk_line_start, chunk_line_end) {
            let mut id = [0u8; 32];
            id.copy_from_slice(key);
            return Some((NodeId(id), node));
        }
    }
    None
}

// ----- shared similarity / codemap helpers (cross-family) -----

#[derive(Debug, Serialize)]
pub(crate) struct ItemRef {
    pub(crate) qualified_name: String,
    pub(crate) item_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) span: Option<(u32, u32)>,
}

#[derive(Debug, Serialize)]
pub(crate) struct SimilarityCluster {
    pub(crate) members: Vec<ItemRef>,
    pub(crate) avg_similarity: f32,
    pub(crate) min_similarity: f32,
    pub(crate) size: usize,
    pub(crate) truncated: bool,
}

/// Build a small ItemRef from a Node (used in semantic_overlaps response).
pub(crate) fn node_to_item_ref(node: &Node, summary: bool) -> ItemRef {
    ItemRef {
        qualified_name: node.qualified_name.clone(),
        item_kind: node.item_kind.map(|k| short_item_kind_label(k).to_string()),
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

/// Single-linkage clustering via union-find. Each edge unions its two endpoints
/// and contributes its score to the resulting cluster's score statistics.
/// Singleton groups are dropped. Sort: by avg_similarity desc, then size desc,
/// then min_similarity desc.
/// Each cluster's member list is capped at `max_members` (sets `truncated=true`
/// when the cap kicks in).
pub(crate) fn build_clusters<F>(
    edges: &[(NodeId, NodeId, f32)],
    max_members: usize,
    lookup: F,
) -> Vec<SimilarityCluster>
where
    F: Fn(NodeId) -> Option<ItemRef>,
{
    // Collect node set.
    let mut nodes: Vec<NodeId> = Vec::new();
    let mut seen: HashMap<NodeId, usize> = HashMap::new();
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

    // Union-find with path compression.
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

    // Group node indices by root.
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = find(&mut parent, i);
        groups.entry(r).or_default().push(i);
    }

    // For each group, collect score stats from the subset of edges whose
    // endpoints both fall in this group.
    let mut clusters: Vec<SimilarityCluster> = Vec::new();
    for (_root, group) in groups {
        if group.len() < 2 {
            continue;
        }
        let group_set: std::collections::HashSet<usize> = group.iter().copied().collect();
        let mut group_scores: Vec<f32> = Vec::new();
        for (a, b, s) in edges {
            let ai = seen[a];
            let bi = seen[b];
            if group_set.contains(&ai) && group_set.contains(&bi) {
                group_scores.push(*s);
            }
        }
        if group_scores.is_empty() {
            continue;
        }
        let sum: f32 = group_scores.iter().sum();
        let avg = sum / group_scores.len() as f32;
        let mut min_sim = group_scores[0];
        for s in &group_scores[1..] {
            if *s < min_sim {
                min_sim = *s;
            }
        }

        let size = group.len();
        let truncated = size > max_members;
        let take_n = max_members.min(size);
        let members: Vec<ItemRef> = group
            .into_iter()
            .take(take_n)
            .filter_map(|i| lookup(nodes[i]))
            .collect();

        clusters.push(SimilarityCluster {
            members,
            avg_similarity: avg,
            min_similarity: min_sim,
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

/// Apply `semantic_overlaps` cluster pagination. `offset` skips complete
/// clusters, then `member_limit` caps the total number of emitted member refs
/// across all returned clusters.
pub(crate) fn page_clusters_by_member_limit(
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

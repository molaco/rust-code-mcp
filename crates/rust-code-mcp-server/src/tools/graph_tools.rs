//! MCP tools backed by the persisted workspace hypergraph.
//!
//! All five tools share the same shape:
//!   1. Resolve `directory` to a `GraphPaths`.
//!   2. Open the current snapshot (or build it for `build_hypergraph`).
//!   3. Resolve user-supplied qualified names to `NodeId`s.
//!   4. Run the corresponding `OpenedSnapshot` query.
//!   5. Serialize the result as JSON.
//!
//! The MCP layer never sees `NodeId`s — only qualified names in and out.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use serde::Serialize;

use rust_code_mcp_graph::{
    Binding, BindingKind, BindingVisibility, CallGraphNode, CrateDeadPub, CrateEdge, CrateMetric,
    DeadPubFinding, EmbeddingRecord, EnrichedCallSite, ForbiddenDependencyRule,
    ForbiddenDependencyViolation, FunctionFilter, FunctionSignature, FunctionWithSignature,
    GraphEnvOptions, GraphPaths, ItemKind, ItemWithAttribute, ModuleTreeNode, Namespace, Node, NodeId, NodeKind,
    OpenedSnapshot, OverlapsReport, PubTypeAliasMasqueradingAsReexport, ReExportChain,
    RecursiveCallersCount, SelfKindFilter, Usage, UsageCategory, UsageSummaryRow, WorkspaceStats,
    BuildOptions, build_and_persist, open_current,
};
use crate::tools::search_tool::{
    BuildHypergraphParams, CallGraphParams, CallersInCrateParams, CallsFromParams,
    CrateDependencyMetricParams, CrateEdgesParams, DeadPubParams, DeadPubReportParams,
    EnumVariantsParams, ForbiddenDependencyCheckParams, FunctionSignatureParams,
    FunctionsWithFilterParams, GraphDeclaredReexportsParams, GraphExportsParams, GraphImportsParams,
    GraphReexportsParams, ItemAttributesParams, ItemsWithAttributeParams, ModuleTreeParams,
    OverlapsParams, PubUsePubTypeAuditParams, ReExportChainParams, RecursiveCallersCountParams,
    SemanticOverlapsParams, SimilarToItemParams, WhoCallsParams, WhoImportsParams, WhoUsesParams,
    WhoUsesSummaryParams, WorkspaceStatsParams,
};

pub async fn build_hypergraph(
    params: BuildHypergraphParams,
) -> Result<CallToolResult, McpError> {
    let dir = PathBuf::from(&params.directory);
    if !dir.exists() {
        return Err(McpError::invalid_params(
            format!("directory does not exist: {}", params.directory),
            None,
        ));
    }
    let opts = BuildOptions {
        force_rebuild: params.force_rebuild.unwrap_or(false),
        ..Default::default()
    };
    // build_and_persist runs `loader::load` + the full extract pass + LMDB
    // writes synchronously (4-18s wall-clock). Hand off to a blocking thread
    // so the tokio runtime worker stays free to handle other tool calls.
    let result = tokio::task::spawn_blocking(move || build_and_persist(&dir, opts))
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))?
        .map_err(|e| McpError::internal_error(format!("build_hypergraph failed: {e:#}"), None))?;

    json_result(&BuildHypergraphResponse {
        graph_id: result.graph_id,
        workspace_root: result.workspace_root.display().to_string(),
        fingerprint: result.fingerprint,
        node_count: result.node_count,
        binding_count: result.binding_count,
        usage_count: result.usage_count,
        reused: result.reused,
        snapshot_path: result.snapshot_path.display().to_string(),
    })
}

pub async fn get_imports(params: GraphImportsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let bindings = snap
        .imports_of(module_id)
        .map_err(internal_error("imports_of"))?;
    let module_name = snap
        .lookup_by_qualified_name(&params.module)
        .ok()
        .flatten()
        .map(|(_, n)| n.qualified_name)
        .unwrap_or(params.module.clone());

    json_result(&BindingsListResponse {
        module: Some(module_name),
        consumer: None,
        target: None,
        bindings: enrich_bindings(&snap, bindings),
    })
}

pub async fn get_exports(params: GraphExportsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let consumer_id = resolve_required_node(&snap, &params.consumer, NodeKind::Module)?;
    let bindings = snap
        .exports_of(module_id, consumer_id)
        .map_err(internal_error("exports_of"))?;

    json_result(&BindingsListResponse {
        module: Some(params.module),
        consumer: Some(params.consumer),
        target: None,
        bindings: enrich_bindings(&snap, bindings),
    })
}

pub async fn get_reexports(params: GraphReexportsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let consumer_id = resolve_required_node(&snap, &params.consumer, NodeKind::Module)?;
    let bindings = snap
        .reexports_of(module_id, consumer_id)
        .map_err(internal_error("reexports_of"))?;

    json_result(&BindingsListResponse {
        module: Some(params.module),
        consumer: Some(params.consumer),
        target: None,
        bindings: enrich_bindings(&snap, bindings),
    })
}

pub async fn get_declared_reexports(
    params: GraphDeclaredReexportsParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let bindings = snap
        .declared_reexports_of(module_id)
        .map_err(internal_error("declared_reexports_of"))?;

    json_result(&BindingsListResponse {
        module: Some(params.module),
        consumer: None,
        target: None,
        bindings: enrich_bindings(&snap, bindings),
    })
}

pub async fn who_imports(params: WhoImportsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    // The target may be any node kind (Item, Module, ExternalSymbol).
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let bindings = snap
        .who_imports(target_id)
        .map_err(internal_error("who_imports"))?;

    json_result(&BindingsListResponse {
        module: None,
        consumer: None,
        target: Some(target_node.qualified_name),
        bindings: enrich_bindings(&snap, bindings),
    })
}

pub async fn who_uses(params: WhoUsesParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let usages = snap
        .usages_of(target_id)
        .map_err(internal_error("usages_of"))?;

    json_result(&UsagesListResponse {
        target: target_node.qualified_name,
        usages: enrich_usages(&snap, usages),
    })
}

pub async fn who_uses_summary(
    params: WhoUsesSummaryParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let rows = snap
        .who_uses_summary(target_id)
        .map_err(internal_error("who_uses_summary"))?;

    json_result(&UsageSummaryResponse {
        target: target_node.qualified_name,
        rows,
    })
}

pub async fn who_calls(params: WhoCallsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let sites = snap
        .who_calls(target_id)
        .map_err(internal_error("who_calls"))?;
    json_result(&CallSitesResponse {
        target: Some(target_node.qualified_name),
        caller: None,
        call_sites: sites,
    })
}

pub async fn calls_from(params: CallsFromParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (caller_id, caller_node) = snap
        .lookup_by_qualified_name(&params.caller)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.caller),
                None,
            )
        })?;
    let sites = snap
        .calls_from(caller_id)
        .map_err(internal_error("calls_from"))?;
    json_result(&CallSitesResponse {
        target: None,
        caller: Some(caller_node.qualified_name),
        call_sites: sites,
    })
}

pub async fn call_graph(params: CallGraphParams) -> Result<CallToolResult, McpError> {
    const DEFAULT_DEPTH: u32 = 3;
    const MAX_DEPTH: u32 = 8;
    let depth = params.depth.unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH);
    let snap = open_workspace_snapshot(&params.directory)?;
    let (root_id, root_node) = snap
        .lookup_by_qualified_name(&params.root)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.root),
                None,
            )
        })?;
    let tree: CallGraphNode = snap
        .call_graph(root_id, depth)
        .map_err(internal_error("call_graph"))?;
    json_result(&CallGraphResponse {
        root: root_node.qualified_name,
        depth,
        tree,
    })
}

pub async fn callers_in_crate(
    params: CallersInCrateParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let sites = snap
        .callers_in_crate(target_id, &params.krate)
        .map_err(internal_error("callers_in_crate"))?;
    json_result(&CallersInCrateResponse {
        target: target_node.qualified_name,
        krate: params.krate,
        call_sites: sites,
    })
}

pub async fn recursive_callers_count(
    params: RecursiveCallersCountParams,
) -> Result<CallToolResult, McpError> {
    const DEFAULT_DEPTH: u32 = 3;
    const MAX_DEPTH: u32 = 8;
    let depth = params.depth.unwrap_or(DEFAULT_DEPTH).min(MAX_DEPTH);
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, _target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let count: RecursiveCallersCount = snap
        .recursive_callers_count(target_id, depth)
        .map_err(internal_error("recursive_callers_count"))?;
    json_result(&count)
}

pub async fn dead_pub_in_crate(params: DeadPubParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;

    // Caller may pass a crate name (e.g. `my_crate`) or a crate root module name —
    // both resolve via `lookup_by_qualified_name`. Promote module → owning crate
    // if a Module came back so the rest of the function only handles Crate.
    let (id, node) = snap
        .lookup_by_qualified_name(&params.krate)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.krate),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node
            .crate_id
            .or(node.parent_id)
            .ok_or_else(|| {
                McpError::invalid_params(
                    format!("`{}` resolves to a Module with no crate_id", params.krate),
                    None,
                )
            })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.krate
                ),
                None,
            ));
        }
    };

    let findings = snap
        .dead_pub_in_crate(crate_id)
        .map_err(internal_error("dead_pub_in_crate"))?;

    json_result(&DeadPubResponse {
        krate: params.krate,
        findings: findings
            .into_iter()
            .map(|f| enrich_dead_pub(&snap, f))
            .collect(),
    })
}

pub async fn dead_pub_report(params: DeadPubReportParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let report = snap
        .dead_pub_report()
        .map_err(internal_error("dead_pub_report"))?;

    let crates: Vec<EnrichedCrateDeadPub> = report
        .into_iter()
        .map(|c| enrich_crate_dead_pub(&snap, c))
        .collect();
    let total: usize = crates.iter().map(|c| c.findings.len()).sum();
    json_result(&DeadPubReportResponse {
        workspace: params.directory,
        total_findings: total,
        crates,
    })
}

pub async fn crate_edges(params: CrateEdgesParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let edges: Vec<CrateEdge> = snap
        .crate_edges()
        .map_err(internal_error("crate_edges"))?;
    json_result(&CrateEdgesResponse { edges })
}

pub async fn enum_variants(params: EnumVariantsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (enum_id, enum_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    if enum_node.item_kind != Some(ItemKind::Enum) {
        return Err(McpError::invalid_params(
            format!(
                "`{}` is not an Enum (got {:?}); enum_variants only enumerates enum variants",
                params.target, enum_node.item_kind
            ),
            None,
        ));
    }
    let variants: Vec<Node> = snap
        .enum_variants(enum_id)
        .map_err(internal_error("enum_variants"))?;

    let enriched: Vec<EnrichedEnumVariant> = variants
        .into_iter()
        .map(|n| EnrichedEnumVariant {
            display_name: n.display_name,
            qualified_name: n.qualified_name,
            file: n.file,
            span: n.span,
        })
        .collect();
    json_result(&EnumVariantsResponse {
        enum_qualified_name: enum_node.qualified_name,
        variant_count: enriched.len(),
        variants: enriched,
    })
}

pub async fn item_attributes(
    params: ItemAttributesParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let attrs = snap
        .item_attributes(target_id)
        .map_err(internal_error("item_attributes"))?;
    json_result(&ItemAttributesResponse {
        target: target_node.qualified_name,
        item_kind: target_node.item_kind.map(item_kind_label),
        file: target_node.file,
        span: target_node.span,
        attribute_count: attrs.len(),
        attributes: attrs,
    })
}

pub async fn items_with_attribute(
    params: ItemsWithAttributeParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (id, node) = snap
        .lookup_by_qualified_name(&params.crate_name)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.crate_name),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "`{}` resolves to a Module with no crate_id",
                    params.crate_name
                ),
                None,
            )
        })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.crate_name
                ),
                None,
            ));
        }
    };
    let hits: Vec<ItemWithAttribute> = snap
        .items_with_attribute(crate_id, &params.attribute_pattern)
        .map_err(internal_error("items_with_attribute"))?;
    let enriched: Vec<EnrichedItemWithAttribute> = hits
        .into_iter()
        .map(|h| EnrichedItemWithAttribute {
            qualified_name: h.qualified_name,
            item_kind: h.item_kind.map(item_kind_label),
            matched_attribute: h.matched_attribute,
            match_location: h.match_location,
            file: h.file,
            span: h.span,
        })
        .collect();
    json_result(&ItemsWithAttributeResponse {
        krate: params.crate_name,
        attribute_pattern: params.attribute_pattern,
        match_count: enriched.len(),
        items: enriched,
    })
}

pub async fn function_signature(
    params: FunctionSignatureParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let signature = snap
        .function_signature(target_id)
        .map_err(internal_error("function_signature"))?;
    json_result(&FunctionSignatureResponse {
        target: target_node.qualified_name,
        signature,
    })
}

/// v0.1 "semantic overlaps": resolve `target` to a hypergraph Item, read its
/// source bytes from (file, span), and run vector_only_search using those
/// bytes as the query. Drops the seed's own chunk (file-path-only match — see
/// limitation note) and applies optional `threshold` / `item_kind` filters.
///
/// Limitation: self-match detection is file-path-only. If the seed file
/// contains other items that match the seed's source semantically, those
/// will be returned. A finer span-overlap check is left for v0.2.
pub async fn similar_to_item(
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

    // 3. Run vector-only search.
    let paths = crate::tools::project_paths::ProjectPaths::from_directory(Path::new(
        &params.directory,
    ));
    let hybrid_search = crate::tools::query_tools::create_hybrid_search(&paths, None).await?;

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
///   3. Batch-embed all cache misses via `EmbeddingGenerator::embed_batch_async`
///      in chunks of `EMBED_CHUNK`; persist each fresh vector to LMDB.
///   4. Identical-source short-circuit (v1.1c): items sharing a content_hash
///      get `score = 1.0` directly (skip cosine for that pair).
///   5. In-memory pairwise cosine over remaining (NodeId, vector) pairs.
///      O(N²) on 384-dim vectors — comfortable for a few thousand items.
///   6. Apply existing filters (cross_crate_only, skip_tests, threshold) and
///      dedupe symmetric edges via canonical (smaller-id-first) key.
///
/// Subsequent scans on unchanged code reuse cached vectors — only freshly
/// modified items pay the embedding cost. The cache lives in LMDB at the
/// `embeddings_by_target` sub-DB; `build_hypergraph --force_rebuild` clears
/// it (the new graph_id implies a fresh snapshot env).
pub async fn semantic_overlaps(
    params: SemanticOverlapsParams,
) -> Result<CallToolResult, McpError> {
    /// Stable identifier for the embedding model + dimension. Cache entries
    /// whose `embedder_version` does not match this string are treated as
    /// misses and refreshed. Bump when the embedder model or dimension
    /// changes.
    const EMBEDDER_VERSION: &str = "fastembed:all-MiniLM-L6-v2:dim384:v1";
    /// Max texts per `embed_batch_async` call. Keeps memory bounded when the
    /// workspace has thousands of seeds.
    const EMBED_CHUNK: usize = 64;

    let directory = params.directory.clone();
    let threshold = params.threshold.unwrap_or(0.85);
    let max_pairs = params.max_pairs.unwrap_or(50);
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

    // 4. v1.1: per-Item embedding cache + pairwise cosine.
    //
    // Per-seed context: id, node, content_hash, optional cached vector,
    // optional source text (Some only when the cache missed and we still
    // need to embed it). After the embedding pass, every retained ctx has
    // `cached_vec = Some(_)` and `source = None`.
    struct SeedCtx {
        id: NodeId,
        node: Node,
        content_hash: [u8; 16],
        cached_vec: Option<Vec<f32>>,
        source: Option<String>,
    }

    let mut seeds_ctx: Vec<SeedCtx> = Vec::new();
    let mut file_cache: HashMap<String, String> = HashMap::new();

    // First pass: read each seed's source, hash it, and check the cache.
    {
        use sha2::{Digest, Sha256};
        let rtxn = snap
            .env
            .read_txn()
            .map_err(|e| McpError::internal_error(format!("read_txn: {e}"), None))?;
        for (seed_id, seed_node) in seeds.drain(..) {
            let seed_file = seed_node.file.as_deref().expect("filtered above");
            let seed_span = seed_node.span.expect("filtered above");
            let abs_seed_path = PathBuf::from(&directory).join(seed_file);
            let abs_seed_str = abs_seed_path.to_string_lossy().to_string();

            if !file_cache.contains_key(&abs_seed_str) {
                match std::fs::read_to_string(&abs_seed_path) {
                    Ok(s) => {
                        file_cache.insert(abs_seed_str.clone(), s);
                    }
                    Err(_) => continue,
                }
            }
            let content = file_cache
                .get(&abs_seed_str)
                .expect("inserted above");
            let (start, end) = (seed_span.0 as usize, seed_span.1 as usize);
            let Some(seed_source_slice) = content.get(start..end) else {
                continue;
            };
            let trimmed = seed_source_slice.trim();
            if trimmed.is_empty() {
                continue;
            }

            // SHA-256(source) truncated to 16 bytes.
            let mut hasher = Sha256::new();
            hasher.update(trimmed.as_bytes());
            let full = hasher.finalize();
            let mut content_hash = [0u8; 16];
            content_hash.copy_from_slice(&full[..16]);

            // Cache lookup. Hit only if content_hash AND embedder_version match.
            let cached: Option<Vec<f32>> = match snap
                .dbs
                .embeddings_by_target
                .get(&rtxn, seed_id.as_bytes())
                .map_err(|e| {
                    McpError::internal_error(
                        format!("embeddings_by_target.get: {e}"),
                        None,
                    )
                })? {
                Some(rec)
                    if rec.content_hash == content_hash
                        && rec.embedder_version == EMBEDDER_VERSION =>
                {
                    Some(rec.vector)
                }
                _ => None,
            };

            let needs_embed = cached.is_none();
            seeds_ctx.push(SeedCtx {
                id: seed_id,
                node: seed_node,
                content_hash,
                cached_vec: cached,
                source: if needs_embed {
                    Some(trimmed.to_string())
                } else {
                    None
                },
            });
        }
    }

    // 5. Batch-embed cache misses (v1.1b) and persist each fresh vector.
    let mut miss_indices: Vec<usize> = Vec::new();
    let mut miss_texts: Vec<String> = Vec::new();
    for (idx, ctx) in seeds_ctx.iter().enumerate() {
        if let Some(ref src) = ctx.source {
            miss_indices.push(idx);
            miss_texts.push(src.clone());
        }
    }

    if !miss_texts.is_empty() {
        let embedder = rust_code_mcp_embeddings::EmbeddingGenerator::new()
            .map_err(|e| {
                McpError::internal_error(
                    format!("EmbeddingGenerator init: {e}"),
                    None,
                )
            })?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut wtxn = snap
            .env
            .write_txn()
            .map_err(|e| McpError::internal_error(format!("write_txn: {e}"), None))?;

        for chunk_start in (0..miss_texts.len()).step_by(EMBED_CHUNK) {
            let chunk_end = (chunk_start + EMBED_CHUNK).min(miss_texts.len());
            let texts: Vec<String> = miss_texts[chunk_start..chunk_end].to_vec();
            let vectors = embedder
                .embed_batch_async(texts)
                .await
                .map_err(|e| {
                    McpError::internal_error(
                        format!("embed_batch_async: {e}"),
                        None,
                    )
                })?;

            for (offset, vector) in vectors.into_iter().enumerate() {
                let idx = miss_indices[chunk_start + offset];
                let ctx = &mut seeds_ctx[idx];
                let rec = EmbeddingRecord {
                    content_hash: ctx.content_hash,
                    vector: vector.clone(),
                    embedder_version: EMBEDDER_VERSION.to_string(),
                    generated_at_unix: now,
                };
                snap.dbs
                    .embeddings_by_target
                    .put(&mut wtxn, ctx.id.as_bytes(), &rec)
                    .map_err(|e| {
                        McpError::internal_error(
                            format!("embeddings_by_target.put: {e}"),
                            None,
                        )
                    })?;
                ctx.cached_vec = Some(vector);
                ctx.source = None;
            }
        }

        wtxn.commit()
            .map_err(|e| McpError::internal_error(format!("wtxn.commit: {e}"), None))?;
    }

    // 6. Edge accumulator. For symmetric dedup we use a canonical
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

    // 7. v1.1c — identical-source short-circuit. Items sharing a content_hash
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

    // 8. In-memory pairwise cosine. O(N²) on 384-dim vectors. Identical-hash
    //    pairs are skipped here (already handled above with score=1.0).
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
            let score = cosine(va, vb);
            if score < threshold {
                continue;
            }
            let key = canonical(a.id, b.id);
            edges.entry(key).or_default().push(score);
        }
    }

    // 9. Symmetric dedup: average the per-direction scores.
    let mut pairs: Vec<(NodeId, NodeId, f32)> = edges
        .into_iter()
        .map(|((a, b), scores)| {
            let avg = scores.iter().sum::<f32>() / scores.len() as f32;
            (a, b, avg)
        })
        .collect();
    pairs.sort_by(|x, y| y.2.partial_cmp(&x.2).unwrap_or(std::cmp::Ordering::Equal));
    let pair_count = pairs.len();

    // 10. Build response. v1.1 only ever produces edges between seeds, so
    //     the lookup table is the seeds themselves — no fallback `node_by_id`
    //     read needed.
    let seed_count = seeds_ctx.len();
    let seed_index: HashMap<NodeId, &Node> =
        seeds_ctx.iter().map(|c| (c.id, &c.node)).collect();
    let lookup_ref = |id: NodeId| -> Option<ItemRef> {
        seed_index.get(&id).map(|node| node_to_item_ref(node))
    };

    let scope = ScopeSummary {
        directory: directory.clone(),
        crate_name: crate_name.clone(),
        item_kind: item_kind_filter_label.clone(),
        seed_count,
    };

    if output_mode == "pairs" {
        let truncated = pairs.into_iter().take(max_pairs);
        let pair_refs: Vec<SimilarityPair> = truncated
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
            pair_count,
            output_mode,
            pairs: Some(pair_refs),
            clusters: None,
        });
    }

    // Clusters mode (default).
    let mut clusters = build_clusters(&pairs, max_pairs, lookup_ref);
    if max_cluster_size > 0 {
        clusters.retain(|c| c.size <= max_cluster_size);
    }
    json_result(&SemanticOverlapsResp {
        scope,
        threshold,
        pair_count,
        output_mode,
        pairs: None,
        clusters: Some(clusters),
    })
}

pub async fn functions_with_filter(
    params: FunctionsWithFilterParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (id, node) = snap
        .lookup_by_qualified_name(&params.krate)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.krate),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "`{}` resolves to a Module with no crate_id",
                    params.krate
                ),
                None,
            )
        })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.krate
                ),
                None,
            ));
        }
    };

    let self_kind = match params.self_kind.as_deref() {
        None => None,
        Some("none") => Some(SelfKindFilter::None),
        Some("owned") => Some(SelfKindFilter::Owned),
        Some("ref") => Some(SelfKindFilter::Ref),
        Some("ref_mut") => Some(SelfKindFilter::RefMut),
        Some(other) => {
            return Err(McpError::invalid_params(
                format!(
                    "self_kind must be one of `none`, `owned`, `ref`, `ref_mut`; got `{other}`"
                ),
                None,
            ));
        }
    };
    let filter = FunctionFilter {
        min_param_count: params.min_param_count,
        has_param_type: params.has_param_type,
        returns_type_pattern: params.returns_type_pattern,
        is_async: params.is_async,
        self_kind,
    };

    let matches: Vec<FunctionWithSignature> = snap
        .functions_with_filter(crate_id, &filter)
        .map_err(internal_error("functions_with_filter"))?;

    // Pagination + summary mode (Item #4 + #5).
    // Slice in the wrapper layer; the query is workspace-bounded and not
    // inherently large — the cost is in serialization payload size.
    let total_match_count = matches.len();
    let offset = params.offset.unwrap_or(0);
    let limit = params.limit.unwrap_or(50);
    let summary = params.summary.unwrap_or(false);

    let sliced = matches
        .into_iter()
        .skip(offset)
        .take(limit);

    let enriched: Vec<FunctionsWithFilterMatch> = sliced
        .map(|m| FunctionsWithFilterMatch {
            target: m.qualified_name.clone(),
            qualified_name: m.qualified_name,
            signature: if summary { None } else { Some(m.signature) },
        })
        .collect();

    json_result(&FunctionsWithFilterResponse {
        krate: params.krate,
        total_match_count,
        offset,
        limit,
        match_count: enriched.len(),
        matches: enriched,
    })
}

pub async fn forbidden_dependency_check(
    params: ForbiddenDependencyCheckParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let rules: Vec<ForbiddenDependencyRule> = params
        .rules
        .into_iter()
        .map(|r| ForbiddenDependencyRule {
            consumer: r.consumer,
            producer: r.producer,
            except: r.except,
            severity: r.severity,
            message: r.message,
        })
        .collect();
    let violations: Vec<ForbiddenDependencyViolation> = snap
        .forbidden_dependency_check(&rules)
        .map_err(internal_error("forbidden_dependency_check"))?;
    json_result(&ForbiddenDependencyCheckResponse {
        rule_count: rules.len(),
        violation_count: violations.len(),
        violations,
    })
}

pub async fn pub_use_pub_type_audit(
    params: PubUsePubTypeAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (id, node) = snap
        .lookup_by_qualified_name(&params.crate_name)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.crate_name),
                None,
            )
        })?;
    let crate_id = match node.kind {
        NodeKind::Crate => id,
        NodeKind::Module => node.crate_id.or(node.parent_id).ok_or_else(|| {
            McpError::invalid_params(
                format!(
                    "`{}` resolves to a Module with no crate_id",
                    params.crate_name
                ),
                None,
            )
        })?,
        other => {
            return Err(McpError::invalid_params(
                format!(
                    "`{}` is a {other:?}, expected a crate or its root module",
                    params.crate_name
                ),
                None,
            ));
        }
    };
    let findings: Vec<PubTypeAliasMasqueradingAsReexport> = snap
        .pub_use_pub_type_audit(crate_id)
        .map_err(internal_error("pub_use_pub_type_audit"))?;
    let enriched: Vec<EnrichedPubTypeAuditFinding> = {
        let rtxn = snap.read_txn().ok();
        findings
            .into_iter()
            .map(|f| {
                let pub_use_target_qualified = rtxn.as_ref().and_then(|t| {
                    snap.node_by_id(t, f.suspicious_pub_use_target_node_id)
                        .ok()
                        .flatten()
                        .map(|n| n.qualified_name)
                });
                EnrichedPubTypeAuditFinding {
                    alias_qualified_name: f.alias_qualified_name,
                    file: f.file,
                    span: f.span,
                    suspicious_pub_use_visible_name: f.suspicious_pub_use_visible_name,
                    suspicious_pub_use_target: pub_use_target_qualified,
                }
            })
            .collect()
    };
    json_result(&PubUsePubTypeAuditResponse {
        krate: params.crate_name,
        finding_count: enriched.len(),
        findings: enriched,
    })
}

pub async fn re_export_chain(
    params: ReExportChainParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let (target_id, target_node) = snap
        .lookup_by_qualified_name(&params.target)
        .map_err(internal_error("lookup_by_qualified_name"))?
        .ok_or_else(|| {
            McpError::invalid_params(
                format!("no node found for qualified name `{}`", params.target),
                None,
            )
        })?;
    let chain: ReExportChain = snap
        .re_export_chain(target_id)
        .map_err(internal_error("re_export_chain"))?;
    let links: Vec<EnrichedReExportLink> = chain
        .links
        .into_iter()
        .map(|l| EnrichedReExportLink {
            from_module: l.from_module_qualified_name,
            visible_name: l.visible_name,
            depth: l.depth,
        })
        .collect();
    json_result(&ReExportChainResponse {
        canonical: target_node.qualified_name,
        link_count: links.len(),
        links,
    })
}

pub async fn crate_dependency_metric(
    params: CrateDependencyMetricParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let mut metrics: Vec<CrateMetric> = snap
        .crate_dependency_metric()
        .map_err(internal_error("crate_dependency_metric"))?;

    // Item #7: optional sort + top_n slicing. Sort first, then slice.
    if let Some(sort_key) = params.sort_by.as_deref() {
        match sort_key {
            "instability" => metrics.sort_by(|a, b| {
                b.instability
                    .partial_cmp(&a.instability)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            "abstractness" => metrics.sort_by(|a, b| {
                b.abstractness
                    .partial_cmp(&a.abstractness)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }),
            "item_count" => metrics.sort_by(|a, b| b.item_count.cmp(&a.item_count)),
            "afferent" => metrics.sort_by(|a, b| b.afferent.cmp(&a.afferent)),
            "efferent" => metrics.sort_by(|a, b| b.efferent.cmp(&a.efferent)),
            other => {
                return Err(McpError::invalid_params(
                    format!(
                        "sort_by must be one of `instability`, `item_count`, `afferent`, `efferent`, `abstractness`; got `{other}`"
                    ),
                    None,
                ));
            }
        }
    }

    if let Some(n) = params.top_n {
        metrics.truncate(n);
    }

    // Render NodeIds as hex strings rather than the raw 32-byte arrays
    // serde_bytes_32 emits for [u8; 32].
    let rendered: Vec<CrateMetricRendered> = metrics
        .into_iter()
        .map(|m| CrateMetricRendered {
            crate_id: m.crate_id.to_hex(),
            crate_name: m.crate_name,
            efferent: m.efferent,
            afferent: m.afferent,
            instability: m.instability,
            abstractness: m.abstractness,
            item_count: m.item_count,
        })
        .collect();
    json_result(&CrateDependencyMetricResponse {
        crate_count: rendered.len(),
        metrics: rendered,
    })
}

pub async fn overlaps(params: OverlapsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let report: OverlapsReport = snap.overlaps().map_err(internal_error("overlaps"))?;
    json_result(&report)
}

pub async fn module_tree(params: ModuleTreeParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let tree: ModuleTreeNode = snap
        .module_tree(&params.krate, params.depth)
        .map_err(internal_error("module_tree"))?;
    json_result(&ModuleTreeResponse { tree })
}

pub async fn workspace_stats(params: WorkspaceStatsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let stats: WorkspaceStats = snap
        .workspace_stats()
        .map_err(internal_error("workspace_stats"))?;
    json_result(&stats)
}

pub async fn unsafe_audit(
    params: crate::tools::search_tool::UnsafeAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    // The audit calls `loader::load` (full RA workspace load, ~2-3s) and
    // then walks every file's syntax tree. Run on a blocking thread so the
    // tokio runtime worker stays free for concurrent tool calls.
    let findings: Vec<rust_code_mcp_graph::unsafe_audit::UnsafeFinding> =
        tokio::task::spawn_blocking(move || -> Result<_, McpError> {
            let snap = open_workspace_snapshot(&directory)?;
            let canonical = std::path::PathBuf::from(&directory)
                .canonicalize()
                .map_err(|e| McpError::invalid_params(format!("canonicalize: {e}"), None))?;
            let loaded = rust_code_mcp_graph::loader::load(&canonical)
                .map_err(internal_error("loader::load"))?;
            snap.unsafe_audit(&loaded)
                .map_err(internal_error("unsafe_audit"))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))??;
    // Render NodeIds as hex strings rather than the raw 32-byte arrays
    // serde_bytes_32 emits for [u8; 32].
    #[derive(serde::Serialize)]
    struct UnsafeFindingRendered {
        file: String,
        span: (u32, u32),
        line_count: u32,
        enclosing_function: Option<String>,
        enclosing_function_name: Option<String>,
        has_safety_comment: bool,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        directory: String,
        finding_count: usize,
        findings: Vec<UnsafeFindingRendered>,
    }
    let rendered: Vec<UnsafeFindingRendered> = findings
        .into_iter()
        .map(|f| UnsafeFindingRendered {
            file: f.file,
            span: f.span,
            line_count: f.line_count,
            enclosing_function: f.enclosing_function.map(|n| n.to_hex()),
            enclosing_function_name: f.enclosing_function_name,
            has_safety_comment: f.has_safety_comment,
        })
        .collect();
    json_result(&Resp {
        directory: params.directory,
        finding_count: rendered.len(),
        findings: rendered,
    })
}

pub async fn mut_static_audit(
    params: crate::tools::search_tool::MutStaticAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let findings = snap
        .mut_static_audit()
        .map_err(internal_error("mut_static_audit"))?;
    // Render NodeIds as hex strings rather than the raw 32-byte arrays
    // serde_bytes_32 emits for [u8; 32].
    #[derive(serde::Serialize)]
    struct MutStaticFindingRendered {
        item: String,
        qualified_name: String,
        matched_pattern: String,
        type_string: String,
        file: Option<String>,
        span: Option<(u32, u32)>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        directory: String,
        finding_count: usize,
        findings: Vec<MutStaticFindingRendered>,
    }
    let rendered: Vec<MutStaticFindingRendered> = findings
        .into_iter()
        .map(|f| MutStaticFindingRendered {
            item: f.item.to_hex(),
            qualified_name: f.qualified_name,
            matched_pattern: f.matched_pattern,
            type_string: f.type_string,
            file: f.file,
            span: f.span,
        })
        .collect();
    json_result(&Resp {
        directory: params.directory,
        finding_count: rendered.len(),
        findings: rendered,
    })
}

pub async fn missing_docs_audit(
    params: crate::tools::search_tool::MissingDocsAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;

    let crate_id_filter: Option<NodeId> = if let Some(qn) = &params.crate_name {
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
                    format!("`{qn}` is a {other:?}, expected a Crate or its root Module"),
                    None,
                ));
            }
        })
    } else {
        None
    };

    let kind_filter = match params.item_kind.as_deref() {
        None => rust_code_mcp_graph::docs_audit::default_kind_filter(),
        Some(labels) => {
            let mut set = std::collections::HashSet::new();
            for label in labels {
                let kind = parse_item_kind_filter(Some(label.as_str()))?
                    .ok_or_else(|| {
                        McpError::invalid_params(
                            format!("empty item_kind label in list"),
                            None,
                        )
                    })?;
                set.insert(kind);
            }
            set
        }
    };

    let opts = rust_code_mcp_graph::docs_audit::AuditOpts {
        crate_id_filter,
        kind_filter,
        skip_test_items: params.skip_test_items.unwrap_or(true),
    };

    let findings = rust_code_mcp_graph::docs_audit::missing_docs_audit(&snap, opts)
        .map_err(internal_error("missing_docs_audit"))?;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct MissingDocsFindingRendered {
        target: String,
        qualified_name: String,
        item_kind: String,
        visibility: String,
        file: Option<String>,
        span: Option<(u32, u32)>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        finding_count: usize,
        findings: Vec<MissingDocsFindingRendered>,
    }

    let rendered: Vec<MissingDocsFindingRendered> = findings
        .into_iter()
        .map(|f| MissingDocsFindingRendered {
            target: f.target.to_hex(),
            qualified_name: f.qualified_name,
            item_kind: item_kind_label(f.item_kind).to_string(),
            visibility: f.visibility,
            file: f.file,
            span: f.span,
        })
        .collect();

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        finding_count: rendered.len(),
        findings: rendered,
    })
}

pub async fn derive_audit(
    params: crate::tools::search_tool::DeriveAuditParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;

    let crate_id_filter: Option<NodeId> = if let Some(qn) = &params.crate_name {
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
                    format!("`{qn}` is a {other:?}, expected a Crate or its root Module"),
                    None,
                ));
            }
        })
    } else {
        None
    };

    let kind_filter = match params.item_kind.as_deref() {
        None => rust_code_mcp_graph::derive_audit::default_kind_filter(),
        Some(labels) => {
            let mut set = std::collections::HashSet::new();
            for label in labels {
                let kind = parse_item_kind_filter(Some(label.as_str()))?
                    .ok_or_else(|| {
                        McpError::invalid_params(
                            "empty item_kind label in list".to_string(),
                            None,
                        )
                    })?;
                match kind {
                    ItemKind::Struct | ItemKind::Enum | ItemKind::Union => {}
                    other => {
                        return Err(McpError::invalid_params(
                            format!(
                                "derive_audit only accepts Struct | Enum | Union, got {other:?}"
                            ),
                            None,
                        ));
                    }
                }
                set.insert(kind);
            }
            set
        }
    };

    if params.required_derives.is_empty() {
        return Err(McpError::invalid_params(
            "required_derives must be a non-empty list of derive identifiers".to_string(),
            None,
        ));
    }
    let required_derives: std::collections::HashSet<String> =
        params.required_derives.iter().cloned().collect();

    let opts = rust_code_mcp_graph::derive_audit::AuditOpts {
        crate_id_filter,
        kind_filter,
        required_derives,
        pub_only: params.pub_only.unwrap_or(true),
        skip_test_items: params.skip_test_items.unwrap_or(true),
    };

    let findings = rust_code_mcp_graph::derive_audit::derive_audit(&snap, opts)
        .map_err(internal_error("derive_audit"))?;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct DeriveFindingRendered {
        target: String,
        qualified_name: String,
        item_kind: String,
        visibility: String,
        file: Option<String>,
        span: Option<(u32, u32)>,
        current_derives: Vec<String>,
        missing_derives: Vec<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        required_derives: Vec<String>,
        finding_count: usize,
        findings: Vec<DeriveFindingRendered>,
    }

    let rendered: Vec<DeriveFindingRendered> = findings
        .into_iter()
        .map(|f| DeriveFindingRendered {
            target: f.target.to_hex(),
            qualified_name: f.qualified_name,
            item_kind: item_kind_label(f.item_kind).to_string(),
            visibility: f.visibility,
            file: f.file,
            span: f.span,
            current_derives: f.current_derives,
            missing_derives: f.missing_derives,
        })
        .collect();

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        required_derives: params.required_derives,
        finding_count: rendered.len(),
        findings: rendered,
    })
}

pub async fn recursion_check(
    params: crate::tools::search_tool::RecursionCheckParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;

    let crate_id_filter: Option<NodeId> = if let Some(qn) = &params.crate_name {
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
                    format!("`{qn}` is a {other:?}, expected a Crate or its root Module"),
                    None,
                ));
            }
        })
    } else {
        None
    };

    let max_cycle_length =
        rust_code_mcp_graph::recursion_check::clamp_cycle_length(params.max_cycle_length);

    let opts = rust_code_mcp_graph::recursion_check::RecursionOpts {
        crate_id_filter,
        max_cycle_length,
    };

    let cycles = rust_code_mcp_graph::recursion_check::recursion_check(&snap, opts)
        .map_err(internal_error("recursion_check"))?;

    let mut rendered: Vec<RecursionCycleRendered> = Vec::with_capacity(cycles.len());
    for cycle in cycles {
        let qualified_names =
            rust_code_mcp_graph::recursion_check::enclosing_fn_qualified_names(&snap, &cycle.fns)
                .map_err(internal_error("enclosing_fn_qualified_names"))?;
        let starting_node_id = cycle
            .fns
            .first()
            .map(|id| id.to_hex())
            .unwrap_or_default();
        rendered.push(RecursionCycleRendered {
            fns: qualified_names,
            cycle_length: cycle.cycle_length,
            direct_recursion: cycle.direct_recursion,
            starting_node_id,
        });
    }

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        max_cycle_length: usize,
        cycle_count: usize,
        cycles: Vec<RecursionCycleRendered>,
    }

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        max_cycle_length,
        cycle_count: rendered.len(),
        cycles: rendered,
    })
}

#[derive(serde::Serialize)]
struct RecursionCycleRendered {
    fns: Vec<String>,
    cycle_length: usize,
    direct_recursion: bool,
    starting_node_id: String,
}

pub async fn channel_capacity_audit(
    params: crate::tools::search_tool::ChannelCapacityAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    let crate_name = params.crate_name.clone();
    let skip_test_fns = params.skip_test_fns.unwrap_or(true);

    let findings: Vec<rust_code_mcp_graph::channel_audit::ChannelFinding> =
        tokio::task::spawn_blocking(move || -> Result<_, McpError> {
            let snap = open_workspace_snapshot(&directory)?;

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
                            format!("`{qn}` is a {other:?}, expected a Crate or its root Module"),
                            None,
                        ));
                    }
                })
            } else {
                None
            };

            let canonical = std::path::PathBuf::from(&directory)
                .canonicalize()
                .map_err(|e| McpError::invalid_params(format!("canonicalize: {e}"), None))?;
            let loaded = rust_code_mcp_graph::loader::load(&canonical)
                .map_err(internal_error("loader::load"))?;

            let opts = rust_code_mcp_graph::channel_audit::ChannelAuditOpts {
                crate_id_filter,
                skip_test_fns,
            };
            rust_code_mcp_graph::channel_audit::channel_capacity_audit(&loaded, &snap, opts)
                .map_err(internal_error("channel_capacity_audit"))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))??;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct ChannelFindingRendered {
        crate_name: String,
        kind: String,
        bounded: bool,
        capacity: Option<u64>,
        file: String,
        span: (u32, u32),
        enclosing_function: Option<String>,
        enclosing_function_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        finding_count: usize,
        findings: Vec<ChannelFindingRendered>,
    }

    let rendered: Vec<ChannelFindingRendered> = findings
        .into_iter()
        .map(|f| ChannelFindingRendered {
            crate_name: f.crate_name,
            kind: f.kind,
            bounded: f.bounded,
            capacity: f.capacity,
            file: f.file,
            span: f.span,
            enclosing_function: f.enclosing_function.map(|n| n.to_hex()),
            enclosing_function_name: f.enclosing_function_name,
        })
        .collect();

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        finding_count: rendered.len(),
        findings: rendered,
    })
}

pub async fn fn_body_audit(
    params: crate::tools::search_tool::FnBodyAuditParams,
) -> Result<CallToolResult, McpError> {
    let directory = params.directory.clone();
    let crate_name = params.crate_name.clone();
    let patterns_input = params.patterns.clone();
    let skip_test_fns = params.skip_test_fns.unwrap_or(true);

    let patterns_set =
        rust_code_mcp_graph::fn_body_audit::parse_pattern_filter(patterns_input.as_deref())
            .map_err(|m| McpError::invalid_params(m, None))?;

    let mut patterns_used: Vec<String> =
        patterns_set.iter().map(|s| s.to_string()).collect();
    patterns_used.sort();

    let findings: Vec<rust_code_mcp_graph::fn_body_audit::FnBodyFinding> =
        tokio::task::spawn_blocking(move || -> Result<_, McpError> {
            let snap = open_workspace_snapshot(&directory)?;

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
                            format!("`{qn}` is a {other:?}, expected a Crate or its root Module"),
                            None,
                        ));
                    }
                })
            } else {
                None
            };

            let canonical = std::path::PathBuf::from(&directory)
                .canonicalize()
                .map_err(|e| McpError::invalid_params(format!("canonicalize: {e}"), None))?;
            let loaded = rust_code_mcp_graph::loader::load(&canonical)
                .map_err(internal_error("loader::load"))?;

            let opts = rust_code_mcp_graph::fn_body_audit::FnBodyAuditOpts {
                crate_id_filter,
                patterns: patterns_set,
                skip_test_fns,
            };
            rust_code_mcp_graph::fn_body_audit::fn_body_audit(&loaded, &snap, opts)
                .map_err(internal_error("fn_body_audit"))
        })
        .await
        .map_err(|e| McpError::internal_error(format!("spawn_blocking join error: {e}"), None))??;

    #[derive(serde::Serialize)]
    struct ScopeSummary {
        directory: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        crate_name: Option<String>,
    }
    #[derive(serde::Serialize)]
    struct FnBodyFindingRendered {
        target: Option<String>,
        qualified_name: Option<String>,
        pattern: String,
        file: String,
        span: (u32, u32),
        context: String,
    }
    #[derive(serde::Serialize)]
    struct Resp {
        scope: ScopeSummary,
        patterns_used: Vec<String>,
        finding_count: usize,
        findings: Vec<FnBodyFindingRendered>,
    }

    let rendered: Vec<FnBodyFindingRendered> = findings
        .into_iter()
        .map(|f| FnBodyFindingRendered {
            target: f.target.map(|n| n.to_hex()),
            qualified_name: f.qualified_name,
            pattern: f.pattern,
            file: f.file,
            span: f.span,
            context: f.context,
        })
        .collect();

    json_result(&Resp {
        scope: ScopeSummary {
            directory: params.directory,
            crate_name: params.crate_name,
        },
        patterns_used,
        finding_count: rendered.len(),
        findings: rendered,
    })
}

// ----- helpers -----

fn open_workspace_snapshot(directory: &str) -> Result<OpenedSnapshot, McpError> {
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

fn resolve_required_node(
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
    // module is expected (e.g., `consumer: "rust_code_mcp_graph"`), promote the
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

fn enrich_bindings(snap: &OpenedSnapshot, bindings: Vec<Binding>) -> Vec<EnrichedBinding> {
    let rtxn = match snap.read_txn() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    bindings
        .into_iter()
        .map(|b| {
            let target_node = snap.node_by_id(&rtxn, b.target).ok().flatten();
            let from_module_node = snap.node_by_id(&rtxn, b.from_module).ok().flatten();
            EnrichedBinding {
                visible_name: b.visible_name,
                namespace: namespace_label(b.namespace),
                kind: binding_kind_label(b.kind),
                visibility: visibility_label(snap, &rtxn, &b.visibility),
                from_module: from_module_node
                    .as_ref()
                    .map(|n| n.qualified_name.clone()),
                target: target_node.as_ref().map(|n| n.qualified_name.clone()),
                target_kind: target_node.as_ref().map(|n| node_kind_label(n)),
            }
        })
        .collect()
}

fn enrich_usages(snap: &OpenedSnapshot, usages: Vec<Usage>) -> Vec<EnrichedUsage> {
    let rtxn = match snap.read_txn() {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    usages
        .into_iter()
        .map(|u| {
            let consumer_node = snap.node_by_id(&rtxn, u.consumer_module).ok().flatten();
            let consumer_function_name = u.consumer_function.and_then(|fn_id| {
                snap.node_by_id(&rtxn, fn_id)
                    .ok()
                    .flatten()
                    .map(|n| n.qualified_name)
            });
            EnrichedUsage {
                file: u.file,
                start: u.start,
                end: u.end,
                category: usage_category_label(u.category),
                consumer_module: consumer_node.as_ref().map(|n| n.qualified_name.clone()),
                consumer_function: consumer_function_name,
            }
        })
        .collect()
}

fn enrich_dead_pub(snap: &OpenedSnapshot, f: DeadPubFinding) -> EnrichedDeadPub {
    let rtxn = snap.read_txn().ok();
    let visibility = match &rtxn {
        Some(t) => visibility_label(snap, t, &f.declared_visibility),
        None => "?".to_string(),
    };
    // Look up file/span for navigability — these live on the Item Node.
    let (file, span) = match &rtxn {
        Some(t) => match snap.node_by_id(t, f.target).ok().flatten() {
            Some(node) => (node.file, node.span),
            None => (None, None),
        },
        None => (None, None),
    };
    EnrichedDeadPub {
        qualified_name: f.qualified_name,
        item_kind: item_kind_label(f.item_kind),
        declared_visibility: visibility,
        file,
        span,
    }
}

fn enrich_crate_dead_pub(snap: &OpenedSnapshot, c: CrateDeadPub) -> EnrichedCrateDeadPub {
    EnrichedCrateDeadPub {
        krate: c.crate_qualified_name,
        findings: c
            .findings
            .into_iter()
            .map(|f| enrich_dead_pub(snap, f))
            .collect(),
    }
}

fn json_result<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
    let json = serde_json::to_string_pretty(value)
        .map_err(|e| McpError::internal_error(format!("serialize: {e}"), None))?;
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

fn internal_error(label: &'static str) -> impl Fn(anyhow::Error) -> McpError {
    move |e| McpError::internal_error(format!("{label}: {e:#}"), None)
}

fn namespace_label(ns: Namespace) -> &'static str {
    match ns {
        Namespace::Type => "Type",
        Namespace::Value => "Value",
    }
}

fn usage_category_label(c: UsageCategory) -> &'static str {
    match c {
        UsageCategory::Read => "Read",
        UsageCategory::Write => "Write",
        UsageCategory::Test => "Test",
        UsageCategory::Other => "Other",
    }
}

fn item_kind_label(k: ItemKind) -> &'static str {
    match k {
        ItemKind::Function => "Function",
        ItemKind::Struct => "Struct",
        ItemKind::Enum => "Enum",
        ItemKind::Union => "Union",
        ItemKind::Trait => "Trait",
        ItemKind::TypeAlias => "TypeAlias",
        ItemKind::Const => "Const",
        ItemKind::Static => "Static",
        ItemKind::AssocFunction => "AssocFunction",
        ItemKind::AssocConst => "AssocConst",
        ItemKind::AssocType => "AssocType",
        ItemKind::Method => "Method",
        ItemKind::EnumVariant => "EnumVariant",
    }
}

fn binding_kind_label(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Declared => "Declared",
        BindingKind::NamedImport => "NamedImport",
        BindingKind::GlobImport => "GlobImport",
        BindingKind::ExternCrateImport => "ExternCrateImport",
    }
}

fn node_kind_label(node: &Node) -> String {
    match node.kind {
        NodeKind::Workspace => "Workspace".to_string(),
        NodeKind::Crate => "Crate".to_string(),
        NodeKind::Module => "Module".to_string(),
        NodeKind::Item => match node.item_kind {
            Some(k) => format!("Item.{}", short_item_kind_label(k)),
            None => "Item".to_string(),
        },
        NodeKind::ExternalSymbol => "ExternalSymbol".to_string(),
    }
}

/// Short variant labels matching the form used by `queries::label_item_kind`
/// (e.g. `Function -> "Fn"`, `AssocFunction -> "AssocFn"`). Pair with
/// `node_kind_label` so a Function Item serializes as `"Item.Fn"` rather than
/// `"Item.Function"`. Keep in sync with `queries::label_item_kind`.
fn short_item_kind_label(k: ItemKind) -> &'static str {
    match k {
        ItemKind::EnumVariant => "EnumVariant",
        ItemKind::Function => "Fn",
        ItemKind::Struct => "Struct",
        ItemKind::Enum => "Enum",
        ItemKind::Union => "Union",
        ItemKind::Trait => "Trait",
        ItemKind::TypeAlias => "TypeAlias",
        ItemKind::Const => "Const",
        ItemKind::Static => "Static",
        ItemKind::AssocFunction => "AssocFn",
        ItemKind::AssocConst => "AssocConst",
        ItemKind::AssocType => "AssocType",
        ItemKind::Method => "Method",
    }
}

fn visibility_label(
    snap: &OpenedSnapshot,
    rtxn: &rust_code_mcp_graph::GraphRoTxn<'_>,
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

// ----- semantic_overlaps helpers -----

/// Parse the user-supplied `item_kind` filter string into an `Option<ItemKind>`.
/// Case-insensitive. None → no filter. Unknown variants return an
/// `invalid_params` error.
fn parse_item_kind_filter(s: Option<&str>) -> Result<Option<ItemKind>, McpError> {
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

/// Pure helper: returns true iff `[a_start, a_end]` and `[b_start, b_end]`
/// overlap as inclusive line ranges. Extracted for unit testing.
///
/// As of v1.1 of `semantic_overlaps` no production caller uses this — kept
/// alive by tests and as a reachable helper for `resolve_chunk_to_item` so
/// future tools can re-introduce chunk → Item resolution.
#[allow(dead_code)]
fn line_range_overlaps(a_start: u32, a_end: u32, b_start: u32, b_end: u32) -> bool {
    a_start <= b_end && a_end >= b_start
}

/// Cosine similarity between two equal-length f32 vectors. Used by
/// `semantic_overlaps` v1.1 for the in-memory pairwise pass. Returns 0.0
/// when either vector has zero norm (instead of NaN); slices of unequal
/// length are silently truncated to the shorter length via `zip`.
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0f32;
    let mut na = 0f32;
    let mut nb = 0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
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
fn resolve_chunk_to_item(
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

/// Build a small ItemRef from a Node (used in semantic_overlaps response).
fn node_to_item_ref(node: &Node) -> ItemRef {
    ItemRef {
        qualified_name: node.qualified_name.clone(),
        item_kind: node.item_kind.map(|k| short_item_kind_label(k).to_string()),
        file: node.file.clone().unwrap_or_default(),
        span: node.span.unwrap_or((0, 0)),
    }
}

/// Single-linkage clustering via union-find. Each edge unions its two endpoints
/// and contributes its score to the resulting cluster's score statistics.
/// Singleton groups are dropped. Sort: by avg_similarity desc, then size desc,
/// then min_similarity desc.
/// Each cluster's member list is capped at `max_members` (sets `truncated=true`
/// when the cap kicks in).
fn build_clusters<F>(
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

// ----- response shapes -----

#[derive(Debug, Serialize)]
struct BuildHypergraphResponse {
    graph_id: String,
    workspace_root: String,
    fingerprint: String,
    node_count: u64,
    binding_count: u64,
    usage_count: u64,
    reused: bool,
    snapshot_path: String,
}

#[derive(Debug, Serialize)]
struct BindingsListResponse {
    #[serde(skip_serializing_if = "Option::is_none")]
    module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    consumer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    bindings: Vec<EnrichedBinding>,
}

#[derive(Debug, Serialize)]
struct EnrichedBinding {
    visible_name: String,
    namespace: &'static str,
    kind: &'static str,
    visibility: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    from_module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct UsagesListResponse {
    target: String,
    usages: Vec<EnrichedUsage>,
}

#[derive(Debug, Serialize)]
struct EnrichedUsage {
    file: String,
    start: u32,
    end: u32,
    category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    consumer_module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    consumer_function: Option<String>,
}

#[derive(Debug, Serialize)]
struct CallSitesResponse {
    /// Set when serving `who_calls(target)`; the resolved callee's qualified
    /// name. None when serving `calls_from(caller)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<String>,
    /// Set when serving `calls_from(caller)`; the resolved caller's qualified
    /// name. None when serving `who_calls(target)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    caller: Option<String>,
    call_sites: Vec<EnrichedCallSite>,
}

#[derive(Debug, Serialize)]
struct DeadPubResponse {
    #[serde(rename = "crate")]
    krate: String,
    findings: Vec<EnrichedDeadPub>,
}

#[derive(Debug, Serialize)]
struct EnrichedDeadPub {
    qualified_name: String,
    item_kind: &'static str,
    declared_visibility: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<(u32, u32)>,
}

#[derive(Debug, Serialize)]
struct DeadPubReportResponse {
    workspace: String,
    total_findings: usize,
    crates: Vec<EnrichedCrateDeadPub>,
}

#[derive(Debug, Serialize)]
struct EnrichedCrateDeadPub {
    #[serde(rename = "crate")]
    krate: String,
    findings: Vec<EnrichedDeadPub>,
}

#[derive(Debug, Serialize)]
struct CrateEdgesResponse {
    edges: Vec<CrateEdge>,
}

#[derive(Debug, Serialize)]
struct EnumVariantsResponse {
    enum_qualified_name: String,
    variant_count: usize,
    variants: Vec<EnrichedEnumVariant>,
}

#[derive(Debug, Serialize)]
struct EnrichedEnumVariant {
    display_name: String,
    qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<(u32, u32)>,
}

#[derive(Debug, Serialize)]
struct ForbiddenDependencyCheckResponse {
    rule_count: usize,
    violation_count: usize,
    violations: Vec<ForbiddenDependencyViolation>,
}

#[derive(Debug, Serialize)]
struct ItemAttributesResponse {
    target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_kind: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<(u32, u32)>,
    attribute_count: usize,
    attributes: Vec<String>,
}

#[derive(Debug, Serialize)]
struct ItemsWithAttributeResponse {
    #[serde(rename = "crate")]
    krate: String,
    attribute_pattern: String,
    match_count: usize,
    items: Vec<EnrichedItemWithAttribute>,
}

#[derive(Debug, Serialize)]
struct EnrichedItemWithAttribute {
    qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_kind: Option<&'static str>,
    matched_attribute: String,
    /// `"attr"` when the pattern matched the start of the attribute string,
    /// `"doc"` when it matched the start of a `///` doc-comment body.
    match_location: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<(u32, u32)>,
}

#[derive(Debug, Serialize)]
struct PubUsePubTypeAuditResponse {
    #[serde(rename = "crate")]
    krate: String,
    finding_count: usize,
    findings: Vec<EnrichedPubTypeAuditFinding>,
}

#[derive(Debug, Serialize)]
struct EnrichedPubTypeAuditFinding {
    alias_qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span: Option<(u32, u32)>,
    suspicious_pub_use_visible_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    suspicious_pub_use_target: Option<String>,
}

#[derive(Debug, Serialize)]
struct ReExportChainResponse {
    canonical: String,
    link_count: usize,
    links: Vec<EnrichedReExportLink>,
}

#[derive(Debug, Serialize)]
struct EnrichedReExportLink {
    from_module: String,
    visible_name: String,
    depth: u8,
}

#[derive(Debug, Serialize)]
struct CrateDependencyMetricResponse {
    crate_count: usize,
    metrics: Vec<CrateMetricRendered>,
}

/// MCP-rendered mirror of `CrateMetric`: emits `crate_id` as a 64-char hex
/// string instead of the raw 32-byte array `serde_bytes_32` would produce
/// for `NodeId`.
#[derive(Debug, Serialize)]
struct CrateMetricRendered {
    crate_id: String,
    crate_name: String,
    efferent: u32,
    afferent: u32,
    instability: f64,
    abstractness: f64,
    item_count: u32,
}

#[derive(Debug, Serialize)]
struct UsageSummaryResponse {
    target: String,
    rows: Vec<UsageSummaryRow>,
}

#[derive(Debug, Serialize)]
struct CallGraphResponse {
    root: String,
    depth: u32,
    tree: CallGraphNode,
}

#[derive(Debug, Serialize)]
struct CallersInCrateResponse {
    target: String,
    #[serde(rename = "crate")]
    krate: String,
    call_sites: Vec<EnrichedCallSite>,
}

#[derive(Debug, Serialize)]
struct ModuleTreeResponse {
    tree: ModuleTreeNode,
}

#[derive(Debug, Serialize)]
struct FunctionSignatureResponse {
    target: String,
    /// `None` when the target is not a function or extraction skipped it.
    signature: Option<FunctionSignature>,
}

#[derive(Debug, Serialize)]
struct SimilarToItemResp {
    seed: SeedItemRef,
    limit: usize,
    threshold: f32,
    item_kind_filter: Option<String>,
    match_count: usize,
    matches: Vec<SimilarMatch>,
}

#[derive(Debug, Serialize)]
struct SeedItemRef {
    qualified_name: String,
    file: String,
    span: (u32, u32),
    /// Short label form (e.g. `"Fn"`, `"Struct"`); `None` when the seed Node
    /// has no `item_kind` (e.g. it's a Module).
    item_kind: Option<String>,
}

#[derive(Debug, Serialize)]
struct SimilarMatch {
    similarity: f32,
    symbol_name: String,
    symbol_kind: String,
    file: String,
    line_start: usize,
    line_end: usize,
    /// First 3 lines of `chunk.content` joined with `\n`.
    preview: String,
}

#[derive(Debug, Serialize)]
struct SemanticOverlapsResp {
    scope: ScopeSummary,
    threshold: f32,
    pair_count: usize,
    output_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pairs: Option<Vec<SimilarityPair>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    clusters: Option<Vec<SimilarityCluster>>,
}

#[derive(Debug, Serialize)]
struct ScopeSummary {
    directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    crate_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    item_kind: Option<String>,
    seed_count: usize,
}

#[derive(Debug, Serialize)]
struct SimilarityPair {
    a: ItemRef,
    b: ItemRef,
    similarity: f32,
}

#[derive(Debug, Serialize)]
struct SimilarityCluster {
    members: Vec<ItemRef>,
    avg_similarity: f32,
    min_similarity: f32,
    size: usize,
    truncated: bool,
}

#[derive(Debug, Serialize)]
struct ItemRef {
    qualified_name: String,
    item_kind: Option<String>,
    file: String,
    span: (u32, u32),
}

#[derive(Debug, Serialize)]
struct FunctionsWithFilterResponse {
    #[serde(rename = "crate")]
    krate: String,
    /// Unfiltered total before `offset`/`limit` slicing — callers compare
    /// this to `offset + match_count` to detect "more pages exist".
    total_match_count: usize,
    /// Offset applied to the match list (after the filter, before the
    /// returned `matches`).
    offset: usize,
    /// Cap applied to the (offset-skipped) match list.
    limit: usize,
    /// Length of the returned `matches` (after offset+limit slicing). Always
    /// `<= limit`, and `<= total_match_count.saturating_sub(offset)`.
    match_count: usize,
    matches: Vec<FunctionsWithFilterMatch>,
}

#[derive(Debug, Serialize)]
struct FunctionsWithFilterMatch {
    /// Convenience alias for `qualified_name` so callers that want one
    /// "navigate-to" string don't have to know which field carries it.
    target: String,
    qualified_name: String,
    /// `None` when `summary=true` (the field is omitted entirely from the
    /// JSON response thanks to `skip_serializing_if`); otherwise carries the
    /// full FunctionSignature.
    #[serde(skip_serializing_if = "Option::is_none")]
    signature: Option<FunctionSignature>,
}

// Path import suppress dead-code on Path when unused.
#[allow(dead_code)]
fn _path_marker(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::search_tool::{
        BuildHypergraphParams, DeadPubParams, DeadPubReportParams, GraphExportsParams,
        GraphImportsParams, WhoImportsParams, WhoUsesParams,
    };
    use std::sync::Mutex;

    // Both tests in this module open the same default data-dir snapshot
    // (`~/.local/share/search/graphs/...`). heed forbids opening the same env
    // twice in the same process, so we serialize them with a shared mutex
    // rather than relying on `--test-threads=1`.
    static DEFAULT_SNAPSHOT_LOCK: Mutex<()> = Mutex::new(());

    fn workspace_root() -> String {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("server crate should live under crates/rust-code-mcp-server")
            .to_string_lossy()
            .to_string()
    }

    /// Round-trip: build_hypergraph → get_imports / who_imports against this
    /// crate. Uses the default data dir so the snapshot lifecycle exercised
    /// here mirrors what an MCP client would see.
    #[tokio::test]
    async fn mcp_round_trip_against_self() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        let build = build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(true),
        })
        .await
        .expect("build_hypergraph");
        // Result is a single text Content with the JSON body.
        let body = first_text(&build);
        assert!(body.contains("\"node_count\""), "build response: {body}");
        assert!(body.contains("\"binding_count\""));

        let imports = get_imports(GraphImportsParams {
            directory: manifest_dir.to_string(),
            module: "rust_code_mcp_graph".to_string(),
        })
        .await
        .expect("get_imports");
        let body = first_text(&imports);
        assert!(
            body.contains("\"visible_name\": \"load\""),
            "expected `load` re-export in graph mod imports: {body}"
        );

        let importers = who_imports(WhoImportsParams {
            directory: manifest_dir.to_string(),
            target: "rust_code_mcp_graph::loader::load".to_string(),
        })
        .await
        .expect("who_imports");
        let body = first_text(&importers);
        assert!(
            body.contains("rust_code_mcp_graph"),
            "expected graph mod among importers of loader::load: {body}"
        );
    }

    /// Regression: passing a Crate qualified name (e.g. `rust_code_mcp_graph`)
    /// where a Module is expected (`get_exports`'s `consumer`) should be
    /// transparent — the resolver should fall through to the crate's root
    /// module rather than erroring with "is a Crate, expected Module".
    #[tokio::test]
    async fn get_exports_accepts_crate_name_as_consumer() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        // Ensure a snapshot exists for the workspace.
        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        let exports = get_exports(GraphExportsParams {
            directory: manifest_dir.to_string(),
            // `rust_code_mcp_graph` re-exports `load` (from loader),
            // visible from anywhere inside the crate.
            module: "rust_code_mcp_graph".to_string(),
            // Crate name, NOT a module path — must be transparently
            // promoted to the crate's root module.
            consumer: "rust_code_mcp_graph".to_string(),
        })
        .await
        .expect("get_exports should accept a crate name as consumer");

        let body = first_text(&exports);
        assert!(
            body.contains("\"bindings\""),
            "expected a bindings array in response: {body}"
        );
        // The visible re-export of `load` from graph mod is one expected entry,
        // but the precise minimum is just "at least one binding".
        assert!(
            body.contains("\"visible_name\""),
            "expected at least one binding entry in response: {body}"
        );
    }

    /// MCP shape of who_uses + dead_pub_in_crate. Reuses the snapshot
    /// produced by other tests in this module via the shared lock.
    #[tokio::test]
    async fn who_uses_and_dead_pub_round_trip() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        // who_uses against a fn we know is referenced inside the lib.
        let users = who_uses(WhoUsesParams {
            directory: manifest_dir.to_string(),
            target: "rust_code_mcp_graph::loader::load".to_string(),
        })
        .await
        .expect("who_uses");
        let body = first_text(&users);
        assert!(
            body.contains("\"usages\""),
            "expected a usages array in response: {body}"
        );
        assert!(
            body.contains("\"file\""),
            "expected at least one usage entry with file path: {body}"
        );

        // dead_pub_in_crate against this very crate. We don't pin a specific
        // qualified_name (the dead-pub set drifts with refactors); just smoke-
        // test that the tool returns a structured findings array.
        let dead = dead_pub_in_crate(DeadPubParams {
            directory: manifest_dir.to_string(),
            krate: "rust_code_mcp_server".to_string(),
        })
        .await
        .expect("dead_pub_in_crate");
        let body = first_text(&dead);
        assert!(
            body.contains("\"findings\""),
            "expected a findings array in response: {body}"
        );

        // dead_pub_report aggregates the same query across all local crates and
        // stamps a `total_findings` count. rust_code_mcp_graph has at least one
        // local crate (itself), so `crates` is non-empty.
        let report = dead_pub_report(DeadPubReportParams {
            directory: manifest_dir.to_string(),
        })
        .await
        .expect("dead_pub_report");
        let body = first_text(&report);
        assert!(
            body.contains("\"total_findings\""),
            "expected total_findings in response: {body}"
        );
        assert!(
            body.contains("\"crates\""),
            "expected crates array in response: {body}"
        );
    }

    /// Item #4: default `limit=50` caps the matches returned by the
    /// wrapper, while `total_match_count` always reflects the unfiltered
    /// (pre-slice) count. We use `is_async=true` as the permissive filter
    /// (signatures.rs::tests confirms this returns >0 matches in the
    /// workspace). The default-limit cap holds whether or not the
    /// workspace currently has > 50 async fns: `match_count <= limit` and
    /// `total_match_count >= match_count` regardless.
    #[tokio::test]
    async fn functions_with_filter_default_limit_caps_results() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        let result = functions_with_filter(crate::tools::search_tool::FunctionsWithFilterParams {
            directory: manifest_dir.to_string(),
            krate: "rust_code_mcp_server".to_string(),
            min_param_count: None,
            has_param_type: None,
            returns_type_pattern: None,
            is_async: Some(true),
            self_kind: None,
            limit: None,
            offset: None,
            summary: None,
        })
        .await
        .expect("functions_with_filter");

        let body = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
        let match_count = v
            .get("match_count")
            .and_then(|x| x.as_u64())
            .expect("match_count present") as usize;
        let limit = v
            .get("limit")
            .and_then(|x| x.as_u64())
            .expect("limit present") as usize;
        let total_match_count = v
            .get("total_match_count")
            .and_then(|x| x.as_u64())
            .expect("total_match_count present") as usize;
        let matches = v
            .get("matches")
            .and_then(|x| x.as_array())
            .expect("matches array present");

        assert_eq!(limit, 50, "default limit should be 50");
        assert!(
            match_count <= 50,
            "match_count must respect default limit 50, got {match_count}"
        );
        assert_eq!(
            matches.len(),
            match_count,
            "matches.len() must equal match_count"
        );
        assert!(
            total_match_count >= match_count,
            "total_match_count ({total_match_count}) must be >= match_count ({match_count})"
        );
    }

    /// Item #5: when `summary=true`, each match drops the full `signature`
    /// payload. We rely on `#[serde(skip_serializing_if = "Option::is_none")]`
    /// on the field, so the JSON object should not contain a `signature`
    /// key at all in summary mode.
    #[tokio::test]
    async fn functions_with_filter_summary_mode_omits_signature() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        let result = functions_with_filter(crate::tools::search_tool::FunctionsWithFilterParams {
            directory: manifest_dir.to_string(),
            krate: "rust_code_mcp_server".to_string(),
            min_param_count: None,
            has_param_type: None,
            returns_type_pattern: None,
            is_async: Some(true),
            self_kind: None,
            limit: Some(10),
            offset: None,
            summary: Some(true),
        })
        .await
        .expect("functions_with_filter");

        let body = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
        let matches = v
            .get("matches")
            .and_then(|x| x.as_array())
            .expect("matches array present");
        assert!(
            !matches.is_empty(),
            "expected at least one match for is_async=true: {body}"
        );
        for (idx, m) in matches.iter().enumerate() {
            let obj = m.as_object().expect("match is an object");
            assert!(
                !obj.contains_key("signature"),
                "summary mode must omit `signature` key from match[{idx}]: {m}"
            );
            assert!(
                obj.contains_key("target"),
                "summary mode must keep `target` key: {m}"
            );
            assert!(
                obj.contains_key("qualified_name"),
                "summary mode must keep `qualified_name` key: {m}"
            );
        }
    }

    /// Item #4: `offset` skips matches; with `offset` >= `total_match_count`
    /// no matches are returned, but `total_match_count` and `limit` still
    /// echo the request inputs.
    #[tokio::test]
    async fn functions_with_filter_offset_pagination() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        // First page.
        let page1 = functions_with_filter(crate::tools::search_tool::FunctionsWithFilterParams {
            directory: manifest_dir.to_string(),
            krate: "rust_code_mcp_server".to_string(),
            min_param_count: None,
            has_param_type: None,
            returns_type_pattern: None,
            is_async: Some(true),
            self_kind: None,
            limit: Some(5),
            offset: Some(0),
            summary: Some(true),
        })
        .await
        .expect("functions_with_filter");
        let body1 = first_text(&page1);
        let v1: serde_json::Value = serde_json::from_str(&body1).expect("page1 JSON");
        let total = v1
            .get("total_match_count")
            .and_then(|x| x.as_u64())
            .expect("total_match_count") as usize;
        let matches1: Vec<String> = v1
            .get("matches")
            .and_then(|x| x.as_array())
            .expect("matches array")
            .iter()
            .map(|m| m.get("qualified_name").and_then(|s| s.as_str()).unwrap_or("").to_string())
            .collect();

        // Second page (offset = 5).
        let page2 = functions_with_filter(crate::tools::search_tool::FunctionsWithFilterParams {
            directory: manifest_dir.to_string(),
            krate: "rust_code_mcp_server".to_string(),
            min_param_count: None,
            has_param_type: None,
            returns_type_pattern: None,
            is_async: Some(true),
            self_kind: None,
            limit: Some(5),
            offset: Some(5),
            summary: Some(true),
        })
        .await
        .expect("functions_with_filter");
        let body2 = first_text(&page2);
        let v2: serde_json::Value = serde_json::from_str(&body2).expect("page2 JSON");
        let matches2: Vec<String> = v2
            .get("matches")
            .and_then(|x| x.as_array())
            .expect("matches array")
            .iter()
            .map(|m| m.get("qualified_name").and_then(|s| s.as_str()).unwrap_or("").to_string())
            .collect();

        assert_eq!(
            v2.get("offset").and_then(|x| x.as_u64()).unwrap() as usize,
            5,
            "offset must echo back the request"
        );
        // If total > 5, page2's first row must differ from page1's first row.
        if total > 5 && !matches1.is_empty() && !matches2.is_empty() {
            assert_ne!(
                matches1[0], matches2[0],
                "offset=5 must shift the match window vs offset=0"
            );
        }
    }

    /// Item #7: `top_n` caps the number of metric rows returned.
    #[tokio::test]
    async fn crate_dependency_metric_top_n_caps_count() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        let result = crate_dependency_metric(
            crate::tools::search_tool::CrateDependencyMetricParams {
                directory: manifest_dir.to_string(),
                top_n: Some(3),
                sort_by: Some("item_count".to_string()),
            },
        )
        .await
        .expect("crate_dependency_metric");

        let body = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
        let metrics = v
            .get("metrics")
            .and_then(|x| x.as_array())
            .expect("metrics array present");
        assert!(
            metrics.len() <= 3,
            "top_n=3 must cap metrics.len(), got {}",
            metrics.len()
        );
        let crate_count = v
            .get("crate_count")
            .and_then(|x| x.as_u64())
            .expect("crate_count present") as usize;
        assert_eq!(
            crate_count,
            metrics.len(),
            "crate_count must equal metrics.len() after slicing"
        );
    }

    /// Item #7: `sort_by=instability` sorts metrics non-increasing.
    #[tokio::test]
    async fn crate_dependency_metric_sort_by_instability_descending() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        let result = crate_dependency_metric(
            crate::tools::search_tool::CrateDependencyMetricParams {
                directory: manifest_dir.to_string(),
                top_n: None,
                sort_by: Some("instability".to_string()),
            },
        )
        .await
        .expect("crate_dependency_metric");

        let body = first_text(&result);
        let v: serde_json::Value = serde_json::from_str(&body)
            .unwrap_or_else(|e| panic!("response was not valid JSON: {e} — body: {body}"));
        let metrics = v
            .get("metrics")
            .and_then(|x| x.as_array())
            .expect("metrics array present");
        let instabilities: Vec<f64> = metrics
            .iter()
            .map(|m| {
                m.get("instability")
                    .and_then(|x| x.as_f64())
                    .expect("instability is a number")
            })
            .collect();
        for w in instabilities.windows(2) {
            assert!(
                w[0] >= w[1],
                "instability must be non-increasing under sort_by=instability: {:?}",
                instabilities
            );
        }
    }

    /// Item #7: an unknown `sort_by` value is rejected with `invalid_params`.
    #[tokio::test]
    async fn crate_dependency_metric_unknown_sort_by_errors() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = workspace_root();

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        let result = crate_dependency_metric(
            crate::tools::search_tool::CrateDependencyMetricParams {
                directory: manifest_dir.to_string(),
                top_n: None,
                sort_by: Some("garbage_key".to_string()),
            },
        )
        .await;

        let err = result.expect_err("unknown sort_by must error");
        let msg = format!("{err:?}");
        assert!(
            msg.contains("sort_by") && msg.contains("garbage_key"),
            "error must mention both `sort_by` and the bad value, got: {msg}"
        );
    }

    fn first_text(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }

    // ----- semantic_overlaps unit tests -----
    //
    // Note: a `resolve_chunk_to_item` end-to-end test would need a fully
    // populated LMDB snapshot, which is hard to make deterministic.
    // The plan defers it; we only test the pure helpers here:
    //   - `line_range_overlaps` (the overlap predicate),
    //   - `build_clusters` (single-linkage union-find).

    fn nid(byte: u8) -> NodeId {
        let mut id = [0u8; 32];
        id[0] = byte;
        NodeId(id)
    }

    #[test]
    fn cosine_basic_identities() {
        // identical → 1.0
        let v = vec![1.0_f32, 2.0, 3.0];
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6);
        // orthogonal → 0.0
        let a = vec![1.0_f32, 0.0];
        let b = vec![0.0_f32, 1.0];
        assert!(cosine(&a, &b).abs() < 1e-6);
        // opposite → -1.0
        let a = vec![1.0_f32, 2.0, 3.0];
        let neg: Vec<f32> = a.iter().map(|x| -x).collect();
        assert!((cosine(&a, &neg) + 1.0).abs() < 1e-6);
        // zero-norm input → 0.0 (no NaN)
        let z = vec![0.0_f32; 3];
        assert_eq!(cosine(&z, &v), 0.0);
        assert_eq!(cosine(&v, &z), 0.0);
    }

    #[test]
    fn line_range_overlaps_inclusive_bounds() {
        // touching at endpoints counts as overlap (inclusive ranges).
        assert!(line_range_overlaps(1, 5, 5, 10));
        assert!(line_range_overlaps(5, 10, 1, 5));
        // strictly disjoint
        assert!(!line_range_overlaps(1, 4, 5, 10));
        assert!(!line_range_overlaps(5, 10, 1, 4));
        // containment
        assert!(line_range_overlaps(1, 100, 50, 60));
        assert!(line_range_overlaps(50, 60, 1, 100));
        // identical
        assert!(line_range_overlaps(7, 7, 7, 7));
    }

    #[test]
    fn build_clusters_two_groups_drops_singletons() {
        let a = nid(1);
        let b = nid(2);
        let c = nid(3);
        let d = nid(4);
        let e = nid(5);
        let edges = vec![
            (a, b, 0.90),
            (b, c, 0.85),
            (d, e, 0.95),
        ];
        let lookup = |id: NodeId| {
            Some(ItemRef {
                qualified_name: format!("n_{}", id.as_bytes()[0]),
                item_kind: Some("Fn".to_string()),
                file: "x.rs".to_string(),
                span: (0, 0),
            })
        };

        let clusters = build_clusters(&edges, 50, lookup);

        // Two clusters {A,B,C} and {D,E}; no singletons.
        assert_eq!(clusters.len(), 2);
        // Sorted by avg_similarity desc: {D,E} (avg 0.95) before {A,B,C} (avg 0.875).
        assert_eq!(clusters[0].size, 2);
        assert_eq!(clusters[1].size, 3);
        assert!(!clusters[0].truncated);
        assert!(!clusters[1].truncated);
        // {D,E} avg / min both 0.95.
        assert!((clusters[0].avg_similarity - 0.95).abs() < 1e-5);
        assert!((clusters[0].min_similarity - 0.95).abs() < 1e-5);
        // {A,B,C} avg of 0.90 and 0.85 = 0.875.
        let abc_avg = clusters[1].avg_similarity;
        assert!((abc_avg - 0.875).abs() < 1e-5, "avg = {abc_avg}");
        // min_similarity for {A,B,C} is 0.85.
        assert!((clusters[1].min_similarity - 0.85).abs() < 1e-5);
    }

    #[test]
    fn build_clusters_max_members_caps_and_marks_truncated() {
        let a = nid(1);
        let b = nid(2);
        let c = nid(3);
        let d = nid(4);
        // 4-node single cluster via single-linkage chain.
        let edges = vec![
            (a, b, 0.90),
            (b, c, 0.90),
            (c, d, 0.90),
        ];
        let lookup = |id: NodeId| {
            Some(ItemRef {
                qualified_name: format!("n_{}", id.as_bytes()[0]),
                item_kind: None,
                file: "x.rs".to_string(),
                span: (0, 0),
            })
        };
        let clusters = build_clusters(&edges, 2, lookup);
        assert_eq!(clusters.len(), 1);
        assert_eq!(clusters[0].size, 4);
        assert_eq!(clusters[0].members.len(), 2);
        assert!(clusters[0].truncated);
    }

    #[test]
    fn build_clusters_empty_input() {
        let edges: Vec<(NodeId, NodeId, f32)> = Vec::new();
        let lookup = |_id: NodeId| -> Option<ItemRef> { None };
        let clusters = build_clusters(&edges, 50, lookup);
        assert!(clusters.is_empty());
    }
}

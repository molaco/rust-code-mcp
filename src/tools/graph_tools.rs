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

use std::path::{Path, PathBuf};

use rmcp::{
    ErrorData as McpError,
    model::{CallToolResult, Content},
};
use serde::Serialize;

use crate::graph::{
    Binding, BindingKind, BindingVisibility, CallGraphNode, CrateDeadPub, CrateEdge, CrateMetric,
    DeadPubFinding, EnrichedCallSite, ForbiddenDependencyRule, ForbiddenDependencyViolation,
    GraphEnvOptions, GraphPaths, ItemKind, ModuleTreeNode, Namespace, Node, NodeId, NodeKind,
    OpenedSnapshot, OverlapsReport, PubTypeAliasMasqueradingAsReexport, ReExportChain,
    RecursiveCallersCount, Usage, UsageCategory, UsageSummaryRow, WorkspaceStats,
    build_and_persist, open_current, snapshot::BuildOptions,
};
use crate::graph::queries::ItemWithAttribute;
use crate::tools::search_tool::{
    BuildHypergraphParams, CallGraphParams, CallersInCrateParams, CallsFromParams,
    CrateDependencyMetricParams, CrateEdgesParams, DeadPubParams, DeadPubReportParams,
    EnumVariantsParams, ForbiddenDependencyCheckParams, GraphDeclaredReexportsParams,
    GraphExportsParams, GraphImportsParams, GraphReexportsParams, ItemAttributesParams,
    ItemsWithAttributeParams, ModuleTreeParams, OverlapsParams, PubUsePubTypeAuditParams,
    ReExportChainParams, RecursiveCallersCountParams, WhoCallsParams, WhoImportsParams,
    WhoUsesParams, WhoUsesSummaryParams, WorkspaceStatsParams,
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
    let result = build_and_persist(&dir, opts)
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
    let metrics: Vec<CrateMetric> = snap
        .crate_dependency_metric()
        .map_err(internal_error("crate_dependency_metric"))?;
    json_result(&CrateDependencyMetricResponse {
        crate_count: metrics.len(),
        metrics,
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
    // module is expected (e.g., `consumer: "file_search_mcp"`), promote the
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
    metrics: Vec<CrateMetric>,
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

    /// Round-trip: build_hypergraph → get_imports / who_imports against this
    /// crate. Uses the default data dir so the snapshot lifecycle exercised
    /// here mirrors what an MCP client would see.
    #[tokio::test]
    async fn mcp_round_trip_against_self() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = env!("CARGO_MANIFEST_DIR");

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
            module: "file_search_mcp::graph".to_string(),
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
            target: "file_search_mcp::graph::loader::load".to_string(),
        })
        .await
        .expect("who_imports");
        let body = first_text(&importers);
        assert!(
            body.contains("file_search_mcp::graph"),
            "expected graph mod among importers of loader::load: {body}"
        );
    }

    /// Regression: passing a Crate qualified name (e.g. `file_search_mcp`)
    /// where a Module is expected (`get_exports`'s `consumer`) should be
    /// transparent — the resolver should fall through to the crate's root
    /// module rather than erroring with "is a Crate, expected Module".
    #[tokio::test]
    async fn get_exports_accepts_crate_name_as_consumer() {
        let _guard = DEFAULT_SNAPSHOT_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let manifest_dir = env!("CARGO_MANIFEST_DIR");

        // Ensure a snapshot exists for the workspace.
        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        let exports = get_exports(GraphExportsParams {
            directory: manifest_dir.to_string(),
            // `file_search_mcp::graph` re-exports `load` (from loader),
            // visible from anywhere inside the crate.
            module: "file_search_mcp::graph".to_string(),
            // Crate name, NOT a module path — must be transparently
            // promoted to the crate's root module.
            consumer: "file_search_mcp".to_string(),
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
        let manifest_dir = env!("CARGO_MANIFEST_DIR");

        build_hypergraph(BuildHypergraphParams {
            directory: manifest_dir.to_string(),
            force_rebuild: Some(false),
        })
        .await
        .expect("build_hypergraph");

        // who_uses against a fn we know is referenced inside the lib.
        let users = who_uses(WhoUsesParams {
            directory: manifest_dir.to_string(),
            target: "file_search_mcp::graph::loader::load".to_string(),
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
            krate: "file_search_mcp".to_string(),
        })
        .await
        .expect("dead_pub_in_crate");
        let body = first_text(&dead);
        assert!(
            body.contains("\"findings\""),
            "expected a findings array in response: {body}"
        );

        // dead_pub_report aggregates the same query across all local crates and
        // stamps a `total_findings` count. file_search_mcp has at least one
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

    fn first_text(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }
}

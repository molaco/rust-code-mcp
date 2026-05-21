//! Core graph-navigation endpoint family.
//!
//! Endpoints that walk the persisted hypergraph along its primary edges —
//! module → imports/exports/re-exports, item → who-uses/who-calls, callable
//! → call graph, and module/workspace structural queries. Every endpoint
//! follows the shape documented in `graph_tools.rs`: resolve directory,
//! open snapshot, resolve qualified names, run the query, serialize.

use std::path::PathBuf;

use serde::Serialize;

use crate::graph::labels::{
    binding_kind_label, item_kind_short_label as short_item_kind_label, node_kind_label,
    usage_category_label,
};
use crate::graph::snapshot::BuildOptions;
use crate::graph::{
    Binding, CallGraphNode, EnrichedCallSite, ModuleDependency, ModuleDependencySymbol,
    ModuleTreeNode, Namespace, NodeKind, OpenedSnapshot, RecursiveCallersCount, Usage,
    UsageSummaryRow, WorkspaceStats, build_and_persist,
};
use crate::tools::graph::response::*;
use crate::tools::params::{
    BuildHypergraphParams, CallGraphParams, CallersInCrateParams, CallsFromParams,
    GraphDeclaredReexportsParams, GraphExportsParams, GraphImportsParams, GraphReexportsParams,
    ModuleDependenciesParams, ModuleTreeParams, RecursiveCallersCountParams, WhoCallsParams,
    WhoImportsParams, WhoUsesParams, WhoUsesSummaryParams, WorkspaceStatsParams,
};

use rmcp::{ErrorData as McpError, model::CallToolResult};

pub(crate) async fn build_hypergraph(
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

#[derive(Debug, Serialize)]
pub(crate) struct BuildHypergraphResponse {
    pub(crate) graph_id: String,
    pub(crate) workspace_root: String,
    pub(crate) fingerprint: String,
    pub(crate) node_count: u64,
    pub(crate) binding_count: u64,
    pub(crate) usage_count: u64,
    pub(crate) reused: bool,
    pub(crate) snapshot_path: String,
}

pub(crate) async fn get_imports(params: GraphImportsParams) -> Result<CallToolResult, McpError> {
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

    let (page, bindings) = page_list(enrich_bindings(&snap, bindings), list_page(&params.pagination));
    json_result(&BindingsListResponse {
        page,
        module: Some(module_name),
        consumer: None,
        target: None,
        bindings,
    })
}

pub(crate) async fn module_dependencies(
    params: ModuleDependenciesParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let dependencies = snap
        .module_dependencies(module_id)
        .map_err(internal_error("module_dependencies"))?;
    let module_name = snap
        .lookup_by_qualified_name(&params.module)
        .ok()
        .flatten()
        .map(|(_, n)| n.qualified_name)
        .unwrap_or(params.module.clone());

    let (page, dependencies) = page_list(dependencies, list_page(&params.pagination));
    let summary = page.summary;
    let dependencies = dependencies
        .into_iter()
        .map(|dependency| module_dependency_view(dependency, summary))
        .collect();
    json_result(&ModuleDependenciesResponse {
        page,
        module: module_name,
        dependencies,
    })
}

pub(crate) async fn get_exports(params: GraphExportsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let consumer_id = resolve_required_node(&snap, &params.consumer, NodeKind::Module)?;
    let bindings = snap
        .exports_of(module_id, consumer_id)
        .map_err(internal_error("exports_of"))?;

    let (page, bindings) = page_list(enrich_bindings(&snap, bindings), list_page(&params.pagination));
    json_result(&BindingsListResponse {
        page,
        module: Some(params.module),
        consumer: Some(params.consumer),
        target: None,
        bindings,
    })
}

pub(crate) async fn get_reexports(params: GraphReexportsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let consumer_id = resolve_required_node(&snap, &params.consumer, NodeKind::Module)?;
    let bindings = snap
        .reexports_of(module_id, consumer_id)
        .map_err(internal_error("reexports_of"))?;

    let (page, bindings) = page_list(enrich_bindings(&snap, bindings), list_page(&params.pagination));
    json_result(&BindingsListResponse {
        page,
        module: Some(params.module),
        consumer: Some(params.consumer),
        target: None,
        bindings,
    })
}

pub(crate) async fn get_declared_reexports(
    params: GraphDeclaredReexportsParams,
) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let module_id = resolve_required_node(&snap, &params.module, NodeKind::Module)?;
    let bindings = snap
        .declared_reexports_of(module_id)
        .map_err(internal_error("declared_reexports_of"))?;

    let (page, bindings) = page_list(enrich_bindings(&snap, bindings), list_page(&params.pagination));
    json_result(&BindingsListResponse {
        page,
        module: Some(params.module),
        consumer: None,
        target: None,
        bindings,
    })
}

pub(crate) async fn who_imports(params: WhoImportsParams) -> Result<CallToolResult, McpError> {
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

    let (page, bindings) = page_list(enrich_bindings(&snap, bindings), list_page(&params.pagination));
    json_result(&BindingsListResponse {
        page,
        module: None,
        consumer: None,
        target: Some(target_node.qualified_name),
        bindings,
    })
}

pub(crate) async fn who_uses(params: WhoUsesParams) -> Result<CallToolResult, McpError> {
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

    let page_req = list_page(&params.pagination);
    let (page, usages) = page_list(enrich_usages(&snap, usages, page_req.summary), page_req);
    json_result(&UsagesListResponse {
        target: target_node.qualified_name,
        page,
        usages,
    })
}

pub(crate) async fn who_uses_summary(
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

    let (page, rows) = page_list(rows, list_page(&params.pagination));
    json_result(&UsageSummaryResponse {
        target: target_node.qualified_name,
        page,
        rows,
    })
}

pub(crate) async fn who_calls(params: WhoCallsParams) -> Result<CallToolResult, McpError> {
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
    let page_req = list_page(&params.pagination);
    let (page, sites) = page_list(sites, page_req);
    let call_sites = call_site_views(sites, page_req.summary);
    json_result(&CallSitesResponse {
        target: Some(target_node.qualified_name),
        caller: None,
        krate: None,
        page,
        call_sites,
    })
}

pub(crate) async fn calls_from(params: CallsFromParams) -> Result<CallToolResult, McpError> {
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
    let page_req = list_page(&params.pagination);
    let (page, sites) = page_list(sites, page_req);
    let call_sites = call_site_views(sites, page_req.summary);
    json_result(&CallSitesResponse {
        target: None,
        caller: Some(caller_node.qualified_name),
        krate: None,
        page,
        call_sites,
    })
}

pub(crate) async fn call_graph(params: CallGraphParams) -> Result<CallToolResult, McpError> {
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

pub(crate) async fn callers_in_crate(
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
    let page_req = list_page(&params.pagination);
    let (page, sites) = page_list(sites, page_req);
    let call_sites = call_site_views(sites, page_req.summary);
    json_result(&CallSitesResponse {
        target: Some(target_node.qualified_name),
        caller: None,
        krate: Some(params.krate),
        page,
        call_sites,
    })
}

pub(crate) async fn recursive_callers_count(
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

pub(crate) async fn module_tree(params: ModuleTreeParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let tree: ModuleTreeNode = snap
        .module_tree(&params.krate, params.depth)
        .map_err(internal_error("module_tree"))?;
    json_result(&ModuleTreeResponse { tree })
}

pub(crate) async fn workspace_stats(params: WorkspaceStatsParams) -> Result<CallToolResult, McpError> {
    let snap = open_workspace_snapshot(&params.directory)?;
    let stats: WorkspaceStats = snap
        .workspace_stats()
        .map_err(internal_error("workspace_stats"))?;
    json_result(&stats)
}

// ----- core-family enrichment helpers -----

pub(crate) fn enrich_bindings(
    snap: &OpenedSnapshot,
    bindings: Vec<Binding>,
) -> Vec<EnrichedBinding> {
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
                target_kind: target_node
                    .as_ref()
                    .map(|n| node_kind_label(n, short_item_kind_label)),
            }
        })
        .collect()
}

pub(crate) fn enrich_usages(
    snap: &OpenedSnapshot,
    usages: Vec<Usage>,
    summary: bool,
) -> Vec<EnrichedUsage> {
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
                file: if summary { None } else { Some(u.file) },
                start: if summary { None } else { Some(u.start) },
                end: if summary { None } else { Some(u.end) },
                category: usage_category_label(u.category),
                consumer_module: consumer_node.as_ref().map(|n| n.qualified_name.clone()),
                consumer_function: consumer_function_name,
            }
        })
        .collect()
}

pub(crate) fn call_site_views(
    sites: Vec<EnrichedCallSite>,
    summary: bool,
) -> Vec<CallSiteView> {
    sites
        .into_iter()
        .map(|site| CallSiteView {
            caller_qualified_name: site.caller_qualified_name,
            callee_qualified_name: site.callee_qualified_name,
            file: if summary { None } else { Some(site.file) },
            start: if summary { None } else { Some(site.start) },
            end: if summary { None } else { Some(site.end) },
            category: site.category,
        })
        .collect()
}

fn namespace_label(ns: Namespace) -> &'static str {
    match ns {
        Namespace::Type => "Type",
        Namespace::Value => "Value",
    }
}

fn module_dependency_view(
    dependency: ModuleDependency,
    summary: bool,
) -> ModuleDependencyView {
    ModuleDependencyView {
        target_module: dependency.target_module,
        target_kind: dependency.target_kind,
        target_crate: dependency.target_crate,
        import_count: dependency.import_count,
        usage_count: dependency.usage_count,
        symbols: if summary { None } else { Some(dependency.symbols) },
    }
}

// ----- core-family response shapes -----

#[derive(Debug, Serialize)]
pub(crate) struct BindingsListResponse {
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) consumer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    pub(crate) bindings: Vec<EnrichedBinding>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ModuleDependenciesResponse {
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) module: String,
    pub(crate) dependencies: Vec<ModuleDependencyView>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ModuleDependencyView {
    pub(crate) target_module: String,
    pub(crate) target_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_crate: Option<String>,
    pub(crate) import_count: usize,
    pub(crate) usage_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) symbols: Option<Vec<ModuleDependencySymbol>>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedBinding {
    pub(crate) visible_name: String,
    pub(crate) namespace: &'static str,
    pub(crate) kind: &'static str,
    pub(crate) visibility: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) from_module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target_kind: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct UsagesListResponse {
    pub(crate) target: String,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) usages: Vec<EnrichedUsage>,
}

#[derive(Debug, Serialize)]
pub(crate) struct EnrichedUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end: Option<u32>,
    pub(crate) category: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) consumer_module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) consumer_function: Option<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CallSitesResponse {
    /// Set when serving `who_calls(target)`; the resolved callee's qualified
    /// name. None when serving `calls_from(caller)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) target: Option<String>,
    /// Set when serving `calls_from(caller)`; the resolved caller's qualified
    /// name. None when serving `who_calls(target)`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) caller: Option<String>,
    /// Set when serving `callers_in_crate`; none for the other call-site queries.
    #[serde(rename = "crate", skip_serializing_if = "Option::is_none")]
    pub(crate) krate: Option<String>,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) call_sites: Vec<CallSiteView>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CallSiteView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) caller_qualified_name: Option<String>,
    pub(crate) callee_qualified_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) start: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) end: Option<u32>,
    pub(crate) category: String,
}

#[derive(Debug, Serialize)]
pub(crate) struct UsageSummaryResponse {
    pub(crate) target: String,
    #[serde(flatten)]
    pub(crate) page: ListMeta,
    pub(crate) rows: Vec<UsageSummaryRow>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CallGraphResponse {
    pub(crate) root: String,
    pub(crate) depth: u32,
    pub(crate) tree: CallGraphNode,
}

#[derive(Debug, Serialize)]
pub(crate) struct ModuleTreeResponse {
    pub(crate) tree: ModuleTreeNode,
}


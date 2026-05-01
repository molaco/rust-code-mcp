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
    Binding, BindingKind, BindingVisibility, GraphEnvOptions, GraphPaths, Namespace, Node,
    NodeId, NodeKind, OpenedSnapshot, build_and_persist, open_current,
    snapshot::BuildOptions,
};
use crate::tools::search_tool::{
    BuildHypergraphParams, GraphExportsParams, GraphImportsParams, GraphReexportsParams,
    WhoImportsParams,
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
    if node.kind != expect_kind {
        return Err(McpError::invalid_params(
            format!(
                "`{qualified_name}` is a {:?}, expected {expect_kind:?}",
                node.kind
            ),
            None,
        ));
    }
    Ok(id)
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
            Some(k) => format!("Item.{k:?}"),
            None => "Item".to_string(),
        },
        NodeKind::ExternalSymbol => "ExternalSymbol".to_string(),
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

// Path import suppress dead-code on Path when unused.
#[allow(dead_code)]
fn _path_marker(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::search_tool::{
        BuildHypergraphParams, GraphImportsParams, WhoImportsParams,
    };

    /// Round-trip: build_hypergraph → get_imports / who_imports against this
    /// crate. Uses the default data dir so the snapshot lifecycle exercised
    /// here mirrors what an MCP client would see.
    #[tokio::test]
    async fn mcp_round_trip_against_self() {
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

    fn first_text(result: &CallToolResult) -> String {
        result
            .content
            .first()
            .and_then(|c| c.as_text())
            .map(|t| t.text.clone())
            .unwrap_or_default()
    }
}

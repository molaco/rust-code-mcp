//! Extraction passes: HIR → ExtractionModel.
//!
//! Layer 2 milestone: Workspace, Crate, Module nodes plus Contains edges.
//! Layer 3 adds the bindings pass which also creates Item nodes and
//! ExternalSymbol stubs as a side effect.

use std::collections::HashMap;

use ra_ap_hir::Crate;
use ra_ap_hir_def::ModuleId;
use ra_ap_hir_def::nameres::{DefMap, crate_def_map};
use ra_ap_ide::RootDatabase;

use super::bindings::extract_bindings;
use super::ids::{NodeId, workspace_hash};
use super::loader::LoadedWorkspace;
use super::model::{ExtractionModel, Node, NodeKind};

pub fn extract(loaded: &LoadedWorkspace) -> ExtractionModel {
    let workspace_hash = workspace_hash(&loaded.workspace_root);
    let workspace_id = NodeId::from_components(&[workspace_hash.as_str(), "workspace"]);

    let mut model = ExtractionModel {
        workspace_root: loaded.workspace_root.clone(),
        workspace_hash: workspace_hash.clone(),
        workspace_id,
        nodes: Default::default(),
        bindings: Vec::new(),
        usages: Vec::new(),
        contains: Vec::new(),
    };

    let workspace_display = loaded
        .workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();

    model.insert_node(Node {
        id: workspace_id,
        kind: NodeKind::Workspace,
        display_name: workspace_display,
        qualified_name: loaded.workspace_root.display().to_string(),
        crate_id: None,
        parent_id: None,
        item_kind: None,
        file: None,
        span: None,
        visibility: None,
    });

    let mut crate_node_for: HashMap<Crate, NodeId> = HashMap::new();
    let mut crate_name_for: HashMap<Crate, String> = HashMap::new();
    let mut module_node_for: HashMap<ModuleId, NodeId> = HashMap::new();

    for &krate in &loaded.local_crates {
        emit_crate(
            &mut model,
            &loaded.db,
            krate,
            workspace_id,
            &mut crate_node_for,
            &mut crate_name_for,
            &mut module_node_for,
        );
    }

    extract_bindings(
        &mut model,
        &loaded.db,
        &loaded.local_crates,
        &crate_node_for,
        &crate_name_for,
        &module_node_for,
    );

    model
}

fn emit_crate(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    krate: Crate,
    workspace_id: NodeId,
    crate_node_for: &mut HashMap<Crate, NodeId>,
    crate_name_for: &mut HashMap<Crate, String>,
    module_node_for: &mut HashMap<ModuleId, NodeId>,
) {
    let crate_name = crate_display_name(db, krate);
    let crate_id = NodeId::from_components(&[
        model.workspace_hash.as_str(),
        "crate",
        crate_name.as_str(),
    ]);

    crate_node_for.insert(krate, crate_id);
    crate_name_for.insert(krate, crate_name.clone());

    model.insert_node(Node {
        id: crate_id,
        kind: NodeKind::Crate,
        display_name: crate_name.clone(),
        qualified_name: crate_name.clone(),
        crate_id: Some(crate_id),
        parent_id: Some(workspace_id),
        item_kind: None,
        file: None,
        span: None,
        visibility: Some("pub".to_string()),
    });
    model.insert_contains(workspace_id, crate_id);

    let def_map = crate_def_map(db, krate.base());
    let root_module_id = def_map.crate_root(db);

    for (module_id, _) in def_map.modules() {
        // Skip block-expression modules.
        if module_id.is_block_module(db) {
            continue;
        }

        let path = module_path_segments(db, def_map, module_id);
        let qualified = if path.is_empty() {
            crate_name.clone()
        } else {
            format!("{crate_name}::{}", path.join("::"))
        };

        let mut hash_parts: Vec<&str> = vec![
            model.workspace_hash.as_str(),
            "module",
            crate_name.as_str(),
        ];
        for seg in &path {
            hash_parts.push(seg.as_str());
        }
        let module_node_id = NodeId::from_components(&hash_parts);
        module_node_for.insert(module_id, module_node_id);

        let parent_id = if module_id == root_module_id {
            Some(crate_id)
        } else if let Some(parent_module) = def_map.containing_module(module_id) {
            // Compute the parent's NodeId now (we may not have inserted it yet —
            // the iteration order from def_map.modules() is not parent-first).
            let parent_path = module_path_segments(db, def_map, parent_module);
            let mut parts: Vec<&str> = vec![
                model.workspace_hash.as_str(),
                "module",
                crate_name.as_str(),
            ];
            for seg in &parent_path {
                parts.push(seg.as_str());
            }
            Some(NodeId::from_components(&parts))
        } else {
            Some(crate_id)
        };

        let display_name = path.last().cloned().unwrap_or_else(|| crate_name.clone());

        model.insert_node(Node {
            id: module_node_id,
            kind: NodeKind::Module,
            display_name,
            qualified_name: qualified,
            crate_id: Some(crate_id),
            parent_id,
            item_kind: None,
            file: None,
            span: None,
            visibility: None,
        });

        if let Some(parent) = parent_id {
            model.insert_contains(parent, module_node_id);
        }
    }
}

fn crate_display_name(db: &RootDatabase, krate: Crate) -> String {
    krate
        .display_name(db)
        .map(|n| n.canonical_name().as_str().to_string())
        .unwrap_or_else(|| "unknown_crate".to_string())
}

/// Build the module path from crate root to `module_id`, e.g. `["graph", "loader"]`
/// for `crate::graph::loader`. Returns empty for the crate root itself.
fn module_path_segments(db: &RootDatabase, def_map: &DefMap, module_id: ModuleId) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = Some(module_id);
    while let Some(m) = cur {
        if let Some(name) = m.name(db) {
            out.push(name.as_str().to_string());
            cur = def_map.containing_module(m);
        } else {
            break;
        }
    }
    out.reverse();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::loader;
    use crate::graph::model::{BindingKind, NodeKind};
    use std::path::Path;
    use std::sync::OnceLock;

    // Load + extract this workspace once and share across all tests in this
    // module. Loading dominates the runtime (~3s release / ~25s debug); the
    // tests below are read-only assertions over the resulting model.
    // We cache only ExtractionModel (Send+Sync); LoadedWorkspace is dropped
    // because RootDatabase is !Sync.
    fn shared_model() -> &'static ExtractionModel {
        static CACHE: OnceLock<ExtractionModel> = OnceLock::new();
        CACHE.get_or_init(|| {
            let loaded = loader::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
            extract(&loaded)
        })
    }

    #[test]
    fn extracts_workspace_crate_modules_for_self() {
        let model = shared_model();

        assert!(model.nodes.contains_key(&model.workspace_id));

        let crate_node = model
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Crate && n.qualified_name == "file_search_mcp")
            .expect("file_search_mcp crate node");

        let root_module = model
            .nodes
            .values()
            .find(|n| {
                n.kind == NodeKind::Module
                    && n.qualified_name == "file_search_mcp"
                    && n.parent_id == Some(crate_node.id)
            })
            .expect("root module under crate");

        let graph_module = model
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Module && n.qualified_name == "file_search_mcp::graph")
            .expect("graph module");
        assert_eq!(graph_module.parent_id, Some(root_module.id));

        assert!(model
            .contains
            .iter()
            .any(|&(p, c)| p == model.workspace_id && c == crate_node.id));
        assert!(model
            .contains
            .iter()
            .any(|&(p, c)| p == crate_node.id && c == root_module.id));
    }

    #[test]
    fn extracts_items_and_bindings_for_self() {
        let model = shared_model();

        // Item: the `load` function we defined in src/graph/loader.rs.
        let load_fn = model.nodes.values().find(|n| {
            n.kind == NodeKind::Item
                && n.qualified_name == "file_search_mcp::graph::loader::load"
        });
        assert!(load_fn.is_some(), "expected graph::loader::load Item node");

        // At least one declared binding.
        assert!(model.bindings.iter().any(|b| b.kind == BindingKind::Declared));

        // At least one re-export (`pub use loader::{LoadedWorkspace, load};` in graph/mod.rs).
        let load_fn_id = load_fn.unwrap().id;
        let reexport_of_load = model.bindings.iter().find(|b| {
            b.kind == BindingKind::NamedImport
                && b.visible_name == "load"
                && b.target == load_fn_id
        });
        assert!(
            reexport_of_load.is_some(),
            "expected NamedImport binding for `pub use loader::load` in graph/mod.rs"
        );
    }
}

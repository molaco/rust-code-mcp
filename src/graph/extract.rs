//! Extraction passes: HIR → ExtractionModel.
//!
//! Layer 2 milestone: Workspace, Crate, Module nodes plus Contains edges.
//! Layer 3 adds the bindings pass which also creates Item nodes and
//! ExternalSymbol stubs as a side effect.

use std::collections::HashMap;
use std::path::Path;

use ra_ap_base_db::FileId;
use ra_ap_hir::Crate;
use ra_ap_hir_def::ModuleId;
use ra_ap_hir_def::nameres::{DefMap, crate_def_map};
use ra_ap_ide::RootDatabase;
use ra_ap_vfs::Vfs;

use super::attributes::extract_attributes;
use super::bindings::extract_bindings;
use super::ids::{NodeId, workspace_hash};
use super::impls::extract_impl_items;
use super::loader::LoadedWorkspace;
use super::model::{ExtractionModel, Node, NodeKind};
use super::signatures::extract_signatures;
use super::statics::extract_statics;
use super::usages::extract_usages;

pub fn extract(loaded: &LoadedWorkspace) -> ExtractionModel {
    let timing = std::env::var_os("EXTRACT_TIMING").is_some();
    let t_total = std::time::Instant::now();

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
        signatures: Vec::new(),
        statics: Vec::new(),
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
        attributes: Vec::new(),
        crate_target_kind: None,
    });

    let mut crate_node_for: HashMap<Crate, NodeId> = HashMap::new();
    let mut crate_name_for: HashMap<Crate, String> = HashMap::new();
    let mut module_node_for: HashMap<ModuleId, NodeId> = HashMap::new();

    let t = std::time::Instant::now();
    for &krate in &loaded.local_crates {
        emit_crate(
            &mut model,
            &loaded.db,
            &loaded.vfs,
            &loaded.workspace_root,
            krate,
            workspace_id,
            &loaded.crate_target_kinds_by_name,
            &loaded.crate_target_kinds_by_root_file,
            &mut crate_node_for,
            &mut crate_name_for,
            &mut module_node_for,
        );
    }
    if timing {
        eprintln!(
            "extract: emit_crates                  {:>9.2?}  ({} crates, {} modules)",
            t.elapsed(),
            loaded.local_crates.len(),
            module_node_for.len()
        );
    }

    let t = std::time::Instant::now();
    let mut def_to_node = extract_bindings(
        &mut model,
        &loaded.db,
        &loaded.local_crates,
        &crate_node_for,
        &crate_name_for,
        &module_node_for,
    );
    if timing {
        eprintln!(
            "extract: extract_bindings             {:>9.2?}  ({} bindings, {} def→node)",
            t.elapsed(),
            model.bindings.len(),
            def_to_node.len()
        );
    }

    // Layer 4: extend def_to_node with methods, assoc consts, assoc types from
    // inherent impls and trait declarations BEFORE running the usages pass —
    // extract_usages iterates def_to_node and queries `Definition::usages`
    // for each Item, so adding the new defs here makes who_uses(Foo::bar)
    // and who_uses(Trait::method) work for free.
    let t = std::time::Instant::now();
    let nodes_before_impls = model.nodes.len();
    extract_impl_items(
        &mut model,
        &loaded.db,
        &loaded.vfs,
        &loaded.local_crates,
        &crate_node_for,
        &crate_name_for,
        &mut def_to_node,
    );
    if timing {
        eprintln!(
            "extract: extract_impl_items           {:>9.2?}  (+{} nodes; {} total)",
            t.elapsed(),
            model.nodes.len() - nodes_before_impls,
            model.nodes.len()
        );
    }

    // v8: per-Item attribute extraction. Runs after impls (so the v5 method /
    // assoc-const / assoc-type and v7 enum-variant Items already exist in
    // `model.nodes` and `def_to_node`) and before usages (purely for ordering
    // — the two passes are independent).
    let t = std::time::Instant::now();
    extract_attributes(
        &mut model,
        &loaded.db,
        &loaded.vfs,
        &loaded.local_crates,
        &def_to_node,
    );
    if timing {
        let with_attrs = model
            .nodes
            .values()
            .filter(|n| !n.attributes.is_empty())
            .count();
        eprintln!(
            "extract: extract_attributes           {:>9.2?}  ({} items have attrs)",
            t.elapsed(),
            with_attrs
        );
    }

    // v9 (Phase 5): per-function signature extraction. Runs after impls /
    // attributes (so def_to_node already includes inherent/trait assoc fns)
    // and before usages (purely for ordering — independent passes).
    let t = std::time::Instant::now();
    extract_signatures(&mut model, &loaded.db, &loaded.vfs, &def_to_node);
    if timing {
        eprintln!(
            "extract: extract_signatures           {:>9.2?}  ({} signatures)",
            t.elapsed(),
            model.signatures.len()
        );
    }

    // v10 (Phase 7 Path B): per-Static metadata extraction. Runs after
    // signatures and before usages (independent passes — order is purely
    // for the timing breakdown).
    let t = std::time::Instant::now();
    extract_statics(&mut model, &loaded.db, &loaded.vfs, &def_to_node);
    if timing {
        eprintln!(
            "extract: extract_statics              {:>9.2?}  ({} statics)",
            t.elapsed(),
            model.statics.len()
        );
    }

    let t = std::time::Instant::now();
    extract_usages(
        &mut model,
        &loaded.db,
        &loaded.vfs,
        &def_to_node,
        &module_node_for,
    );
    if timing {
        eprintln!(
            "extract: extract_usages               {:>9.2?}  ({} usages)",
            t.elapsed(),
            model.usages.len()
        );
        eprintln!(
            "extract: TOTAL                        {:>9.2?}",
            t_total.elapsed()
        );
    }

    model
}

fn emit_crate(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    workspace_root: &Path,
    krate: Crate,
    workspace_id: NodeId,
    crate_target_kinds_by_name: &HashMap<String, String>,
    crate_target_kinds_by_root_file: &HashMap<String, String>,
    crate_node_for: &mut HashMap<Crate, NodeId>,
    crate_name_for: &mut HashMap<Crate, String>,
    module_node_for: &mut HashMap<ModuleId, NodeId>,
) {
    let crate_name = crate_display_name(db, krate);
    let def_map = crate_def_map(db, krate.base());
    let root_module_id = def_map.crate_root(db);
    let crate_target_kind = crate_target_kind_for(
        db,
        vfs,
        workspace_root,
        &crate_name,
        def_map,
        root_module_id,
        crate_target_kinds_by_name,
        crate_target_kinds_by_root_file,
    );
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
        attributes: Vec::new(),
        crate_target_kind,
    });
    model.insert_contains(workspace_id, crate_id);

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
            attributes: Vec::new(),
            crate_target_kind: None,
        });

        if let Some(parent) = parent_id {
            model.insert_contains(parent, module_node_id);
        }
    }
}

fn crate_target_kind_for(
    db: &RootDatabase,
    vfs: &Vfs,
    workspace_root: &Path,
    crate_name: &str,
    def_map: &DefMap,
    root_module_id: ra_ap_hir_def::ModuleId,
    crate_target_kinds_by_name: &HashMap<String, String>,
    crate_target_kinds_by_root_file: &HashMap<String, String>,
) -> Option<String> {
    let root_file_id = def_map[root_module_id]
        .definition_source_file_id()
        .original_file(db)
        .file_id(db);
    resolve_workspace_relative(vfs, root_file_id, workspace_root)
        .and_then(|root_file| crate_target_kinds_by_root_file.get(&root_file).cloned())
        .or_else(|| crate_target_kinds_by_name.get(crate_name).cloned())
        .or_else(|| Some("lib".to_string()))
}

fn resolve_workspace_relative(
    vfs: &Vfs,
    file_id: FileId,
    workspace_root: &Path,
) -> Option<String> {
    let vfs_path = vfs.file_path(file_id);
    let abs = vfs_path.as_path()?;
    let abs_pathbuf: std::path::PathBuf = abs.to_path_buf().into();
    abs_pathbuf
        .strip_prefix(workspace_root)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
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
            .find(|n| n.kind == NodeKind::Crate && n.qualified_name == "rust_code_mcp")
            .expect("rust_code_mcp crate node");

        let root_module = model
            .nodes
            .values()
            .find(|n| {
                n.kind == NodeKind::Module
                    && n.qualified_name == "rust_code_mcp"
                    && n.parent_id == Some(crate_node.id)
            })
            .expect("root module under crate");

        let graph_module = model
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Module && n.qualified_name == "rust_code_mcp::graph")
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
    fn extracts_impl_items_for_self() {
        use crate::graph::model::ItemKind;

        let model = shared_model();

        // The host type — `OpenedSnapshot` lives in `graph::snapshot`.
        let host = model
            .nodes
            .values()
            .find(|n| {
                n.kind == NodeKind::Item
                    && matches!(n.item_kind, Some(ItemKind::Struct))
                    && n.qualified_name == "rust_code_mcp::graph::snapshot::OpenedSnapshot"
            })
            .expect("OpenedSnapshot struct Item node");

        // Layer 4 should have emitted a Method Item for `usages_of` whose
        // parent is the host struct's Item NodeId. `usages_of` is declared in
        // the inherent `impl OpenedSnapshot { ... }` block in queries.rs.
        let method = model
            .nodes
            .values()
            .find(|n| {
                n.kind == NodeKind::Item
                    && matches!(n.item_kind, Some(ItemKind::Method))
                    && n.display_name == "usages_of"
                    && n.parent_id == Some(host.id)
            })
            .expect(
                "expected Method Item node for OpenedSnapshot::usages_of with \
                 parent_id pointing at the host struct's Item",
            );
        assert_eq!(
            method.qualified_name,
            "rust_code_mcp::graph::snapshot::OpenedSnapshot::usages_of"
        );
        // Layer 4 backfills file/span via try_to_nav.
        assert!(method.file.is_some(), "method Item should have a file path");
        assert!(method.span.is_some(), "method Item should have a span");
    }

    #[test]
    fn extracts_items_and_bindings_for_self() {
        let model = shared_model();

        // Item: the `load` function we defined in src/graph/loader.rs.
        let load_fn = model.nodes.values().find(|n| {
            n.kind == NodeKind::Item
                && n.qualified_name == "rust_code_mcp::graph::loader::load"
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

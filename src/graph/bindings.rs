//! Layer 3 — bindings pass.
//!
//! Enumerates every entry in every local module's `ItemScope` and produces:
//!   * Item nodes (for declared local items, lazily on first encounter)
//!   * ExternalSymbol stubs (for non-local targets)
//!   * Binding records carrying provenance + structured visibility
//!   * Contains edges (for the declared-item case)

use std::collections::{HashMap, HashSet};

use ra_ap_hir::Crate;
use ra_ap_hir_def::HasModule;
use ra_ap_hir_def::item_scope::{ImportOrExternCrate, ImportOrGlob};
use ra_ap_hir_def::nameres::{DefMap, crate_def_map};
use ra_ap_hir_def::per_ns::Item;
use ra_ap_hir_def::visibility::Visibility as HirVisibility;
use ra_ap_hir_def::{AdtId, Lookup, ModuleDefId, ModuleId, UseId};
use ra_ap_ide::RootDatabase;
use ra_ap_syntax::ast::HasVisibility as _;

use super::ids::NodeId;
use super::labels::{
    crate_display_name, item_kind_id_label as item_kind_label, module_qualified_path,
};
use super::model::{
    Binding, BindingKind, BindingVisibility, ExtractionModel, ItemKind, Namespace, Node, NodeKind,
};

/// Returns `def_to_node` so the usage-extraction pass can map ModuleDefIds
/// back to local Item NodeIds without re-running the bindings walk.
pub fn extract_bindings(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    local_crates: &[Crate],
    crate_node_for: &HashMap<Crate, NodeId>,
    crate_name_for: &HashMap<Crate, String>,
    module_node_for: &HashMap<ModuleId, NodeId>,
) -> HashMap<ModuleDefId, NodeId> {
    let mut def_to_node: HashMap<ModuleDefId, NodeId> = HashMap::new();

    // Seed the def→node map with all local modules so module-target imports resolve.
    for (&module_id, &node_id) in module_node_for {
        def_to_node.insert(ModuleDefId::ModuleId(module_id), node_id);
    }

    for &krate in local_crates {
        let def_map = crate_def_map(db, krate.base());
        let crate_name = crate_name_for.get(&krate).cloned().unwrap_or_default();
        let crate_node_id = match crate_node_for.get(&krate).copied() {
            Some(id) => id,
            None => continue,
        };

        for (module_id, _) in def_map.modules() {
            if module_id.is_block_module(db) {
                continue;
            }
            let module_node_id = match module_node_for.get(&module_id).copied() {
                Some(id) => id,
                None => continue,
            };

            let item_scope = &def_map[module_id].scope;

            for (name, type_item) in item_scope.types() {
                let Item { def, vis, import } = type_item;
                let (binding_kind, use_id) = classify_type_provenance(import);
                process_entry(
                    model,
                    db,
                    def_map,
                    &mut def_to_node,
                    module_node_for,
                    module_node_id,
                    crate_node_id,
                    &crate_name,
                    name.as_str(),
                    Namespace::Type,
                    def,
                    vis,
                    binding_kind,
                    use_id,
                );
            }

            for (name, value_item) in item_scope.values() {
                let Item { def, vis, import } = value_item;
                let (binding_kind, use_id) = classify_value_provenance(import);
                process_entry(
                    model,
                    db,
                    def_map,
                    &mut def_to_node,
                    module_node_for,
                    module_node_id,
                    crate_node_id,
                    &crate_name,
                    name.as_str(),
                    Namespace::Value,
                    def,
                    vis,
                    binding_kind,
                    use_id,
                );
            }
        }
    }

    // Post-hoc dedup. ADTs (especially unit/tuple structs and unit variants)
    // appear in BOTH the type and value namespaces with the same target — the
    // two walks above emit one Binding each. We dedup on
    // (from_module, visible_name, target, kind), keeping the first occurrence
    // (the type-walk row). Records still survive when they genuinely diverge:
    // if classify_type_provenance and classify_value_provenance produce
    // different BindingKinds for the same import, both rows are retained.
    let mut seen: HashSet<(NodeId, String, NodeId, BindingKind)> =
        HashSet::with_capacity(model.bindings.len());
    model
        .bindings
        .retain(|b| seen.insert((b.from_module, b.visible_name.clone(), b.target, b.kind)));

    def_to_node
}

#[allow(clippy::too_many_arguments)]
fn process_entry(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    def_map: &DefMap,
    def_to_node: &mut HashMap<ModuleDefId, NodeId>,
    module_node_for: &HashMap<ModuleId, NodeId>,
    from_module: NodeId,
    from_crate: NodeId,
    from_crate_name: &str,
    visible_name: &str,
    namespace: Namespace,
    def_id: ModuleDefId,
    vis: HirVisibility,
    binding_kind: BindingKind,
    use_id: Option<UseId>,
) {
    // v1 exclusions: macros, builtins, enum variants in scope.
    if matches!(
        def_id,
        ModuleDefId::MacroId(_)
            | ModuleDefId::BuiltinType(_)
            | ModuleDefId::EnumVariantId(_)
    ) {
        return;
    }

    let target_node_id = match resolve_or_create_target(
        model,
        db,
        def_to_node,
        module_node_for,
        def_id,
        from_crate_name,
        visible_name,
    ) {
        Some(id) => id,
        None => return,
    };

    // Contains edge for declared local items (only on first declaration encounter).
    if binding_kind == BindingKind::Declared
        && matches!(model.nodes.get(&target_node_id).map(|n| n.kind), Some(NodeKind::Item))
        && model.nodes.get(&target_node_id).and_then(|n| n.parent_id) == Some(from_module)
    {
        // Skip the contains edge if it would duplicate one already added by this same call.
        // (Re-encounter via a glob import will hit the early `def_to_node` cache and never get here.)
        let already = model
            .contains
            .iter()
            .any(|&(p, c)| p == from_module && c == target_node_id);
        if !already {
            model.insert_contains(from_module, target_node_id);
        }
    }

    let visibility = encode_visibility(model, db, def_map, vis, from_crate, module_node_for);
    let is_explicit_pub_use = match use_id {
        Some(uid) => use_has_explicit_visibility(db, uid),
        None => false,
    };

    model.bindings.push(Binding {
        from_module,
        namespace,
        visible_name: visible_name.to_string(),
        target: target_node_id,
        kind: binding_kind,
        visibility,
        is_explicit_pub_use,
    });
}

fn resolve_or_create_target(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    def_to_node: &mut HashMap<ModuleDefId, NodeId>,
    module_node_for: &HashMap<ModuleId, NodeId>,
    def_id: ModuleDefId,
    from_crate_name: &str,
    visible_name: &str,
) -> Option<NodeId> {
    if let Some(id) = def_to_node.get(&def_id).copied() {
        return Some(id);
    }

    let owner_module_id = module_def_owner_module(db, def_id)?;

    if let Some(&local_module_node_id) = module_node_for.get(&owner_module_id) {
        // Local item: create an Item node now.
        let crate_name = crate_display_name(db, owner_module_id.krate(db).into());
        let node_id = create_local_item_node(
            model,
            db,
            def_id,
            local_module_node_id,
            &crate_name,
            visible_name,
        )?;
        def_to_node.insert(def_id, node_id);
        Some(node_id)
    } else {
        // Non-local item: stub.
        let stub_qualified = stub_qualified_name(db, def_id, from_crate_name, visible_name);
        let node_id = NodeId::from_components(&[
            model.workspace_hash.as_str(),
            "external_symbol",
            stub_qualified.as_str(),
        ]);
        if !model.nodes.contains_key(&node_id) {
            let display = stub_qualified
                .rsplit("::")
                .next()
                .unwrap_or(stub_qualified.as_str())
                .to_string();
            model.insert_node(Node {
                id: node_id,
                kind: NodeKind::ExternalSymbol,
                display_name: display,
                qualified_name: stub_qualified,
                crate_id: None,
                parent_id: None,
                item_kind: None,
                file: None,
                span: None,
                visibility: None,
                attributes: Vec::new(),
                crate_target_kind: None,
            });
        }
        def_to_node.insert(def_id, node_id);
        Some(node_id)
    }
}

fn create_local_item_node(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    def_id: ModuleDefId,
    module_node_id: NodeId,
    from_crate_name: &str,
    fallback_name: &str,
) -> Option<NodeId> {
    let item_kind = item_kind_for_def(def_id)?;
    let name = name_for_def(db, def_id).unwrap_or_else(|| fallback_name.to_string());

    let module_qual = model
        .nodes
        .get(&module_node_id)
        .map(|n| n.qualified_name.clone())
        .unwrap_or_else(|| from_crate_name.to_string());

    let qualified = format!("{module_qual}::{name}");

    let node_id = NodeId::from_components(&[
        model.workspace_hash.as_str(),
        "item",
        from_crate_name,
        module_qual.as_str(),
        item_kind_label(item_kind),
        name.as_str(),
    ]);

    let crate_id = model.nodes.get(&module_node_id).and_then(|n| n.crate_id);

    model.insert_node(Node {
        id: node_id,
        kind: NodeKind::Item,
        display_name: name,
        qualified_name: qualified,
        crate_id,
        parent_id: Some(module_node_id),
        item_kind: Some(item_kind),
        file: None,
        span: None,
        visibility: None,
        attributes: Vec::new(),
        crate_target_kind: None,
    });

    Some(node_id)
}

fn item_kind_for_def(def_id: ModuleDefId) -> Option<ItemKind> {
    Some(match def_id {
        ModuleDefId::FunctionId(_) => ItemKind::Function,
        ModuleDefId::AdtId(AdtId::StructId(_)) => ItemKind::Struct,
        ModuleDefId::AdtId(AdtId::EnumId(_)) => ItemKind::Enum,
        ModuleDefId::AdtId(AdtId::UnionId(_)) => ItemKind::Union,
        ModuleDefId::TraitId(_) => ItemKind::Trait,
        ModuleDefId::TypeAliasId(_) => ItemKind::TypeAlias,
        ModuleDefId::ConstId(_) => ItemKind::Const,
        ModuleDefId::StaticId(_) => ItemKind::Static,
        _ => return None,
    })
}

fn name_for_def(db: &RootDatabase, def_id: ModuleDefId) -> Option<String> {
    use ra_ap_hir::{Const, Enum, Function, Static, Struct, Trait, TypeAlias, Union};
    Some(match def_id {
        ModuleDefId::FunctionId(id) => Function::from(id).name(db).as_str().to_string(),
        ModuleDefId::AdtId(AdtId::StructId(id)) => Struct::from(id).name(db).as_str().to_string(),
        ModuleDefId::AdtId(AdtId::EnumId(id)) => Enum::from(id).name(db).as_str().to_string(),
        ModuleDefId::AdtId(AdtId::UnionId(id)) => Union::from(id).name(db).as_str().to_string(),
        ModuleDefId::TraitId(id) => Trait::from(id).name(db).as_str().to_string(),
        ModuleDefId::TypeAliasId(id) => TypeAlias::from(id).name(db).as_str().to_string(),
        ModuleDefId::ConstId(id) => Const::from(id).name(db)?.as_str().to_string(),
        ModuleDefId::StaticId(id) => Static::from(id).name(db).as_str().to_string(),
        _ => return None,
    })
}

fn module_def_owner_module(db: &RootDatabase, def_id: ModuleDefId) -> Option<ModuleId> {
    Some(match def_id {
        ModuleDefId::ModuleId(id) => id,
        ModuleDefId::FunctionId(id) => id.module(db),
        ModuleDefId::AdtId(AdtId::StructId(id)) => id.module(db),
        ModuleDefId::AdtId(AdtId::EnumId(id)) => id.module(db),
        ModuleDefId::AdtId(AdtId::UnionId(id)) => id.module(db),
        ModuleDefId::TraitId(id) => id.module(db),
        ModuleDefId::TypeAliasId(id) => id.module(db),
        ModuleDefId::ConstId(id) => id.module(db),
        ModuleDefId::StaticId(id) => id.module(db),
        _ => return None,
    })
}

fn stub_qualified_name(
    db: &RootDatabase,
    def_id: ModuleDefId,
    from_crate_name: &str,
    visible_name: &str,
) -> String {
    if let Some(owner) = module_def_owner_module(db, def_id) {
        let qual = module_qualified_path(db, owner);
        if let Some(n) = name_for_def(db, def_id) {
            return format!("{qual}::{n}");
        }
        return format!("{qual}::{visible_name}");
    }
    format!("extern::{from_crate_name}::{visible_name}")
}

fn classify_type_provenance(p: Option<ImportOrExternCrate>) -> (BindingKind, Option<UseId>) {
    match p {
        None => (BindingKind::Declared, None),
        Some(ImportOrExternCrate::Import(id)) => (BindingKind::NamedImport, Some(id.use_)),
        Some(ImportOrExternCrate::Glob(id)) => (BindingKind::GlobImport, Some(id.use_)),
        // ExternCrate doesn't carry a UseId. We don't try to recover its
        // syntactic visibility; downstream filters treat extern-crate
        // bindings as never explicitly `pub`-marked.
        Some(ImportOrExternCrate::ExternCrate(_)) => (BindingKind::ExternCrateImport, None),
    }
}

fn classify_value_provenance(p: Option<ImportOrGlob>) -> (BindingKind, Option<UseId>) {
    match p {
        None => (BindingKind::Declared, None),
        Some(ImportOrGlob::Import(id)) => (BindingKind::NamedImport, Some(id.use_)),
        Some(ImportOrGlob::Glob(id)) => (BindingKind::GlobImport, Some(id.use_)),
    }
}

/// True iff the `use` declaration at `use_id` carries an explicit visibility
/// modifier (`pub`, `pub(crate)`, `pub(in path)`, or `pub(super)`) in syntax.
/// HIR normalizes inherited visibilities, so consulting the post-resolution
/// `Visibility` would conflate "explicitly inherited from a `pub` module" with
/// "explicitly marked `pub`". We instead read the source AST directly.
fn use_has_explicit_visibility(db: &RootDatabase, use_id: UseId) -> bool {
    let loc = use_id.lookup(db);
    let use_node = loc.id.to_node(db);
    use_node.visibility().is_some()
}

fn encode_visibility(
    model: &ExtractionModel,
    db: &RootDatabase,
    _def_map: &DefMap,
    vis: HirVisibility,
    from_crate: NodeId,
    module_node_for: &HashMap<ModuleId, NodeId>,
) -> BindingVisibility {
    match vis {
        HirVisibility::Public => BindingVisibility::Public,
        HirVisibility::PubCrate(crate_id) => {
            let crate_name = ra_ap_hir::Crate::from(crate_id)
                .display_name(db)
                .map(|n| n.canonical_name().as_str().to_string())
                .unwrap_or_default();
            let crate_node_id = NodeId::from_components(&[
                model.workspace_hash.as_str(),
                "crate",
                crate_name.as_str(),
            ]);
            if model.nodes.contains_key(&crate_node_id) {
                BindingVisibility::Crate(crate_node_id)
            } else if from_crate == crate_node_id {
                BindingVisibility::Crate(from_crate)
            } else {
                BindingVisibility::Private
            }
        }
        HirVisibility::Module(restrict_module_id, _explicitness) => {
            // Restricted to a specific module subtree.
            if let Some(&node_id) = module_node_for.get(&restrict_module_id) {
                BindingVisibility::RestrictedTo(node_id)
            } else {
                BindingVisibility::Private
            }
        }
    }
}

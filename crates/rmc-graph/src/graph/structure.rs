//! Structural owners absent from module ItemScopes: fields and block items.
use std::collections::HashMap;

use ra_ap_hir::{Adt, Crate, HasCrate, ModuleDef, Semantics, attach_db};
use ra_ap_hir_def::{ModuleDefId, ModuleId};
use ra_ap_ide::TryToNav;
use ra_ap_ide_db::{RootDatabase, defs::Definition};
use ra_ap_syntax::{AstNode, SyntaxNode, ast};
use ra_ap_vfs::Vfs;

use super::audit_util::resolve_workspace_relative;
use super::ids::NodeId;
use super::model::{ExtractionModel, ItemKind, Node, NodeKind};
use super::usages::{TargetIds, local_module_files, target_def_id};

pub(super) fn declaration_definition<'db>(sema: &Semantics<'db, RootDatabase>, node: &SyntaxNode) -> Option<Definition<'db>> {
    macro_rules! resolve {
        ($ast:ty, $variant:ident) => {
            if let Some(item) = <$ast>::cast(node.clone()) {
                return sema.to_def(&item).map(Definition::$variant);
            }
        };
    }
    resolve!(ast::Fn, Function);
    resolve!(ast::RecordField, Field);
    resolve!(ast::TupleField, Field);
    resolve!(ast::TypeAlias, TypeAlias);
    resolve!(ast::Const, Const);
    resolve!(ast::Static, Static);
    resolve!(ast::Adt, Adt);
    resolve!(ast::Trait, Trait);
    resolve!(ast::Variant, EnumVariant);
    None
}

/// A lossless parse still contains disabled cfg/test items. Name resolution
/// inside one may fall back to a module resolver and produce plausible false
/// edges. Check every enclosing declaration, including inline cfg modules.
pub(super) fn is_active(sema: &Semantics<'_, RootDatabase>, node: &SyntaxNode) -> bool {
    sema.ancestors_with_macros(node.clone()).all(|node| {
        if let Some(item) = ast::Module::cast(node.clone()) {
            return sema.to_def(&item).is_some();
        }
        if let Some(item) = ast::Impl::cast(node.clone()) {
            return sema.to_def(&item).is_some();
        }
        let declaration = ast::Fn::can_cast(node.kind()) || ast::Adt::can_cast(node.kind())
            || ast::Trait::can_cast(node.kind()) || ast::TypeAlias::can_cast(node.kind())
            || ast::Const::can_cast(node.kind()) || ast::Static::can_cast(node.kind())
            || ast::RecordField::can_cast(node.kind()) || ast::TupleField::can_cast(node.kind())
            || ast::Variant::can_cast(node.kind());
        !declaration || declaration_definition(sema, &node).is_some()
    })
}

pub(super) fn extract_structural_items(
    model: &mut ExtractionModel, db: &RootDatabase, vfs: &Vfs, crates: &[Crate],
    defs: &mut HashMap<ModuleDefId, NodeId>, modules: &HashMap<ModuleId, NodeId>,
) {
    let root = model.workspace_root.clone();
    let hash = model.workspace_hash.clone();
    let mut targets = TargetIds::new(vfs, &root, &hash, modules);
    attach_db(db, || {
        for &krate in crates {
          let sema = Semantics::new_for_crate(db, krate);
          for file in local_module_files(db, &[krate]) {
            let tree = sema.parse(file).syntax().clone();
            for node in tree.descendants() {
                let Some(def) = declaration_definition(&sema, &node) else { continue; };
                if !is_active(&sema, &node) { continue; }
                let Some(id) = targets.of_definition(&sema, def) else { continue; };
                if let Some(def_id) = target_def_id(def) { defs.entry(def_id).or_insert(id); }
                if model.nodes.contains_key(&id) { continue; }
                let parent = sema.ancestors_with_macros(node.clone()).skip(1)
                    .filter_map(|n| declaration_definition(&sema, &n))
                    .find_map(|d| targets.of_definition(&sema, d))
                    .or_else(|| {
                        let module = def.module(db)?.nearest_non_block_module(db);
                        modules.get(&module.into()).copied()
                    });
                let Some(parent_node) = parent.and_then(|id| model.nodes.get(&id)).cloned() else { continue; };
                let Some(nav) = def.try_to_nav(&sema).map(|n| n.call_site) else { continue; };
                let Some(file) = resolve_workspace_relative(vfs, nav.file_id, &root) else { continue; };
                let Some(name) = def.name(db).map(|n| n.as_str().to_owned()) else { continue; };
                let kind = match def {
                    Definition::Field(_) => ItemKind::Field,
                    Definition::EnumVariant(_) => ItemKind::EnumVariant,
                    _ => super::bindings::item_kind_for_def(target_def_id(def).unwrap()).unwrap(),
                };
                model.insert_node(Node {
                    id, kind: NodeKind::Item, display_name: name.clone(),
                    qualified_name: format!("{}::{name}", parent_node.qualified_name),
                    crate_id: parent_node.crate_id, parent_id: Some(parent_node.id), item_kind: Some(kind),
                    file: Some(file), span: Some((u32::from(nav.full_range.start()), u32::from(nav.full_range.end()))),
                    visibility: None, attributes: Vec::new(), crate_target_kind: None,
                    declaration_module: def.module(db).and_then(|m| modules.get(&m.nearest_non_block_module(db).into()).copied()),
                });
                model.insert_contains(parent_node.id, id);
            }
          }
        }
        // Macro-written field declarations are not AST descendants of the
        // source file. HIR ADTs still expose them, and their owning node must
        // exist before a consumer can resolve a field target.
        let sema = Semantics::new(db);
        let fields: Vec<_> = defs.keys().filter_map(|id| match ModuleDef::from(*id) {
            ModuleDef::Adt(adt) if crates.contains(&adt.krate(db)) => Some(match adt {
                Adt::Struct(s) => s.fields(db),
                Adt::Union(u) => u.fields(db),
                Adt::Enum(e) => e.variants(db).into_iter().flat_map(|v| v.fields(db)).collect(),
            }),
            _ => None,
        }).flatten().collect();
        for field in fields {
            let def = Definition::Field(field);
            let Some(id) = targets.of_definition(&sema, def) else { continue; };
            if model.nodes.contains_key(&id) { continue; }
            let Some(parent) = targets.of_definition(&sema, field.parent_def(db).into()) else { continue; };
            let Some(parent_node) = model.nodes.get(&parent).cloned() else { continue; };
            let Some(nav) = def.try_to_nav(&sema).map(|n| n.call_site) else { continue; };
            let Some(file) = resolve_workspace_relative(vfs, nav.file_id, &root) else { continue; };
            let name = field.name(db).as_str().to_owned();
            model.insert_node(Node {
                id, kind: NodeKind::Item, display_name: name.clone(),
                qualified_name: format!("{}::{name}", parent_node.qualified_name),
                crate_id: parent_node.crate_id, parent_id: Some(parent), item_kind: Some(ItemKind::Field),
                file: Some(file), span: Some((u32::from(nav.full_range.start()), u32::from(nav.full_range.end()))),
                visibility: None, attributes: Vec::new(), crate_target_kind: None,
                declaration_module: parent_node.declaration_module,
            });
            model.insert_contains(parent, id);
        }
    });
}

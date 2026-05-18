//! Layer 4 — impl-block and trait-declaration item extraction.
//!
//! Walks every inherent `impl Foo { ... }` block of every local Adt and every
//! local trait declaration's items, emitting Item nodes for the methods,
//! associated consts, and associated types found inside. These nodes become
//! valid targets for `Definition::usages` so that `who_uses(Foo::bar)` and
//! `who_uses(Trait::method)` can answer non-empty.
//!
//! Trait *impl* method bodies (`impl T for Foo { fn m() {...} }`) are
//! deliberately NOT extracted — RA's `Definition::usages` resolves call sites
//! `x.m()` and `Foo::m()` back to the trait declaration's def, so the trait
//! Item alone covers `who_uses` for trait dispatch. Adding impl-body items
//! would emit duplicate nodes that just shadow the trait declaration.

use std::collections::HashMap;
use std::path::Path;

use ra_ap_hir::{AssocItem, Crate, Enum, EnumVariant, HasCrate, Impl, Semantics, Trait, attach_db};
use ra_ap_hir_def::{AdtId, ModuleDefId, TraitId};
use ra_ap_ide::TryToNav;
use ra_ap_ide_db::RootDatabase;
use ra_ap_ide_db::defs::Definition;
use ra_ap_vfs::Vfs;

use super::audit_util::resolve_workspace_relative;
use super::ids::NodeId;
use super::model::{ExtractionModel, ItemKind, Node, NodeKind};

pub fn extract_impl_items(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    local_crates: &[Crate],
    crate_node_for: &HashMap<Crate, NodeId>,
    crate_name_for: &HashMap<Crate, String>,
    def_to_node: &mut HashMap<ModuleDefId, NodeId>,
) {
    let workspace_root = model.workspace_root.clone();

    // Snapshot ADT and Trait def→node mappings up front. Iterating
    // `def_to_node` directly while mutating it inside the loop would borrow-conflict.
    let adt_node_for: HashMap<AdtId, NodeId> = def_to_node
        .iter()
        .filter_map(|(&def, &node)| match def {
            ModuleDefId::AdtId(id) => Some((id, node)),
            _ => None,
        })
        .collect();
    let trait_node_for: HashMap<TraitId, NodeId> = def_to_node
        .iter()
        .filter_map(|(&def, &node)| match def {
            ModuleDefId::TraitId(id) => Some((id, node)),
            _ => None,
        })
        .collect();

    attach_db(db, || {
        let sema = Semantics::new(db);

        for &krate in local_crates {
            let crate_node_id = match crate_node_for.get(&krate).copied() {
                Some(id) => id,
                None => continue,
            };
            let crate_name = crate_name_for.get(&krate).cloned().unwrap_or_default();

            // Inherent-impl items.
            for impl_ in Impl::all_in_crate(db, krate) {
                // Only inherent impls (no trait): trait-impl bodies are deferred.
                if impl_.trait_(db).is_some() {
                    continue;
                }
                let Some(adt) = impl_.self_ty(db).as_adt() else {
                    continue;
                };
                let adt_id: AdtId = adt.into();
                let Some(&adt_node_id) = adt_node_for.get(&adt_id) else {
                    // Out-of-workspace ADT — skip. (Inherent impls of types
                    // declared in dep crates aren't tracked in v1.)
                    continue;
                };
                if adt.krate(db) != krate {
                    // Defensive: an inherent impl found in this crate's
                    // def_map should always have a self-ty defined here, but
                    // skip gracefully if not.
                    continue;
                }

                for assoc in impl_.items(db) {
                    emit_assoc_item(
                        model,
                        &sema,
                        vfs,
                        &workspace_root,
                        def_to_node,
                        crate_node_id,
                        &crate_name,
                        adt_node_id,
                        assoc,
                    );
                }
            }

            // Trait-declaration items.
            for (&trait_id, &trait_node_id) in &trait_node_for {
                let trait_: Trait = trait_id.into();
                if trait_.krate(db) != krate {
                    continue;
                }
                for assoc in trait_.items(db) {
                    emit_assoc_item(
                        model,
                        &sema,
                        vfs,
                        &workspace_root,
                        def_to_node,
                        crate_node_id,
                        &crate_name,
                        trait_node_id,
                        assoc,
                    );
                }
            }

            // v7: enum-variant items. Variants don't appear in module
            // ItemScope by default (only when brought in via `use Foo::*`),
            // so the bindings pass would miss them — walk every local
            // enum's variants directly here.
            for (&adt_id, &enum_node_id) in &adt_node_for {
                let AdtId::EnumId(enum_id) = adt_id else {
                    continue;
                };
                let enum_: Enum = enum_id.into();
                if enum_.krate(db) != krate {
                    continue;
                }
                for variant in enum_.variants(db) {
                    emit_enum_variant(
                        model,
                        &sema,
                        vfs,
                        &workspace_root,
                        def_to_node,
                        crate_node_id,
                        &crate_name,
                        enum_node_id,
                        variant,
                    );
                }
            }
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn emit_assoc_item(
    model: &mut ExtractionModel,
    sema: &Semantics<'_, RootDatabase>,
    vfs: &Vfs,
    workspace_root: &Path,
    def_to_node: &mut HashMap<ModuleDefId, NodeId>,
    crate_node_id: NodeId,
    crate_name: &str,
    parent_node_id: NodeId,
    assoc: AssocItem,
) {
    let db = sema.db;
    let (item_kind, def, def_id, name): (ItemKind, Definition, ModuleDefId, String) = match assoc {
        AssocItem::Function(f) => {
            let n = f.name(db).as_str().to_string();
            let id = match ra_ap_hir_def::FunctionId::try_from(f) {
                Ok(id) => id,
                Err(_) => return,
            };
            (
                ItemKind::Method,
                Definition::Function(f),
                ModuleDefId::FunctionId(id),
                n,
            )
        }
        AssocItem::Const(c) => {
            // Anonymous consts (impl `const _: ()`) have no name — skip.
            let n = match c.name(db) {
                Some(n) => n.as_str().to_string(),
                None => return,
            };
            let id: ra_ap_hir_def::ConstId = c.into();
            (
                ItemKind::AssocConst,
                Definition::Const(c),
                ModuleDefId::ConstId(id),
                n,
            )
        }
        AssocItem::TypeAlias(t) => {
            let n = t.name(db).as_str().to_string();
            let id: ra_ap_hir_def::TypeAliasId = t.into();
            (
                ItemKind::AssocType,
                Definition::TypeAlias(t),
                ModuleDefId::TypeAliasId(id),
                n,
            )
        }
    };

    // Resolve declaration site for byte-range disambiguation.
    let nav = match def.try_to_nav(sema) {
        Some(n) => n.call_site,
        None => return, // macro-only / synthetic; can't disambiguate.
    };
    let rel_path = match resolve_workspace_relative(vfs, nav.file_id, workspace_root) {
        Some(p) => p,
        None => return, // declaration in dep-crate / sysroot file, not our problem.
    };
    let start: u32 = u32::from(nav.full_range.start());
    let end: u32 = u32::from(nav.full_range.end());

    let parent_qual = model
        .nodes
        .get(&parent_node_id)
        .map(|n| n.qualified_name.clone())
        .unwrap_or_default();
    let qualified = if parent_qual.is_empty() {
        name.clone()
    } else {
        format!("{parent_qual}::{name}")
    };

    // NodeId scheme (per Layer 4 spec): mix workspace_hash, kind label, crate,
    // file path, byte offset, name. The byte offset disambiguates multiple
    // inherent impls on the same type that re-declare a same-named method.
    let byte_offset = start.to_string();
    let kind_label = match item_kind {
        ItemKind::Method => "method",
        ItemKind::AssocConst => "assoc_const",
        ItemKind::AssocType => "assoc_type",
        _ => unreachable!(),
    };
    let node_id = NodeId::from_components(&[
        model.workspace_hash.as_str(),
        kind_label,
        crate_name,
        rel_path.as_str(),
        byte_offset.as_str(),
        name.as_str(),
    ]);

    // If we somehow already have this id (re-entry during refactors), fold
    // into existing node and just register the def→node mapping.
    if !model.nodes.contains_key(&node_id) {
        model.insert_node(Node {
            id: node_id,
            kind: NodeKind::Item,
            display_name: name,
            qualified_name: qualified,
            crate_id: Some(crate_node_id),
            parent_id: Some(parent_node_id),
            item_kind: Some(item_kind),
            file: Some(rel_path),
            span: Some((start, end)),
            visibility: None,
            attributes: Vec::new(),
            crate_target_kind: None,
        });
        model.insert_contains(parent_node_id, node_id);
    }

    // Register so extract_usages picks this def up. Don't overwrite if a
    // canonical mapping already exists (e.g. trait fn referenced from outside
    // before trait extraction).
    def_to_node.entry(def_id).or_insert(node_id);
}

/// v7: emit an Item node for one enum variant, parented to the host enum's
/// Item NodeId. NodeId scheme mirrors `emit_assoc_item`'s
/// `[workspace_hash, "enum_variant", crate, file, byte_offset, name]` so two
/// enums in the same module declaring same-named variants don't collide.
/// Visibility is `None` (variants inherit from the parent enum, just like
/// Method/AssocConst/AssocType).
#[allow(clippy::too_many_arguments)]
fn emit_enum_variant(
    model: &mut ExtractionModel,
    sema: &Semantics<'_, RootDatabase>,
    vfs: &Vfs,
    workspace_root: &Path,
    def_to_node: &mut HashMap<ModuleDefId, NodeId>,
    crate_node_id: NodeId,
    crate_name: &str,
    enum_node_id: NodeId,
    variant: EnumVariant,
) {
    let db = sema.db;
    let name = variant.name(db).as_str().to_string();

    let nav = match Definition::EnumVariant(variant).try_to_nav(sema) {
        Some(n) => n.call_site,
        None => return, // synthetic / macro-only — skip.
    };
    let rel_path = match resolve_workspace_relative(vfs, nav.file_id, workspace_root) {
        Some(p) => p,
        None => return, // declaration not under the workspace root.
    };
    let start: u32 = u32::from(nav.full_range.start());
    let end: u32 = u32::from(nav.full_range.end());

    let parent_qual = model
        .nodes
        .get(&enum_node_id)
        .map(|n| n.qualified_name.clone())
        .unwrap_or_default();
    let qualified = if parent_qual.is_empty() {
        name.clone()
    } else {
        format!("{parent_qual}::{name}")
    };

    let byte_offset = start.to_string();
    let node_id = NodeId::from_components(&[
        model.workspace_hash.as_str(),
        "enum_variant",
        crate_name,
        rel_path.as_str(),
        byte_offset.as_str(),
        name.as_str(),
    ]);

    if !model.nodes.contains_key(&node_id) {
        model.insert_node(Node {
            id: node_id,
            kind: NodeKind::Item,
            display_name: name,
            qualified_name: qualified,
            crate_id: Some(crate_node_id),
            parent_id: Some(enum_node_id),
            item_kind: Some(ItemKind::EnumVariant),
            file: Some(rel_path),
            span: Some((start, end)),
            visibility: None,
            attributes: Vec::new(),
            crate_target_kind: None,
        });
        model.insert_contains(enum_node_id, node_id);
    }

    let variant_id: ra_ap_hir_def::EnumVariantId = variant.into();
    def_to_node
        .entry(ModuleDefId::EnumVariantId(variant_id))
        .or_insert(node_id);
}

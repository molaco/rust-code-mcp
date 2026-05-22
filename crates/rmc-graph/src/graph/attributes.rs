//! Layer 4 (v8) — item-attribute extraction.
//!
//! For each local Item already emitted by Layers 3/4, walk the AST source
//! node (via `HasSource::source(db)`), collect the outer attributes
//! (`#[derive(...)]`, `#[must_use]`, `#[non_exhaustive]`, `#[inline]`, etc.)
//! and doc-comment lines (`/// ...`), and stash them on
//! `Node.attributes` so the read-side queries don't have to re-parse source
//! to answer attribute questions.
//!
//! Storage shape: `Vec<String>`, one entry per syntactic attribute or per
//! doc-comment line. Doc comments are normalized to `"/// <line>"` form so
//! callers can pattern-match without worrying about whether the source had
//! `///` (outer) or `//!` (inner) — this pass only collects outer doc
//! comments anyway. Inner attributes / inner doc-comments on items aren't
//! a thing for items per se, only for modules and the crate root.
//!
//! Items whose AST source can't be resolved (macro-only / synthetic) are
//! skipped silently — `def.source(db)` returns `None`.
//!
//! Mirrors `impls.rs` shape: open `Semantics`, walk `Module::declarations`
//! for top-level items, walk `Trait::items` and `Impl::all_in_crate(...).items`
//! for associated items, walk `Enum::variants` for variant attrs.

use std::collections::HashMap;

use ra_ap_hir::{
    Adt, AssocItem, Crate, HasCrate, HasSource, Module, ModuleDef, Semantics, attach_db,
};
use ra_ap_hir_def::{AdtId, ModuleDefId};
use ra_ap_ide_db::RootDatabase;
use ra_ap_syntax::ast::{HasAttrs, HasDocComments};
use ra_ap_syntax::{AstNode, AstToken};
use ra_ap_vfs::Vfs;

use super::ids::NodeId;
use super::model::ExtractionModel;

pub(crate) fn extract_attributes(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    _vfs: &Vfs,
    local_crates: &[Crate],
    def_to_node: &HashMap<ModuleDefId, NodeId>,
) {
    attach_db(db, || {
        let sema = Semantics::new(db);

        for &krate in local_crates {
            // Walk every module reachable from the crate root.
            let root: Module = krate.root_module(db);
            visit_module(model, &sema, def_to_node, krate, root);
        }
    });
}

fn visit_module(
    model: &mut ExtractionModel,
    sema: &Semantics<'_, RootDatabase>,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    krate: Crate,
    module: Module,
) {
    let db = sema.db;

    // 1. Top-level declarations.
    for def in module.declarations(db) {
        match def {
            ModuleDef::Module(child) => {
                // Recurse into sub-modules (only those owned by this crate).
                if child.krate(db) == krate {
                    visit_module(model, sema, def_to_node, krate, child);
                }
            }
            ModuleDef::Function(f) => {
                let id = match ra_ap_hir_def::FunctionId::try_from(f) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                if let Some(src) = f.source(db) {
                    set_attrs_for(model, def_to_node, ModuleDefId::FunctionId(id), &src.value);
                }
            }
            ModuleDef::Adt(adt) => {
                visit_adt(model, sema, def_to_node, adt);
            }
            ModuleDef::Trait(t) => {
                let trait_id: ra_ap_hir_def::TraitId = t.into();
                if let Some(src) = t.source(db) {
                    set_attrs_for(model, def_to_node, ModuleDefId::TraitId(trait_id), &src.value);
                }
                // Trait-declaration assoc items.
                for assoc in t.items(db) {
                    visit_assoc_item(model, sema, def_to_node, assoc);
                }
            }
            ModuleDef::TypeAlias(t) => {
                let id: ra_ap_hir_def::TypeAliasId = t.into();
                if let Some(src) = t.source(db) {
                    set_attrs_for(model, def_to_node, ModuleDefId::TypeAliasId(id), &src.value);
                }
            }
            ModuleDef::Const(c) => {
                let id: ra_ap_hir_def::ConstId = c.into();
                if let Some(src) = c.source(db) {
                    set_attrs_for(model, def_to_node, ModuleDefId::ConstId(id), &src.value);
                }
            }
            ModuleDef::Static(s) => {
                let id: ra_ap_hir_def::StaticId = s.into();
                if let Some(src) = s.source(db) {
                    set_attrs_for(model, def_to_node, ModuleDefId::StaticId(id), &src.value);
                }
            }
            // Macros, builtin types, variants-as-decls, and trait aliases
            // aren't modeled as Item Nodes (filtered upstream in
            // bindings.rs::process_entry).
            _ => {}
        }
    }

    // 2. Inherent impls of every local Adt — assoc items inside.
    for impl_ in module.impl_defs(db) {
        // Skip trait impls (their bodies aren't extracted as Item nodes,
        // matching impls.rs's policy).
        if impl_.trait_(db).is_some() {
            continue;
        }
        // Defensive: only walk impls whose self-ty is a local ADT.
        let Some(adt) = impl_.self_ty(db).as_adt() else {
            continue;
        };
        if adt.krate(db) != krate {
            continue;
        }
        for assoc in impl_.items(db) {
            visit_assoc_item(model, sema, def_to_node, assoc);
        }
    }
}

fn visit_adt(
    model: &mut ExtractionModel,
    sema: &Semantics<'_, RootDatabase>,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    adt: Adt,
) {
    let db = sema.db;
    match adt {
        Adt::Struct(s) => {
            let id: AdtId = AdtId::StructId(s.into());
            if let Some(src) = s.source(db) {
                set_attrs_for(model, def_to_node, ModuleDefId::AdtId(id), &src.value);
            }
        }
        Adt::Union(u) => {
            let id: AdtId = AdtId::UnionId(u.into());
            if let Some(src) = u.source(db) {
                set_attrs_for(model, def_to_node, ModuleDefId::AdtId(id), &src.value);
            }
        }
        Adt::Enum(e) => {
            let id: AdtId = AdtId::EnumId(e.into());
            if let Some(src) = e.source(db) {
                set_attrs_for(model, def_to_node, ModuleDefId::AdtId(id), &src.value);
            }
            // v7 enum variants — each variant carries its own attrs
            // (`#[non_exhaustive]` per-variant, doc comments, etc.).
            for variant in e.variants(db) {
                let variant_id: ra_ap_hir_def::EnumVariantId = variant.into();
                if let Some(src) = variant.source(db) {
                    set_attrs_for(
                        model,
                        def_to_node,
                        ModuleDefId::EnumVariantId(variant_id),
                        &src.value,
                    );
                }
            }
        }
    }
}

fn visit_assoc_item(
    model: &mut ExtractionModel,
    sema: &Semantics<'_, RootDatabase>,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    assoc: AssocItem,
) {
    let db = sema.db;
    match assoc {
        AssocItem::Function(f) => {
            let id = match ra_ap_hir_def::FunctionId::try_from(f) {
                Ok(id) => id,
                Err(_) => return,
            };
            if let Some(src) = f.source(db) {
                set_attrs_for(model, def_to_node, ModuleDefId::FunctionId(id), &src.value);
            }
        }
        AssocItem::Const(c) => {
            let id: ra_ap_hir_def::ConstId = c.into();
            if let Some(src) = c.source(db) {
                set_attrs_for(model, def_to_node, ModuleDefId::ConstId(id), &src.value);
            }
        }
        AssocItem::TypeAlias(t) => {
            let id: ra_ap_hir_def::TypeAliasId = t.into();
            if let Some(src) = t.source(db) {
                set_attrs_for(model, def_to_node, ModuleDefId::TypeAliasId(id), &src.value);
            }
        }
    }
}

/// Look up the Item Node corresponding to `def_id` and set its
/// `attributes` field by walking `node`'s outer attrs and doc comments.
/// No-op if the def isn't in `def_to_node` (shouldn't happen for items the
/// earlier passes already extracted) or if the Node was already populated
/// (defensive against double-visits via re-exports).
fn set_attrs_for<N>(
    model: &mut ExtractionModel,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
    def_id: ModuleDefId,
    node: &N,
) where
    N: HasAttrs + HasDocComments,
{
    let Some(&node_id) = def_to_node.get(&def_id) else {
        return;
    };
    let Some(item_node) = model.nodes.get_mut(&node_id) else {
        return;
    };
    if !item_node.attributes.is_empty() {
        return;
    }
    let mut out: Vec<String> = Vec::new();
    for attr in node.attrs() {
        // Outer attributes only; inner attrs (`#![...]`) on items aren't
        // collected (they apply to the enclosing scope, not the item).
        if !attr.kind().is_outer() {
            continue;
        }
        let text = attr.syntax().text().to_string();
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_string());
        }
    }
    for comment in node.doc_comments() {
        let text = comment.text();
        let stripped: &str = text
            .strip_prefix("///")
            .or_else(|| text.strip_prefix("//!"))
            .unwrap_or(&text);
        // Multi-line `/** ... */` comments arrive as one Comment whose text
        // contains embedded newlines. Split so each source line lands as its
        // own queryable entry.
        for line in stripped.split('\n') {
            let line = line.trim_end();
            // Preserve leading single-space indent of the doc body but trim
            // outer whitespace so substring matches don't have to worry
            // about leading spaces.
            let body = line.trim_start();
            out.push(format!("/// {body}"));
        }
    }
    item_node.attributes = out;
}

#[cfg(test)]
mod tests {
    //! Tests use the shared snapshot from `queries::tests` — see that module
    //! for the snapshot lifecycle. Assertions target known items in this very
    //! workspace whose attributes are stable across refactors:
    //!   * `Node` struct (model.rs) — `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]`
    //!   * `ItemKind` enum (model.rs) — `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]`
    //!   * `extract_attributes` itself — has no attrs, returns empty Vec.
    use crate::graph::test_support::shared_snapshot;

    fn attrs_of(qualified: &str) -> Vec<String> {
        let snap = shared_snapshot();
        let (id, _node) = snap
            .lookup_by_qualified_name(qualified)
            .unwrap()
            .unwrap_or_else(|| panic!("`{qualified}` not in snapshot"));
        snap.item_attributes(id).expect("item_attributes failed")
    }

    #[test]
    fn attributes_of_known_struct() {
        let attrs = attrs_of("rmc_graph::graph::model::Node");
        let derive = attrs
            .iter()
            .find(|s| s.starts_with("#[derive("))
            .unwrap_or_else(|| panic!("no derive attr on Node, got {attrs:?}"));
        for trait_name in [
            "Debug",
            "Clone",
            "PartialEq",
            "Eq",
            "Serialize",
            "Deserialize",
        ] {
            assert!(
                derive.contains(trait_name),
                "Node derive should mention `{trait_name}`, got `{derive}`"
            );
        }
    }

    #[test]
    fn attributes_of_known_enum() {
        let attrs = attrs_of("rmc_graph::graph::model::ItemKind");
        let derive = attrs
            .iter()
            .find(|s| s.starts_with("#[derive("))
            .unwrap_or_else(|| panic!("no derive attr on ItemKind, got {attrs:?}"));
        for trait_name in [
            "Debug",
            "Clone",
            "Copy",
            "PartialEq",
            "Eq",
            "Hash",
            "Serialize",
            "Deserialize",
        ] {
            assert!(
                derive.contains(trait_name),
                "ItemKind derive should mention `{trait_name}`, got `{derive}`"
            );
        }
    }

    #[test]
    fn attributes_of_item_with_no_attrs_is_empty() {
        // `extract_attributes` itself has a doc comment but no `#[...]`
        // attributes. We can still pick something simpler — a closure-free
        // private helper. `set_attrs_for` is a generic fn but its qualified
        // name is `rust_code_mcp::graph::attributes::set_attrs_for`.
        // It has no derive / must_use / inline attrs.
        let attrs = attrs_of("rmc_graph::graph::attributes::set_attrs_for");
        let non_doc: Vec<&String> = attrs.iter().filter(|s| !s.starts_with("///")).collect();
        assert!(
            non_doc.is_empty(),
            "expected no non-doc attrs on set_attrs_for, got {non_doc:?}"
        );
    }
}

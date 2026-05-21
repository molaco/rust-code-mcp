use ra_ap_hir::Crate;
use ra_ap_hir_def::ModuleId;
use ra_ap_hir_def::nameres::DefMap;
use ra_ap_ide::RootDatabase;

use super::model::{BindingKind, ItemKind, Node, NodeKind, UsageCategory};

pub fn usage_category_label(c: UsageCategory) -> &'static str {
    match c {
        UsageCategory::Read => "Read",
        UsageCategory::Write => "Write",
        UsageCategory::Test => "Test",
        UsageCategory::Other => "Other",
    }
}

pub fn binding_kind_label(kind: BindingKind) -> &'static str {
    match kind {
        BindingKind::Declared => "Declared",
        BindingKind::NamedImport => "NamedImport",
        BindingKind::GlobImport => "GlobImport",
        BindingKind::ExternCrateImport => "ExternCrateImport",
    }
}

pub fn node_kind_label(
    node: &Node,
    item_kind_label: fn(ItemKind) -> &'static str,
) -> String {
    match node.kind {
        NodeKind::Workspace => "Workspace".to_string(),
        NodeKind::Crate => "Crate".to_string(),
        NodeKind::Module => "Module".to_string(),
        NodeKind::Item => match node.item_kind {
            Some(k) => format!("Item.{}", item_kind_label(k)),
            None => "Item".to_string(),
        },
        NodeKind::ExternalSymbol => "ExternalSymbol".to_string(),
    }
}

pub fn item_kind_short_label(k: ItemKind) -> &'static str {
    match k {
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
        ItemKind::EnumVariant => "EnumVariant",
    }
}

pub fn item_kind_display_label(k: ItemKind) -> &'static str {
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

pub(crate) fn item_kind_id_label(kind: ItemKind) -> &'static str {
    match kind {
        ItemKind::Function => "function",
        ItemKind::Struct => "struct",
        ItemKind::Enum => "enum",
        ItemKind::Union => "union",
        ItemKind::Trait => "trait",
        ItemKind::TypeAlias => "type_alias",
        ItemKind::Const => "const",
        ItemKind::Static => "static",
        ItemKind::AssocFunction => "assoc_function",
        ItemKind::AssocConst => "assoc_const",
        ItemKind::AssocType => "assoc_type",
        ItemKind::Method => "method",
        ItemKind::EnumVariant => "enum_variant",
    }
}

pub(crate) fn crate_display_name(db: &RootDatabase, krate: Crate) -> String {
    krate
        .display_name(db)
        .map(|n| n.canonical_name().as_str().to_string())
        .unwrap_or_else(|| "unknown_crate".to_string())
}

/// Build the module path from crate root to `module_id`, e.g. `["graph", "loader"]`
/// for `crate::graph::loader`. Returns empty for the crate root itself.
pub(crate) fn module_path_segments(
    db: &RootDatabase,
    def_map: &DefMap,
    module_id: ModuleId,
) -> Vec<String> {
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

pub(crate) fn module_qualified_path(db: &RootDatabase, module_id: ModuleId) -> String {
    let crate_name = crate_display_name(db, module_id.krate(db).into());
    let def_map = module_id.def_map(db);
    let segs = module_path_segments(db, def_map, module_id);
    if segs.is_empty() {
        crate_name
    } else {
        format!("{crate_name}::{}", segs.join("::"))
    }
}

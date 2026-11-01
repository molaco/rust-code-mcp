//! Target and declaration identities, independent of byte offsets and RA ids.
use std::path::Path;
use ra_ap_hir::{AssocItem, AssocItemContainer, Crate, HasSource, Semantics};
use ra_ap_ide_db::{RootDatabase, base_db::TargetKind};
use ra_ap_syntax::AstNode;
use ra_ap_vfs::Vfs;

use super::{audit_util::resolve_workspace_relative, ids::NodeId, labels::crate_display_name, model::ItemKind};

pub(super) fn target_kind(db: &RootDatabase, krate: Crate) -> &'static str {
    match krate.target_kind(db) {
        TargetKind::Lib { .. } => "lib",
        TargetKind::Bin => "bin",
        TargetKind::Example => "example",
        TargetKind::Test => "test",
        TargetKind::Bench => "bench",
        TargetKind::BuildScript => "build",
        TargetKind::Other => "unknown",
    }
}

/// Also the manifest's incremental write-unit key. Root path keeps equally
/// named targets in different packages distinct; kind distinguishes a lib
/// and a bin even when Cargo explicitly gives them the same root file.
pub(super) fn target_key(db: &RootDatabase, vfs: &Vfs, root: &Path, krate: Crate) -> String {
    let path = resolve_workspace_relative(vfs, krate.root_file(db), root)
        .expect("a local graph target has a workspace-relative root file");
    format!("{}[{}]@{path}", crate_display_name(db, krate), target_kind(db, krate))
}

pub(super) fn crate_node_id(hash: &str, key: &str) -> NodeId {
    NodeId::from_components(&[hash, "crate", key])
}

pub(super) fn module_node_id(crate_id: NodeId, segments: &[String]) -> NodeId {
    let crate_hex = crate_id.to_hex();
    let mut parts = vec![crate_hex.as_str(), "module"];
    parts.extend(segments.iter().map(String::as_str));
    NodeId::from_components(&parts)
}

/// A declaration's lexical module and normalized container header distinguish
/// trait implementations and generic specializations. Comments/whitespace,
/// body edits and declarations inserted earlier in the file do not enter ids.
/// Repeated inherent blocks can share a header: Rust still requires unique
/// associated names within that self type. Invalid duplicate declarations
/// are not assigned an invented order-dependent identity.
pub(super) fn assoc_item_node_id(
    sema: &Semantics<'_, RootDatabase>, module: NodeId, assoc: AssocItem,
    kind: ItemKind, name: &str,
) -> Option<NodeId> {
    let db = sema.db;
    let header: Vec<String> = match assoc.container(db) {
        AssocItemContainer::Trait(trait_) => vec!["trait".to_owned(), trait_.name(db).as_str().to_owned()],
        AssocItemContainer::Impl(impl_) => {
            let source = impl_.source(db)?;
            let items = source.value.assoc_item_list()?;
            source.value.syntax().descendants_with_tokens().filter_map(|e| e.into_token())
                .take_while(|t| t.text_range().start() < items.syntax().text_range().start())
                .filter(|t| !t.kind().is_trivia()).map(|t| t.text().to_owned()).collect()
        }
    };
    let module_hex = module.to_hex();
    let kind_label = super::labels::item_kind_id_label(kind);
    let mut parts = vec![module_hex.as_str(), kind_label, name];
    parts.extend(header.iter().map(String::as_str));
    Some(NodeId::from_components(&parts))
}

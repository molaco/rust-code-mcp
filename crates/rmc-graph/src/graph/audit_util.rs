use std::path::Path;

use ra_ap_hir::Semantics;
use ra_ap_ide_db::RootDatabase;
use ra_ap_syntax::ast::{self, AstNode};
use ra_ap_syntax::{SyntaxNode, TokenAtOffset};
use ra_ap_vfs::{FileId, Vfs};

use super::ids::NodeId;
use super::snapshot::OpenedSnapshot;

pub(in crate::graph) fn canonical_function_path(db: &RootDatabase, func: ra_ap_hir::Function) -> String {
    let module = func.module(db);
    let krate = module.krate(db);
    let crate_name = krate
        .display_name(db)
        .map(|n| n.canonical_name().as_str().to_string())
        .unwrap_or_default();
    let mut path_segs: Vec<String> = Vec::new();
    for ancestor in module.path_to_root(db).into_iter().rev() {
        if let Some(name) = ancestor.name(db) {
            path_segs.push(name.as_str().to_string());
        }
    }
    let fn_name = func.name(db).as_str().to_string();
    let mut prefix = String::new();
    if !crate_name.is_empty() {
        prefix.push_str(&crate_name);
    }
    if !path_segs.is_empty() {
        if !prefix.is_empty() {
            prefix.push_str("::");
        }
        prefix.push_str(&path_segs.join("::"));
    }
    if prefix.is_empty() {
        fn_name
    } else {
        format!("{prefix}::{fn_name}")
    }
}

pub(in crate::graph) fn enclosed_by_cfg_test(node: &SyntaxNode) -> bool {
    let mut cur = Some(node.clone());
    while let Some(n) = cur {
        if let Some(item) = ast::Item::cast(n.clone()) {
            if item_has_cfg_test(&item) {
                return true;
            }
        }
        cur = n.parent();
    }
    false
}

pub(in crate::graph) fn item_has_cfg_test(item: &ast::Item) -> bool {
    use ra_ap_syntax::ast::HasAttrs;

    let attrs: Box<dyn Iterator<Item = ast::Attr>> = match item {
        ast::Item::Fn(f) => Box::new(f.attrs()),
        ast::Item::Module(m) => Box::new(m.attrs()),
        ast::Item::Impl(i) => Box::new(i.attrs()),
        ast::Item::Trait(t) => Box::new(t.attrs()),
        ast::Item::Const(c) => Box::new(c.attrs()),
        ast::Item::Static(s) => Box::new(s.attrs()),
        ast::Item::Struct(s) => Box::new(s.attrs()),
        ast::Item::Enum(e) => Box::new(e.attrs()),
        ast::Item::Union(u) => Box::new(u.attrs()),
        _ => return false,
    };
    for attr in attrs {
        let text = attr.syntax().text().to_string();
        let stripped: String = text.split_whitespace().collect();
        if stripped.contains("cfg(test)")
            || stripped.contains("cfg(any(test")
            || stripped.contains("cfg(all(test")
        {
            return true;
        }
    }
    false
}

pub(in crate::graph) fn resolve_enclosing_function(
    sema: &Semantics<'_, RootDatabase>,
    syntax_root: &SyntaxNode,
    offset: ra_ap_syntax::TextSize,
    snap: &OpenedSnapshot,
    db: &RootDatabase,
) -> (Option<NodeId>, Option<String>) {
    let token = match syntax_root.token_at_offset(offset) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(t) => Some(t),
        TokenAtOffset::Between(a, b) => Some(b).or(Some(a)),
    };
    let scope_node = token.as_ref().and_then(|t| t.parent());
    let scope = match scope_node.as_ref() {
        Some(p) => sema.scope_at_offset(p, offset),
        None => sema.scope_at_offset(syntax_root, offset),
    };
    let fn_hir = match scope.and_then(|s| s.containing_function()) {
        Some(f) => f,
        None => return (None, None),
    };
    let qualified = canonical_function_path(db, fn_hir);
    let node_id = match snap.lookup_by_qualified_name(&qualified) {
        Ok(Some((id, _))) => Some(id),
        _ => None,
    };
    (node_id, Some(qualified))
}

pub(in crate::graph) fn resolve_workspace_relative(
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
        .map(|p| p.to_string_lossy().into_owned())
}

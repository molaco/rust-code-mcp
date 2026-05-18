//! Phase 8 — `channel_capacity_audit`.
//!
//! AST-walk audit: locate every channel-construction call site across the
//! workspace's local crates and classify it (bounded vs unbounded). Mirrors
//! the `unsafe_audit` pattern: `loader::load` is performed by the caller, we
//! iterate every local Module's source file via
//! `definition_source_file_id`, walk the syntax tree for `CallExpr` nodes,
//! and resolve each call's path through `Semantics::resolve_path` (so
//! aliased imports such as `use tokio::sync::mpsc; mpsc::channel(N)` still
//! match the canonical entry).

use std::collections::HashSet;

use anyhow::Result;
use ra_ap_hir::{Semantics, attach_db};
use ra_ap_hir_def::nameres::crate_def_map;
use ra_ap_syntax::AstToken;
use ra_ap_syntax::ast::{self, AstNode, HasArgList};
use ra_ap_vfs::FileId;
use serde::{Deserialize, Serialize};

use super::ast_resolve::resolve_call_to_function;
use super::audit_util::{
    canonical_function_path, enclosed_by_cfg_test, resolve_enclosing_function,
    resolve_workspace_relative,
};
use super::ids::NodeId;
use super::loader::LoadedWorkspace;
use super::snapshot::OpenedSnapshot;

#[derive(Debug, Clone)]
pub struct ChannelAuditOpts {
    pub crate_id_filter: Option<NodeId>,
    pub skip_test_fns: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelFinding {
    pub crate_name: String,
    pub kind: String,
    pub bounded: bool,
    pub capacity: Option<u64>,
    pub file: String,
    pub span: (u32, u32),
    pub enclosing_function: Option<NodeId>,
    pub enclosing_function_name: Option<String>,
}

pub fn classify_channel_path(canonical_path: &str) -> Option<(&'static str, bool)> {
    match canonical_path {
        "tokio::sync::mpsc::channel" | "tokio::sync::mpsc::bounded::channel" => {
            Some(("tokio_mpsc", true))
        }
        "tokio::sync::mpsc::unbounded_channel"
        | "tokio::sync::mpsc::unbounded::unbounded_channel" => Some(("tokio_unbounded", false)),
        "std::sync::mpsc::channel" => Some(("std_mpsc", false)),
        "std::sync::mpsc::sync_channel" => Some(("std_sync_channel", true)),
        "crossbeam_channel::bounded" => Some(("crossbeam_bounded", true)),
        "crossbeam_channel::unbounded" => Some(("crossbeam_unbounded", false)),
        "flume::bounded" => Some(("flume_bounded", true)),
        "flume::unbounded" => Some(("flume_unbounded", false)),
        _ => None,
    }
}

pub fn parse_capacity_arg(arg_text: &str) -> Option<u64> {
    let cleaned = arg_text.trim().replace('_', "");
    if cleaned.is_empty() {
        return None;
    }
    cleaned.parse::<u64>().ok()
}

pub fn channel_capacity_audit(
    loaded: &LoadedWorkspace,
    snap: &OpenedSnapshot,
    opts: ChannelAuditOpts,
) -> Result<Vec<ChannelFinding>> {
    let workspace_root = loaded.workspace_root.clone();
    let db = &loaded.db;
    let vfs = &loaded.vfs;

    let mut findings: Vec<ChannelFinding> = Vec::new();

    attach_db(db, || {
        let sema = Semantics::new(db);

        let mut local_crate_filter: Option<HashSet<String>> = None;
        if let Some(filter_id) = opts.crate_id_filter {
            let rtxn = match snap.env.read_txn() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(error = %e, "channel_audit: read_txn failed");
                    return;
                }
            };
            if let Ok(Some(node)) = snap.dbs.nodes_by_id.get(&rtxn, filter_id.0.as_slice()) {
                local_crate_filter = Some(
                    [node.qualified_name.clone()].iter().cloned().collect(),
                );
            }
        }

        let mut file_to_crate: std::collections::HashMap<FileId, String> =
            std::collections::HashMap::new();
        for &krate in &loaded.local_crates {
            let crate_name = krate
                .display_name(db)
                .map(|n| n.canonical_name().as_str().to_string())
                .unwrap_or_default();
            if let Some(filter) = &local_crate_filter {
                if !filter.contains(&crate_name) {
                    continue;
                }
            }
            let def_map = crate_def_map(db, krate.base());
            for (module_id, module_data) in def_map.modules() {
                if module_id.is_block_module(db) {
                    continue;
                }
                let hir_file_id = module_data.definition_source_file_id();
                let editioned = hir_file_id.original_file(db);
                let file_id = editioned.file_id(db);
                file_to_crate.entry(file_id).or_insert_with(|| crate_name.clone());
            }
        }

        for (file_id, crate_name) in file_to_crate {
            let rel_path = match resolve_workspace_relative(vfs, file_id, &workspace_root) {
                Some(p) => p,
                None => continue,
            };

            let source_file = sema.parse_guess_edition(file_id);
            let syntax_root = source_file.syntax();

            for node in syntax_root.descendants() {
                if !ast::CallExpr::can_cast(node.kind()) {
                    continue;
                }
                let call = match ast::CallExpr::cast(node.clone()) {
                    Some(c) => c,
                    None => continue,
                };

                if !matches!(call.expr(), Some(ast::Expr::PathExpr(_))) {
                    continue;
                }

                let func = match resolve_call_to_function(&sema, &call) {
                    Some(f) => f,
                    None => continue,
                };

                let canonical = canonical_function_path(db, func);
                let (kind, bounded) = match classify_channel_path(&canonical) {
                    Some(v) => v,
                    None => continue,
                };

                if opts.skip_test_fns && enclosed_by_cfg_test(&node) {
                    continue;
                }

                let capacity: Option<u64> = if bounded {
                    call.arg_list()
                        .and_then(|al| al.args_maybe_empty().flatten().next())
                        .and_then(|expr| extract_int_literal(&expr))
                } else {
                    None
                };

                let range = call.syntax().text_range();
                let start: u32 = u32::from(range.start());
                let end: u32 = u32::from(range.end());

                let (enclosing_function, enclosing_function_name) =
                    resolve_enclosing_function(&sema, syntax_root, range.start(), snap, db);

                findings.push(ChannelFinding {
                    crate_name: crate_name.clone(),
                    kind: kind.to_string(),
                    bounded,
                    capacity,
                    file: rel_path.clone(),
                    span: (start, end),
                    enclosing_function,
                    enclosing_function_name,
                });
            }
        }
    });

    findings.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.span.0.cmp(&b.span.0))
    });
    Ok(findings)
}

fn extract_int_literal(expr: &ast::Expr) -> Option<u64> {
    let lit = match expr {
        ast::Expr::Literal(l) => l,
        _ => return None,
    };
    let kind = lit.kind();
    let int = match kind {
        ast::LiteralKind::IntNumber(i) => i,
        _ => return None,
    };
    let raw = int.syntax().text().to_string();
    parse_capacity_arg(&raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_tokio_mpsc_channel() {
        assert_eq!(
            classify_channel_path("tokio::sync::mpsc::channel"),
            Some(("tokio_mpsc", true))
        );
    }

    #[test]
    fn classify_tokio_unbounded() {
        assert_eq!(
            classify_channel_path("tokio::sync::mpsc::unbounded_channel"),
            Some(("tokio_unbounded", false))
        );
    }

    #[test]
    fn classify_std_mpsc_channel() {
        assert_eq!(
            classify_channel_path("std::sync::mpsc::channel"),
            Some(("std_mpsc", false))
        );
    }

    #[test]
    fn classify_std_sync_channel() {
        assert_eq!(
            classify_channel_path("std::sync::mpsc::sync_channel"),
            Some(("std_sync_channel", true))
        );
    }

    #[test]
    fn classify_crossbeam_bounded() {
        assert_eq!(
            classify_channel_path("crossbeam_channel::bounded"),
            Some(("crossbeam_bounded", true))
        );
    }

    #[test]
    fn classify_crossbeam_unbounded() {
        assert_eq!(
            classify_channel_path("crossbeam_channel::unbounded"),
            Some(("crossbeam_unbounded", false))
        );
    }

    #[test]
    fn classify_flume_bounded() {
        assert_eq!(
            classify_channel_path("flume::bounded"),
            Some(("flume_bounded", true))
        );
    }

    #[test]
    fn classify_flume_unbounded() {
        assert_eq!(
            classify_channel_path("flume::unbounded"),
            Some(("flume_unbounded", false))
        );
    }

    #[test]
    fn classify_tokio_unbounded_via_defining_module() {
        assert_eq!(
            classify_channel_path("tokio::sync::mpsc::unbounded::unbounded_channel"),
            Some(("tokio_unbounded", false))
        );
    }

    #[test]
    fn classify_tokio_bounded_via_defining_module() {
        assert_eq!(
            classify_channel_path("tokio::sync::mpsc::bounded::channel"),
            Some(("tokio_mpsc", true))
        );
    }

    #[test]
    fn classify_unknown_path_is_none() {
        assert_eq!(classify_channel_path("tokio::time::sleep"), None);
    }

    #[test]
    fn classify_truncated_path_is_none() {
        assert_eq!(classify_channel_path("mpsc::channel"), None);
    }

    #[test]
    fn parse_capacity_plain_int() {
        assert_eq!(parse_capacity_arg("100"), Some(100));
    }

    #[test]
    fn parse_capacity_with_underscore() {
        assert_eq!(parse_capacity_arg("1_024"), Some(1024));
    }

    #[test]
    fn parse_capacity_const_name_is_none() {
        assert_eq!(parse_capacity_arg("BUF_SIZE"), None);
    }

    #[test]
    fn parse_capacity_arithmetic_is_none() {
        assert_eq!(parse_capacity_arg("100 + 1"), None);
    }

    #[test]
    fn parse_capacity_empty_is_none() {
        assert_eq!(parse_capacity_arg(""), None);
    }

    #[test]
    fn parse_capacity_negative_is_none() {
        assert_eq!(parse_capacity_arg("-1"), None);
    }

    #[test]
    fn parse_capacity_zero() {
        assert_eq!(parse_capacity_arg("0"), Some(0));
    }
}

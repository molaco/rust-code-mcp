//! Phase 6 — unsafe-block audit.
//!
//! For every `.rs` file in the workspace's local crates, find every
//! `unsafe { ... }` block via syntax-tree traversal. Per finding:
//!  - byte span and line count
//!  - enclosing fn (via Semantics::scope_at_offset + containing_function)
//!    resolved to a snapshot NodeId when possible
//!  - whether a `// SAFETY:` comment appears in the preceding 5 source lines
//!
//! Live computation; nothing is cached. Per-invocation cost is dominated
//! by `loader::load` in the MCP wrapper (~2-3s); the AST scan itself is
//! workspace-size proportional but cheap (no semantic analysis besides
//! the per-block containing_function lookup).
//!
//! Scope mirrors impls.rs: only iterate local crates (workspace members).
//! Files outside workspace_root are skipped defensively.

use std::collections::HashSet;

use anyhow::Result;
use ra_ap_hir::{Semantics, attach_db};
use ra_ap_hir_def::nameres::crate_def_map;
use ra_ap_syntax::ast::{self, AstNode};
use ra_ap_vfs::FileId;
use serde::{Deserialize, Serialize};

use super::audit_util::resolve_workspace_relative;
use super::ids::NodeId;
use super::loader::LoadedWorkspace;
use super::snapshot::OpenedSnapshot;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnsafeFinding {
    /// Workspace-relative path to the file containing the unsafe block.
    pub file: String,
    /// Byte range of the `unsafe { ... }` expression (including curlies).
    pub span: (u32, u32),
    /// Number of source lines spanned by the unsafe block (>= 1).
    pub line_count: u32,
    /// Snapshot NodeId of the enclosing fn when resolvable.
    pub enclosing_function: Option<NodeId>,
    /// Qualified name of the enclosing fn when resolvable; informational.
    pub enclosing_function_name: Option<String>,
    /// `true` if `// SAFETY` (case-sensitive) appears in the 5 source lines
    /// preceding the `unsafe` keyword. Heuristic — see
    /// `has_safety_comment_in_preceding_lines` for matching behaviour.
    pub has_safety_comment: bool,
}

pub fn unsafe_audit_impl(
    loaded: &LoadedWorkspace,
    snap: &OpenedSnapshot,
) -> Result<Vec<UnsafeFinding>> {
    let workspace_root = loaded.workspace_root.clone();
    let db = &loaded.db;
    let vfs = &loaded.vfs;

    let mut findings: Vec<UnsafeFinding> = Vec::new();

    attach_db(db, || {
        let sema = Semantics::new(db);

        // 1. Collect unique FileIds across all local crates' modules.
        let mut file_ids: HashSet<FileId> = HashSet::new();
        for &krate in &loaded.local_crates {
            let def_map = crate_def_map(db, krate.base());
            for (module_id, module_data) in def_map.modules() {
                if module_id.is_block_module(db) {
                    continue;
                }
                let hir_file_id = module_data.definition_source_file_id();
                let editioned = hir_file_id.original_file(db);
                let file_id = editioned.file_id(db);
                file_ids.insert(file_id);
            }
        }

        // 2. For each file, parse and walk its AST.
        for file_id in file_ids {
            let rel_path = match resolve_workspace_relative(vfs, file_id, &workspace_root) {
                Some(p) => p,
                None => {
                    tracing::trace!(?file_id, "skipping file outside workspace_root");
                    continue;
                }
            };

            let abs_path = workspace_root.join(&rel_path);
            let file_text = match std::fs::read_to_string(&abs_path) {
                Ok(t) => t,
                Err(e) => {
                    tracing::trace!(path = %abs_path.display(), error = %e, "failed to read file");
                    continue;
                }
            };

            let source_file = sema.parse_guess_edition(file_id);
            let syntax_root = source_file.syntax();

            for node in syntax_root.descendants() {
                if !ast::BlockExpr::can_cast(node.kind()) {
                    continue;
                }
                let block = match ast::BlockExpr::cast(node.clone()) {
                    Some(b) => b,
                    None => continue,
                };
                if block.unsafe_token().is_none() {
                    continue;
                }

                let range = block.syntax().text_range();
                let start: u32 = u32::from(range.start());
                let end: u32 = u32::from(range.end());

                let (start_us, end_us) = (start as usize, end as usize);
                let block_slice = file_text
                    .get(start_us..end_us.min(file_text.len()))
                    .unwrap_or("");
                let line_count: u32 = (block_slice.matches('\n').count() as u32).saturating_add(1);

                let has_safety_comment =
                    has_safety_comment_in_preceding_lines(&file_text, start_us);

                // Enclosing-fn resolution.
                let (enclosing_function, enclosing_function_name) = {
                    let token = match syntax_root.token_at_offset(range.start()) {
                        ra_ap_syntax::TokenAtOffset::None => None,
                        ra_ap_syntax::TokenAtOffset::Single(t) => Some(t),
                        ra_ap_syntax::TokenAtOffset::Between(a, b) => Some(b).or(Some(a)),
                    };
                    let scope_node = token.as_ref().and_then(|t| t.parent());
                    let scope = match scope_node.as_ref() {
                        Some(p) => sema.scope_at_offset(p, range.start()),
                        None => sema.scope_at_offset(syntax_root, range.start()),
                    };
                    let fn_hir = scope.and_then(|s| s.containing_function());
                    match fn_hir {
                        Some(f) => {
                            let module = f.module(db);
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
                            let fn_name = f.name(db).as_str().to_string();
                            let qualified = if path_segs.is_empty() {
                                if crate_name.is_empty() {
                                    fn_name.clone()
                                } else {
                                    format!("{crate_name}::{fn_name}")
                                }
                            } else {
                                let mut prefix = String::new();
                                if !crate_name.is_empty() {
                                    prefix.push_str(&crate_name);
                                    prefix.push_str("::");
                                }
                                prefix.push_str(&path_segs.join("::"));
                                format!("{prefix}::{fn_name}")
                            };

                            let node_id = match snap.lookup_by_qualified_name(&qualified) {
                                Ok(Some((id, _))) => Some(id),
                                Ok(None) => None,
                                Err(e) => {
                                    tracing::trace!(
                                        qualified = %qualified,
                                        error = %e,
                                        "lookup_by_qualified_name failed"
                                    );
                                    None
                                }
                            };

                            (node_id, Some(qualified))
                        }
                        None => (None, None),
                    }
                };

                findings.push(UnsafeFinding {
                    file: rel_path.clone(),
                    span: (start, end),
                    line_count,
                    enclosing_function,
                    enclosing_function_name,
                    has_safety_comment,
                });
            }
        }
    });

    findings.sort_by(|a, b| a.file.cmp(&b.file).then_with(|| a.span.0.cmp(&b.span.0)));
    Ok(findings)
}

/// Heuristic: scan the up-to-five source lines preceding `unsafe_offset` for a
/// `// SAFETY` (case-sensitive) substring. Matches `// SAFETY:`, `/* SAFETY:
/// ... */`, and even bare `// SAFETY notes`. Doesn't require the trailing
/// colon — anything containing the literal `SAFETY` token in a comment-bearing
/// region of the preceding 5 lines counts.
pub(crate) fn has_safety_comment_in_preceding_lines(text: &str, unsafe_offset: usize) -> bool {
    if unsafe_offset > text.len() {
        return false;
    }
    let prefix = &text[..unsafe_offset];
    // Find the start of the line containing the unsafe keyword.
    let line_start = prefix.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let preceding = &prefix[..line_start];
    if preceding.is_empty() {
        return false;
    }
    // Take the last up-to-5 lines of `preceding`. Lines are separated by '\n'.
    let lines: Vec<&str> = preceding.split('\n').collect();
    // The final element of `split` is the (possibly empty) chunk after the last
    // '\n'. Since `preceding` ends at a '\n' (it's the prefix BEFORE the line
    // with the unsafe keyword), that final chunk is empty — drop it.
    let lines = if lines.last().map_or(false, |s| s.is_empty()) {
        &lines[..lines.len() - 1]
    } else {
        &lines[..]
    };
    let take = lines.len().min(5);
    let start = lines.len() - take;
    for line in &lines[start..] {
        if line.contains("SAFETY") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::loader;
    use crate::graph::queries::tests::shared_snapshot;
    use std::path::Path;

    #[test]
    fn finds_at_least_one_unsafe_block_in_self_workspace() {
        let snap = shared_snapshot();
        let loaded = loader::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let findings = unsafe_audit_impl(&loaded, snap).unwrap();
        assert!(
            !findings.is_empty(),
            "expected >=1 unsafe block in this workspace"
        );
        for f in &findings {
            assert!(!f.file.is_empty(), "file should be non-empty");
            assert!(f.span.1 >= f.span.0, "span end must be >= start");
            assert!(f.line_count >= 1, "line_count must be >=1");
        }
    }

    #[test]
    fn unsafe_blocks_in_snapshot_rs_have_no_safety_comment() {
        let snap = shared_snapshot();
        let loaded = loader::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let findings = unsafe_audit_impl(&loaded, snap).unwrap();
        let snapshot_findings: Vec<&UnsafeFinding> = findings
            .iter()
            .filter(|f| f.file.ends_with("snapshot.rs"))
            .collect();
        assert!(
            !snapshot_findings.is_empty(),
            "expected unsafe blocks in snapshot.rs"
        );
        for f in snapshot_findings {
            assert!(
                !f.has_safety_comment,
                "snapshot.rs unsafe block has unexpected SAFETY comment: {f:?}"
            );
        }
    }

    #[test]
    fn enclosing_function_resolves_for_snapshot_unsafe() {
        let snap = shared_snapshot();
        let loaded = loader::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
        let findings = unsafe_audit_impl(&loaded, snap).unwrap();
        let in_build_and_persist = findings.iter().find(|f| {
            f.file.ends_with("snapshot.rs")
                && f.enclosing_function_name
                    .as_deref()
                    .is_some_and(|n| n.contains("build_and_persist"))
        });
        assert!(
            in_build_and_persist.is_some(),
            "expected an unsafe block to resolve to build_and_persist; got {findings:#?}"
        );
    }

    #[test]
    fn safety_comment_heuristic_matches_expected_patterns() {
        let text = "// SAFETY: foo\n// next\nunsafe { bar(); }";
        assert!(has_safety_comment_in_preceding_lines(
            text,
            text.find("unsafe").unwrap()
        ));
        let text2 = "// no marker here\nunsafe { bar(); }";
        assert!(!has_safety_comment_in_preceding_lines(
            text2,
            text2.find("unsafe").unwrap()
        ));
        // SAFETY 6 lines back — should NOT match (window is 5 lines).
        let text3 = "// SAFETY: x\n// 1\n// 2\n// 3\n// 4\n// 5\nunsafe { bar(); }";
        assert!(!has_safety_comment_in_preceding_lines(
            text3,
            text3.find("unsafe").unwrap()
        ));
        // Unsafe at file start — handle gracefully.
        let text4 = "unsafe { bar(); }";
        assert!(!has_safety_comment_in_preceding_lines(
            text4,
            text4.find("unsafe").unwrap()
        ));
    }
}

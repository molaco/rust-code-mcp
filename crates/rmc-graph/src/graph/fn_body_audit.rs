//! Phase 8 — `fn_body_audit`.
//!
//! AST-walk audit: walks fn bodies in every local crate and emits per-pattern
//! findings against eight built-in patterns covering rust-guidelines §3, §9,
//! §12, §19, §22 (no `unwrap`/`expect`/panic macros / lock across `.await` /
//! direct self-recursion / unbounded `loop {}` / `transmute` / `unwrap_unchecked`).
//! Mirrors the loader + Semantics pattern from `channel_audit.rs`.

use std::collections::HashSet;

use anyhow::Result;
use ra_ap_hir::{Semantics, attach_db};
use ra_ap_hir_def::nameres::crate_def_map;
use ra_ap_syntax::SyntaxNode;
use ra_ap_syntax::ast::{self, AstNode, HasLoopBody};
use ra_ap_vfs::FileId;
use serde::{Deserialize, Serialize};

use super::ast_resolve::resolve_call_to_function;
use super::audit_util::{
    canonical_function_path, enclosed_by_cfg_test,
    resolve_enclosing_function as enclosing_fn_for_body_offset, resolve_workspace_relative,
};
use super::ids::NodeId;
use super::loader::LoadedWorkspace;
use super::snapshot::OpenedSnapshot;

pub(crate) const ALL_PATTERNS: &[&str] = &[
    "unwrap",
    "expect",
    "panic_macros",
    "unwrap_unchecked",
    "transmute",
    "await_in_guard_scope",
    "self_recursion",
    "unbounded_loop",
];

#[derive(Debug, Clone)]
pub struct FnBodyAuditOpts {
    pub crate_id_filter: Option<NodeId>,
    pub patterns: HashSet<&'static str>,
    pub skip_test_fns: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FnBodyFinding {
    pub target: Option<NodeId>,
    pub qualified_name: Option<String>,
    pub pattern: String,
    pub file: String,
    pub span: (u32, u32),
    pub context: String,
}

#[derive(Debug, Clone)]
pub(crate) struct RawFinding {
    pub pattern: &'static str,
    pub span: (u32, u32),
    pub syntax_node: SyntaxNode,
}

pub fn parse_pattern_filter(input: Option<&[String]>) -> Result<HashSet<&'static str>, String> {
    let valid: &[&'static str] = ALL_PATTERNS;
    match input {
        None => Ok(valid.iter().copied().collect()),
        Some(items) if items.is_empty() => Ok(valid.iter().copied().collect()),
        Some(items) => {
            let mut out: HashSet<&'static str> = HashSet::new();
            for n in items {
                let lit = valid
                    .iter()
                    .find(|v| **v == n.as_str())
                    .ok_or_else(|| {
                        format!("unknown pattern `{n}`; valid: {valid:?}")
                    })?;
                out.insert(*lit);
            }
            Ok(out)
        }
    }
}

pub(crate) fn match_unwrap(body: &SyntaxNode) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for node in body.descendants() {
        if let Some(mc) = ast::MethodCallExpr::cast(node) {
            let name = match mc.name_ref() {
                Some(n) => n.text().to_string(),
                None => continue,
            };
            if name == "unwrap" {
                let r = mc.syntax().text_range();
                out.push(RawFinding {
                    pattern: "unwrap",
                    span: (u32::from(r.start()), u32::from(r.end())),
                    syntax_node: mc.syntax().clone(),
                });
            }
        }
    }
    out
}

pub(crate) fn match_expect(body: &SyntaxNode) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for node in body.descendants() {
        if let Some(mc) = ast::MethodCallExpr::cast(node) {
            let name = match mc.name_ref() {
                Some(n) => n.text().to_string(),
                None => continue,
            };
            if name == "expect" {
                let r = mc.syntax().text_range();
                out.push(RawFinding {
                    pattern: "expect",
                    span: (u32::from(r.start()), u32::from(r.end())),
                    syntax_node: mc.syntax().clone(),
                });
            }
        }
    }
    out
}

pub(crate) fn match_panic_macros(body: &SyntaxNode) -> Vec<RawFinding> {
    let names: &[&str] = &["panic", "unreachable", "todo", "unimplemented"];
    let mut out = Vec::new();
    for node in body.descendants() {
        if let Some(mc) = ast::MacroCall::cast(node) {
            let path = match mc.path() {
                Some(p) => p,
                None => continue,
            };
            let seg = match path.segment() {
                Some(s) => s,
                None => continue,
            };
            let nr = match seg.name_ref() {
                Some(n) => n,
                None => continue,
            };
            let text = nr.text().to_string();
            if names.iter().any(|n| *n == text.as_str()) {
                let r = mc.syntax().text_range();
                out.push(RawFinding {
                    pattern: "panic_macros",
                    span: (u32::from(r.start()), u32::from(r.end())),
                    syntax_node: mc.syntax().clone(),
                });
            }
        }
    }
    out
}

pub(crate) fn match_unwrap_unchecked(body: &SyntaxNode) -> Vec<RawFinding> {
    let names: &[&str] = &["unwrap_unchecked", "unwrap_err_unchecked"];
    let mut out = Vec::new();
    for node in body.descendants() {
        if let Some(mc) = ast::MethodCallExpr::cast(node) {
            let name = match mc.name_ref() {
                Some(n) => n.text().to_string(),
                None => continue,
            };
            if names.iter().any(|n| *n == name.as_str()) {
                let r = mc.syntax().text_range();
                out.push(RawFinding {
                    pattern: "unwrap_unchecked",
                    span: (u32::from(r.start()), u32::from(r.end())),
                    syntax_node: mc.syntax().clone(),
                });
            }
        }
    }
    out
}

pub(crate) fn match_unbounded_loop(body: &SyntaxNode) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for node in body.descendants() {
        if let Some(loop_expr) = ast::LoopExpr::cast(node) {
            let body_block = match loop_expr.loop_body() {
                Some(b) => b,
                None => continue,
            };
            let body_syntax = body_block.syntax();
            let mut has_exit = false;
            for d in body_syntax.descendants() {
                if ast::BreakExpr::cast(d.clone()).is_some()
                    || ast::ReturnExpr::cast(d.clone()).is_some()
                    || ast::TryExpr::cast(d).is_some()
                {
                    has_exit = true;
                    break;
                }
            }
            if !has_exit {
                let r = loop_expr.syntax().text_range();
                out.push(RawFinding {
                    pattern: "unbounded_loop",
                    span: (u32::from(r.start()), u32::from(r.end())),
                    syntax_node: loop_expr.syntax().clone(),
                });
            }
        }
    }
    out
}

const GUARD_TEXT_NEEDLES: &[&str] = &[
    "MutexGuard",
    "RwLockReadGuard",
    "RwLockWriteGuard",
    "Guard",
    "Ref<",
    "RefMut<",
    ".lock()",
    ".read()",
    ".write()",
];

pub(crate) fn match_await_in_guard_scope(body: &SyntaxNode) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for node in body.descendants() {
        let await_expr = match ast::AwaitExpr::cast(node) {
            Some(a) => a,
            None => continue,
        };
        let await_start = await_expr.syntax().text_range().start();
        let mut block: Option<ast::BlockExpr> = None;
        let mut anc = await_expr.syntax().parent();
        while let Some(a) = anc {
            if let Some(b) = ast::BlockExpr::cast(a.clone()) {
                block = Some(b);
                break;
            }
            anc = a.parent();
        }
        let block = match block {
            Some(b) => b,
            None => continue,
        };
        let mut found_match = false;
        for stmt in block.statements() {
            if let ast::Stmt::LetStmt(let_stmt) = stmt {
                let r = let_stmt.syntax().text_range();
                if r.end() >= await_start {
                    continue;
                }
                let init_text = match let_stmt.initializer() {
                    Some(e) => e.syntax().text().to_string(),
                    None => String::new(),
                };
                let ty_text = match let_stmt.ty() {
                    Some(t) => t.syntax().text().to_string(),
                    None => String::new(),
                };
                let combined = format!("{init_text}\n{ty_text}");
                if GUARD_TEXT_NEEDLES.iter().any(|n| combined.contains(n)) {
                    found_match = true;
                    break;
                }
            }
        }
        if found_match {
            let r = await_expr.syntax().text_range();
            out.push(RawFinding {
                pattern: "await_in_guard_scope",
                span: (u32::from(r.start()), u32::from(r.end())),
                syntax_node: await_expr.syntax().clone(),
            });
        }
    }
    out
}

pub(crate) fn match_transmute(
    body: &SyntaxNode,
    sema: &Semantics<'_, ra_ap_ide_db::RootDatabase>,
    db: &ra_ap_ide_db::RootDatabase,
) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for node in body.descendants() {
        let call = match ast::CallExpr::cast(node) {
            Some(c) => c,
            None => continue,
        };
        if !matches!(call.expr(), Some(ast::Expr::PathExpr(_))) {
            continue;
        }
        let func = match resolve_call_to_function(sema, &call) {
            Some(f) => f,
            None => continue,
        };
        let canonical = canonical_function_path(db, func);
        if canonical == "std::mem::transmute" || canonical == "core::mem::transmute" {
            let r = call.syntax().text_range();
            out.push(RawFinding {
                pattern: "transmute",
                span: (u32::from(r.start()), u32::from(r.end())),
                syntax_node: call.syntax().clone(),
            });
        }
    }
    out
}

pub(crate) fn match_self_recursion(
    body: &SyntaxNode,
    self_qualified_name: &str,
    sema: &Semantics<'_, ra_ap_ide_db::RootDatabase>,
    db: &ra_ap_ide_db::RootDatabase,
) -> Vec<RawFinding> {
    let mut out = Vec::new();
    for node in body.descendants() {
        if let Some(call) = ast::CallExpr::cast(node.clone()) {
            if !matches!(call.expr(), Some(ast::Expr::PathExpr(_))) {
                continue;
            }
            let func = match resolve_call_to_function(sema, &call) {
                Some(f) => f,
                None => continue,
            };
            let canonical = canonical_function_path(db, func);
            if canonical == self_qualified_name {
                let r = call.syntax().text_range();
                out.push(RawFinding {
                    pattern: "self_recursion",
                    span: (u32::from(r.start()), u32::from(r.end())),
                    syntax_node: call.syntax().clone(),
                });
            }
        } else if let Some(mc) = ast::MethodCallExpr::cast(node) {
            let func = match sema.resolve_method_call(&mc) {
                Some(f) => f,
                None => continue,
            };
            let canonical = canonical_function_path(db, func);
            if canonical == self_qualified_name {
                let r = mc.syntax().text_range();
                out.push(RawFinding {
                    pattern: "self_recursion",
                    span: (u32::from(r.start()), u32::from(r.end())),
                    syntax_node: mc.syntax().clone(),
                });
            }
        }
    }
    out
}

pub fn fn_body_audit(
    loaded: &LoadedWorkspace,
    snap: &OpenedSnapshot,
    opts: FnBodyAuditOpts,
) -> Result<Vec<FnBodyFinding>> {
    let workspace_root = loaded.workspace_root.clone();
    let db = &loaded.db;
    let vfs = &loaded.vfs;

    let mut findings: Vec<FnBodyFinding> = Vec::new();
    let mut file_text_cache: std::collections::HashMap<FileId, String> =
        std::collections::HashMap::new();

    attach_db(db, || {
        let sema = Semantics::new(db);

        let mut local_crate_filter: Option<HashSet<String>> = None;
        if let Some(filter_id) = opts.crate_id_filter {
            let rtxn = match snap.env.read_txn() {
                Ok(t) => t,
                Err(e) => {
                    tracing::warn!(error = %e, "fn_body_audit: read_txn failed");
                    return;
                }
            };
            if let Ok(Some(node)) = snap.dbs.nodes_by_id.get(&rtxn, filter_id.0.as_slice()) {
                local_crate_filter =
                    Some([node.qualified_name.clone()].iter().cloned().collect());
            }
        }

        let mut file_ids: HashSet<FileId> = HashSet::new();
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
                file_ids.insert(file_id);
            }
        }

        for file_id in file_ids {
            let rel_path = match resolve_workspace_relative(vfs, file_id, &workspace_root) {
                Some(p) => p,
                None => continue,
            };

            let abs_path = workspace_root.join(&rel_path);
            let file_text = match file_text_cache.entry(file_id) {
                std::collections::hash_map::Entry::Occupied(e) => e.into_mut().clone(),
                std::collections::hash_map::Entry::Vacant(v) => {
                    let txt = match std::fs::read_to_string(&abs_path) {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    v.insert(txt).clone()
                }
            };

            let source_file = sema.parse_guess_edition(file_id);
            let syntax_root = source_file.syntax();

            for ast_node in syntax_root.descendants() {
                let fn_node = match ast::Fn::cast(ast_node) {
                    Some(f) => f,
                    None => continue,
                };
                let body = match fn_node.body() {
                    Some(b) => b,
                    None => continue,
                };
                let body_syntax = body.syntax().clone();

                if opts.skip_test_fns && enclosed_by_cfg_test(fn_node.syntax()) {
                    continue;
                }

                let (enclosing_id, enclosing_qn) =
                    enclosing_fn_for_body_offset(&sema, syntax_root, body_syntax.text_range().start(), snap, db);

                let mut raw: Vec<RawFinding> = Vec::new();

                if opts.patterns.contains("unwrap") {
                    raw.extend(match_unwrap(&body_syntax));
                }
                if opts.patterns.contains("expect") {
                    raw.extend(match_expect(&body_syntax));
                }
                if opts.patterns.contains("panic_macros") {
                    raw.extend(match_panic_macros(&body_syntax));
                }
                if opts.patterns.contains("unwrap_unchecked") {
                    raw.extend(match_unwrap_unchecked(&body_syntax));
                }
                if opts.patterns.contains("unbounded_loop") {
                    raw.extend(match_unbounded_loop(&body_syntax));
                }
                if opts.patterns.contains("await_in_guard_scope") {
                    raw.extend(match_await_in_guard_scope(&body_syntax));
                }
                if opts.patterns.contains("transmute") {
                    raw.extend(match_transmute(&body_syntax, &sema, db));
                }
                if opts.patterns.contains("self_recursion") {
                    if let Some(qn) = enclosing_qn.as_deref() {
                        raw.extend(match_self_recursion(&body_syntax, qn, &sema, db));
                    }
                }

                for rf in raw {
                    let context = build_context(&file_text, rf.span.0 as usize, rf.span.1 as usize);
                    findings.push(FnBodyFinding {
                        target: enclosing_id,
                        qualified_name: enclosing_qn.clone(),
                        pattern: rf.pattern.to_string(),
                        file: rel_path.clone(),
                        span: rf.span,
                        context,
                    });
                }
            }
        }
    });

    findings.sort_by(|a, b| {
        a.file
            .cmp(&b.file)
            .then_with(|| a.span.0.cmp(&b.span.0))
            .then_with(|| a.pattern.cmp(&b.pattern))
    });
    Ok(findings)
}

fn build_context(file_text: &str, start: usize, end: usize) -> String {
    if start >= file_text.len() {
        return String::new();
    }
    let end = end.min(file_text.len());
    let line_start = file_text[..start].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end_search = file_text[end..]
        .find('\n')
        .map(|p| end + p)
        .unwrap_or(file_text.len());
    let prev_line_start = if line_start == 0 {
        line_start
    } else {
        file_text[..line_start - 1]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0)
    };
    let next_line_end = if line_end_search >= file_text.len() {
        line_end_search
    } else {
        file_text[line_end_search + 1..]
            .find('\n')
            .map(|p| line_end_search + 1 + p)
            .unwrap_or(file_text.len())
    };
    let slice = &file_text[prev_line_start..next_line_end];
    slice.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ra_ap_syntax::SourceFile;

    fn parse_fn_body(src: &str) -> SyntaxNode {
        let parsed = SourceFile::parse(src, ra_ap_syntax::Edition::Edition2024);
        let tree = parsed.tree();
        let f = tree
            .syntax()
            .descendants()
            .find_map(ast::Fn::cast)
            .expect("expected an `fn` in the test source");
        f.body().expect("expected a body").syntax().clone()
    }

    #[test]
    fn parse_pattern_filter_default_is_all() {
        let f = parse_pattern_filter(None).unwrap();
        assert_eq!(f.len(), ALL_PATTERNS.len());
    }

    #[test]
    fn parse_pattern_filter_empty_is_all() {
        let v: Vec<String> = vec![];
        let f = parse_pattern_filter(Some(&v)).unwrap();
        assert_eq!(f.len(), ALL_PATTERNS.len());
    }

    #[test]
    fn parse_pattern_filter_subset() {
        let v = vec!["unwrap".to_string(), "expect".to_string()];
        let f = parse_pattern_filter(Some(&v)).unwrap();
        assert_eq!(f.len(), 2);
        assert!(f.contains("unwrap"));
        assert!(f.contains("expect"));
    }

    #[test]
    fn parse_pattern_filter_unknown_errors() {
        let v = vec!["bogus".to_string()];
        assert!(parse_pattern_filter(Some(&v)).is_err());
    }

    #[test]
    fn unwrap_matcher_fires_on_method_call() {
        let body = parse_fn_body("fn x() -> u32 { Some(5).unwrap() }");
        let f = match_unwrap(&body);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].pattern, "unwrap");
    }

    #[test]
    fn unwrap_matcher_no_match() {
        let body = parse_fn_body("fn x() -> u32 { 5 }");
        let f = match_unwrap(&body);
        assert!(f.is_empty());
    }

    #[test]
    fn expect_matcher_fires() {
        let body = parse_fn_body("fn x() -> u32 { Some(5).expect(\"y\") }");
        let f = match_expect(&body);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn panic_macros_matcher_fires_on_each_macro() {
        let body = parse_fn_body(
            "fn x() { panic!(\"a\"); unreachable!(); todo!(); unimplemented!(); }",
        );
        let f = match_panic_macros(&body);
        assert_eq!(f.len(), 4);
    }

    #[test]
    fn panic_macros_does_not_fire_on_println() {
        let body = parse_fn_body("fn x() { println!(\"hi\"); }");
        let f = match_panic_macros(&body);
        assert!(f.is_empty());
    }

    #[test]
    fn unwrap_unchecked_matcher_fires() {
        let body = parse_fn_body(
            "fn x() { let v = Some(1); v.unwrap_unchecked(); let r: Result<u32,()> = Ok(1); r.unwrap_err_unchecked(); }",
        );
        let f = match_unwrap_unchecked(&body);
        assert_eq!(f.len(), 2);
    }

    #[test]
    fn unbounded_loop_fires_on_bare_loop() {
        let body = parse_fn_body("fn x() { loop { let _ = 1; } }");
        let f = match_unbounded_loop(&body);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn unbounded_loop_does_not_fire_on_loop_with_break() {
        let body = parse_fn_body("fn x() { loop { if true { break; } } }");
        let f = match_unbounded_loop(&body);
        assert!(f.is_empty());
    }

    #[test]
    fn unbounded_loop_does_not_fire_on_loop_with_return() {
        let body = parse_fn_body("fn x() -> u32 { loop { return 1; } }");
        let f = match_unbounded_loop(&body);
        assert!(f.is_empty());
    }

    #[test]
    fn unbounded_loop_does_not_fire_on_for_or_while() {
        let body = parse_fn_body("fn x() { for _ in 0..10 { } while false { } }");
        let f = match_unbounded_loop(&body);
        assert!(f.is_empty());
    }

    #[test]
    fn await_in_guard_scope_fires_on_lock_let() {
        let body = parse_fn_body(
            "async fn x() { let g = mutex.lock().unwrap(); foo().await; drop(g); }",
        );
        let f = match_await_in_guard_scope(&body);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn await_in_guard_scope_no_let_no_match() {
        let body = parse_fn_body("async fn x() { foo().await; }");
        let f = match_await_in_guard_scope(&body);
        assert!(f.is_empty());
    }

    #[test]
    fn await_in_guard_scope_let_after_await_no_match() {
        let body = parse_fn_body(
            "async fn x() { foo().await; let g = mutex.lock().unwrap(); drop(g); }",
        );
        let f = match_await_in_guard_scope(&body);
        assert!(f.is_empty());
    }

    #[test]
    fn await_in_guard_scope_with_type_annotation_fires() {
        let body = parse_fn_body(
            "async fn x() { let g: MutexGuard<'_, u32> = something(); foo().await; drop(g); }",
        );
        let f = match_await_in_guard_scope(&body);
        assert_eq!(f.len(), 1);
    }
}

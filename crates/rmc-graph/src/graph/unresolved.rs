//! Preserve observed call sites that cannot become local graph edges.
use ra_ap_hir::{CallableKind, Semantics};
use ra_ap_ide_db::{RootDatabase, defs::Definition};
use ra_ap_syntax::{AstNode, SyntaxNode, ast};

use super::model::{UnresolvedCall, UnresolvedCallReason};
use super::structure::{declaration_definition, is_active};
use super::usages::TargetIds;

pub(super) fn scan_unresolved(
    sema: &Semantics<'_, RootDatabase>, targets: &mut TargetIds<'_>, tree: &SyntaxNode,
    calls: &mut Vec<UnresolvedCall>,
) {
    for node in tree.descendants() {
        if !ast::CallExpr::can_cast(node.kind()) && !ast::MethodCallExpr::can_cast(node.kind()) { continue; }
        if !is_active(sema, &node) { continue; }
        let (expression, resolved, indirect) = if let Some(call) = ast::CallExpr::cast(node.clone()) {
            let Some(callee) = call.expr() else { continue; };
            let resolved = sema.resolve_expr_as_callable(&callee).map(|c| c.kind());
            let mut unwrapped = callee.clone();
            while let ast::Expr::ParenExpr(paren) = &unwrapped {
                let Some(inner) = paren.expr() else { break; };
                unwrapped = inner;
            }
            let indirect = match unwrapped {
                ast::Expr::PathExpr(path) => path.path().and_then(|p| sema.resolve_path(&p))
                    .is_some_and(|p| matches!(p, ra_ap_hir::PathResolution::Local(_))),
                _ => true,
            };
            (callee.syntax().to_string(), resolved, indirect)
        } else if let Some(call) = ast::MethodCallExpr::cast(node.clone()) {
            (call.name_ref().map(|n| n.to_string()).unwrap_or_default(), sema.resolve_method_call(&call).map(CallableKind::Function), false)
        } else { continue; };
        let reason = if indirect { UnresolvedCallReason::IndirectCallable } else { match resolved {
            Some(CallableKind::TupleStruct(_) | CallableKind::TupleEnumVariant(_)) => continue,
            Some(CallableKind::Function(func)) if targets.of_definition(sema, Definition::Function(func)).is_some() => continue,
            Some(CallableKind::Function(_)) => UnresolvedCallReason::TargetOutsideGraph,
            Some(_) => UnresolvedCallReason::IndirectCallable,
            None => UnresolvedCallReason::Unresolved,
        }};
        let Some(scope) = sema.scope(&node) else { continue; };
        let Some(consumer_module) = targets.module_node(scope.module().nearest_non_block_module(sema.db).into()) else { continue; };
        let owner = sema.ancestors_with_macros(node.clone()).find_map(|n| declaration_definition(sema, &n));
        let consumer_item = owner.and_then(|d| targets.of_definition(sema, d));
        let range = sema.original_range(&node);
        let Some(file) = targets.source_file(sema.db, range.file_id) else { continue; };
        calls.push(UnresolvedCall {
            consumer_module, consumer_item, file,
            start: u32::from(range.range.start()), end: u32::from(range.range.end()), expression, reason,
        });
    }
}

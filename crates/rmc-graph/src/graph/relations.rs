//! Site semantics: target kind alone cannot distinguish a call from a value.
use ra_ap_hir::{AsAssocItem, AssocItemContainer, GenericDef, GenericParam, GenericSubstitution, Semantics};
use ra_ap_ide_db::{RootDatabase, defs::Definition};
use ra_ap_syntax::{AstNode, SyntaxNode, ast};

use super::model::UsageRelation;

/// Resolve a trait contract's method only when RA has an implementation.
/// Const/lifetime trait substitutions are not supported by RA's public helper;
/// leave those as explicit contract dispatch instead of guessing an impl.
pub(super) fn concrete_function<'db>(
    sema: &Semantics<'db, RootDatabase>, def: Definition<'db>, subst: Option<&GenericSubstitution<'db>>,
) -> Definition<'db> {
    let Definition::Function(func) = def else { return def; };
    let Some(assoc) = func.as_assoc_item(sema.db) else { return def; };
    let AssocItemContainer::Trait(trait_) = assoc.container(sema.db) else { return def; };
    let params = GenericDef::Trait(trait_).params(sema.db);
    if !params.iter().all(|p| matches!(p, GenericParam::TypeParam(_))) { return def; }
    let Some(types) = subst.map(|s| s.all_types(sema.db)) else { return def; };
    if types.len() < params.len() { return def; }
    let Some(env) = types.first().cloned() else { return def; };
    sema.resolve_trait_impl_method(env, trait_, func, types.into_iter().take(params.len()))
        .map(Definition::Function).unwrap_or(def)
}

fn is_callee(node: &SyntaxNode) -> bool {
    let Some(path) = node.parent().and_then(ast::PathSegment::cast).map(|s| s.parent_path()) else { return false; };
    // A prefix such as `Type` in `Type::function()` is not the callee.
    if path.parent_path().is_some() { return false; }
    let Some(expr) = path.syntax().parent().and_then(ast::PathExpr::cast) else { return false; };
    let mut expr = expr.syntax().clone();
    while let Some(parent) = expr.parent().and_then(ast::ParenExpr::cast) {
        expr = parent.syntax().clone();
    }
    expr.parent().and_then(ast::CallExpr::cast).and_then(|c| c.expr())
        .is_some_and(|callee| callee.syntax() == &expr)
}

pub(super) fn relation(sema: &Semantics<'_, RootDatabase>, def: Definition<'_>, node: &SyntaxNode) -> UsageRelation {
    match def {
        Definition::Function(func) => {
            let method = node.parent().and_then(ast::MethodCallExpr::cast).is_some();
            if !method && !is_callee(node) { return UsageRelation::FunctionReference; }
            match func.as_assoc_item(sema.db).map(|a| a.container(sema.db)) {
                Some(AssocItemContainer::Trait(_)) => UsageRelation::TraitDispatch,
                _ => UsageRelation::Call,
            }
        }
        Definition::Adt(_) | Definition::EnumVariant(_) => {
            let in_type = node.ancestors().skip(1).take_while(|n| {
                !ast::Expr::can_cast(n.kind()) && !ast::Pat::can_cast(n.kind())
            }).any(|n| ast::Type::can_cast(n.kind()));
            if in_type { return UsageRelation::TypeRef; }
            // ADT prefixes in paths and pattern occurrences are dependencies,
            // not constructors. Unit/record/tuple value expressions construct.
            let path = node.parent().and_then(ast::PathSegment::cast).map(|s| s.parent_path());
            if path.is_some_and(|p| p.parent_path().is_none() && p.syntax().parent().is_some_and(|n| {
                ast::PathExpr::can_cast(n.kind()) || ast::RecordExpr::can_cast(n.kind())
            })) { UsageRelation::Construct } else { UsageRelation::TypeRef }
        }
        Definition::Trait(_) | Definition::TypeAlias(_) | Definition::SelfType(_) => UsageRelation::TypeRef,
        Definition::Field(_) => {
            // Only the final place is written: `obj.child.field = ...` reads
            // `child` and writes `field`. Do not classify arbitrary lhs refs.
            let Some(field) = node.parent().and_then(ast::FieldExpr::cast) else { return UsageRelation::Use; };
            let mut place = field.syntax().clone();
            while let Some(parent) = place.parent().and_then(ast::ParenExpr::cast) { place = parent.syntax().clone(); }
            if place.parent().and_then(ast::BinExpr::cast).is_some_and(|b| {
                matches!(b.op_kind(), Some(ast::BinaryOp::Assignment { .. }))
                    && b.lhs().is_some_and(|lhs| lhs.syntax() == &place)
            }) { UsageRelation::Write } else { UsageRelation::Read }
        }
        _ => UsageRelation::Use,
    }
}

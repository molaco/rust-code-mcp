//! Shared helpers for resolving AST call sites to HIR `Function`s.
//!
//! `Semantics::resolve_path` can return `None` for paths that carry a
//! `GenericArgList` (turbofish) — e.g. `tokio::sync::mpsc::unbounded_channel::<T>()`.
//! Routing through the path-expression's inferred type via
//! `Semantics::resolve_expr_as_callable` recovers the underlying `Function`
//! and preserves use-alias correctness because type inference resolves the
//! alias to the canonical definition.

use ra_ap_hir::{CallableKind, Function, Semantics};
use ra_ap_ide_db::RootDatabase;
use ra_ap_syntax::ast;

/// Resolve a `CallExpr`'s callee to a HIR `Function`, robust against turbofish
/// generic-argument lists in the path. Returns `None` for non-function
/// callables (tuple-struct / tuple-enum-variant constructors, closures, fn
/// pointers).
pub(crate) fn resolve_call_to_function<'db>(
    sema: &Semantics<'db, RootDatabase>,
    call: &ast::CallExpr,
) -> Option<Function> {
    let callee = call.expr()?;
    let callable = sema.resolve_expr_as_callable(&callee)?;
    match callable.kind() {
        CallableKind::Function(f) => Some(f),
        _ => None,
    }
}

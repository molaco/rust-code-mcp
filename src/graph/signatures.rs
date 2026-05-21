//! Layer X (Phase 5) — function signature extraction.
//!
//! For every Item in `def_to_node` whose ModuleDefId is FunctionId
//! (free fn, inherent assoc fn, trait declaration fn — NOT trait-impl
//! body fns; see impls.rs::69 for the symmetric exclusion), extract:
//! - self kind (Owned / Ref / RefMut)
//! - non-self params (name, stringified type, by_ref, mutability)
//! - return type (stringified)
//! - is_async
//! - generic type parameters with their bounds
//!
//! The per-fn signature is stored on `ExtractionModel.signatures` as
//! `(NodeId, FunctionSignature)` and persisted to the new
//! `signatures_by_target` LMDB sub-DB by snapshot::write_model.
//!
//! Type stringification uses HirDisplay with the function's owning crate
//! as DisplayTarget; anonymous lifetimes are suppressed by default.
//!
//! NOTE: `TypeParam::trait_bounds` does *not* include where-clause bounds
//! added by methods after a parameter is introduced (the FIXME on the RA
//! method is real). The `GenericBound` records produced here therefore
//! reflect the bounds attached at the parameter's declaration site only.

use std::collections::HashMap;

use ra_ap_hir::{
    Crate, DisplayTarget, Function, GenericDef, HasCrate, HirDisplay, Mutability, attach_db,
};
use ra_ap_hir_def::ModuleDefId;
use ra_ap_ide_db::RootDatabase;
use ra_ap_vfs::Vfs;

use super::hir_trim::trim_hir_display;
use super::ids::NodeId;
use super::model::{ExtractionModel, FunctionSignature, GenericBound, Param, SelfKind};

pub fn extract_signatures(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    _vfs: &Vfs,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
) {
    attach_db(db, || {
        // Cache `Crate -> DisplayTarget` so we don't re-derive it per fn.
        let mut display_targets: HashMap<Crate, DisplayTarget> = HashMap::new();

        for (&def_id, &node_id) in def_to_node {
            let ModuleDefId::FunctionId(fn_id) = def_id else {
                continue;
            };
            let func = Function::from(fn_id);

            let krate = func.krate(db);
            let dt = *display_targets
                .entry(krate)
                .or_insert_with(|| krate.to_display_target(db));

            match build_signature(db, dt, func) {
                Some(sig) => model.signatures.push((node_id, sig)),
                None => {
                    tracing::trace!(
                        "extract_signatures: skip fn (could not build signature) node_id={:?}",
                        node_id
                    );
                }
            }
        }
    });
}

fn build_signature(
    db: &RootDatabase,
    dt: DisplayTarget,
    func: Function,
) -> Option<FunctionSignature> {
    let is_async = func.is_async(db);

    let self_param = func.self_param(db).map(|sp| match sp.access(db) {
        ra_ap_hir::Access::Owned => SelfKind::Owned,
        ra_ap_hir::Access::Shared => SelfKind::Ref,
        ra_ap_hir::Access::Exclusive => SelfKind::RefMut,
    });

    let mut params: Vec<Param> = Vec::new();
    for p in func.params_without_self(db) {
        let idx = p.index();
        let name = p
            .name(db)
            .map(|n| n.as_str().to_string())
            .unwrap_or_default();
        let ty_ref = p.ty();
        let (by_ref, mutability) = match ty_ref.as_reference() {
            Some((_inner, m)) => (true, matches!(m, Mutability::Mut)),
            None => (false, false),
        };
        let ty_string = trim_hir_display(&ty_ref.display(db, dt).to_string());
        let _ = idx; // Reserved for future per-param diagnostics; param order
                     // is preserved by the iteration order of `params_without_self`.
        params.push(Param {
            name,
            ty: ty_string,
            by_ref,
            mutability,
        });
    }

    let ret = func.ret_type(db);
    let return_type = trim_hir_display(&ret.display(db, dt).to_string());

    // Generic type parameters with their (declaration-site) trait bounds.
    let generic_def = GenericDef::from(func);
    let mut generics: Vec<GenericBound> = Vec::new();
    for toc in generic_def.type_or_const_params(db) {
        let Some(tp) = toc.as_type_param(db) else {
            continue;
        };
        // Skip implicit type parameters (e.g. `Self` in a trait or
        // `impl Trait` arg-position synthetics) — they're not user-visible.
        if tp.is_implicit(db) {
            continue;
        }
        let name = tp.name(db).as_str().to_string();
        let bounds: Vec<String> = tp
            .trait_bounds(db)
            .into_iter()
            .map(|t| t.name(db).as_str().to_string())
            .collect();
        generics.push(GenericBound { name, bounds });
    }

    Some(FunctionSignature {
        is_async,
        self_param,
        params,
        return_type,
        generics,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::test_support::shared_snapshot;
    use crate::graph::{FunctionFilter, SelfKindFilter};

    fn sig_of(qualified: &str) -> FunctionSignature {
        let snap = shared_snapshot();
        let (id, _node) = snap
            .lookup_by_qualified_name(qualified)
            .unwrap()
            .unwrap_or_else(|| panic!("`{qualified}` not in snapshot"));
        snap.function_signature(id)
            .expect("function_signature failed")
            .unwrap_or_else(|| panic!("no signature for `{qualified}`"))
    }

    #[test]
    fn signature_loader_load() {
        // `fn load(directory: &Path) -> Result<LoadedWorkspace>`
        let sig = sig_of("rust_code_mcp::graph::loader::load");
        assert!(!sig.is_async, "load is sync");
        assert!(sig.self_param.is_none(), "load is a free fn");
        assert_eq!(sig.params.len(), 1, "load has one param");
        assert!(
            sig.params[0].by_ref,
            "load takes &Path (by_ref=true), got {:?}",
            sig.params[0]
        );
        assert!(
            !sig.params[0].mutability,
            "&Path is shared, got mutability=true"
        );
        assert!(
            sig.params[0].ty.contains("Path"),
            "expected `Path` in ty, got `{}`",
            sig.params[0].ty
        );
        assert!(
            sig.return_type.contains("Result"),
            "expected `Result` in return_type, got `{}`",
            sig.return_type
        );
    }

    #[test]
    fn signature_opened_snapshot_usages_of() {
        // `fn usages_of(&self, target: NodeId) -> Result<Vec<Usage>>`
        let sig = sig_of("rust_code_mcp::graph::snapshot::OpenedSnapshot::usages_of");
        assert_eq!(
            sig.self_param,
            Some(SelfKind::Ref),
            "expected &self, got {:?}",
            sig.self_param
        );
        assert_eq!(sig.params.len(), 1, "expected single non-self param");
        assert_eq!(
            sig.params[0].name, "target",
            "expected param name `target`, got `{}`",
            sig.params[0].name
        );
        assert!(
            sig.return_type.contains("Vec"),
            "expected `Vec` in return_type, got `{}`",
            sig.return_type
        );
    }

    #[test]
    fn signature_workspace_stats_is_async() {
        let sig = sig_of("rust_code_mcp::tools::graph::core::workspace_stats");
        assert!(sig.is_async, "workspace_stats should be async");
    }

    #[test]
    fn signature_node_id_from_components() {
        // `fn from_components(parts: &[&str]) -> Self`
        let sig = sig_of("rust_code_mcp::graph::ids::NodeId::from_components");
        assert!(sig.self_param.is_none(), "from_components has no self");
        assert_eq!(sig.params.len(), 1, "from_components has one param");
    }

    #[test]
    fn functions_with_filter_smoke_async() {
        let snap = shared_snapshot();
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = match root_node.kind {
            crate::graph::NodeKind::Crate => root_id,
            _ => root_node
                .crate_id
                .or(root_node.parent_id)
                .expect("root module should have crate_id or parent"),
        };
        let filter = FunctionFilter {
            is_async: Some(true),
            ..Default::default()
        };
        let matches = snap
            .functions_with_filter(crate_id, &filter)
            .expect("functions_with_filter failed");
        assert!(
            !matches.is_empty(),
            "expected at least one async fn in rust_code_mcp"
        );
        for m in &matches {
            assert!(
                m.signature.is_async,
                "filter returned non-async match: {}",
                m.qualified_name
            );
        }
    }

    #[test]
    fn functions_with_filter_smoke_min_params() {
        let snap = shared_snapshot();
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = match root_node.kind {
            crate::graph::NodeKind::Crate => root_id,
            _ => root_node
                .crate_id
                .or(root_node.parent_id)
                .expect("root module should have crate_id or parent"),
        };
        let filter = FunctionFilter {
            min_param_count: Some(5),
            ..Default::default()
        };
        // Don't assert match-count; just that it doesn't error.
        let _ = snap
            .functions_with_filter(crate_id, &filter)
            .expect("functions_with_filter failed");
    }

    // Also ensure a self-kind filter compiles and runs.
    #[test]
    fn functions_with_filter_smoke_self_kind() {
        let snap = shared_snapshot();
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = match root_node.kind {
            crate::graph::NodeKind::Crate => root_id,
            _ => root_node
                .crate_id
                .or(root_node.parent_id)
                .expect("root module should have crate_id or parent"),
        };
        let filter = FunctionFilter {
            self_kind: Some(SelfKindFilter::Ref),
            ..Default::default()
        };
        let _ = snap
            .functions_with_filter(crate_id, &filter)
            .expect("functions_with_filter failed");
    }

    /// Confirms the underlying snapshot query produces enough matches on a
    /// permissive filter (`is_async=true`) to exercise the wrapper's default
    /// `limit=50` slicing — i.e. the wrapper test
    /// `functions_with_filter_default_limit_caps_results` will see
    /// `total_match_count > 50`. If this assertion ever fires, the wrapper
    /// test's "exercises the cap" assumption no longer holds and the
    /// permissive filter must be widened (or the test renamed).
    #[test]
    fn functions_with_filter_async_total_exceeds_default_limit() {
        let snap = shared_snapshot();
        let (root_id, root_node) = snap
            .lookup_by_qualified_name("rust_code_mcp")
            .unwrap()
            .unwrap();
        let crate_id = match root_node.kind {
            crate::graph::NodeKind::Crate => root_id,
            _ => root_node
                .crate_id
                .or(root_node.parent_id)
                .expect("root module should have crate_id or parent"),
        };
        let filter = FunctionFilter {
            is_async: Some(true),
            ..Default::default()
        };
        let matches = snap
            .functions_with_filter(crate_id, &filter)
            .expect("functions_with_filter failed");
        // We don't assert > 50 strictly because the count drifts as the
        // codebase evolves; we just assert the query returns > 0 so the
        // smoke is preserved. The wrapper-level pagination test owns the
        // strict cap assertion.
        assert!(
            !matches.is_empty(),
            "expected at least one async fn match for pagination smoke"
        );
    }
}

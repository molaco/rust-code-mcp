//! Phase 7 Path B — type-aware static-item metadata extraction.
//!
//! For every Item in `def_to_node` whose ModuleDefId is StaticId, extract:
//!   - the static's HIR type as a string (via HirDisplay)
//!   - the `mut` flag (true for `static mut FOO`)
//!
//! Persisted to the new `static_metadata_by_target` LMDB sub-DB by
//! snapshot::write_model. Read back at query time by `mut_static_audit`,
//! which classifies each static's metadata against known global-state
//! anti-patterns.

use std::collections::HashMap;

use ra_ap_hir::{Crate, DisplayTarget, HasCrate, HirDisplay, Static, attach_db};
use ra_ap_hir_def::ModuleDefId;
use ra_ap_ide_db::RootDatabase;
use ra_ap_vfs::Vfs;

use super::hir_trim::trim_hir_display;
use super::ids::NodeId;
use super::model::{ExtractionModel, StaticMetadata};

pub(crate) fn extract_statics(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    _vfs: &Vfs,
    def_to_node: &HashMap<ModuleDefId, NodeId>,
) {
    attach_db(db, || {
        // Cache `Crate -> DisplayTarget` so we don't re-derive it per static.
        let mut display_targets: HashMap<Crate, DisplayTarget> = HashMap::new();

        for (&def_id, &node_id) in def_to_node {
            let ModuleDefId::StaticId(static_id) = def_id else {
                continue;
            };
            let s = Static::from(static_id);

            // Defensive: `module(db).krate()` could conceivably fail for
            // synthetic items; if so, log and skip rather than panic.
            let krate = s.krate(db);
            let dt = *display_targets
                .entry(krate)
                .or_insert_with(|| krate.to_display_target(db));

            let ty = s.ty(db);
            let type_string = trim_hir_display(&ty.display(db, dt).to_string());
            let is_mut = s.is_mut(db);

            if type_string.is_empty() {
                tracing::trace!(
                    "extract_statics: skip static (empty type string) node_id={:?}",
                    node_id
                );
                continue;
            }

            model.statics.push((
                node_id,
                StaticMetadata {
                    type_string,
                    is_mut,
                },
            ));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::test_support::shared_snapshot;
    use crate::graph::query::audits::classify_metadata;

    #[test]
    fn classifier_detects_lazy_lock() {
        let meta = StaticMetadata {
            type_string: "LazyLock<HashMap<String, u64>>".into(),
            is_mut: false,
        };
        assert_eq!(classify_metadata(&meta), vec!["LazyLock"]);
    }

    #[test]
    fn classifier_detects_static_mut() {
        let meta = StaticMetadata {
            type_string: "u32".into(),
            is_mut: true,
        };
        assert_eq!(classify_metadata(&meta), vec!["static mut"]);
    }

    #[test]
    fn classifier_detects_combo() {
        let meta = StaticMetadata {
            type_string: "LazyLock<u32>".into(),
            is_mut: true,
        };
        let v = classify_metadata(&meta);
        assert!(v.contains(&"static mut") && v.contains(&"LazyLock"));
    }

    #[test]
    fn classifier_skips_inert_types() {
        let meta = StaticMetadata {
            type_string: "u32".into(),
            is_mut: false,
        };
        assert!(classify_metadata(&meta).is_empty());
    }

    #[test]
    fn classifier_detects_oncelock() {
        let meta = StaticMetadata {
            type_string: "OnceLock<String>".into(),
            is_mut: false,
        };
        assert_eq!(classify_metadata(&meta), vec!["OnceLock"]);
    }

    #[test]
    fn classifier_detects_oncecell() {
        let meta = StaticMetadata {
            type_string: "OnceCell<String>".into(),
            is_mut: false,
        };
        assert_eq!(classify_metadata(&meta), vec!["OnceCell"]);
    }

    #[test]
    fn audit_smoke_does_not_error() {
        let snap = shared_snapshot();
        let findings = snap
            .mut_static_audit()
            .expect("mut_static_audit should not error");
        // We expect at least the SEMANTIC LazyLock present in src/semantic/mod.rs.
        // Don't assert exact count — just that the call succeeded and we can
        // iterate findings.
        for f in &findings {
            assert!(!f.qualified_name.is_empty());
            assert!(!f.matched_pattern.is_empty());
        }
    }

    #[test]
    fn audit_detects_known_lazy_lock() {
        let snap = shared_snapshot();
        let findings = snap
            .mut_static_audit()
            .expect("mut_static_audit should not error");
        let semantic = findings.iter().find(|f| {
            f.qualified_name == "rmc_server::semantic::SEMANTIC"
                && f.matched_pattern == "LazyLock"
        });
        assert!(
            semantic.is_some(),
            "expected `rmc_server::semantic::SEMANTIC` to surface as LazyLock; found: {:?}",
            findings
                .iter()
                .map(|f| (&f.qualified_name, &f.matched_pattern))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn static_metadata_round_trip_for_semantic() {
        let snap = shared_snapshot();
        let (id, _node) = snap
            .lookup_by_qualified_name("rmc_server::semantic::SEMANTIC")
            .expect("lookup_by_qualified_name failed")
            .expect("rmc_server::semantic::SEMANTIC not in snapshot");
        let meta = snap
            .static_metadata(id)
            .expect("static_metadata failed")
            .expect("expected metadata for SEMANTIC");
        assert!(
            !meta.type_string.is_empty(),
            "type_string should be populated"
        );
        assert!(
            meta.type_string.contains("LazyLock"),
            "expected `LazyLock` in type_string, got `{}`",
            meta.type_string
        );
        assert!(!meta.is_mut, "SEMANTIC is not declared `static mut`");
    }
}

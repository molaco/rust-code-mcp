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
    use crate::graph::ids::NodeId;
    use crate::graph::model::{ItemKind, Node, NodeKind};
    use crate::graph::query::audits::classify_metadata;
    use crate::graph::snapshot::{OpenedSnapshot, persist_test_model};
    use crate::graph::storage::GraphEnvOptions;
    use std::collections::BTreeMap;
    use std::sync::OnceLock;

    struct StaticFixtureSnap {
        _workspace_td: tempfile::TempDir,
        _data_td: tempfile::TempDir,
        snap: OpenedSnapshot,
    }

    fn static_fixture_snapshot() -> &'static OpenedSnapshot {
        static CACHE: OnceLock<StaticFixtureSnap> = OnceLock::new();
        &CACHE
            .get_or_init(|| {
                let workspace_td = tempfile::tempdir().expect("create workspace tempdir");
                let data_td = tempfile::tempdir().expect("create graph data tempdir");
                let workspace_id = NodeId::from_components(&["static-fixture", "workspace"]);
                let crate_id = NodeId::from_components(&["static-fixture", "crate"]);
                let module_id = NodeId::from_components(&["static-fixture", "module"]);
                let global_count_id =
                    NodeId::from_components(&["static-fixture", "GLOBAL_COUNT"]);
                let name_cache_id = NodeId::from_components(&["static-fixture", "NAME_CACHE"]);
                let values_id = NodeId::from_components(&["static-fixture", "VALUES"]);
                let mut nodes = BTreeMap::new();
                nodes.insert(
                    workspace_id,
                    Node {
                        id: workspace_id,
                        kind: NodeKind::Workspace,
                        display_name: "static fixture".into(),
                        qualified_name: "static_fixture_crate".into(),
                        crate_id: None,
                        parent_id: None,
                        item_kind: None,
                        file: None,
                        span: None,
                        visibility: None,
                        attributes: Vec::new(),
                        crate_target_kind: None,
                    },
                );
                nodes.insert(
                    crate_id,
                    Node {
                        id: crate_id,
                        kind: NodeKind::Crate,
                        display_name: "static_fixture_crate".into(),
                        qualified_name: "static_fixture_crate".into(),
                        crate_id: Some(crate_id),
                        parent_id: Some(workspace_id),
                        item_kind: None,
                        file: None,
                        span: None,
                        visibility: None,
                        attributes: Vec::new(),
                        crate_target_kind: Some("lib".into()),
                    },
                );
                nodes.insert(
                    module_id,
                    Node {
                        id: module_id,
                        kind: NodeKind::Module,
                        display_name: "static_fixture_crate".into(),
                        qualified_name: "static_fixture_crate".into(),
                        crate_id: Some(crate_id),
                        parent_id: Some(crate_id),
                        item_kind: None,
                        file: Some("src/lib.rs".into()),
                        span: None,
                        visibility: Some("pub".into()),
                        attributes: Vec::new(),
                        crate_target_kind: None,
                    },
                );
                for (id, name) in [
                    (global_count_id, "GLOBAL_COUNT"),
                    (name_cache_id, "NAME_CACHE"),
                    (values_id, "VALUES"),
                ] {
                    nodes.insert(
                        id,
                        Node {
                            id,
                            kind: NodeKind::Item,
                            display_name: name.into(),
                            qualified_name: format!("static_fixture_crate::{name}"),
                            crate_id: Some(crate_id),
                            parent_id: Some(module_id),
                            item_kind: Some(ItemKind::Static),
                            file: Some("src/lib.rs".into()),
                            span: Some((0, 1)),
                            visibility: Some("pub".into()),
                            attributes: Vec::new(),
                            crate_target_kind: None,
                        },
                    );
                }
                let model = ExtractionModel {
                    workspace_root: workspace_td.path().to_path_buf(),
                    workspace_hash: "static-fixture".into(),
                    workspace_id,
                    nodes,
                    bindings: Vec::new(),
                    usages: Vec::new(),
                    contains: vec![
                        (workspace_id, crate_id),
                        (crate_id, module_id),
                        (module_id, global_count_id),
                        (module_id, name_cache_id),
                        (module_id, values_id),
                    ],
                    signatures: Vec::new(),
                    statics: vec![
                        (
                            global_count_id,
                            StaticMetadata {
                                type_string: "usize".into(),
                                is_mut: true,
                            },
                        ),
                        (
                            name_cache_id,
                            StaticMetadata {
                                type_string: "OnceLock<String>".into(),
                                is_mut: false,
                            },
                        ),
                        (
                            values_id,
                            StaticMetadata {
                                type_string: "LazyLock<Vec<usize>>".into(),
                                is_mut: false,
                            },
                        ),
                    ],
                };
                let env_opts = GraphEnvOptions {
                    map_size: 16 << 20,
                    ..Default::default()
                };
                let snap = persist_test_model(data_td.path(), &model, env_opts)
                    .expect("persist static fixture graph");
                StaticFixtureSnap {
                    _workspace_td: workspace_td,
                    _data_td: data_td,
                    snap,
                }
            })
            .snap
    }

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
        let snap = static_fixture_snapshot();
        let findings = snap
            .mut_static_audit()
            .expect("mut_static_audit should not error");
        for f in &findings {
            assert!(!f.qualified_name.is_empty());
            assert!(!f.matched_pattern.is_empty());
        }
    }

    #[test]
    fn audit_detects_known_static_mut() {
        let snap = static_fixture_snapshot();
        let findings = snap
            .mut_static_audit()
            .expect("mut_static_audit should not error");
        let global_count = findings.iter().find(|f| {
            f.qualified_name == "static_fixture_crate::GLOBAL_COUNT"
                && f.matched_pattern == "static mut"
        });
        assert!(
            global_count.is_some(),
            "expected `static_fixture_crate::GLOBAL_COUNT` to surface as static mut; found: {:?}",
            findings
                .iter()
                .map(|f| (&f.qualified_name, &f.matched_pattern))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn static_metadata_round_trip_for_static_mut_fixture() {
        let snap = static_fixture_snapshot();
        let (id, _node) = snap
            .lookup_by_qualified_name("static_fixture_crate::GLOBAL_COUNT")
            .expect("lookup_by_qualified_name failed")
            .expect("GLOBAL_COUNT not in snapshot");
        let meta = snap
            .static_metadata(id)
            .expect("static_metadata failed")
            .expect("expected metadata for GLOBAL_COUNT");
        assert!(
            !meta.type_string.is_empty(),
            "type_string should be populated"
        );
        assert!(
            meta.type_string.contains("usize"),
            "expected `usize` in type_string, got `{}`",
            meta.type_string
        );
        assert!(meta.is_mut, "GLOBAL_COUNT is declared `static mut`");
    }
}

//! Extraction passes: HIR → ExtractionModel.
//!
//! Layer 2 milestone: Workspace, Crate, Module nodes plus Contains edges.
//! Layer 3 adds the bindings pass which also creates Item nodes and
//! ExternalSymbol stubs as a side effect.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Result, ensure};
use ra_ap_hir::Crate;
use ra_ap_hir_def::ModuleId;
use ra_ap_hir_def::nameres::crate_def_map;
use ra_ap_ide::RootDatabase;
use ra_ap_vfs::Vfs;

use super::audit_util::resolve_workspace_relative;
use super::attributes::extract_attributes;
use super::bindings::extract_bindings;
use super::ids::{NodeId, workspace_hash};
use super::identity::{crate_node_id, module_node_id, target_key};
use super::labels::{crate_display_name, module_path_segments};
use super::impls::extract_impl_items;
use super::loader::{LoadedWorkspace, prime_for};
use super::model::{ExtractionModel, Node, NodeKind};
use super::signatures::extract_signatures;
use super::statics::extract_statics;
use super::usages::{UsagesPass, extract_usages};

/// The whole workspace in one model, every pass over every crate at once.
/// The reference the per-crate build ([`extract_by_crate`]) is checked
/// against; it goes once that build has been accepted on a large workspace.
pub fn extract(loaded: &LoadedWorkspace) -> ExtractionModel {
    extract_with(loaded, UsagesPass::from_env())
}

pub(crate) fn extract_with(loaded: &LoadedWorkspace, usages_pass: UsagesPass) -> ExtractionModel {
    let timing = std::env::var_os("EXTRACT_TIMING").is_some();
    let t_total = std::time::Instant::now();
    let Skeleton { mut model, maps } = emit_skeleton(loaded, &loaded.local_crates, timing);
    run_passes(
        &mut model,
        loaded,
        &loaded.local_crates,
        &maps,
        usages_pass,
        timing,
    );
    if timing {
        eprintln!(
            "extract: TOTAL                        {:>9.2?}",
            t_total.elapsed()
        );
    }
    model
}

/// A write unit of the per-crate build, keyed by Cargo target identity.
/// Same-name lib/bin targets have independent rows and dependency edges.
#[derive(Debug, Clone)]
pub(crate) struct CrateGroup {
    pub id: NodeId,
    pub name: String,
    pub crates: Vec<Crate>,
}

/// Build the model of the crates in `dirty`, one [`CrateGroup`] at a time,
/// handing each group's model to `write` and giving rust-analyzer's memory
/// back before the next group.
///
/// Each model holds the whole skeleton of the scope (workspace, crate and
/// module nodes of `dirty` and its local dependencies) plus what the passes
/// produced walking the group alone; rows owned by other crates may appear in
/// it (an imported item's node, recreated by the bindings pass) and are the
/// writer's to filter out — `ownership::write_owned_rows` with
/// `|c| c == group.id` writes exactly the group's rows. Every row a crate owns
/// is computed from that crate's own files (plan step
/// `consumer-centric-usages`), so the groups are independent of each other
/// and of their order; the dependency order is kept only so that a log reads
/// bottom-up.
///
/// `dirty` must hold whole target groups. A lib and a same-named bin are
/// independent groups and can each be rebuilt alone.
///
/// # Memory
///
/// Every LRU-capped rust-analyzer query is cut to capacity 1 (0 would turn
/// eviction off), and after priming and after each group a synthetic write
/// bumps the revision — the one moment salsa evicts (`reset_for_new_revision`)
/// — which drops the parse trees, bodies and inference results of what was
/// just walked. Then `release_free_memory`, if given, returns the freed pages:
/// the allocator is the binary's choice, so the binary supplies the call.
///
/// What stays is the floor every group needs anyway: DefMaps, item trees and
/// trait impls of the whole scope (plus the symbol index priming builds for
/// local crates, which the graph never reads). Measured on this repository
/// (7 crates, 2026-10-21): 3.6 GB at the end of priming — the peak of the
/// build — and 2.2–2.3 GB after each group.
///
/// What is NOT done is the type-interner sweep (`hir::collect_ty_garbage`)
/// that `AnalysisHost::trigger_garbage_collection` adds. It is process-global
/// and unsound while any other rust-analyzer database in the process is
/// mid-query; inside the daemon a build holds the analysis gate shared
/// (`rmc-server` `deep_stack`), so others may be.
pub(crate) fn extract_by_crate(
    loaded: &mut LoadedWorkspace,
    dirty: &[Crate],
    usages_pass: UsagesPass,
    release_free_memory: Option<fn() -> bool>,
    write: &mut dyn FnMut(&ExtractionModel, &CrateGroup) -> Result<()>,
) -> Result<()> {
    use ra_ap_ide_db::base_db::salsa::{Database as _, Durability};

    let timing = std::env::var_os("EXTRACT_TIMING").is_some();
    let t_total = std::time::Instant::now();

    let groups = crate_groups(loaded, dirty)?;

    // Capacities first: they only act when the revision bumps, and the first
    // bump comes right after priming, which parses every file in scope.
    let db = &mut loaded.db;
    for name in db.lru_capacity_names() {
        db.set_lru_capacity_by_name(name, 1);
    }
    let give_back = |loaded: &mut LoadedWorkspace| {
        loaded.db.synthetic_write(Durability::LOW);
        if let Some(release) = release_free_memory {
            release();
        }
    };

    let t = std::time::Instant::now();
    prime_for(loaded, dirty);
    // The skeleton covers `dirty` and the local crates it depends on: a group's
    // bindings, visibilities and usage targets name modules of those and of no
    // other crate. Kept in `local_crates` order, because `insert_node` keeps
    // the first of two merging nodes and the whole-workspace model emits in
    // that order.
    let scope = local_dependency_closure(loaded, dirty);
    let skeleton = emit_skeleton(loaded, &scope, false);
    give_back(loaded);
    if timing {
        eprintln!(
            "extract: prime + skeleton             {:>9.2?}  ({} dirty crates, {} in scope, {} modules; RSS {} MiB)",
            t.elapsed(),
            dirty.len(),
            scope.len(),
            skeleton.maps.module_node_for.len(),
            rss_mib()
        );
    }

    for group in &groups {
        let t = std::time::Instant::now();
        let mut model = skeleton.model.clone();
        let passes = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_passes(
            &mut model,
            loaded,
            &group.crates,
            &skeleton.maps,
            usages_pass,
            timing,
        )));
        if let Err(payload) = passes {
            match payload.downcast::<ra_ap_ide_db::base_db::salsa::Cancelled>() {
                Ok(cancelled) => return Err(anyhow::Error::new(*cancelled).context(format!(
                    "extracting crate `{}`; source drift: {:?}", group.name, loaded.db.file_text_drift()))),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        write(&model, group)?;
        drop(model);
        give_back(loaded);
        if timing {
            eprintln!(
                "extract: crate {:<28} {:>9.2?}  ({} crates; RSS {} MiB)",
                group.name,
                t.elapsed(),
                group.crates.len(),
                rss_mib()
            );
        }
    }
    if timing {
        eprintln!(
            "extract: TOTAL by crate               {:>9.2?}  ({} groups)",
            t_total.elapsed(),
            groups.len()
        );
    }
    Ok(())
}

/// Resident set size in MiB, for the timing log (0 where `/proc` is not
/// there). `rmc-server` has the same reading as `mcp::memory::rss_kib`; this
/// crate sits below it.
fn rss_mib() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
            line.split_whitespace().nth(1)?.parse::<u64>().ok()
        })
        .map_or(0, |kib| kib / 1024)
}

/// The groups of `dirty`, dependencies before dependents.
pub(crate) fn crate_groups(loaded: &LoadedWorkspace, dirty: &[Crate]) -> Result<Vec<CrateGroup>> {
    let db = &loaded.db;
    let hash = workspace_hash(&loaded.workspace_root);
    let dirty_set: HashSet<Crate> = dirty.iter().copied().collect();
    for &krate in dirty {
        ensure!(
            loaded.local_crates.contains(&krate),
            "crate `{}` is not a local crate of {}",
            crate_display_name(db, krate),
            loaded.workspace_root.display()
        );
    }

    let mut groups: Vec<CrateGroup> = Vec::new();
    let mut group_of: HashMap<Crate, usize> = HashMap::new();
    for &krate in &loaded.local_crates {
        let name = target_key(db, &loaded.vfs, &loaded.workspace_root, krate);
        let id = crate_node_id(&hash, &name);
        let index = match groups.iter().position(|g| g.id == id) {
            Some(index) => index,
            None => {
                groups.push(CrateGroup {
                    id,
                    name,
                    crates: Vec::new(),
                });
                groups.len() - 1
            }
        };
        groups[index].crates.push(krate);
        group_of.insert(krate, index);
    }
    groups.sort_by(|a, b| a.name.cmp(&b.name));
    group_of.clear();
    for (index, group) in groups.iter().enumerate() {
        for &krate in &group.crates { group_of.insert(krate, index); }
    }
    for group in &groups {
        let inside = group
            .crates
            .iter()
            .filter(|k| dirty_set.contains(k))
            .count();
        ensure!(
            inside == 0 || inside == group.crates.len(),
            "crate `{}` is {} crates sharing one node; rebuild all of them or none",
            group.name,
            group.crates.len()
        );
    }

    // Depth-first post-order over the groups' dependency edges; cargo's crate
    // graph has no cycles, and the lib ← bin edge inside a group is skipped.
    fn visit(
        index: usize,
        db: &RootDatabase,
        groups: &[CrateGroup],
        group_of: &HashMap<Crate, usize>,
        done: &mut [bool],
        order: &mut Vec<usize>,
    ) {
        if done[index] {
            return;
        }
        done[index] = true;
        for krate in &groups[index].crates {
            for dep in krate.dependencies(db) {
                if let Some(&dep_index) = group_of.get(&dep.krate) {
                    visit(dep_index, db, groups, group_of, done, order);
                }
            }
        }
        order.push(index);
    }
    let mut done = vec![false; groups.len()];
    let mut order = Vec::new();
    for index in 0..groups.len() {
        visit(index, db, &groups, &group_of, &mut done, &mut order);
    }
    let mut slots: Vec<Option<CrateGroup>> = groups.into_iter().map(Some).collect();
    Ok(order
        .into_iter()
        .map(|index| slots[index].take().unwrap())
        .filter(|group| dirty_set.contains(&group.crates[0]))
        .collect())
}

/// `crates` and the local crates they depend on, transitively, in
/// `local_crates` order.
fn local_dependency_closure(loaded: &LoadedWorkspace, crates: &[Crate]) -> Vec<Crate> {
    let local: HashSet<Crate> = loaded.local_crates.iter().copied().collect();
    let mut reached: HashSet<Crate> = HashSet::new();
    let mut stack: Vec<Crate> = crates.to_vec();
    while let Some(krate) = stack.pop() {
        if !reached.insert(krate) {
            continue;
        }
        stack.extend(
            krate
                .dependencies(&loaded.db)
                .into_iter()
                .map(|dep| dep.krate)
                .filter(|dep| local.contains(dep)),
        );
    }
    loaded
        .local_crates
        .iter()
        .copied()
        .filter(|krate| reached.contains(krate))
        .collect()
}

/// The RA-id → node maps every pass reads.
struct CrateMaps {
    crate_node_for: HashMap<Crate, NodeId>,
    crate_name_for: HashMap<Crate, String>,
    module_node_for: HashMap<ModuleId, NodeId>,
}

/// What a pass over some crates needs to know about all crates in scope: the
/// workspace node, the crate and module nodes with their contains edges, and
/// the maps to them.
struct Skeleton {
    model: ExtractionModel,
    maps: CrateMaps,
}

fn emit_skeleton(loaded: &LoadedWorkspace, crates: &[Crate], timing: bool) -> Skeleton {
    let workspace_hash = workspace_hash(&loaded.workspace_root);
    let workspace_id = NodeId::from_components(&[workspace_hash.as_str(), "workspace"]);

    let mut model = ExtractionModel {
        workspace_root: loaded.workspace_root.clone(),
        workspace_hash: workspace_hash.clone(),
        workspace_id,
        nodes: Default::default(),
        bindings: Vec::new(),
        usages: Vec::new(),
        unresolved_calls: Vec::new(),
        contains: Vec::new(),
        signatures: Vec::new(),
        statics: Vec::new(),
    };

    let workspace_display = loaded
        .workspace_root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("workspace")
        .to_string();

    model.insert_node(Node {
        id: workspace_id,
        kind: NodeKind::Workspace,
        display_name: workspace_display,
        qualified_name: loaded.workspace_root.display().to_string(),
        crate_id: None,
        parent_id: None,
        item_kind: None,
        file: None,
        span: None,
        visibility: None,
        attributes: Vec::new(),
        crate_target_kind: None,
        declaration_module: None,
    });

    let mut crate_node_for: HashMap<Crate, NodeId> = HashMap::new();
    let mut crate_name_for: HashMap<Crate, String> = HashMap::new();
    let mut module_node_for: HashMap<ModuleId, NodeId> = HashMap::new();

    let t = std::time::Instant::now();
    for &krate in crates {
        emit_crate(
            &mut model,
            &loaded.db,
            &loaded.vfs,
            &loaded.workspace_root,
            krate,
            workspace_id,
            &mut crate_node_for,
            &mut crate_name_for,
            &mut module_node_for,
        );
    }
    if timing {
        eprintln!(
            "extract: emit_crates                  {:>9.2?}  ({} crates, {} modules)",
            t.elapsed(),
            crates.len(),
            module_node_for.len()
        );
    }

    Skeleton {
        model,
        maps: CrateMaps {
            crate_node_for,
            crate_name_for,
            module_node_for,
        },
    }
}

/// The six passes after the skeleton, walking `crates` only. `model` is a
/// skeleton covering at least `crates` and everything they depend on.
fn run_passes(
    model: &mut ExtractionModel,
    loaded: &LoadedWorkspace,
    crates: &[Crate],
    maps: &CrateMaps,
    usages_pass: UsagesPass,
    timing: bool,
) {
    let CrateMaps {
        crate_node_for,
        crate_name_for,
        module_node_for,
    } = maps;

    let t = std::time::Instant::now();
    let mut def_to_node = extract_bindings(
        model,
        &loaded.db,
        crates,
        crate_node_for,
        module_node_for,
    );
    if timing {
        eprintln!(
            "extract: extract_bindings            {:>9.2?}  ({} bindings, {} def→node)",
            t.elapsed(),
            model.bindings.len(),
            def_to_node.len()
        );
    }

    // Layer 4: extend def_to_node with methods, assoc consts, assoc types from
    // inherent impls and trait declarations BEFORE running the usages pass —
    // that makes them usage targets (who_uses(Foo::bar),
    // who_uses(Trait::method)). The consumer-side usages pass recomputes their
    // ids itself (`usages::TargetIds`); which defs count is mirrored there.
    let t = std::time::Instant::now();
    let nodes_before_impls = model.nodes.len();
    extract_impl_items(
        model,
        &loaded.db,
        &loaded.vfs,
        crates,
        crate_node_for,
        crate_name_for,
        &mut def_to_node,
    );
    if timing {
        eprintln!(
            "extract: extract_impl_items           {:>9.2?}  (+{} nodes; {} total)",
            t.elapsed(),
            model.nodes.len() - nodes_before_impls,
            model.nodes.len()
        );
    }

    // v8: per-Item attribute extraction. Runs after impls (so the v5 method /
    // assoc-const / assoc-type and v7 enum-variant Items already exist in
    // `model.nodes` and `def_to_node`) and before usages (purely for ordering
    // — the two passes are independent).
    super::structure::extract_structural_items(model, &loaded.db, &loaded.vfs, crates, &mut def_to_node, module_node_for);
    let t = std::time::Instant::now();
    extract_attributes(model, &loaded.db, &loaded.vfs, crates, &def_to_node);
    if timing {
        let with_attrs = model
            .nodes
            .values()
            .filter(|n| !n.attributes.is_empty())
            .count();
        eprintln!(
            "extract: extract_attributes           {:>9.2?}  ({} items have attrs)",
            t.elapsed(),
            with_attrs
        );
    }

    // v9 (Phase 5): per-function signature extraction. Runs after impls /
    // attributes (so def_to_node already includes inherent/trait assoc fns)
    // and before usages (purely for ordering — independent passes).
    let t = std::time::Instant::now();
    extract_signatures(model, &loaded.db, &loaded.vfs, &def_to_node);
    if timing {
        eprintln!(
            "extract: extract_signatures           {:>9.2?}  ({} signatures)",
            t.elapsed(),
            model.signatures.len()
        );
    }

    // v10 (Phase 7 Path B): per-Static metadata extraction. Runs after
    // signatures and before usages (independent passes — order is purely
    // for the timing breakdown).
    let t = std::time::Instant::now();
    extract_statics(model, &loaded.db, &loaded.vfs, &def_to_node);
    if timing {
        eprintln!(
            "extract: extract_statics              {:>9.2?}  ({} statics)",
            t.elapsed(),
            model.statics.len()
        );
    }

    let t = std::time::Instant::now();
    extract_usages(
        model,
        &loaded.db,
        &loaded.vfs,
        crates,
        &def_to_node,
        module_node_for,
        usages_pass,
    );
    // Scanning both macro inputs and generated bodies may discover the same
    // expanded occurrence twice. Normalize the raw model by the persisted
    // observation identity, retaining distinct expansion paths/token ranges.
    let mut seen = HashSet::new();
    model.usages.retain(|u| seen.insert(super::snapshot::usage_id_for(u)));
    if timing {
        eprintln!(
            "extract: extract_usages ({usages_pass:?}) {:>9.2?}  ({} usages)",
            t.elapsed(),
            model.usages.len()
        );
    }
}

fn emit_crate(
    model: &mut ExtractionModel,
    db: &RootDatabase,
    vfs: &Vfs,
    workspace_root: &Path,
    krate: Crate,
    workspace_id: NodeId,
    crate_node_for: &mut HashMap<Crate, NodeId>,
    crate_name_for: &mut HashMap<Crate, String>,
    module_node_for: &mut HashMap<ModuleId, NodeId>,
) {
    let crate_name = crate_display_name(db, krate);
    let def_map = crate_def_map(db, krate.base());
    let root_module_id = def_map.crate_root(db);
    let crate_target_kind = Some(super::identity::target_kind(db, krate).to_owned());
    let crate_id = crate_node_id(&model.workspace_hash, &target_key(db, vfs, workspace_root, krate));

    crate_node_for.insert(krate, crate_id);
    crate_name_for.insert(krate, crate_name.clone());

    model.insert_node(Node {
        id: crate_id,
        kind: NodeKind::Crate,
        display_name: crate_name.clone(),
        qualified_name: crate_name.clone(),
        crate_id: Some(crate_id),
        parent_id: Some(workspace_id),
        item_kind: None,
        file: resolve_workspace_relative(vfs, krate.root_file(db), workspace_root),
        span: None,
        visibility: Some("pub".to_string()),
        attributes: Vec::new(),
        crate_target_kind,
        declaration_module: None,
    });
    model.insert_contains(workspace_id, crate_id);

    for (module_id, _) in def_map.modules() {
        // Skip block-expression modules.
        if module_id.is_block_module(db) {
            continue;
        }

        let path = module_path_segments(db, def_map, module_id);
        let qualified = if path.is_empty() {
            crate_name.clone()
        } else {
            format!("{crate_name}::{}", path.join("::"))
        };

        let module_node_id = module_node_id(crate_id, &path);
        module_node_for.insert(module_id, module_node_id);

        let parent_id = if module_id == root_module_id {
            Some(crate_id)
        } else if let Some(parent_module) = def_map.containing_module(module_id) {
            // Compute the parent's NodeId now (we may not have inserted it yet —
            // the iteration order from def_map.modules() is not parent-first).
            let parent_path = module_path_segments(db, def_map, parent_module);
            Some(super::identity::module_node_id(crate_id, &parent_path))
        } else {
            Some(crate_id)
        };

        let display_name = path.last().cloned().unwrap_or_else(|| crate_name.clone());
        let source = ra_ap_hir::Module::from(module_id).definition_source(db);
        let (source_file, source_range) = if let Some(call) = source.file_id.original_call_node(db) {
            (call.file_id, call.value.text_range())
        } else { (source.file_id.original_file(db), source.value.node().text_range()) };

        model.insert_node(Node {
            id: module_node_id,
            kind: NodeKind::Module,
            display_name,
            qualified_name: qualified,
            crate_id: Some(crate_id),
            parent_id,
            item_kind: None,
            file: resolve_workspace_relative(vfs, source_file.file_id(db), workspace_root),
            span: Some((u32::from(source_range.start()), u32::from(source_range.end()))),
            visibility: None,
            attributes: Vec::new(),
            crate_target_kind: None,
            declaration_module: Some(module_node_id),
        });

        if let Some(parent) = parent_id {
            model.insert_contains(parent, module_node_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::loader;
    use crate::graph::model::{BindingKind, NodeKind};
    use crate::graph::ownership::{delete_crate_rows, write_owned_rows};
    use crate::graph::snapshot::{OpenedSnapshot, persist_test_by_crate, persist_test_model};
    use crate::graph::snapshot_compare::compare_snapshots;
    use crate::graph::storage::GraphEnvOptions;
    use std::path::Path;
    use std::sync::OnceLock;

    // Load + extract this workspace once and share across all tests in this
    // module. Loading dominates the runtime (~3s release / ~25s debug); the
    // tests below are read-only assertions over the resulting model.
    // We cache only ExtractionModel (Send+Sync); LoadedWorkspace is dropped
    // because RootDatabase is !Sync.
    fn shared_model() -> &'static ExtractionModel {
        static CACHE: OnceLock<ExtractionModel> = OnceLock::new();
        CACHE.get_or_init(|| {
            let loaded = loader::load(Path::new(env!("CARGO_MANIFEST_DIR"))).unwrap();
            extract(&loaded)
        })
    }

    /// Same-name lib/bin, including one physical module compiled by both.
    fn write_fixture_with_bin(root: &Path) {
        crate::graph::test_support::write_two_crate_fixture(root);
        let lib = root.join("beta/src/lib.rs");
        std::fs::write(&lib, format!("pub mod shared;\n{}", std::fs::read_to_string(&lib).unwrap())).unwrap();
        std::fs::write(root.join("beta/src/shared.rs"), "pub fn helper() -> i32 { alpha::double(1) }\n").unwrap();
        std::fs::write(
            root.join("beta/src/main.rs"),
            "mod shared;\nfn main() { let _ = shared::helper();\n    let (a, b) = beta::ordered(2, 1);\n    \
             println!(\"{}\", alpha::quadruple(a + b));\n}\n",
        )
        .unwrap();
    }

    fn group_crates(loaded: &LoadedWorkspace, name: &str) -> Vec<Crate> {
        crate_groups(loaded, &loaded.local_crates)
            .unwrap()
            .into_iter()
            .filter(|g| g.name.starts_with(&format!("{name}[")))
            .flat_map(|g| g.crates)
            .collect::<Vec<_>>()
    }

    fn group_names(loaded: &LoadedWorkspace, dirty: &[Crate]) -> Vec<String> {
        crate_groups(loaded, dirty)
            .unwrap()
            .into_iter().map(|g| g.name).collect()
    }

    fn assert_same(a: &OpenedSnapshot, b: &OpenedSnapshot, what: &str) {
        let diff = compare_snapshots(a, b, 20).unwrap();
        assert!(diff.is_empty(), "{what}:\n{diff}");
    }

    /// The per-crate build of a workspace is the whole-workspace build, row for
    /// row — including the lib + bin pair sharing a crate node.
    #[test]
    fn by_crate_build_equals_whole_build_on_fixture() {
        let workspace = tempfile::tempdir().unwrap();
        write_fixture_with_bin(workspace.path());
        let mut loaded = loader::load_unprimed(workspace.path()).unwrap();
        assert_eq!(group_crates(&loaded, "beta").len(), 2, "lib + bin `beta`");

        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let by_crate =
            persist_test_by_crate(data_a.path(), &mut loaded, GraphEnvOptions::default()).unwrap();
        let whole =
            persist_test_model(data_b.path(), &extract(&loaded), GraphEnvOptions::default())
                .unwrap();
        assert_same(&whole, &by_crate, "whole build (left) vs by crate (right)");
    }

    /// The two-crate fixture plus a method returning `impl Iterator` (RPIT) in
    /// `alpha`, called from a module file walked before its own and from
    /// `beta`, where the iterator's item type decides what `.number()` is.
    /// Shaped after a real-world `Shape::faces` accessor.
    fn write_fixture_with_rpit(root: &Path) {
        crate::graph::test_support::write_two_crate_fixture(root);
        let lib = root.join("alpha/src/lib.rs");
        let text = std::fs::read_to_string(&lib).unwrap();
        std::fs::write(&lib, format!("pub mod user;\npub mod solid;\n{text}")).unwrap();
        std::fs::write(
            root.join("alpha/src/solid.rs"),
            "#[derive(Debug, Clone, Copy, PartialEq, Eq)]\n\
             pub struct FaceIx(pub u32);\n\n\
             impl FaceIx {\n    pub fn number(self) -> u32 {\n        self.0\n    }\n}\n\n\
             pub struct Solid(pub u32);\n\n\
             impl Solid {\n    \
             pub fn face_count(&self) -> u32 {\n        self.0\n    }\n\n    \
             pub fn faces(&self) -> impl Iterator<Item = FaceIx> + use<> {\n        \
             (0..self.face_count()).map(FaceIx)\n    }\n}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("alpha/src/user.rs"),
            "use crate::solid::{FaceIx, Solid};\n\n\
             pub fn all(solid: &Solid) -> Vec<FaceIx> {\n    \
             solid.faces().collect::<Vec<_>>()\n}\n",
        )
        .unwrap();
        let beta = root.join("beta/src/lib.rs");
        let mut text = std::fs::read_to_string(&beta).unwrap();
        text.push_str(
            "\npub fn face_numbers(solid: &alpha::solid::Solid) -> Vec<u32> {\n    \
             solid.faces().map(|f| f.number()).collect()\n}\n",
        );
        std::fs::write(&beta, text).unwrap();
    }

    /// Types behind an `impl Trait` return survive the per-crate build: the
    /// LRU capacity 1 and the revision bump after each group evict the
    /// inference of `Solid::faces` while its caller in `beta` is still to come.
    /// Each side gets its own fresh load, as each build in the daemon does.
    #[test]
    fn by_crate_build_keeps_types_behind_impl_trait() {
        let workspace = tempfile::tempdir().unwrap();
        write_fixture_with_rpit(workspace.path());
        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let by_crate = {
            let mut loaded = loader::load_unprimed(workspace.path()).unwrap();
            persist_test_by_crate(data_a.path(), &mut loaded, GraphEnvOptions::default()).unwrap()
        };
        let whole = {
            let loaded = loader::load_unprimed(workspace.path()).unwrap();
            persist_test_model(data_b.path(), &extract(&loaded), GraphEnvOptions::default())
                .unwrap()
        };
        assert_same(&whole, &by_crate, "whole build (left) vs by crate (right)");
    }

    /// rust-analyzer bug, not ours (found 2026-10-21 on a private workspace, plan step
    /// `per-crate-build-loop`): in a database that already went through a
    /// per-crate build — `InferenceResult::for_body` at LRU capacity 1 and a
    /// revision bump per group — a later whole extraction loses the usages in
    /// the body of `Solid::faces` (`-> impl Iterator`): its inference comes back
    /// empty. Gone with either the LRU cap on `for_body` left alone or no bump
    /// (the cap on the other LRU queries is harmless). The fork's
    /// `lru = 2024` on `for_body` (RA `792fc9fd96`) exposes a long-lived
    /// database to the same thing. The graph build is not affected — each build
    /// loads fresh ([`by_crate_build_keeps_types_behind_impl_trait`]) — so this
    /// test only pins the bug down for whoever fixes it in the fork.
    #[test]
    #[ignore = "rust-analyzer bug: an evicted inference re-run loses an RPIT body"]
    fn evicted_inference_of_an_impl_trait_fn_comes_back_whole() {
        let workspace = tempfile::tempdir().unwrap();
        write_fixture_with_rpit(workspace.path());
        let mut loaded = loader::load_unprimed(workspace.path()).unwrap();
        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let by_crate =
            persist_test_by_crate(data_a.path(), &mut loaded, GraphEnvOptions::default()).unwrap();
        let whole_after =
            persist_test_model(data_b.path(), &extract(&loaded), GraphEnvOptions::default())
                .unwrap();
        assert_same(
            &by_crate,
            &whole_after,
            "by crate (left) vs whole build in the same database afterwards (right)",
        );
    }

    /// An external module imported under different names by two crates is one
    /// stub, whichever crate the walk meets first. Its stub used to be named
    /// `<module path>::<name of the first import>`, so the whole build (one
    /// walk over both crates) and the per-crate build (a walk per crate) minted
    /// different stubs for the same `std::fmt`.
    #[test]
    fn an_external_module_imported_under_two_names_is_one_stub() {
        let workspace = tempfile::tempdir().unwrap();
        write_fixture_with_bin(workspace.path());
        for (file, import) in [
            ("alpha/src/lib.rs", "pub use std::fmt;\n"),
            ("beta/src/lib.rs", "pub use std::fmt as f;\n"),
        ] {
            let path = workspace.path().join(file);
            let text = std::fs::read_to_string(&path).unwrap();
            std::fs::write(&path, format!("{import}{text}")).unwrap();
        }
        let mut loaded = loader::load_unprimed(workspace.path()).unwrap();

        let whole_model = extract(&loaded);
        let (data_a, data_b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let whole =
            persist_test_model(data_a.path(), &whole_model, GraphEnvOptions::default()).unwrap();
        let by_crate =
            persist_test_by_crate(data_b.path(), &mut loaded, GraphEnvOptions::default()).unwrap();
        assert_same(&whole, &by_crate, "whole build (left) vs by crate (right)");

        // `std::fmt` is a re-export of `alloc::fmt`.
        let fmt_stubs: Vec<&str> = whole_model
            .nodes
            .values()
            .filter(|n| n.kind == NodeKind::ExternalSymbol)
            .filter(|n| matches!(n.display_name.as_str(), "fmt" | "f"))
            .map(|n| n.qualified_name.as_str())
            .collect();
        assert_eq!(fmt_stubs, vec!["alloc::fmt"], "the stub is the module's own path");
    }

    /// Deleting one crate's rows from a full snapshot and building only that
    /// crate into it gives the full snapshot back. `alpha` depends on no local
    /// crate, so its build's skeleton leaves `beta` out entirely; `beta` is
    /// the lib + bin group.
    #[test]
    fn rebuilding_one_crate_into_a_full_snapshot_restores_it() {
        let workspace = tempfile::tempdir().unwrap();
        write_fixture_with_bin(workspace.path());
        let full_model = extract(&loader::load(workspace.path()).unwrap());
        let (data_full, data_work) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let full =
            persist_test_model(data_full.path(), &full_model, GraphEnvOptions::default()).unwrap();
        let work =
            persist_test_model(data_work.path(), &full_model, GraphEnvOptions::default()).unwrap();

        for name in ["alpha", "beta"] {
            // A fresh load, so the build primes and walks this crate alone.
            let mut loaded = loader::load_unprimed(workspace.path()).unwrap();
            let dirty = group_crates(&loaded, name);
            let expected_names = group_names(&loaded, &dirty);

            let mut wtxn = work.write_txn().unwrap();
            for key in &expected_names {
                let group_id = crate_node_id(&full_model.workspace_hash, key);
                assert!(delete_crate_rows(&mut wtxn, &work.dbs, group_id).unwrap() > 0);
            }
            wtxn.commit().unwrap();

            let mut groups = Vec::new();
            extract_by_crate(
                &mut loaded,
                &dirty,
                UsagesPass::from_env(),
                None,
                &mut |model, group| {
                    groups.push(group.name.clone());
                    let mut wtxn = work.write_txn()?;
                    write_owned_rows(&mut wtxn, &work.dbs, model, &|c| c == group.id)?;
                    wtxn.commit()?;
                    Ok(())
                },
            )
            .unwrap();
            assert_eq!(groups, expected_names);
            assert_same(
                &full,
                &work,
                &format!("full (left) vs `{name}` rebuilt (right)"),
            );
        }
    }

    #[test]
    fn lib_and_bin_can_be_rebuilt_independently() {
        let workspace = tempfile::tempdir().unwrap();
        write_fixture_with_bin(workspace.path());
        let mut loaded = loader::load_unprimed(workspace.path()).unwrap();
        let half = vec![group_crates(&loaded, "beta")[0]];
        let groups = crate_groups(&loaded, &group_crates(&loaded, "beta")).unwrap();
        assert_eq!(groups.len(), 2);
        assert_ne!(groups[0].id, groups[1].id);
        assert!(groups.iter().all(|g| g.crates.len() == 1));
        let mut written = Vec::new();
        extract_by_crate(
            &mut loaded,
            &half,
            UsagesPass::from_env(),
            None,
            &mut |_, group| { written.push(group.id); Ok(()) },
        )
        .unwrap();
        assert_eq!(written.len(), 1);
    }

    /// Acceptance on a real workspace, `RMC_SNAPSHOT_COMPARE_WORKSPACE=<path>`:
    /// three snapshots — the whole-workspace build with usages from targets,
    /// the same from consumers (plan step `consumer-centric-usages`), both from
    /// one primed load, and the per-crate build — compared in turn. Each model
    /// is written to disk and dropped before the next is built.
    ///
    /// The per-crate build gets a fresh unprimed load of its own, as in the
    /// daemon, and the first load is dropped before it. Built in the database
    /// the whole builds had used, it lost the bodies of `impl Trait` fns on
    /// a private workspace — a rust-analyzer bug, see
    /// [`evicted_inference_of_an_impl_trait_fn_comes_back_whole`].
    #[test]
    #[ignore = "needs RMC_SNAPSHOT_COMPARE_WORKSPACE and three extractions of it"]
    fn by_crate_build_equals_whole_build_on_a_real_workspace() {
        let workspace = std::env::var("RMC_SNAPSHOT_COMPARE_WORKSPACE")
            .expect("set RMC_SNAPSHOT_COMPARE_WORKSPACE to a workspace root");
        let loaded = loader::load(Path::new(&workspace)).unwrap();
        let env = GraphEnvOptions::default();
        let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();

        let t = std::time::Instant::now();
        let from_targets = persist_test_model(
            dirs[0].path(),
            &extract_with(&loaded, UsagesPass::FromTargets),
            env,
        )
        .unwrap();
        eprintln!("whole, usages from targets:   {:.2?}", t.elapsed());
        let t = std::time::Instant::now();
        let from_consumers = persist_test_model(
            dirs[1].path(),
            &extract_with(&loaded, UsagesPass::FromConsumers),
            env,
        )
        .unwrap();
        eprintln!("whole, usages from consumers: {:.2?}", t.elapsed());
        drop(loaded);
        let t = std::time::Instant::now();
        let mut loaded = loader::load_unprimed(Path::new(&workspace)).unwrap();
        let by_crate = persist_test_by_crate(dirs[2].path(), &mut loaded, env).unwrap();
        eprintln!("by crate:                     {:.2?}", t.elapsed());

        assert_same(
            &from_targets,
            &from_consumers,
            &format!("usages from targets (left) vs from consumers (right) on {workspace}"),
        );
        assert_same(
            &from_consumers,
            &by_crate,
            &format!("whole build (left) vs by crate (right) on {workspace}"),
        );
    }

    #[test]
    fn extracts_workspace_crate_modules_for_self() {
        let model = shared_model();

        assert!(model.nodes.contains_key(&model.workspace_id));

        let crate_node = model
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Crate && n.qualified_name == "rmc_graph")
            .expect("rmc_graph crate node");

        let root_module = model
            .nodes
            .values()
            .find(|n| {
                n.kind == NodeKind::Module
                    && n.qualified_name == "rmc_graph"
                    && n.parent_id == Some(crate_node.id)
            })
            .expect("root module under crate");

        let graph_module = model
            .nodes
            .values()
            .find(|n| n.kind == NodeKind::Module && n.qualified_name == "rmc_graph::graph")
            .expect("graph module");
        assert_eq!(graph_module.parent_id, Some(root_module.id));

        assert!(model
            .contains
            .iter()
            .any(|&(p, c)| p == model.workspace_id && c == crate_node.id));
        assert!(model
            .contains
            .iter()
            .any(|&(p, c)| p == crate_node.id && c == root_module.id));
    }

    #[test]
    fn extracts_impl_items_for_self() {
        use crate::graph::model::ItemKind;

        let model = shared_model();

        // The host type — `OpenedSnapshot` lives in `graph::snapshot`.
        let host = model
            .nodes
            .values()
            .find(|n| {
                n.kind == NodeKind::Item
                    && matches!(n.item_kind, Some(ItemKind::Struct))
                    && n.qualified_name == "rmc_graph::graph::snapshot::OpenedSnapshot"
            })
            .expect("OpenedSnapshot struct Item node");

        // Layer 4 should have emitted a Method Item for `usages_of` whose
        // parent is the host struct's Item NodeId. `usages_of` is declared in
        // the inherent `impl OpenedSnapshot { ... }` block in queries.rs.
        let method = model
            .nodes
            .values()
            .find(|n| {
                n.kind == NodeKind::Item
                    && matches!(n.item_kind, Some(ItemKind::Method))
                    && n.display_name == "usages_of"
                    && n.parent_id == Some(host.id)
            })
            .expect(
                "expected Method Item node for OpenedSnapshot::usages_of with \
                 parent_id pointing at the host struct's Item",
            );
        assert_eq!(
            method.qualified_name,
            "rmc_graph::graph::snapshot::OpenedSnapshot::usages_of"
        );
        // Layer 4 backfills file/span via try_to_nav.
        assert!(method.file.is_some(), "method Item should have a file path");
        assert!(method.span.is_some(), "method Item should have a span");
    }

    #[test]
    fn extracts_items_and_bindings_for_self() {
        let model = shared_model();

        // Item: the `load` function we defined in src/graph/loader.rs.
        let load_fn = model.nodes.values().find(|n| {
            n.kind == NodeKind::Item
                && n.qualified_name == "rmc_graph::graph::loader::load"
        });
        assert!(load_fn.is_some(), "expected graph::loader::load Item node");

        // At least one declared binding.
        assert!(model.bindings.iter().any(|b| b.kind == BindingKind::Declared));

        // At least one re-export (`pub use loader::{LoadedWorkspace, load};` in graph/mod.rs).
        let load_fn_id = load_fn.unwrap().id;
        let reexport_of_load = model.bindings.iter().find(|b| {
            b.kind == BindingKind::NamedImport
                && b.visible_name == "load"
                && b.target == load_fn_id
        });
        assert!(
            reexport_of_load.is_some(),
            "expected NamedImport binding for `pub use loader::load` in graph/mod.rs"
        );
    }
}

//! Which crates a build has to rebuild (plan step `package-fingerprint`).
//!
//! The unit of invalidation is the cargo package ([`BuildInputs`]). A build
//! starts from the snapshot `CURRENT` names (the *base*) and decides:
//!
//! - **full** when there is no usable base, or when the global inputs changed
//!   — the root `Cargo.lock`, the toolchain or cargo config, any `Cargo.toml`
//!   (and with it the set of packages) — or when the set of crates changed
//!   without a manifest edit (a `src/main.rs` or `src/bin/*.rs` appearing);
//! - **incremental** otherwise: a crate is *directly dirty* when one of the
//!   packages its module files lie in changed (`GraphManifest.crate_packages`),
//!   and the rebuild takes every directly dirty crate plus ALL its transitive
//!   dependents inside the workspace. Conservative on purpose: method ids carry
//!   byte offsets and re-exports pass names through, so a dependent's rows can
//!   change when the producer's surface did not (narrowing that is
//!   `surface-hash-narrowing`, outside the plan).
//!
//! Every row a crate owns is computed from its own files and the crates it
//! depends on (`ownership.rs`, `extract::extract_by_crate`), so a crate that
//! is neither dirty nor a dependent of one has exactly the rows it had in the
//! base, and the copy keeps them.
//!
//! Not tracked: files a crate reads through `include!` and friends (only
//! module files are recorded), and the toolchain changing without a
//! `rust-toolchain*` edit — both were outside the old whole-tree fingerprint
//! too.

use std::collections::{BTreeSet, HashMap, HashSet};

use anyhow::Result;
use ra_ap_hir::Crate;
use ra_ap_hir_def::nameres::crate_def_map;

use super::audit_util::resolve_workspace_relative;
use super::extract::{CrateGroup, crate_groups};
use super::ids::NodeId;
use super::loader::LoadedWorkspace;
use super::snapshot::OpenedSnapshot;
use super::storage::BuildInputs;

/// What a build will do, decided after loading the workspace.
pub(crate) enum BuildPlan {
    /// Build every crate into an empty snapshot.
    Full { reason: String },
    /// Copy `base`, then delete and rebuild the crates of `dirty`.
    Incremental {
        base: OpenedSnapshot,
        /// Whole groups, dependencies before dependents.
        dirty: Vec<CrateGroup>,
        /// The packages whose hash changed since `base`.
        changed_packages: BTreeSet<String>,
    },
}

/// Decide between a full and an incremental build of `loaded` (whose tree
/// hashes to `inputs`) on top of `base`, the published snapshot if any.
pub(crate) fn plan_build(
    base: Option<OpenedSnapshot>,
    inputs: &BuildInputs,
    loaded: &LoadedWorkspace,
) -> Result<BuildPlan> {
    let full = |reason: &str| {
        Ok(BuildPlan::Full {
            reason: reason.to_string(),
        })
    };
    let Some(base) = base else {
        return full("no published snapshot");
    };
    let Some(base_inputs) = base.manifest.inputs.clone() else {
        return full("the published snapshot records no build inputs");
    };
    if base_inputs.global != inputs.global {
        return full("Cargo.lock, a Cargo.toml, the toolchain or the cargo config changed");
    }
    let groups = crate_groups(loaded, &loaded.local_crates)?;
    let names: BTreeSet<&str> = groups.iter().map(|g| g.name.as_str()).collect();
    let base_names: BTreeSet<&str> = base
        .manifest
        .crate_packages
        .keys()
        .map(String::as_str)
        .collect();
    if names != base_names {
        return full("the set of crates changed");
    }

    let changed_packages = inputs.changed_packages(&base_inputs);
    let direct: HashSet<NodeId> = groups
        .iter()
        .filter(|g| {
            base.manifest.crate_packages[&g.name]
                .iter()
                .any(|p| changed_packages.contains(p))
        })
        .map(|g| g.id)
        .collect();
    let dirty_ids = with_dependents(loaded, &groups, direct);
    let dirty = groups
        .into_iter()
        .filter(|g| dirty_ids.contains(&g.id))
        .collect();
    Ok(BuildPlan::Incremental {
        base,
        dirty,
        changed_packages,
    })
}

/// `seeds` and every group that depends on one of them, transitively, among
/// `groups` (all groups of the workspace).
fn with_dependents(
    loaded: &LoadedWorkspace,
    groups: &[CrateGroup],
    seeds: HashSet<NodeId>,
) -> HashSet<NodeId> {
    let group_of: HashMap<Crate, NodeId> = groups
        .iter()
        .flat_map(|g| g.crates.iter().map(move |&k| (k, g.id)))
        .collect();
    // dependency group → the groups depending on it.
    let mut dependents: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for group in groups {
        for krate in &group.crates {
            for dep in krate.dependencies(&loaded.db) {
                if let Some(&dep_group) = group_of.get(&dep.krate) {
                    if dep_group != group.id {
                        dependents.entry(dep_group).or_default().push(group.id);
                    }
                }
            }
        }
    }
    let mut reached = HashSet::new();
    let mut stack: Vec<NodeId> = seeds.into_iter().collect();
    while let Some(id) = stack.pop() {
        if reached.insert(id) {
            stack.extend(dependents.get(&id).into_iter().flatten().copied());
        }
    }
    reached
}

/// The packages the module files of `group` lie in — what
/// `GraphManifest.crate_packages` records for it. Reads the group's DefMaps,
/// which the build has just primed.
pub(crate) fn crate_packages(
    loaded: &LoadedWorkspace,
    group: &CrateGroup,
    inputs: &BuildInputs,
) -> BTreeSet<String> {
    let db = &loaded.db;
    let mut out = BTreeSet::new();
    for krate in &group.crates {
        let def_map = crate_def_map(db, krate.base());
        for (module_id, _) in def_map.modules() {
            if module_id.is_block_module(db) {
                continue;
            }
            let file_id = def_map[module_id]
                .definition_source_file_id()
                .original_file(db)
                .file_id(db);
            // A module file outside the tree is not fingerprinted at all.
            if let Some(rel) =
                resolve_workspace_relative(&loaded.vfs, file_id, &loaded.workspace_root)
            {
                out.insert(inputs.package_of(&rel.replace('\\', "/")).to_string());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::graph::model::{EmbeddingRecord, NodeKind};
    use crate::graph::snapshot::{
        BuildKind, BuildOptions, BuildResult, build_and_persist, open_current,
    };
    use crate::graph::snapshot_compare::compare_snapshots;
    use crate::graph::storage::{GraphEnvOptions, GraphPaths};
    use crate::graph::test_support::write_two_crate_fixture;

    /// The two-crate fixture plus `gamma`, which depends on both: `alpha` is
    /// the bottom, `beta` the middle, `gamma` the leaf.
    fn write_three_crate_fixture(root: &Path) {
        write_two_crate_fixture(root);
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"alpha\", \"beta\", \"gamma\"]\nresolver = \"2\"\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("gamma/src")).unwrap();
        fs::write(
            root.join("gamma/Cargo.toml"),
            "[package]\nname = \"gamma\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nalpha = { path = \"../alpha\" }\nbeta = { path = \"../beta\" }\n",
        )
        .unwrap();
        fs::write(
            root.join("gamma/src/lib.rs"),
            "pub fn sorted_pair(a: i32, b: i32) -> (i32, i32) {\n    beta::ordered(a, b)\n}\n\n\
             pub fn twice_doubled(x: i32) -> i32 {\n    alpha::double(beta::twice(x))\n}\n",
        )
        .unwrap();
    }

    fn edit(root: &Path, file: &str, from: &str, to: &str) {
        let path = root.join(file);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(from), "`{from}` not in {file}");
        fs::write(&path, text.replacen(from, to, 1)).unwrap();
    }

    fn build(workspace: &Path, data: &Path, force_rebuild: bool) -> BuildResult {
        build_and_persist(
            workspace,
            BuildOptions {
                force_rebuild,
                data_dir_override: Some(data.to_path_buf()),
                ..Default::default()
            },
        )
        .unwrap()
    }

    fn open(data: &Path, result: &BuildResult) -> OpenedSnapshot {
        open_current(
            &GraphPaths::for_workspace_in(data, &result.workspace_root),
            GraphEnvOptions::default(),
        )
        .unwrap()
        .unwrap()
    }

    /// Build the edited tree incrementally into `data` and from scratch into a
    /// fresh dir: the snapshots must be equal row for row, and so must the
    /// manifests' inputs and crate → packages records. Returns what the
    /// incremental build did.
    fn assert_incremental_equals_full(workspace: &Path, data: &Path, what: &str) -> BuildKind {
        let t = std::time::Instant::now();
        let incremental = build(workspace, data, false);
        let incremental_time = t.elapsed();
        let full_data = tempfile::tempdir().unwrap();
        let t = std::time::Instant::now();
        let full = build(workspace, full_data.path(), true);
        eprintln!(
            "{what}: incremental {incremental_time:.2?}, full {:.2?}",
            t.elapsed()
        );
        assert_eq!(
            incremental.graph_id, full.graph_id,
            "{what}: same tree, same id"
        );
        let (a, b) = (open(data, &incremental), open(full_data.path(), &full));
        let diff = compare_snapshots(&a, &b, 20).unwrap();
        assert!(
            diff.is_empty(),
            "{what}: incremental (left) vs full (right):\n{diff}"
        );
        assert_eq!(a.manifest.inputs, b.manifest.inputs, "{what}");
        assert_eq!(
            a.manifest.crate_packages, b.manifest.crate_packages,
            "{what}"
        );
        assert_eq!(
            (
                a.manifest.node_count,
                a.manifest.binding_count,
                a.manifest.usage_count
            ),
            (
                b.manifest.node_count,
                b.manifest.binding_count,
                b.manifest.usage_count
            ),
            "{what}: counts"
        );
        incremental.kind
    }

    fn incremental(changed: &[&str], rebuilt: &[&str]) -> BuildKind {
        BuildKind::Incremental {
            changed_packages: changed.iter().map(|s| s.to_string()).collect(),
            rebuilt_crates: rebuilt.iter().map(|s| match *s {
                "gamma-bin" => "gamma[bin]@gamma/src/main.rs".to_owned(),
                "tool" => "tool[bin]@gamma/src/bin/tool.rs".to_owned(),
                name => format!("{name}[lib]@{name}/src/lib.rs"),
            }).collect(),
        }
    }

    /// The plan's three edit scenarios on the fixture, one after another on
    /// the same snapshot history, each judged against a full build of the
    /// same tree; plus an edit outside every crate and the embedding cache.
    #[test]
    fn incremental_builds_equal_full_builds_of_the_same_tree() {
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        write_three_crate_fixture(ws);
        let data = tempfile::tempdir().unwrap();

        let first = build(ws, data.path(), false);
        assert_eq!(
            first.kind,
            BuildKind::Full {
                reason: "no published snapshot".into()
            }
        );
        {
            let snap = open(data.path(), &first);
            let packages = &snap.manifest.crate_packages;
            assert_eq!(packages.len(), 3, "{packages:?}");
            assert!(packages["beta[lib]@beta/src/lib.rs"].iter().eq(["beta"]), "{packages:?}");
        }

        // An embedding cached for an `alpha` item and one for a `gamma` item.
        let (alpha_item, gamma_item) = {
            let snap = open(data.path(), &first);
            let rtxn = snap.read_txn().unwrap();
            let item_named = |name: &str| {
                snap.dbs
                    .nodes_by_id
                    .iter(&rtxn)
                    .unwrap()
                    .map(|e| e.unwrap().1)
                    .find(|n| n.kind == NodeKind::Item && n.qualified_name == name)
                    .unwrap_or_else(|| panic!("no item `{name}`"))
                    .id
            };
            let ids = (
                item_named("alpha::double"),
                item_named("gamma::sorted_pair"),
            );
            drop(rtxn);
            let record = EmbeddingRecord {
                content_hash: [0; 16],
                vector: vec![1.0],
                embedder_version: "test".into(),
                generated_at_unix: 0,
            };
            let mut wtxn = snap.write_txn().unwrap();
            for id in [ids.0, ids.1] {
                snap.dbs
                    .embeddings_by_target
                    .put(&mut wtxn, id.as_bytes(), &record)
                    .unwrap();
            }
            wtxn.commit().unwrap();
            ids
        };

        // 1. A function body in the leaf package: only the leaf is rebuilt.
        edit(
            ws,
            "gamma/src/lib.rs",
            "    beta::ordered(a, b)\n",
            "    let _ = a;\n    beta::ordered(a, b)\n",
        );
        let kind = assert_incremental_equals_full(ws, data.path(), "leaf body edit");
        assert_eq!(kind, incremental(&["gamma"], &["gamma"]));
        {
            let snap = open(data.path(), &build(ws, data.path(), false));
            let rtxn = snap.read_txn().unwrap();
            let cached = |id: NodeId| {
                snap.dbs
                    .embeddings_by_target
                    .get(&rtxn, id.as_bytes())
                    .unwrap()
                    .is_some()
            };
            assert!(
                cached(alpha_item),
                "a clean crate's embedding survives the rebuild"
            );
            assert!(
                !cached(gamma_item),
                "a rebuilt crate's embedding goes with its rows"
            );
        }

        // 2. A pub fn renamed in the middle package, its caller in the leaf
        // left as it was: the leaf's hash is unchanged, but it depends on the
        // middle, so it is rebuilt too (its usage of `ordered` no longer
        // resolves). The bottom package stays.
        edit(
            ws,
            "beta/src/lib.rs",
            "pub fn ordered(",
            "pub fn ordered_pair(",
        );
        let kind = assert_incremental_equals_full(ws, data.path(), "middle pub fn rename");
        assert_eq!(kind, incremental(&["beta"], &["beta", "gamma"]));

        // 3. A `.rs` file outside every crate: a new snapshot, nothing rebuilt.
        fs::create_dir_all(ws.join("scripts")).unwrap();
        fs::write(ws.join("scripts/probe.rs"), "fn main() {}\n").unwrap();
        let kind = assert_incremental_equals_full(ws, data.path(), "file outside every crate");
        assert_eq!(kind, incremental(&[""], &[]));

        // 4. A `Cargo.toml` edit: a full build.
        edit(
            ws,
            "gamma/Cargo.toml",
            "version = \"0.1.0\"",
            "version = \"0.1.1\"",
        );
        let kind = assert_incremental_equals_full(ws, data.path(), "Cargo.toml edit");
        assert!(matches!(kind, BuildKind::Full { .. }), "{kind:?}");

        // 5. The bottom package: everything depends on it.
        edit(
            ws,
            "alpha/src/lib.rs",
            "pub fn double(",
            "pub fn double_it(",
        );
        let kind = assert_incremental_equals_full(ws, data.path(), "bottom pub fn rename");
        assert_eq!(kind, incremental(&["alpha"], &["alpha", "beta", "gamma"]));
    }

    /// Bin targets appearing without any `Cargo.toml` edit (cargo finds
    /// `src/main.rs` and `src/bin/*.rs` itself). `src/main.rs` is a bin named
    /// like the lib — the same crate group, which its package's change
    /// rebuilds whole; `src/bin/tool.rs` is a new crate name, so the set of
    /// crates changed and the build is full.
    #[test]
    fn auto_discovered_bin_targets() {
        let workspace = tempfile::tempdir().unwrap();
        let ws = workspace.path();
        write_three_crate_fixture(ws);
        let data = tempfile::tempdir().unwrap();
        build(ws, data.path(), false);

        fs::write(
            ws.join("gamma/src/main.rs"),
            "fn main() {\n    gamma::sorted_pair(1, 2);\n}\n",
        )
        .unwrap();
        let kind = assert_incremental_equals_full(ws, data.path(), "bin named like the lib");
        assert_eq!(kind, BuildKind::Full { reason: "the set of crates changed".into() });

        fs::create_dir_all(ws.join("gamma/src/bin")).unwrap();
        fs::write(
            ws.join("gamma/src/bin/tool.rs"),
            "fn main() {\n    gamma::twice_doubled(3);\n}\n",
        )
        .unwrap();
        let kind = assert_incremental_equals_full(ws, data.path(), "bin with a new name");
        assert_eq!(
            kind,
            BuildKind::Full {
                reason: "the set of crates changed".into()
            }
        );

        edit(ws, "gamma/src/main.rs", "(1, 2)", "(2, 1)");
        let kind = assert_incremental_equals_full(ws, data.path(), "bin body edit");
        assert_eq!(kind, incremental(&["gamma"], &["gamma", "gamma-bin", "tool"]));
    }

    /// Edits listed in `RMC_INCREMENTAL_EDITS` applied one after another to
    /// `RMC_INCREMENTAL_WORKSPACE` — a tree this test may modify, a scratch copy
    /// rather than a working checkout; every file is restored at the end, on
    /// failure too. Each edit is `file|from|to` (workspace-relative file, first
    /// occurrence of `from` replaced), edits separated by `;;`. After each
    /// one: incremental build ≡ full build of the same tree.
    #[test]
    #[ignore = "needs RMC_INCREMENTAL_WORKSPACE, RMC_INCREMENTAL_EDITS and two builds per edit"]
    fn incremental_builds_equal_full_builds_on_a_real_workspace() {
        let workspace = std::env::var("RMC_INCREMENTAL_WORKSPACE")
            .expect("set RMC_INCREMENTAL_WORKSPACE to a workspace root the test may edit");
        let edits = std::env::var("RMC_INCREMENTAL_EDITS")
            .expect("set RMC_INCREMENTAL_EDITS to `file|from|to;;file|from|to…`");
        let ws = Path::new(&workspace);

        struct Restore(Vec<(std::path::PathBuf, String)>);
        impl Drop for Restore {
            fn drop(&mut self) {
                for (path, text) in self.0.iter().rev() {
                    fs::write(path, text).unwrap();
                }
            }
        }
        let mut restore = Restore(Vec::new());

        let data = tempfile::tempdir().unwrap();
        let t = std::time::Instant::now();
        let base = build(ws, data.path(), true);
        eprintln!("base full build: {:.2?} ({:?})", t.elapsed(), base.kind);
        for spec in edits.split(";;").filter(|s| !s.trim().is_empty()) {
            let [file, from, to] = spec.splitn(3, '|').collect::<Vec<_>>()[..] else {
                panic!("edit `{spec}` is not `file|from|to`");
            };
            let path = ws.join(file);
            restore
                .0
                .push((path.clone(), fs::read_to_string(&path).unwrap()));
            edit(ws, file, from, to);
            let t = std::time::Instant::now();
            let kind = assert_incremental_equals_full(ws, data.path(), spec);
            eprintln!(
                "edit `{spec}`: incremental + full + compare {:.2?}: {kind:?}",
                t.elapsed()
            );
        }
    }
}

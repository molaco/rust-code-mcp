//! Workspace loader for the hypergraph layer.
//!
//! Loads a Cargo workspace through rust-analyzer and returns the
//! `RootDatabase`, `Vfs`, and the filtered set of *local* crates — the crates
//! living in the tree we were pointed at, decided by crate-root path rather
//! than by `CrateOrigin::is_local` (see `filter_local_crates` for why that flag
//! is not the set it sounds like).
//!
//! ### Cross-crate resolution
//!
//! `no_deps: false` + `sysroot: Some(Discover)` give RA the full cargo
//! resolve graph (workspace-internal dep edges + sysroot crates). With those
//! edges, `use burn_tensor::Tensor` in `burn_core` resolves to the canonical
//! `StructId` and our binding pass picks it up via burn_core's `ItemScope`,
//! enabling cross-crate `who_imports`. RA ≥ 0.0.328 uses
//! `CARGO_RESOLVER_LOCKFILE_PATH` env var instead of `--lockfile-path` to
//! avoid mutating Cargo.lock (older versions used the flag, which broke on
//! cargo versions where metadata didn't accept it).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cargo_metadata::{MetadataCommand, TargetKind};
use ra_ap_hir::Crate;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::{CargoConfig, CargoFeatures, RustLibSource};
use ra_ap_vfs::Vfs;

/// Cargo targets of this workspace's members, as reported by
/// `cargo metadata --no-deps`.
struct WorkspaceTargets {
    /// Target kind by normalized crate name.
    by_name: HashMap<String, String>,
    /// Target kind by crate-root path relative to the requested directory.
    by_root_file: HashMap<String, String>,
    /// Target kind by *absolute* (canonicalized) crate-root path.
    ///
    /// This is the dependable "is it ours" key. The two maps above are relative
    /// to the directory the caller asked us to load, and that directory is
    /// frequently a member's subdirectory rather than the workspace root — in
    /// which case they are nearly empty and match nothing.
    by_abs_root_file: HashMap<PathBuf, String>,
    /// The workspace root Cargo itself reports, canonicalized. `None` when
    /// `cargo metadata` did not run.
    ///
    /// Vendored patch crates (`[patch.crates-io] foo = { path = "patches/foo" }`)
    /// are not members, yet they are part of the tree we were pointed at and have
    /// always been indexed. This is what keeps them in while path dependencies
    /// pointing *outside* the tree stay out.
    workspace_root: Option<PathBuf>,
}

pub struct LoadedWorkspace {
    pub workspace_root: PathBuf,
    pub db: RootDatabase,
    pub vfs: Vfs,
    pub local_crates: Vec<Crate>,
    pub crate_target_kinds_by_name: HashMap<String, String>,
    pub crate_target_kinds_by_root_file: HashMap<String, String>,
}

/// Load `directory` with every crate's caches primed in parallel — what every
/// one-shot analysis (audits, skeleton, tests) wants.
pub fn load(directory: &Path) -> Result<LoadedWorkspace> {
    load_with(directory, true)
}

/// Load `directory` without priming anything: the caller primes the crates it
/// is about to walk with [`prime_for`]. The per-crate snapshot build does
/// this, so a build of a few crates does not pay for the whole crate graph.
pub(crate) fn load_unprimed(directory: &Path) -> Result<LoadedWorkspace> {
    load_with(directory, false)
}

/// Prime, in parallel, the caches of `crates` and of everything they depend
/// on, transitively — DefMaps first, as `prefill_caches` does for the whole
/// graph. Without it the extraction walk builds DefMaps lazily, one at a time
/// (measured ~30× slower on burn, see [`load`]).
///
/// The scope is closed under dependencies because the scheduler only waits
/// for in-scope dependencies (`parallel_prime_caches`). For a full build it
/// is every crate a local lib/bin target reaches — less than `prefill_caches`
/// primes, which also takes the tests, benches, examples and build scripts
/// the graph never walks.
pub(crate) fn prime_for(loaded: &LoadedWorkspace, crates: &[Crate]) {
    let db = &loaded.db;
    let mut scope: Vec<Crate> = Vec::new();
    let mut seen: HashSet<Crate> = HashSet::new();
    let mut stack: Vec<Crate> = crates.to_vec();
    while let Some(krate) = stack.pop() {
        if !seen.insert(krate) {
            continue;
        }
        scope.push(krate);
        stack.extend(krate.dependencies(db).into_iter().map(|dep| dep.krate));
    }
    let scope: Vec<_> = scope.into_iter().map(Crate::base).collect();
    ra_ap_ide_db::prime_caches::parallel_prime_caches(
        db,
        &scope,
        num_cpus::get_physical(),
        &|_| (),
    );
}

fn load_with(directory: &Path, prefill_caches: bool) -> Result<LoadedWorkspace> {
    let canonical = directory
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", directory.display()))?;
    let workspace_root = canonical.clone();
    let targets = load_crate_target_kinds(&workspace_root);

    let cargo_config = CargoConfig {
        sysroot: Some(RustLibSource::Discover),
        no_deps: false,
        features: CargoFeatures::All,
        all_targets: false,
        set_test: false,
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        // Build every workspace crate's DefMap in parallel during load. Without
        // this, DefMaps are constructed lazily on first access during the
        // serial extraction walk — measured ~30× slower on burn. When it is
        // off, the caller primes its own scope (`prime_for`).
        prefill_caches,
        num_worker_threads: num_cpus::get_physical(),
        proc_macro_processes: 1,
    };

    let (db, vfs, _proc_macro) =
        load_workspace_at(&canonical, &cargo_config, &load_config, &|_| {})
            .with_context(|| format!("failed to load workspace at {}", canonical.display()))?;

    let local_crates = filter_local_crates(&db, &vfs, &targets, &workspace_root);

    Ok(LoadedWorkspace {
        workspace_root,
        db,
        vfs,
        local_crates,
        crate_target_kinds_by_name: targets.by_name,
        crate_target_kinds_by_root_file: targets.by_root_file,
    })
}

/// Keep only crates under the requested directory that are backed by
/// normal library/binary Cargo targets.
///
/// ### Why `CrateOrigin::is_local` is not enough
///
/// It reads like "workspace member", but rust-analyzer derives it from
/// `is_local = source.is_none()` (`project-model/src/cargo_workspace.rs`), i.e.
/// *any package that lives on the local filesystem rather than a registry*.
/// Every path dependency pointing outside the workspace passes it. While all
/// our external deps came from crates.io the distinction never showed; the
/// moment rust-code-mcp switched its `ra_ap_*` deps to path dependencies on our
/// rust-analyzer fork, 32 rust-analyzer crates plus salsa became "local" and the
/// hypergraph silently started indexing that tree too — minutes of extra work
/// and a stack overflow in the extraction walk.
///
/// So a crate is ours when its root file is either a Cargo target of a
/// workspace member, or at least lives somewhere under the workspace root.
/// The second half is deliberate: vendored patch crates (`[patch.crates-io]`
/// pointing at `patches/…`) are not members but are part of the tree we were
/// asked to load, and dropping them would silently shrink what the graph can
/// answer about a repo that vendors its dependencies.
///
/// The target-kind filter applies to members only, keeping production targets.
/// rust-analyzer creates crate graph entries for workspace integration tests,
/// benches, examples, and build scripts too. Those targets can be expensive and
/// can trigger HIR/body-inference bugs in large workspaces even though the
/// persisted hypergraph's architectural queries only need production targets.
///
/// Cargo membership does not widen a member-directory request to its siblings:
/// those sources are outside the declared fingerprint slice. No Cargo workspace
/// root means the target-kind policy falls back to unknown/default handling.
fn filter_local_crates(db: &RootDatabase, vfs: &Vfs, targets: &WorkspaceTargets, requested_root: &Path) -> Vec<Crate> {
    Crate::all(db)
        .into_iter()
        .filter(|krate| krate.origin(db).is_local())
        .filter(|krate| {
            match crate_root_abs_path(db, vfs, *krate) {
                Some(root_file) => root_file.starts_with(requested_root) && should_index_crate_root(&root_file, targets),
                // No resolvable root file: keep it only when we have no
                // workspace boundary to judge against anyway.
                None => targets.workspace_root.is_none(),
            }
        })
        .collect()
}

/// The membership decision itself, separated from rust-analyzer so it can be
/// exercised directly. See [`filter_local_crates`] for the reasoning.
fn should_index_crate_root(root_file: &Path, targets: &WorkspaceTargets) -> bool {
    let Some(workspace_root) = targets.workspace_root.as_ref() else {
        return true;
    };
    match targets.by_abs_root_file.get(root_file) {
        Some(kind) => should_index_target_kind(kind),
        None => root_file.starts_with(workspace_root),
    }
}

/// Absolute, canonicalized path of the crate's root file (`lib.rs`/`main.rs`).
///
/// Read from the crate graph, not from the crate's DefMap: this runs for every
/// local-origin crate before anything is primed, and a DefMap asked for here
/// would be built serially — the whole graph's, tests and build scripts
/// included.
fn crate_root_abs_path(db: &RootDatabase, vfs: &Vfs, krate: Crate) -> Option<PathBuf> {
    let root_file_id = krate.root_file(db);
    let abs: PathBuf = vfs.file_path(root_file_id).as_path()?.to_path_buf().into();
    Some(canonical_or_self(&abs))
}

/// Canonicalize for comparison, falling back to the path itself when the file
/// cannot be resolved (deleted, or a VFS-only overlay).
fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn should_index_target_kind(kind: &str) -> bool {
    matches!(kind, "lib" | "bin")
}

fn load_crate_target_kinds(workspace_root: &Path) -> WorkspaceTargets {
    let manifest_path = workspace_root.join("Cargo.toml");
    let mut command = MetadataCommand::new();
    // `current_dir` is not cosmetic: rustup picks the toolchain by the working
    // directory (not by `--manifest-path`), and cargo walks up from it looking
    // for `.cargo/config.toml`. Without this the daemon would analyse every
    // workspace under the toolchain and registries of whatever tree it was
    // started in. rust-analyzer sets it on every subprocess it spawns; this was
    // the one place in the tree that did not.
    command
        .current_dir(workspace_root)
        .manifest_path(manifest_path)
        .no_deps();
    let metadata = match command.exec() {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(
                "failed to load cargo metadata for {}; crate target-kind filters will fall back to unknown/default handling: {}",
                workspace_root.display(),
                error
            );
            return WorkspaceTargets {
                by_name: HashMap::new(),
                by_root_file: HashMap::new(),
                by_abs_root_file: HashMap::new(),
                workspace_root: None,
            };
        }
    };

    let workspace_members: HashSet<_> = metadata.workspace_members.iter().cloned().collect();
    let mut by_name = HashMap::new();
    let mut by_root_file = HashMap::new();
    let mut by_abs_root_file = HashMap::new();

    for package in metadata
        .packages
        .iter()
        .filter(|package| workspace_members.contains(&package.id))
    {
        for target in &package.targets {
            let kind = target_kind_label(&target.kind).to_string();
            insert_preferred_target_kind(
                &mut by_name,
                normalize_crate_name(&target.name),
                kind.clone(),
            );
            insert_preferred_target_kind(
                &mut by_abs_root_file,
                canonical_or_self(target.src_path.as_std_path()),
                kind.clone(),
            );
            if let Some(root_file) =
                workspace_relative_path(target.src_path.as_std_path(), workspace_root)
            {
                insert_preferred_target_kind(&mut by_root_file, root_file, kind);
            }
        }
    }

    WorkspaceTargets {
        by_name,
        by_root_file,
        by_abs_root_file,
        workspace_root: Some(canonical_or_self(metadata.workspace_root.as_std_path())),
    }
}

fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn workspace_relative_path(path: &Path, workspace_root: &Path) -> Option<String> {
    path.strip_prefix(workspace_root)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

fn insert_preferred_target_kind<K: std::hash::Hash + Eq>(
    map: &mut HashMap<K, String>,
    key: K,
    kind: String,
) {
    match map.get(&key) {
        Some(current) if target_kind_rank(current) <= target_kind_rank(&kind) => {}
        _ => {
            map.insert(key, kind);
        }
    }
}

fn target_kind_label(kinds: &[TargetKind]) -> &'static str {
    kinds
        .iter()
        .map(canonical_target_kind)
        .min_by_key(|kind| target_kind_rank(kind))
        .unwrap_or("unknown")
}

fn canonical_target_kind(kind: &TargetKind) -> &'static str {
    match kind {
        TargetKind::Bench => "bench",
        TargetKind::Bin => "bin",
        TargetKind::CustomBuild => "build",
        TargetKind::Example => "example",
        TargetKind::Test => "test",
        TargetKind::Lib
        | TargetKind::RLib
        | TargetKind::DyLib
        | TargetKind::CDyLib
        | TargetKind::StaticLib
        | TargetKind::ProcMacro => "lib",
        TargetKind::Unknown(_) => "unknown",
        _ => "unknown",
    }
}

fn target_kind_rank(kind: &str) -> u8 {
    match kind {
        "lib" => 0,
        "bin" => 1,
        "example" => 2,
        "test" => 3,
        "bench" => 4,
        "build" => 5,
        _ => 6,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_self_workspace() {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let loaded = load(Path::new(manifest_dir)).expect("load this workspace");
        assert!(
            !loaded.local_crates.is_empty(),
            "expected at least one local crate"
        );
        let names: Vec<String> = loaded
            .local_crates
            .iter()
            .map(|k| {
                k.display_name(&loaded.db)
                    .map(|n| n.canonical_name().as_str().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert_eq!(names, ["rmc_graph"], "a member-directory request indexes that declared subtree, not sibling workspace members");

        // Negative half of the same load: nothing from outside this workspace.
        //
        // The scene is real rather than synthetic — this workspace depends on
        // our rust-analyzer and salsa forks through path dependencies, and
        // rust-analyzer tags every path dependency `CrateOrigin::Local`. Before
        // the crate-root membership check those 30+ crates were indexed as if
        // they were ours, which overflowed the extraction walk's stack.
        let workspace_root = Path::new(manifest_dir);
        let workspace_root = canonical_or_self(workspace_root);
        let strays: Vec<PathBuf> = loaded
            .local_crates
            .iter()
            .filter_map(|krate| crate_root_abs_path(&loaded.db, &loaded.vfs, *krate))
            .filter(|root| !root.starts_with(&workspace_root))
            .collect();
        assert!(
            strays.is_empty(),
            "local crates must be members of this workspace, but these roots live elsewhere: {strays:?}"
        );
    }

    /// Workspace at `/ws` with one member library and one member example.
    fn targets_fixture() -> WorkspaceTargets {
        let mut by_abs_root_file = HashMap::new();
        by_abs_root_file.insert(
            PathBuf::from("/ws/crates/member/src/lib.rs"),
            "lib".to_string(),
        );
        by_abs_root_file.insert(
            PathBuf::from("/ws/crates/member/examples/demo.rs"),
            "example".to_string(),
        );
        WorkspaceTargets {
            by_name: HashMap::new(),
            by_root_file: HashMap::new(),
            by_abs_root_file,
            workspace_root: Some(PathBuf::from("/ws")),
        }
    }

    #[test]
    fn member_targets_are_indexed_by_kind() {
        let targets = targets_fixture();
        assert!(should_index_crate_root(
            Path::new("/ws/crates/member/src/lib.rs"),
            &targets
        ));
        assert!(!should_index_crate_root(
            Path::new("/ws/crates/member/examples/demo.rs"),
            &targets
        ));
    }

    #[test]
    fn vendored_patch_crates_inside_the_tree_stay_indexed() {
        // `[patch.crates-io] eframe = { path = "patches/eframe-0.36.0" }` is not
        // a workspace member, but it is part of the tree we were pointed at.
        let targets = targets_fixture();
        assert!(should_index_crate_root(
            Path::new("/ws/patches/eframe-0.36.0/src/lib.rs"),
            &targets
        ));
    }

    #[test]
    fn path_dependencies_outside_the_tree_are_dropped() {
        // The regression this filter exists for: rust-analyzer tags these
        // `CrateOrigin::Local` exactly like our own crates.
        let targets = targets_fixture();
        assert!(!should_index_crate_root(
            Path::new("/elsewhere/rust-analyzer/crates/hir/src/lib.rs"),
            &targets
        ));
        // A sibling directory sharing the root's name prefix is still outside.
        assert!(!should_index_crate_root(
            Path::new("/ws-other/src/lib.rs"),
            &targets
        ));
    }

    #[test]
    fn without_cargo_metadata_every_local_crate_is_kept() {
        // A metadata failure must not silently produce an empty graph.
        let targets = WorkspaceTargets {
            by_name: HashMap::new(),
            by_root_file: HashMap::new(),
            by_abs_root_file: HashMap::new(),
            workspace_root: None,
        };
        assert!(should_index_crate_root(
            Path::new("/elsewhere/whatever/src/lib.rs"),
            &targets
        ));
    }

    #[test]
    fn target_kind_label_collapses_cargo_kinds() {
        assert_eq!(target_kind_label(&[TargetKind::Lib]), "lib");
        assert_eq!(target_kind_label(&[TargetKind::RLib]), "lib");
        assert_eq!(target_kind_label(&[TargetKind::Bin]), "bin");
        assert_eq!(target_kind_label(&[TargetKind::Example]), "example");
        assert_eq!(target_kind_label(&[TargetKind::CustomBuild]), "build");
    }

    #[test]
    fn should_index_only_library_and_binary_targets() {
        assert!(should_index_target_kind("lib"));
        assert!(should_index_target_kind("bin"));
        assert!(!should_index_target_kind("test"));
        assert!(!should_index_target_kind("bench"));
        assert!(!should_index_target_kind("example"));
        assert!(!should_index_target_kind("build"));
        assert!(!should_index_target_kind("unknown"));
    }

    #[test]
    fn load_crate_target_kinds_finds_workspace_targets() {
        // CARGO_MANIFEST_DIR is `crates/rmc-graph/`. Resolve up two levels to
        // the virtual workspace root; target source paths are still rooted in
        // their workspace-member directories.
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root is two levels above crates/rmc-graph");
        let by_root_file = load_crate_target_kinds(manifest_dir).by_root_file;

        assert_eq!(
            by_root_file
                .get("crates/rmc-graph/src/lib.rs")
                .map(String::as_str),
            Some("lib")
        );
        assert_eq!(
            by_root_file
                .get("crates/rust-code-mcp/src/main.rs")
                .map(String::as_str),
            Some("bin")
        );
        assert_eq!(
            by_root_file
                .get("crates/rust-code-mcp/examples/graph_burn.rs")
                .map(String::as_str),
            Some("example")
        );
    }
}

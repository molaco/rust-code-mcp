//! Workspace loader for the hypergraph layer.
//!
//! Loads a Cargo workspace through rust-analyzer and returns the
//! `RootDatabase`, `Vfs`, and the filtered set of *local* crates (workspace
//! members only, identified via `CrateOrigin::is_local`).
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

pub struct LoadedWorkspace {
    pub workspace_root: PathBuf,
    pub db: RootDatabase,
    pub vfs: Vfs,
    pub local_crates: Vec<Crate>,
    pub crate_target_kinds_by_name: HashMap<String, String>,
    pub crate_target_kinds_by_root_file: HashMap<String, String>,
}

pub fn load(directory: &Path) -> Result<LoadedWorkspace> {
    let canonical = directory
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", directory.display()))?;
    let workspace_root = canonical.clone();

    let cargo_config = CargoConfig {
        sysroot: Some(RustLibSource::Discover),
        no_deps: false,
        features: CargoFeatures::All,
        all_targets: true,
        set_test: true,
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        // Build every workspace crate's DefMap in parallel during load. Without
        // this, DefMaps are constructed lazily on first access during the
        // serial extraction walk — measured ~30× slower on burn.
        prefill_caches: true,
        num_worker_threads: num_cpus::get_physical(),
        proc_macro_processes: 1,
    };

    let (db, vfs, _proc_macro) =
        load_workspace_at(&canonical, &cargo_config, &load_config, &|_| {})
            .with_context(|| format!("failed to load workspace at {}", canonical.display()))?;

    let local_crates = filter_local_crates(&db);
    let (crate_target_kinds_by_name, crate_target_kinds_by_root_file) =
        load_crate_target_kinds(&workspace_root);

    Ok(LoadedWorkspace {
        workspace_root,
        db,
        vfs,
        local_crates,
        crate_target_kinds_by_name,
        crate_target_kinds_by_root_file,
    })
}

/// Keep only crates RA tagged as workspace members (`CrateOrigin::Local`).
/// This is the same filter rust-analyzer's own `view_crate_graph` uses for
/// its workspace-only mode — it correctly excludes crates.io deps, the
/// sysroot, the rustc workspace, and proc-macro-host crates.
fn filter_local_crates(db: &RootDatabase) -> Vec<Crate> {
    Crate::all(db)
        .into_iter()
        .filter(|krate| krate.origin(db).is_local())
        .collect()
}

fn load_crate_target_kinds(
    workspace_root: &Path,
) -> (HashMap<String, String>, HashMap<String, String>) {
    let manifest_path = workspace_root.join("Cargo.toml");
    let mut command = MetadataCommand::new();
    command.manifest_path(manifest_path).no_deps();
    let metadata = match command.exec() {
        Ok(metadata) => metadata,
        Err(error) => {
            tracing::warn!(
                "failed to load cargo metadata for {}; crate target-kind filters will fall back to unknown/default handling: {}",
                workspace_root.display(),
                error
            );
            return (HashMap::new(), HashMap::new());
        }
    };

    let workspace_members: HashSet<_> = metadata.workspace_members.iter().cloned().collect();
    let mut by_name = HashMap::new();
    let mut by_root_file = HashMap::new();

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
            if let Some(root_file) =
                workspace_relative_path(target.src_path.as_std_path(), workspace_root)
            {
                insert_preferred_target_kind(&mut by_root_file, root_file, kind);
            }
        }
    }

    (by_name, by_root_file)
}

fn normalize_crate_name(name: &str) -> String {
    name.replace('-', "_")
}

fn workspace_relative_path(path: &Path, workspace_root: &Path) -> Option<String> {
    path.strip_prefix(workspace_root)
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
}

fn insert_preferred_target_kind(map: &mut HashMap<String, String>, key: String, kind: String) {
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
        assert!(!loaded.local_crates.is_empty(), "expected at least one local crate");
        let names: Vec<String> = loaded
            .local_crates
            .iter()
            .map(|k| {
                k.display_name(&loaded.db)
                    .map(|n| n.canonical_name().as_str().to_string())
                    .unwrap_or_default()
            })
            .collect();
        assert!(
            names.iter().any(|n| n == "rust_code_mcp"),
            "expected rust_code_mcp in local crates, got {names:?}"
        );
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
    fn load_crate_target_kinds_finds_workspace_targets() {
        // CARGO_MANIFEST_DIR is `crates/rmc-graph/` after Phase 7 B.7.
        // The asserts below expect workspace-root targets (src/main.rs, examples/*.rs),
        // so resolve up two levels to the workspace root.
        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("workspace root is two levels above crates/rmc-graph");
        let (_by_name, by_root_file) = load_crate_target_kinds(manifest_dir);

        assert_eq!(
            by_root_file.get("src/lib.rs").map(String::as_str),
            Some("lib")
        );
        assert_eq!(
            by_root_file.get("src/main.rs").map(String::as_str),
            Some("bin")
        );
        assert_eq!(
            by_root_file.get("examples/graph_burn.rs").map(String::as_str),
            Some("example")
        );
    }
}

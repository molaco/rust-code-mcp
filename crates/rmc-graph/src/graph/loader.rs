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
use ra_ap_hir_def::nameres::crate_def_map;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::{CargoConfig, CargoFeatures, RustLibSource};
use ra_ap_vfs::Vfs;

use super::audit_util::resolve_workspace_relative;

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
    let (crate_target_kinds_by_name, crate_target_kinds_by_root_file) =
        load_crate_target_kinds(&workspace_root);

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
        // serial extraction walk — measured ~30× slower on burn.
        prefill_caches: true,
        num_worker_threads: num_cpus::get_physical(),
        proc_macro_processes: 1,
    };

    let (db, vfs, _proc_macro) =
        load_workspace_at(&canonical, &cargo_config, &load_config, &|_| {})
            .with_context(|| format!("failed to load workspace at {}", canonical.display()))?;

    let local_crates = filter_local_crates(
        &db,
        &vfs,
        &workspace_root,
        &crate_target_kinds_by_root_file,
    );

    Ok(LoadedWorkspace {
        workspace_root,
        db,
        vfs,
        local_crates,
        crate_target_kinds_by_name,
        crate_target_kinds_by_root_file,
    })
}

/// Keep only crates RA tagged as workspace members (`CrateOrigin::Local`) and
/// backed by normal library/binary Cargo targets.
///
/// rust-analyzer creates crate graph entries for workspace integration tests,
/// benches, examples, and build scripts too. Those targets can be expensive and
/// can trigger HIR/body-inference bugs in large workspaces even though the
/// persisted hypergraph's architectural queries only need production targets.
/// This is the same filter rust-analyzer's own `view_crate_graph` uses for
/// its workspace-only mode — it correctly excludes crates.io deps, the
/// sysroot, the rustc workspace, and proc-macro-host crates — with an
/// additional Cargo target-kind filter for workspace-only extra targets.
fn filter_local_crates(
    db: &RootDatabase,
    vfs: &Vfs,
    workspace_root: &Path,
    crate_target_kinds_by_root_file: &HashMap<String, String>,
) -> Vec<Crate> {
    Crate::all(db)
        .into_iter()
        .filter(|krate| krate.origin(db).is_local())
        .filter(|krate| {
            if crate_target_kinds_by_root_file.is_empty() {
                return true;
            }
            match crate_root_target_kind(
                db,
                vfs,
                workspace_root,
                *krate,
                crate_target_kinds_by_root_file,
            ) {
                Some(kind) => should_index_target_kind(kind),
                None => true,
            }
        })
        .collect()
}

fn crate_root_target_kind<'a>(
    db: &RootDatabase,
    vfs: &Vfs,
    workspace_root: &Path,
    krate: Crate,
    crate_target_kinds_by_root_file: &'a HashMap<String, String>,
) -> Option<&'a str> {
    let def_map = crate_def_map(db, krate.base());
    let root_module_id = def_map.crate_root(db);
    let root_file_id = def_map[root_module_id]
        .definition_source_file_id()
        .original_file(db)
        .file_id(db);
    let root_file = resolve_workspace_relative(vfs, root_file_id, workspace_root)?;
    crate_target_kinds_by_root_file.get(&root_file).map(String::as_str)
}

fn should_index_target_kind(kind: &str) -> bool {
    matches!(kind, "lib" | "bin")
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
            names.iter().any(|n| n == "rust-code-mcp" || n == "rust_code_mcp"),
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
        let (_by_name, by_root_file) = load_crate_target_kinds(manifest_dir);

        assert_eq!(
            by_root_file.get("crates/rmc-graph/src/lib.rs").map(String::as_str),
            Some("lib")
        );
        assert_eq!(
            by_root_file.get("crates/rust-code-mcp/src/main.rs").map(String::as_str),
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

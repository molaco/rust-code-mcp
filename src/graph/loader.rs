//! Workspace loader for the hypergraph layer.
//!
//! Loads a Cargo workspace through rust-analyzer with `no_deps = true` and
//! returns the `RootDatabase`, `Vfs`, and the filtered set of *local* crates
//! (workspace members only — not deps, not sysroot, not path-deps that happen
//! to live outside the workspace).

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ra_ap_hir::Crate;
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace};
use ra_ap_paths::AbsPathBuf;
use ra_ap_project_model::{CargoConfig, ProjectManifest, ProjectWorkspace};
use ra_ap_vfs::Vfs;

pub struct LoadedWorkspace {
    pub workspace_root: PathBuf,
    pub db: RootDatabase,
    pub vfs: Vfs,
    pub local_crates: Vec<Crate>,
}

pub fn load(directory: &Path) -> Result<LoadedWorkspace> {
    let canonical = directory
        .canonicalize()
        .with_context(|| format!("failed to canonicalize {}", directory.display()))?;
    let root = AbsPathBuf::assert_utf8(canonical);
    let manifest = ProjectManifest::discover_single(&root)
        .with_context(|| format!("failed to discover Cargo project from {}", directory.display()))?;

    let cargo_config = CargoConfig {
        sysroot: None,
        no_deps: true,
        ..Default::default()
    };

    let workspace = ProjectWorkspace::load(manifest, &cargo_config, &|_| {})
        .context("failed to load Cargo workspace")?;
    let workspace_root: PathBuf = workspace.workspace_root().to_path_buf().into();

    let member_roots: HashSet<PathBuf> = collect_member_roots(&workspace);

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: true,
    };

    let (db, vfs, _proc_macro) = load_workspace(workspace, &Default::default(), &load_config)
        .context("failed to load rust-analyzer workspace")?;

    let local_crates = filter_local_crates(&db, &vfs, &member_roots);

    Ok(LoadedWorkspace {
        workspace_root,
        db,
        vfs,
        local_crates,
    })
}

fn collect_member_roots(workspace: &ProjectWorkspace) -> HashSet<PathBuf> {
    let mut out = HashSet::new();
    if let ra_ap_project_model::ProjectWorkspaceKind::Cargo { cargo, .. } = &workspace.kind {
        for package_id in cargo.packages() {
            let package = &cargo[package_id];
            if !package.is_member {
                continue;
            }
            for &target_id in &package.targets {
                let target = &cargo[target_id];
                let path: PathBuf = target.root.clone().into();
                out.insert(path);
            }
        }
    }
    out
}

fn filter_local_crates(db: &RootDatabase, vfs: &Vfs, member_roots: &HashSet<PathBuf>) -> Vec<Crate> {
    let mut out = Vec::new();
    for krate in Crate::all(db) {
        if krate.is_builtin(db) {
            continue;
        }
        let root_file = krate.root_file(db);
        let Some(path) = vfs.file_path(root_file).as_path() else {
            continue;
        };
        let path_buf: PathBuf = path.to_path_buf().into();
        if member_roots.contains(&path_buf) {
            out.push(krate);
        }
    }
    out
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
            names.iter().any(|n| n == "file_search_mcp"),
            "expected file_search_mcp in local crates, got {names:?}"
        );
    }
}

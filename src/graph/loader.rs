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

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
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

    Ok(LoadedWorkspace {
        workspace_root,
        db,
        vfs,
        local_crates,
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
}

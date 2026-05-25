//! Project loading with rust-analyzer

use std::path::Path;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::{CargoConfig, CargoFeatures, RustLibSource};
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;
use anyhow::{Result, Context};

/// Load a Cargo project for semantic analysis
///
/// Uses no_deps=true for fast loading (~120ms).
/// Only local project code is analyzed.
pub(crate) fn load_project(path: &Path) -> Result<(AnalysisHost, Vfs)> {
    load_project_with_config(path, fast_project_cargo_config())
}

/// Load a Cargo project with full workspace dependency edges for rename.
pub(super) fn load_project_full(path: &Path) -> Result<(AnalysisHost, Vfs)> {
    load_project_with_config(path, full_workspace_cargo_config())
}

fn fast_project_cargo_config() -> CargoConfig {
    CargoConfig {
        sysroot: None,
        no_deps: true,
        ..Default::default()
    }
}

fn full_workspace_cargo_config() -> CargoConfig {
    CargoConfig {
        sysroot: Some(RustLibSource::Discover),
        no_deps: false,
        features: CargoFeatures::All,
        all_targets: true,
        set_test: true,
        ..Default::default()
    }
}

fn load_project_with_config(path: &Path, cargo_config: CargoConfig) -> Result<(AnalysisHost, Vfs)> {
    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: true,
        num_worker_threads: num_cpus::get_physical(),
        proc_macro_processes: 1,
    };

    let (db, vfs, _) = load_workspace_at(path, &cargo_config, &load_config, &|_| {})
        .context("Failed to load workspace")?;

    let host = AnalysisHost::with_database(db);

    Ok((host, vfs))
}

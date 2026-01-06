//! Project loading with rust-analyzer

use std::path::Path;
use ra_ap_load_cargo::{LoadCargoConfig, ProcMacroServerChoice, load_workspace_at};
use ra_ap_project_model::CargoConfig;
use ra_ap_ide::AnalysisHost;
use ra_ap_vfs::Vfs;
use anyhow::{Result, Context};

/// Load a Cargo project for semantic analysis
///
/// Uses no_deps=true for fast loading (~120ms).
/// Only local project code is analyzed.
pub fn load_project(path: &Path) -> Result<(AnalysisHost, Vfs)> {
    let cargo_config = CargoConfig {
        sysroot: None,
        no_deps: true,
        ..Default::default()
    };

    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: false,
        with_proc_macro_server: ProcMacroServerChoice::None,
        prefill_caches: true,
    };

    let (db, vfs, _) = load_workspace_at(path, &cargo_config, &load_config, &|_| {})
        .context("Failed to load workspace")?;

    let host = AnalysisHost::with_database(db);

    Ok((host, vfs))
}

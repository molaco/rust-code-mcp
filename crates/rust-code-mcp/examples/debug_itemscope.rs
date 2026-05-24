//! Dump every ItemScope entry for every module in burn_core, to see whether
//! RA actually resolves cross-crate imports like `use burn_tensor::Tensor`.

use rmc_graph::graph::load;
use ra_ap_hir::Crate;
use ra_ap_hir_def::nameres::crate_def_map;
use ra_ap_hir_def::ModuleDefId;
use std::path::Path;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("error")),
        )
        .with_writer(std::io::stderr)
        .init();
    let loaded = load(Path::new("/home/molaco/Documents/burn")).expect("load");
    eprintln!("local_crates = {}", loaded.local_crates.len());

    let target_crate_name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "burn_core".to_string());

    let krate: Option<Crate> = loaded.local_crates.iter().find(|k| {
        k.display_name(&loaded.db)
            .map(|n| n.canonical_name().as_str().to_string())
            == Some(target_crate_name.clone())
    }).copied();

    let Some(krate) = krate else {
        eprintln!("ERR: {target_crate_name} not in local_crates");
        std::process::exit(1);
    };

    eprintln!("inspecting crate: {target_crate_name}");

    // What does RA's CrateGraph think burn_core's deps are?
    let deps = krate.dependencies(&loaded.db);
    eprintln!();
    eprintln!("=== {target_crate_name}.dependencies() ({}) ===", deps.len());
    for d in deps.iter().take(40) {
        let dep_name = d.krate.display_name(&loaded.db)
            .map(|n| n.canonical_name().as_str().to_string())
            .unwrap_or_else(|| "?".to_string());
        eprintln!("  {} (as `{}`)", dep_name, d.name.as_str());
    }
    eprintln!();

    let def_map = crate_def_map(&loaded.db, krate.base());

    let mut total_modules = 0;
    let mut total_entries = 0;
    let mut cross_crate_entries = 0;
    let mut tensor_entries = 0;
    let mut sample: Vec<String> = Vec::new();

    for (mod_id, _) in def_map.modules() {
        if mod_id.is_block_module(&loaded.db) {
            continue;
        }
        total_modules += 1;
        let scope = &def_map[mod_id].scope;

        for (name, ty_item) in scope.types() {
            total_entries += 1;
            let krate_of_def = krate_of(&loaded.db, ty_item.def);
            if krate_of_def.is_some_and(|k| k != krate) {
                cross_crate_entries += 1;
                if name.as_str() == "Tensor" {
                    tensor_entries += 1;
                    if sample.len() < 5 {
                        sample.push(format!(
                            "  TYPES Tensor in {target_crate_name}: import_provenance={:?}",
                            ty_item.import
                        ));
                    }
                }
            }
        }
        for (name, val_item) in scope.values() {
            total_entries += 1;
            let krate_of_def = krate_of(&loaded.db, val_item.def);
            if krate_of_def.is_some_and(|k| k != krate) {
                cross_crate_entries += 1;
                if name.as_str() == "Tensor" {
                    tensor_entries += 1;
                    if sample.len() < 10 {
                        sample.push(format!(
                            "  VALUES Tensor in {target_crate_name}: import_provenance={:?}",
                            val_item.import
                        ));
                    }
                }
            }
        }
    }

    eprintln!();
    eprintln!("=== summary for {target_crate_name} ===");
    eprintln!("total non-block modules:        {total_modules}");
    eprintln!("total ItemScope entries:        {total_entries}");
    eprintln!("entries pointing at OTHER crate: {cross_crate_entries}");
    eprintln!("of those, named `Tensor`:       {tensor_entries}");
    eprintln!();
    eprintln!("sample Tensor entries:");
    for s in sample {
        eprintln!("{s}");
    }

    // Bonus: histogram of the OTHER crate names that DO get cross-crate entries.
    let mut hist: std::collections::BTreeMap<String, usize> = Default::default();
    for (mod_id, _) in def_map.modules() {
        if mod_id.is_block_module(&loaded.db) {
            continue;
        }
        let scope = &def_map[mod_id].scope;
        let mut count = |def_id: ModuleDefId| {
            if let Some(other) = krate_of(&loaded.db, def_id) {
                if other != krate {
                    let name = other
                        .display_name(&loaded.db)
                        .map(|n| n.canonical_name().as_str().to_string())
                        .unwrap_or_else(|| "?".to_string());
                    *hist.entry(name).or_default() += 1;
                }
            }
        };
        for (_n, ty) in scope.types() { count(ty.def); }
        for (_n, v) in scope.values() { count(v.def); }
    }
    eprintln!();
    eprintln!("=== cross-crate target histogram ===");
    for (k, v) in hist.iter().rev().take(15) {
        eprintln!("  {v:>4}  {k}");
    }
}

fn krate_of(db: &ra_ap_ide_db::RootDatabase, def_id: ModuleDefId) -> Option<Crate> {
    use ra_ap_hir_def::HasModule;
    use ra_ap_hir_def::AdtId;
    let module_id = match def_id {
        ModuleDefId::ModuleId(id) => id,
        ModuleDefId::FunctionId(id) => id.module(db),
        ModuleDefId::AdtId(AdtId::StructId(id)) => id.module(db),
        ModuleDefId::AdtId(AdtId::EnumId(id)) => id.module(db),
        ModuleDefId::AdtId(AdtId::UnionId(id)) => id.module(db),
        ModuleDefId::TraitId(id) => id.module(db),
        ModuleDefId::TypeAliasId(id) => id.module(db),
        ModuleDefId::ConstId(id) => id.module(db),
        ModuleDefId::StaticId(id) => id.module(db),
        _ => return None,
    };
    Some(ra_ap_hir::Crate::from(module_id.krate(db)))
}

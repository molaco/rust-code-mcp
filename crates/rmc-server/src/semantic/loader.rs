//! Project loading with rust-analyzer

use anyhow::{Context, Result};
use ra_ap_ide::AnalysisHost;
use ra_ap_ide_db::RootDatabase;
use ra_ap_ide_db::base_db::file_text_store::FileTextStore;
use ra_ap_ide_db::base_db::{AbsPathBuf, Crate, SourceDatabase, all_crates, salsa};
use ra_ap_ide_db::prime_caches::parallel_prime_caches;
use ra_ap_load_cargo::{
    LoadCargoConfig, ProcMacroServerChoice, load_workspace_into_db, project_workspace_at,
};
use ra_ap_project_model::{CargoConfig, CargoFeatures, RustLibSource};
use ra_ap_vfs::Vfs;
use std::collections::HashSet;
use std::path::Path;

use crate::mcp::project_paths::data_dir;

/// Load a Cargo project for semantic analysis
///
/// Uses no_deps=true for fast loading (~120ms).
/// Only local project code is analyzed.
pub(crate) fn load_project(path: &Path) -> Result<(AnalysisHost, Vfs)> {
    load_project_with_config(path, fast_project_cargo_config())
}

/// Load a Cargo project with full workspace dependency edges for rename.
pub(super) fn load_project_full(path: &Path) -> Result<(AnalysisHost, Vfs)> {
    load_project_with_config(path, full_workspace_cargo_config()?)
}

fn fast_project_cargo_config() -> CargoConfig {
    CargoConfig {
        sysroot: None,
        no_deps: true,
        ..Default::default()
    }
}

fn full_workspace_cargo_config() -> Result<CargoConfig> {
    Ok(CargoConfig {
        sysroot: Some(RustLibSource::Discover),
        no_deps: false,
        features: CargoFeatures::All,
        all_targets: true,
        set_test: true,
        load_weak_only_optional_deps: parse_weak_optional_deps(
            std::env::var(WEAK_OPTIONAL_DEPS_ENV).ok().as_deref(),
        )?,
        ..Default::default()
    })
}

/// On-switch for loading the optional dependencies cargo never builds:
/// `RMC_LOAD_WEAK_OPTIONAL_DEPS=1` loads them, as it was before.
///
/// # What they are
///
/// `cargo metadata` lists an optional dependency that only a weak `dep?/feat`
/// feature names, though no build compiles it (`cargo tree` leaves it out).
/// nalgebra 0.34's default `std` names glam 0.14 … 0.32 that way. On a large
/// workspace that was 74 of 2023 packages — 18 glam versions alone ≈ 90 MiB of
/// def maps, plus item trees, signatures and trait impls every crate depending
/// on nalgebra had in scope. rust-analyzer now drops them; see
/// `project_model::unbuilt_optional_deps`.
pub(crate) const WEAK_OPTIONAL_DEPS_ENV: &str = "RMC_LOAD_WEAK_OPTIONAL_DEPS";

/// Default is off, like cargo; `1` is the only other setting.
fn parse_weak_optional_deps(value: Option<&str>) -> Result<bool> {
    match value.map(str::trim) {
        None => Ok(false),
        Some("1") => Ok(true),
        Some(other) => {
            anyhow::bail!("{WEAK_OPTIONAL_DEPS_ENV}={other:?}: the only setting is `1`")
        }
    }
}

/// Off-switch for running build scripts: `RMC_BUILD_SCRIPTS=0` (`off`/`false`/
/// `no`) loads the workspace without them, as it was before.
pub(crate) const BUILD_SCRIPTS_ENV: &str = "RMC_BUILD_SCRIPTS";

/// Whether to discover `OUT_DIR` by running the workspace's build scripts.
///
/// # What is invisible without it
///
/// Code pulled in with `include!(concat!(env!("OUT_DIR"), …))` — prost message
/// types, generated tables — has no `OUT_DIR` to expand until build scripts
/// have run, so the generated module is not in the tree at all and every
/// reference *into* it resolves to nothing. Measured on a large workspace: seven reads
/// of a proto field in `roundtrip_invariants.rs` counted as zero. That is the
/// same silent zero the rest of this file's switches produce — indistinguishable
/// from "nobody reads this".
///
/// # What it costs
///
/// One cargo invocation per project load, sharing the workspace's own `target/`
/// (so it reuses what is already built, and can block on the directory lock
/// while another cargo holds it). With cargo ≥ 1.89 rust-analyzer passes
/// `--compile-time-deps`, which builds *only* build scripts and proc macros
/// rather than checking the workspace; older cargo falls back to a full
/// `cargo check`, which is where the cost would actually bite.
///
/// `wrap_rustc_in_build_scripts` stays off deliberately: it points
/// `RUSTC_WRAPPER` at `current_exe()`, which is rust-analyzer's own binary in
/// rust-analyzer and *ours* here — we do not implement the `RA_RUSTC_WRAPPER`
/// protocol, so turning it on would wreck the build rather than speed it up.
fn build_scripts_enabled() -> bool {
    parse_build_scripts(std::env::var(BUILD_SCRIPTS_ENV).ok().as_deref())
}

/// Default is on: the switch exists to be turned off, not to be turned on.
fn parse_build_scripts(value: Option<&str>) -> bool {
    let Some(value) = value else {
        return true;
    };
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "0" | "off" | "false" | "no"
    )
}

fn load_project_with_config(path: &Path, cargo_config: CargoConfig) -> Result<(AnalysisHost, Vfs)> {
    let (mut db, vfs) = load_unprimed(path, &cargo_config)?;

    prime_in_batches(&mut db, prime_batch_files(), num_cpus::get_physical());

    let host = AnalysisHost::with_database(db);

    Ok((host, vfs))
}

/// The workspace loaded into a database, caches cold.
fn load_unprimed(path: &Path, cargo_config: &CargoConfig) -> Result<(RootDatabase, Vfs)> {
    let load_config = LoadCargoConfig {
        load_out_dirs_from_check: build_scripts_enabled(),
        with_proc_macro_server: ProcMacroServerChoice::None,
        // Primed by the caller instead, in batches: see `prime_in_batches`.
        prefill_caches: false,
        num_worker_threads: num_cpus::get_physical(),
        proc_macro_processes: 1,
    };

    // `load_workspace_at`, opened up so the store goes in before any text does.
    let workspace = project_workspace_at(path, cargo_config, &load_config, &|_| {})
        .context("Failed to load workspace")?;
    // Read here because `load_workspace` reads it, and `lru_sweep` drives it.
    let lru_cap = std::env::var("RA_LRU_CAP")
        .ok()
        .and_then(|it| it.parse::<u16>().ok());
    let mut db = RootDatabase::new(lru_cap);
    apply_query_lru_capacities(
        &mut db,
        &parse_query_lru_capacities(std::env::var(QUERY_LRU_CAPS_ENV).ok().as_deref())?,
    )?;
    if let Some(store) = file_text_store()? {
        db.set_file_text_store(store);
    }
    let (vfs, _) =
        load_workspace_into_db(workspace, &cargo_config.extra_env, &load_config, &mut db)
            .context("Failed to load workspace")?;
    Ok((db, vfs))
}

/// Per-query LRU capacities applied at load, before any memo exists:
/// `RMC_LRU_CAPS=MacroCallId::macro_arg_=0,EditionedFileId::parse_=64`. The
/// names are salsa `debug_name`s exactly as `lru_capacity_names()` gives them
/// (an associated query carries a trailing `_`); `0` disables eviction. This is
/// rust-analyzer's `lruQueryCapacities` for the daemon, and the knob a capacity
/// sweep turns: set at load, because a capacity changed afterwards cannot
/// bring back what the load already evicted, and a zero set afterwards forgets
/// the use order (see `lru_sweep`).
pub(crate) const QUERY_LRU_CAPS_ENV: &str = "RMC_LRU_CAPS";

/// `name=capacity` pairs, comma-separated; unset or empty is no override.
fn parse_query_lru_capacities(value: Option<&str>) -> Result<Vec<(String, usize)>> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    value
        .split(',')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            // `rsplit_once`: a self type in a name may hold `=`-free generics
            // and spaces, but the capacity is always the last field.
            let (name, capacity) = pair.rsplit_once('=').with_context(|| {
                format!("{QUERY_LRU_CAPS_ENV}: `{pair}` is not `query_name=capacity`")
            })?;
            let capacity = capacity.trim().parse::<usize>().with_context(|| {
                format!("{QUERY_LRU_CAPS_ENV}: the capacity in `{pair}` is not a whole number")
            })?;
            Ok((name.trim().to_owned(), capacity))
        })
        .collect()
}

/// A name that matches no tunable query is an error, not a no-op: a probe run
/// under a misspelt name would price the default and report it as the override.
fn apply_query_lru_capacities(db: &mut RootDatabase, capacities: &[(String, usize)]) -> Result<()> {
    for (name, capacity) in capacities {
        if salsa::Database::set_lru_capacity_by_name(db, name, *capacity) == 0 {
            anyhow::bail!(
                "{QUERY_LRU_CAPS_ENV}: no query named `{name}` has a tunable LRU capacity; \
                 tunable names are {:?}",
                salsa::Database::lru_capacity_names(db)
            );
        }
        tracing::info!("LRU capacity of `{name}` set to {capacity} ({QUERY_LRU_CAPS_ENV})");
    }
    Ok(())
}

/// Off-switch for keeping file texts out of the heap: `RMC_FILE_TEXTS=heap`
/// keeps every text in the heap, as rust-analyzer does.
pub(crate) const FILE_TEXTS_ENV: &str = "RMC_FILE_TEXTS";

/// Where the loaded workspace keeps its file texts; see
/// `base_db::file_text_store`.
///
/// # Why
///
/// The texts are an input, and salsa never evicts an input: on a large
/// workspace with its dependencies that is ≈ 49 000 files and ≈ 780 MiB of
/// heap for the life of the context, 85 % of it library sources read once at
/// load. With the store, library sources under the cargo registry, git
/// checkouts and toolchains are read back from disk when wanted again (length
/// and hash checked), every other text — the workspace's own, build-script
/// output — is a private copy in an unlinked file, mapped, and the heap holds
/// only the few texts read recently.
///
/// # What a changed original costs
///
/// A read-back file found changed (cargo's cache GC, a toolchain reinstalled
/// in place) fails the call that read it, and `with_semantic` unloads the
/// context so the next call loads it afresh — see
/// [`super::SemanticService::unload_drifted`].
fn file_text_store() -> Result<Option<FileTextStore>> {
    match std::env::var(FILE_TEXTS_ENV) {
        Ok(value) if value.trim() == "heap" => return Ok(None),
        Ok(value) => anyhow::bail!("{FILE_TEXTS_ENV}={value:?}: the only setting is `heap`"),
        Err(_) => {}
    }
    let copy_dir = data_dir().join("file-texts");
    std::fs::create_dir_all(&copy_dir)
        .with_context(|| format!("creating {}", copy_dir.display()))?;
    let copy_dir = AbsPathBuf::try_from(copy_dir.to_string_lossy().as_ref())
        .map_err(|path| anyhow::anyhow!("{path} is not an absolute path"))?;
    Ok(Some(FileTextStore {
        read_back_dirs: read_back_dirs(),
        copy_dir,
    }))
}

/// Directories whose files do not change under a loaded workspace: the
/// registry's unpacked crates (one directory per version), git dependencies
/// (one checkout per revision) and toolchains (one directory per toolchain).
/// `CARGO_HOME` and `RUSTUP_HOME` as cargo and rustup resolve them.
fn read_back_dirs() -> Vec<AbsPathBuf> {
    let home = std::env::home_dir();
    let under = |var: &str, default: &str| {
        std::env::var_os(var)
            .map(std::path::PathBuf::from)
            .or_else(|| home.as_ref().map(|home| home.join(default)))
    };
    let cargo = under("CARGO_HOME", ".cargo");
    let rustup = under("RUSTUP_HOME", ".rustup");
    [
        cargo
            .as_ref()
            .map(|cargo| cargo.join("registry").join("src")),
        cargo
            .as_ref()
            .map(|cargo| cargo.join("git").join("checkouts")),
        rustup.map(|rustup| rustup.join("toolchains")),
    ]
    .into_iter()
    .flatten()
    // A relative `CARGO_HOME` is cargo's business to resolve, not ours to
    // guess: such a directory is simply not read back, its files are copied.
    .filter_map(|dir| AbsPathBuf::try_from(dir.to_str()?).ok())
    .collect()
}

/// Size of one priming batch, in files: `RMC_PRIME_BATCH_FILES=<n>`; `0`
/// primes the whole crate graph in one pass, as `load-cargo`'s own
/// `prefill_caches` does.
pub(crate) const PRIME_BATCH_FILES_ENV: &str = "RMC_PRIME_BATCH_FILES";

/// Default batch: a few percent of a large workspace's files (≈ 49 000 with
/// dependencies), so the syntax trees alive at once are a few hundred MB, not
/// the 8 GB of the whole graph — while one batch still holds enough crates to
/// keep every worker thread busy.
const DEFAULT_PRIME_BATCH_FILES: usize = 2000;

fn prime_batch_files() -> usize {
    match std::env::var(PRIME_BATCH_FILES_ENV) {
        Ok(value) => value
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("{PRIME_BATCH_FILES_ENV}={value:?} is not a file count")),
        Err(_) => DEFAULT_PRIME_BATCH_FILES,
    }
}

/// Warm the caches crate batch by crate batch, evicting the LRU-capped memos
/// between batches.
///
/// # The peak this removes
///
/// `parallel_prime_caches` over all crates runs inside one revision, and salsa
/// evicts LRU-capped memos only when told to (a new revision or
/// `trigger_lru_eviction`). So during a one-pass prime the capacities of
/// `parse` (128) and `parse_macro_expansion` (512) do nothing: on a large
/// workspace every syntax tree of the graph — ≈ 4 + 4.3 GB — is alive at the
/// end of the load. The first garbage collection drops them, but the holes
/// they leave between the load's long-lived objects (item trees, def maps,
/// macro arguments) stay resident: that is the allocator's floor after trim.
/// Evicting after each batch keeps the trees alive at once to those of one
/// batch, and the next batch reuses the holes of the last.
///
/// # Why the batches are sound
///
/// `all_crates` is in topological order (dependencies first), and a batch is
/// a contiguous slice of it, so every dependency of a batch crate is in an
/// earlier batch — already primed — or in the same one, where the scheduler
/// of `parallel_prime_caches` orders it. Nothing is computed that the
/// one-pass prime would not compute; only trees are dropped earlier, and a
/// tree asked for again is re-parsed (same text, same tree).
///
/// # Why `trigger_lru_eviction`, not a revision bump
///
/// A synthetic write (what `trigger_garbage_collection` does) would make
/// every low-durability memo — the workspace's own crates — re-verify on its
/// next read, once per batch. Eviction without a new revision costs nothing
/// to verify. rust-analyzer avoids it in its main loop because a *cancelled*
/// fixpoint cycle is only cleared by a revision bump; here nothing is
/// cancelled: this function holds `&mut` on the only database, and each
/// `parallel_prime_caches` has returned, its workers gone, before the call.
fn prime_in_batches(db: &mut RootDatabase, batch_files: usize, num_worker_threads: usize) {
    let all = all_crates(db);
    if batch_files == 0 {
        parallel_prime_caches(db, &all, num_worker_threads, &|_| ());
        return;
    }
    let batches = prime_batches(db, &all, batch_files);
    tracing::info!(
        "priming {} crates in {} batch(es) of ≤ {batch_files} files",
        all.len(),
        batches.len()
    );
    for batch in batches {
        parallel_prime_caches(db, &batch, num_worker_threads, &|_| ());
        salsa::Database::trigger_lru_eviction(db);
    }
}

/// Cut the topological order into contiguous batches of about `batch_files`
/// files each (a crate bigger than that is a batch of its own).
///
/// A crate weighs the files of its source root. Crates of one package share
/// the root (lib, bins, tests), so a root counts once per batch: its files are
/// parsed once for all of them.
fn prime_batches(db: &RootDatabase, all: &[Crate], batch_files: usize) -> Vec<Vec<Crate>> {
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut roots_in_batch = HashSet::new();
    let mut files_in_batch = 0;
    let root_files = |root| db.source_root(root).source_root(db).iter().count();
    for &krate in all {
        let root = db
            .file_source_root(krate.data(db).root_file_id)
            .source_root_id(db);
        let files = if roots_in_batch.contains(&root) {
            0
        } else {
            root_files(root)
        };
        // A crate whose root is already in the batch adds nothing to parse,
        // so it never closes the batch — even one already over budget because
        // a single root is bigger than the budget.
        if files > 0 && !batch.is_empty() && files_in_batch + files > batch_files {
            batches.push(std::mem::take(&mut batch));
            roots_in_batch.clear();
            files_in_batch = 0;
        }
        roots_in_batch.insert(root);
        files_in_batch += files;
        batch.push(krate);
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::{full_workspace_cargo_config, load_unprimed, parse_build_scripts, prime_batches};
    use ra_ap_ide_db::base_db::{Crate, SourceDatabase, all_crates};
    use std::collections::{HashMap, HashSet};
    use std::path::Path;

    fn write_file(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a file has a parent")).expect("mkdir");
        std::fs::write(path, text).expect("write fixture file");
    }

    /// The batches are what the soundness argument of `prime_in_batches`
    /// says they are: the topological order cut into contiguous pieces,
    /// every dependency in the same or an earlier batch, and the file budget
    /// broken only by a single root bigger than it.
    ///
    /// The graph is a real one — a two-crate workspace plus the sysroot, whose
    /// roots run to hundreds of files — because the property that matters,
    /// "dependencies first", is `all_crates`' ordering, which only a loaded
    /// crate graph has.
    #[test]
    fn priming_batches_put_dependencies_first_and_keep_the_budget() {
        let _analysis = crate::deep_stack::test_holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        write_file(
            &root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"base\", \"top\"]\nresolver = \"2\"\n",
        );
        write_file(
            &root.join("base/Cargo.toml"),
            "[package]\nname = \"base\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        );
        write_file(&root.join("base/src/lib.rs"), "pub struct Base;\n");
        write_file(
            &root.join("top/Cargo.toml"),
            "[package]\nname = \"top\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [dependencies]\nbase = { path = \"../base\" }\n",
        );
        write_file(&root.join("top/src/lib.rs"), "pub use base::Base;\n");

        let (db, _vfs) = load_unprimed(
            root,
            &full_workspace_cargo_config().expect("the default config"),
        )
        .expect("load the fixture");
        let all = all_crates(&db);
        let budget = 50;
        let batches = prime_batches(&db, &all, budget);
        assert!(
            batches.len() > 2,
            "a {budget}-file budget over the sysroot must cut several batches, got {}",
            batches.len()
        );

        let flat: Vec<Crate> = batches.concat();
        assert_eq!(
            flat,
            all.to_vec(),
            "the batches must be the crate order itself, cut: nothing lost, nothing moved"
        );

        let batch_of: HashMap<Crate, usize> = batches
            .iter()
            .enumerate()
            .flat_map(|(index, batch)| batch.iter().map(move |&krate| (krate, index)))
            .collect();
        let mut edges_across = 0;
        for (index, batch) in batches.iter().enumerate() {
            for &krate in batch {
                for dep in &krate.data(&db).dependencies {
                    let dep_batch = batch_of[&dep.crate_id];
                    assert!(
                        dep_batch <= index,
                        "a dependency primed after its dependent: batch {dep_batch} > {index}"
                    );
                    edges_across += usize::from(dep_batch < index);
                }
            }

            let roots: HashSet<_> = batch
                .iter()
                .map(|krate| {
                    db.file_source_root(krate.data(&db).root_file_id)
                        .source_root_id(&db)
                })
                .collect();
            let files: usize = roots
                .iter()
                .map(|&root| db.source_root(root).source_root(&db).iter().count())
                .sum();
            assert!(
                files <= budget || roots.len() == 1,
                "batch {index} holds {files} files over {} roots, over the {budget}-file budget",
                roots.len()
            );
        }
        // Without edges between batches the ordering check above checked nothing.
        assert!(edges_across > 0, "no dependency crosses a batch boundary");
    }

    /// The direction matters more than the spellings: an unset variable has to
    /// mean *on*, or the fix ships switched off for everyone who never heard of
    /// it.
    #[test]
    fn build_scripts_are_on_unless_switched_off() {
        assert!(parse_build_scripts(None), "unset must mean on");
        for off in ["0", "off", "false", "no", " OFF ", "No"] {
            assert!(
                !parse_build_scripts(Some(off)),
                "{off:?} must switch build scripts off"
            );
        }
        for on in ["1", "true", "yes", "", "please"] {
            assert!(
                parse_build_scripts(Some(on)),
                "{on:?} is not an off-switch and must leave build scripts on"
            );
        }
    }

    /// Unset means "like cargo" — the fix ships on; a misspelt value fails
    /// the load instead of silently meaning one or the other.
    #[test]
    fn weak_optional_deps_stay_out_unless_switched_on() {
        use super::parse_weak_optional_deps;

        assert!(!parse_weak_optional_deps(None).expect("unset"));
        assert!(parse_weak_optional_deps(Some("1")).expect("on"));
        assert!(parse_weak_optional_deps(Some(" 1 ")).expect("on, padded"));
        for bad in ["0", "yes", "true", ""] {
            assert!(
                parse_weak_optional_deps(Some(bad)).is_err(),
                "{bad:?} must be refused"
            );
        }
    }

    /// The switch reaches rust-analyzer: a dependency outside the workspace
    /// names an optional `weak_only` only as `weak_only?/std`, which cargo
    /// never builds. Loaded by default it is no crate; with the switch it is,
    /// as before. The second load is the vacuity check: without it a fixture
    /// cargo never put into the metadata would pass too.
    #[test]
    fn weak_only_optional_deps_are_no_crates_unless_switched_on() {
        let _analysis = crate::deep_stack::test_holding_an_analysis();
        let fixture = tempfile::tempdir().expect("create fixture tempdir");
        let root = fixture.path();
        let package = |name: &str| {
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")
        };
        write_file(
            &root.join("ws/Cargo.toml"),
            "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
        );
        write_file(
            &root.join("ws/app/Cargo.toml"),
            &(package("app") + "\n[dependencies]\nfacade = { path = \"../../facade\" }\n"),
        );
        write_file(&root.join("ws/app/src/lib.rs"), "pub use facade::*;\n");
        write_file(
            &root.join("facade/Cargo.toml"),
            &(package("facade")
                + "\n[dependencies]\nweak_only = { path = \"../weak_only\", optional = true }\n\n\
                   [features]\ndefault = [\"std\"]\nstd = [\"weak_only?/std\"]\n"),
        );
        write_file(&root.join("facade/src/lib.rs"), "pub struct Facade;\n");
        write_file(
            &root.join("weak_only/Cargo.toml"),
            &(package("weak_only") + "\n[features]\nstd = []\n"),
        );
        write_file(&root.join("weak_only/src/lib.rs"), "pub struct WeakOnly;\n");

        let crate_names = |load_weak: bool| -> HashSet<String> {
            let config = ra_ap_project_model::CargoConfig {
                // The sysroot adds nothing to the question and costs a load.
                sysroot: None,
                load_weak_only_optional_deps: load_weak,
                ..full_workspace_cargo_config().expect("the default config")
            };
            let (db, _vfs) = load_unprimed(&root.join("ws"), &config).expect("load the fixture");
            all_crates(&db)
                .iter()
                .filter_map(|krate| {
                    let name = krate.extra_data(&db).display_name.as_ref()?;
                    Some(name.to_string())
                })
                .collect()
        };

        let default = crate_names(false);
        assert!(
            default.contains("app") && default.contains("facade"),
            "{default:?}"
        );
        assert!(!default.contains("weak_only"), "{default:?}");
        let switched_on = crate_names(true);
        assert!(switched_on.contains("weak_only"), "{switched_on:?}");
    }

    /// A per-query capacity lands on the query it names, and a name that
    /// matches nothing fails the load instead of pricing the default under the
    /// override's label.
    #[test]
    fn query_lru_capacities_apply_by_name_and_refuse_a_stranger() {
        use super::{apply_query_lru_capacities, parse_query_lru_capacities};

        assert!(parse_query_lru_capacities(None).expect("unset").is_empty());
        assert!(parse_query_lru_capacities(Some(" ")).expect("blank").is_empty());
        assert_eq!(
            parse_query_lru_capacities(Some("MacroCallId::macro_arg_=0, EditionedFileId::parse_=64"))
                .expect("two pairs"),
            vec![
                ("MacroCallId::macro_arg_".to_owned(), 0),
                ("EditionedFileId::parse_".to_owned(), 64)
            ]
        );
        assert!(parse_query_lru_capacities(Some("MacroCallId::macro_arg_")).is_err());
        assert!(parse_query_lru_capacities(Some("MacroCallId::macro_arg_=lots")).is_err());

        let mut db = ra_ap_ide_db::RootDatabase::new(None);
        // The query this knob was added for has to be tunable at all: without
        // the fork's `lru` attribute it is not, and the override fails loudly.
        apply_query_lru_capacities(&mut db, &[("MacroCallId::macro_arg_".to_owned(), 0)])
            .expect("`macro_arg` carries an LRU in the fork");
        let error = apply_query_lru_capacities(&mut db, &[("MacroCallId::macro_arg".to_owned(), 1)])
            .expect_err("the name without salsa's trailing `_` matches nothing");
        assert!(error.to_string().contains("MacroCallId::macro_arg_"), "{error}");
    }
}

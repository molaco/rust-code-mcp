//! Semantic code analysis using rust-analyzer

mod loader;
mod position;
mod rename;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ra_ap_ide::AnalysisHost;
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_vfs::{FileExcluded, Vfs, VfsPath};
use anyhow::{Context as _, Result};
use rmc_indexing::indexing::collect_project_rust_files;
use serde::Serialize;

pub(crate) use position::Location;
pub(crate) use rename::RenamePreview;

/// What a `stat` can see about a file — enough to decide whether it is worth
/// reading, and cheap enough to collect for a whole workspace on every query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: SystemTime,
    len: u64,
}

/// What the working tree did since a project context was built.
#[derive(Debug)]
enum Staleness {
    /// Same files, same stamps: the cached analysis still describes the code.
    Fresh,
    /// The same set of files, some of them edited. Their new text can be
    /// pushed into the existing database.
    Edited(Vec<PathBuf>),
    /// Files appeared or disappeared. A new module, a new crate or a deleted
    /// file changes the module tree and possibly the crate graph, and neither
    /// can be patched file-by-file — the project has to be loaded again.
    StructureChanged,
}

/// Cached project context
struct ProjectContext {
    host: AnalysisHost,
    vfs: Vfs,
    load_kind: LoadKind,
    /// The working tree as it was when this context was built or last
    /// refreshed. Without it the cache has no way to notice that the code
    /// moved on — see [`SemanticService::refresh_if_stale`].
    stamps: HashMap<PathBuf, FileStamp>,
    /// Value of [`SemanticService::clock`] when this context was last asked a
    /// question. Orders eviction — see [`SemanticService::evict_to_capacity`].
    ///
    /// A counter rather than a `SystemTime`: eviction needs the *order* of
    /// uses, and a wall clock can step backwards (NTP, suspend) and reorder
    /// them.
    last_used: u64,
}

fn stamp_of(path: &Path) -> Option<FileStamp> {
    let meta = std::fs::metadata(path).ok()?;
    Some(FileStamp {
        modified: meta.modified().ok()?,
        len: meta.len(),
    })
}

/// Stat every `*.rs` file of the project, using the same walker as the indexer
/// so both sides mean the same thing by "a project file".
///
/// Files that vanish between the walk and the stat are simply absent from the
/// result, which reads as a deletion — the correct conclusion.
fn collect_stamps(root: &Path) -> HashMap<PathBuf, FileStamp> {
    let (files, walk_errors) = collect_project_rust_files(root);
    if walk_errors > 0 {
        tracing::warn!(
            "{} unreadable entries while checking {} for staleness",
            walk_errors,
            root.display()
        );
    }
    files
        .into_iter()
        .filter_map(|path| stamp_of(&path).map(|stamp| (path, stamp)))
        .collect()
}

fn classify_staleness(
    old: &HashMap<PathBuf, FileStamp>,
    new: &HashMap<PathBuf, FileStamp>,
) -> Staleness {
    if old.len() != new.len() {
        return Staleness::StructureChanged;
    }

    let mut edited = Vec::new();
    for (path, stamp) in new {
        match old.get(path) {
            None => return Staleness::StructureChanged,
            Some(previous) if previous == stamp => {}
            Some(_) => edited.push(path.clone()),
        }
    }

    if edited.is_empty() {
        Staleness::Fresh
    } else {
        edited.sort();
        Staleness::Edited(edited)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoadKind {
    Fast,
    Full,
}

impl LoadKind {
    fn as_str(self) -> &'static str {
        match self {
            LoadKind::Fast => "fast",
            LoadKind::Full => "full",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SemanticProjectStatus {
    pub path: String,
    pub load_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SemanticServiceStatus {
    pub project_count: usize,
    pub projects: Vec<SemanticProjectStatus>,
}

/// Push the current contents of `paths` into an already-loaded analysis.
///
/// Fails rather than skips when a file cannot be mapped into the analysis: the
/// caller turns that into a full reload. Silently leaving one file at its old
/// revision would reproduce the very defect this module now guards against,
/// only harder to notice.
fn apply_edits(ctx: &mut ProjectContext, paths: &[PathBuf]) -> Result<()> {
    let mut updates = Vec::with_capacity(paths.len());

    // Read everything before mutating anything: a change applied halfway
    // would leave the database describing a mixture of two revisions.
    for path in paths {
        let vfs_path = VfsPath::new_real_path(path.to_string_lossy().into_owned());
        let (file_id, excluded) = ctx.vfs.file_id(&vfs_path).ok_or_else(|| {
            anyhow::anyhow!("{} is not part of the loaded analysis", path.display())
        })?;
        if excluded == FileExcluded::Yes {
            anyhow::bail!("{} is excluded from the loaded analysis", path.display());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading {}", path.display()))?;
        updates.push((vfs_path, file_id, text));
    }

    let mut change = ChangeWithProcMacros::default();
    for (vfs_path, file_id, text) in updates {
        ctx.vfs
            .set_file_contents(vfs_path, Some(text.clone().into_bytes()));
        change.change_file(file_id, Some(text));
    }
    ctx.host.apply_change(change);

    Ok(())
}

/// How many project contexts may stay loaded at once. `0` means unlimited.
///
/// Derived rather than guessed. A freshly started daemon costs ~2.3 GB before
/// it loads anything (ONNX runtime, embedding model, GPU probe); one
/// `Fast`-loaded workspace of ~4000 files adds ~3 GB. Three of them come to
/// ~11.3 GB, which is what the watchdog's soft limit is set to accommodate
/// (12288 MB); a fourth is memory that guard has already decided against.
///
/// It was 2 while the daemon key included the working directory, and 2 was the
/// right number then: each daemon served one directory, so a second project was
/// already unusual. One daemon now serves every directory, so the projects a
/// person moves between during a day all land in the same map — and evicting
/// one costs the next question in that project a full reload, measured in
/// minutes on a large workspace.
///
/// What this bounds is a *count*, which is only a proxy for memory — ten tiny
/// crates cost less than one workspace. The proxy is deliberate: it is the
/// cheap half of the job, and the watchdog remains the half that measures
/// actual RSS.
const DEFAULT_MAX_PROJECTS: usize = 3;

const MAX_PROJECTS_ENV: &str = "RMC_MAX_PROJECTS";

fn max_projects_from_env() -> usize {
    std::env::var(MAX_PROJECTS_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(DEFAULT_MAX_PROJECTS)
}

/// Service for semantic code queries
pub(crate) struct SemanticService {
    projects: HashMap<PathBuf, ProjectContext>,
    /// Upper bound on `projects`; `0` disables eviction entirely.
    ///
    /// # The defect this closes
    ///
    /// Before it, the map had no TTL, no LRU and no ceiling: every directory a
    /// session ever asked about kept its rust-analyzer context until the
    /// process died. A daemon outlives its clients by design, so nothing
    /// bounded that — measured 2026-08-27, one daemon sat on 14.6 GB, most of
    /// it contexts for directories nobody had touched in hours.
    max_projects: usize,
    /// Monotonic tick, handed out by [`Self::touch`]. Only its order matters.
    clock: u64,
}

impl SemanticService {
    pub(crate) fn new() -> Self {
        Self::with_max_projects(max_projects_from_env())
    }

    /// The cap as an argument rather than as an environment variable, so a test
    /// can state the capacity it is judging instead of arranging the process
    /// environment around it.
    pub(crate) fn with_max_projects(max_projects: usize) -> Self {
        Self {
            projects: HashMap::new(),
            max_projects,
            clock: 0,
        }
    }

    pub(crate) fn project_count(&self) -> usize {
        self.projects.len()
    }

    pub(crate) fn status(&self) -> SemanticServiceStatus {
        let mut projects = self
            .projects
            .iter()
            .map(|(path, ctx)| SemanticProjectStatus {
                path: path.display().to_string(),
                load_kind: ctx.load_kind.as_str().to_string(),
            })
            .collect::<Vec<_>>();
        projects.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then_with(|| a.load_kind.cmp(&b.load_kind))
        });
        SemanticServiceStatus {
            project_count: projects.len(),
            projects,
        }
    }

    pub(crate) fn clear_all(&mut self) -> usize {
        let count = self.projects.len();
        self.projects.clear();
        count
    }

    pub(crate) fn clear_project(&mut self, project_path: &Path) -> usize {
        let canonical =
            std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
        if self.projects.remove(&canonical).is_some() {
            1
        } else {
            0
        }
    }

    /// Give every loaded analysis a revision bump and sweep the type interner.
    ///
    /// # Why a daemon has to do this by hand
    ///
    /// salsa evicts an LRU-capped query's memos in exactly one place:
    /// `for_each_evicted`, reached from `reset_for_new_revision`. That is to
    /// say, **capacities only mean something at the moment the revision bumps**,
    /// and a revision bumps when a file changes. An IDE gets those for free on
    /// every keystroke; a project sitting in this daemon that nobody has edited
    /// for hours never bumps at all, so its capacity — whatever it is set to —
    /// evicts nothing, ever. Those are precisely the cold contexts that made a
    /// daemon 14.6 GB. This call is where the bump comes from instead.
    ///
    /// `AnalysisHost::trigger_garbage_collection` is a synthetic write plus a
    /// mark-and-sweep over the interned type storage, and rust-analyzer's own
    /// main loop calls it the same way when its worker pools go quiet.
    ///
    /// # Why calling it with several hosts loaded is sound
    ///
    /// The sweep is process-global — the type interner is shared by every
    /// `AnalysisHost` here — but its roots are refcounts (`Arc::strong_count(item) > 1`
    /// marks alive), so a type another host holds is a root and survives. What
    /// the `unsafe` really demands is that no query be *in flight*, holding a
    /// type it has not recorded anywhere yet. Every path into this service goes
    /// through one `Mutex<SemanticService>`, and this method needs `&mut self`,
    /// so holding that lock means no analysis is running in any project.
    ///
    /// # Cost
    ///
    /// The sweep runs once per host, so N loaded projects pay N sweeps; the
    /// caller keeps this off the hot path (see the watchdog's interval) rather
    /// than the method trying to be clever about it. The bump itself is paid
    /// later, by the next query re-validating its memos.
    ///
    /// Returns the number of contexts collected, for the log line.
    pub(crate) fn collect_garbage(&mut self) -> usize {
        for (path, ctx) in self.projects.iter_mut() {
            tracing::debug!("collecting garbage in {}", path.display());
            ctx.host.trigger_garbage_collection();
        }
        self.projects.len()
    }

    /// Mark `canonical` as the most recently used context.
    fn touch(&mut self, canonical: &Path) {
        self.clock += 1;
        let clock = self.clock;
        if let Some(ctx) = self.projects.get_mut(canonical) {
            ctx.last_used = clock;
        }
    }

    /// Drop the least recently used contexts until [`Self::max_projects`] holds.
    ///
    /// `keep` is never evicted whatever its age: it is the project the caller is
    /// about to answer a question about, and evicting it would mean reloading it
    /// on the very next line. With a cap of 1 that is the whole rule — the
    /// working project stays, everything else goes.
    ///
    /// Returns how many contexts were dropped.
    fn evict_to_capacity(&mut self, keep: &Path) -> usize {
        if self.max_projects == 0 {
            return 0;
        }

        let mut evicted = 0;
        while self.projects.len() > self.max_projects {
            let victim = self
                .projects
                .iter()
                .filter(|(path, _)| path.as_path() != keep)
                .min_by_key(|(_, ctx)| ctx.last_used)
                .map(|(path, _)| path.clone());

            // Only `keep` is left: the cap is smaller than one project, and one
            // is the least we can hold and still answer.
            let Some(victim) = victim else { break };

            tracing::info!(
                "unloading {} to stay within {}={} loaded project(s)",
                victim.display(),
                MAX_PROJECTS_ENV,
                self.max_projects
            );
            self.projects.remove(&victim);
            evicted += 1;
        }
        evicted
    }

    /// Get or load project (lazy loading)
    fn get_or_load(&mut self, project_path: &Path) -> Result<()> {
        self.get_or_load_kind(project_path, LoadKind::Fast)
    }

    /// Get or load project with full workspace dependency edges.
    fn get_or_load_full(&mut self, project_path: &Path) -> Result<()> {
        self.get_or_load_kind(project_path, LoadKind::Full)
    }

    fn get_or_load_kind(&mut self, project_path: &Path, requested: LoadKind) -> Result<()> {
        let canonical = project_path.canonicalize()?;

        let needs_load = match self.projects.get(&canonical) {
            Some(ctx) => requested == LoadKind::Full && ctx.load_kind == LoadKind::Fast,
            None => true,
        };

        let outcome = if needs_load {
            self.load_kind_into_cache(&canonical, requested)
        } else {
            // Cached — but "cached" said nothing about "current" until this call
            // existed. See [`Self::refresh_if_stale`].
            self.refresh_if_stale(&canonical, requested)
        };
        outcome?;

        // Both branches, and only after they succeeded: a load that failed left
        // no context to order, and a query that never happened is not a use.
        self.touch(&canonical);
        self.evict_to_capacity(&canonical);
        Ok(())
    }

    fn load_kind_into_cache(&mut self, canonical: &Path, requested: LoadKind) -> Result<()> {
        tracing::info!(
            "Loading {:?} IDE for project: {}",
            requested,
            canonical.display()
        );
        // Stamps are taken BEFORE the load, not after: a file edited while
        // rust-analyzer was reading the workspace must come out stale on the
        // next query, not silently accepted as already analysed.
        let stamps = collect_stamps(canonical);
        let (host, vfs) = match requested {
            LoadKind::Fast => loader::load_project(canonical)?,
            LoadKind::Full => loader::load_project_full(canonical)?,
        };
        self.clock += 1;
        self.projects.insert(
            canonical.to_path_buf(),
            ProjectContext {
                host,
                vfs,
                load_kind: requested,
                stamps,
                last_used: self.clock,
            },
        );
        tracing::info!("IDE loaded successfully");
        Ok(())
    }

    /// Bring a cached project up to date with the working tree.
    ///
    /// # The defect this closes
    ///
    /// A `ProjectContext` is an `AnalysisHost` built from the files as they
    /// were at load time. Nothing ever invalidated it: the only reload
    /// condition was upgrading a `Fast` context to `Full`. So every answer
    /// after the first edit described code that no longer existed —
    /// `find_references` reporting 2 call sites where the file on disk had 7,
    /// with no error and no warning anywhere. The only cure was knowing to
    /// call `clear_runtime scope=semantic_only`, which requires already
    /// suspecting the answer, which is exactly what a confident wrong answer
    /// prevents.
    ///
    /// # Why two repair paths
    ///
    /// Edits to existing files are pushed straight into the salsa database
    /// (`apply_change`), which invalidates precisely the derived queries that
    /// depended on those files — milliseconds, and it is what a real IDE does
    /// on every keystroke. Added or deleted files are not patchable that way:
    /// they change the module tree, and a new crate changes the crate graph,
    /// so those force a reload. Reloading costs seconds (`Fast`, `no_deps`) to
    /// minutes (`Full`), which is why it is reserved for the case that needs
    /// it rather than used for everything.
    ///
    /// # Known limit
    ///
    /// Only `*.rs` files are watched. A `Cargo.toml` edited on its own —
    /// dropping a dependency, renaming a feature — is invisible here. Adding a
    /// crate is not: its sources are new files, which trips the structural
    /// path anyway.
    fn refresh_if_stale(&mut self, canonical: &Path, requested: LoadKind) -> Result<()> {
        let current = collect_stamps(canonical);

        let staleness = {
            let ctx = self
                .projects
                .get(canonical)
                .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;
            classify_staleness(&ctx.stamps, &current)
        };

        match staleness {
            Staleness::Fresh => Ok(()),
            Staleness::StructureChanged => {
                tracing::info!(
                    "Project {} gained or lost files since it was analysed; reloading",
                    canonical.display()
                );
                self.load_kind_into_cache(canonical, requested)
            }
            Staleness::Edited(paths) => {
                let patched = {
                    let ctx = self
                        .projects
                        .get_mut(canonical)
                        .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;
                    apply_edits(ctx, &paths).map(|()| ctx.stamps = current)
                };
                match patched {
                    Ok(()) => {
                        tracing::info!(
                            "Refreshed {} edited file(s) in {}",
                            paths.len(),
                            canonical.display()
                        );
                        Ok(())
                    }
                    Err(e) => {
                        // A file the analysis does not know (excluded from the
                        // build, unreadable, non-UTF-8) cannot be patched in.
                        // Falling back to a reload is slow but honest; keeping
                        // the stale context would be the original defect.
                        tracing::info!(
                            "Cannot patch {} incrementally ({e}); reloading",
                            canonical.display()
                        );
                        self.load_kind_into_cache(canonical, requested)
                    }
                }
            }
        }
    }

    /// Search for symbols by name (for find_definition)
    pub(crate) fn symbol_search(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
    ) -> Result<Vec<Location>> {
        self.symbol_search_with_exact(project_path, symbol_name, limit, false)
    }

    /// Search for symbols by name with optional full-name filtering.
    pub(crate) fn symbol_search_with_exact(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
        exact: bool,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::symbol_search_with_exact(&ctx.host, &ctx.vfs, symbol_name, limit, exact)
    }

    /// Find all references to symbols matching a name
    /// First finds all symbols matching the name, then finds references for each
    pub(crate) fn find_references_by_name(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
    ) -> Result<Vec<Location>> {
        self.find_references_by_name_with_exact(project_path, symbol_name, false)
    }

    /// Find all references to symbols matching a name with optional exact filtering.
    pub(crate) fn find_references_by_name_with_exact(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        exact: bool,
    ) -> Result<Vec<Location>> {
        self.get_or_load(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        position::find_references_by_name_with_exact(&ctx.host, &ctx.vfs, symbol_name, exact)
    }

    /// Preview rename of a symbol by name. Does not modify any files.
    pub(crate) fn rename_by_name(
        &mut self,
        project_path: &Path,
        symbol_name: &str,
        new_name: &str,
    ) -> Result<RenamePreview> {
        self.get_or_load_full(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        rename::rename_by_name(&ctx.host, &ctx.vfs, symbol_name, new_name)
    }

    /// Preview rename of a symbol at a concrete file position. Does not modify any files.
    pub(crate) fn rename_by_position(
        &mut self,
        project_path: &Path,
        file_path: &Path,
        line: u32,
        column: u32,
        symbol_name: &str,
        new_name: &str,
    ) -> Result<RenamePreview> {
        self.get_or_load_full(project_path)?;

        let canonical = project_path.canonicalize()?;
        let ctx = self.projects.get(&canonical)
            .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

        rename::rename_by_position(
            &ctx.host,
            &ctx.vfs,
            file_path,
            line,
            column,
            symbol_name,
            new_name,
        )
    }

    #[cfg(test)]
    pub(crate) fn insert_test_project_fast(&mut self, project_path: PathBuf) {
        let canonical =
            std::fs::canonicalize(&project_path).unwrap_or(project_path);
        self.clock += 1;
        self.projects.insert(
            canonical,
            ProjectContext {
                host: AnalysisHost::new(None),
                vfs: Vfs::default(),
                load_kind: LoadKind::Fast,
                stamps: HashMap::new(),
                last_used: self.clock,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn rename_preview_includes_workspace_reverse_dependencies() {
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let workspace_path = workspace.path();

        write_file(
            &workspace_path.join("Cargo.toml"),
            r#"
[workspace]
members = ["engine_sdk", "engine_consumer"]
resolver = "2"
"#,
        );
        write_file(
            &workspace_path.join("engine_sdk/Cargo.toml"),
            r#"
[package]
name = "engine_sdk"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        );
        let sdk_lib = workspace_path.join("engine_sdk/src/lib.rs");
        write_file(
            &sdk_lib,
            r#"pub trait Engine {
    fn tick(&self);
}
"#,
        );
        write_file(
            &workspace_path.join("engine_consumer/Cargo.toml"),
            r#"
[package]
name = "engine_consumer"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
engine_sdk = { path = "../engine_sdk" }
"#,
        );
        write_file(
            &workspace_path.join("engine_consumer/src/lib.rs"),
            r#"use engine_sdk::Engine;

pub struct Candle;

impl Engine for Candle {
    fn tick(&self) {}
}

pub fn run(engine: &dyn Engine) {
    engine.tick();
}
"#,
        );

        let mut service = SemanticService::new();
        let preview = service
            .rename_by_position(
                workspace_path,
                &sdk_lib,
                1,
                11,
                "Engine",
                "RenamedEngine",
            )
            .expect("rename preview");

        assert!(
            preview
                .edits
                .iter()
                .any(|edit| edit.file_path.ends_with("engine_sdk/src/lib.rs")),
            "expected declaration edit in engine_sdk, got {:?}",
            preview.edits
        );
        assert!(
            preview
                .edits
                .iter()
                .any(|edit| edit.file_path.ends_with("engine_consumer/src/lib.rs")),
            "expected downstream edit in engine_consumer, got {:?}",
            preview.edits
        );
    }

    #[test]
    fn runtime_semantic_status_and_clear_are_workspace_scoped() {
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let mut service = SemanticService::new();
        service.insert_test_project_fast(workspace.path().join("."));

        let status = service.status();
        assert_eq!(status.project_count, 1);
        assert_eq!(status.projects[0].load_kind, "fast");

        assert_eq!(service.clear_project(workspace.path()), 1);
        assert_eq!(service.project_count(), 0);
        assert_eq!(service.clear_all(), 0);
    }

    /// Call sites only. `find_references_by_name` returns the declaration
    /// alongside them (tagged `"target"` rather than `"reference"`), and
    /// counting both would make the assertions read one higher than the
    /// number they are actually about.
    fn call_sites(locations: &[Location]) -> usize {
        locations
            .iter()
            .filter(|location| location.name == "reference")
            .count()
    }

    /// A single-crate workspace with `target()` and one call site.
    fn one_crate_workspace(root: &Path) -> PathBuf {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[package]
name = "staleness_probe"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        );
        let lib = root.join("src/lib.rs");
        write_file(
            &lib,
            r#"pub fn target() {}

pub fn first() {
    target();
}
"#,
        );
        lib
    }

    /// The defect: a cached rust-analyzer context was never invalidated, so
    /// every answer after the first edit described the code as it was at load
    /// time. Here the second call site is added AFTER the first query, and the
    /// same service must see it.
    #[test]
    fn references_see_an_edit_made_after_the_project_was_cached() {
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        let lib = one_crate_workspace(root);

        let mut service = SemanticService::new();
        let before = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("first query");
        assert_eq!(
            call_sites(&before),
            1,
            "positive control: the fixture must start with exactly one call site, got {before:?}"
        );

        write_file(
            &lib,
            r#"pub fn target() {}

pub fn first() {
    target();
}

pub fn second() {
    target();
}
"#,
        );

        let after = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("query after edit");
        assert_eq!(
            call_sites(&after),
            2,
            "the edit must be visible without clearing the cache, got {after:?}"
        );
    }

    /// A new file changes the module tree, which cannot be patched into the
    /// database file-by-file — the refresh has to notice and reload.
    #[test]
    fn references_see_a_call_site_added_in_a_new_file() {
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        let lib = one_crate_workspace(root);

        let mut service = SemanticService::new();
        let before = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("first query");
        assert_eq!(call_sites(&before), 1, "positive control: {before:?}");

        write_file(
            &root.join("src/extra.rs"),
            r#"pub fn third() {
    crate::target();
}
"#,
        );
        write_file(
            &lib,
            r#"pub mod extra;

pub fn target() {}

pub fn first() {
    target();
}
"#,
        );

        let after = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("query after new file");
        assert_eq!(
            call_sites(&after),
            2,
            "a call site in a file added after load must be visible, got {after:?}"
        );
    }

    /// Untouched code must not pay for the check: same stamps, no reload, and
    /// crucially the same answer.
    #[test]
    fn an_untouched_project_is_not_reloaded() {
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        one_crate_workspace(root);

        let mut service = SemanticService::new();
        service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("first query");

        let canonical = root.canonicalize().expect("canonicalize");
        let stamps_before = service.projects[&canonical].stamps.clone();

        service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("second query");

        assert!(
            matches!(
                classify_staleness(&stamps_before, &service.projects[&canonical].stamps),
                Staleness::Fresh
            ),
            "an untouched tree must classify as fresh"
        );
    }

    fn write_file(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create parent dir");
        }
        fs::write(path, contents.trim_start()).expect("write fixture file");
    }

    /// Which projects the service is currently holding, canonicalized, so an
    /// assertion can name them instead of counting them.
    fn loaded(service: &SemanticService) -> Vec<PathBuf> {
        let mut paths: Vec<_> = service.projects.keys().cloned().collect();
        paths.sort();
        paths
    }

    /// The cap has to evict by *use*, not by arrival. A queue that dropped the
    /// oldest arrival would throw away the project the session actually works
    /// in, which is the one it will ask about next.
    #[test]
    fn a_third_project_evicts_the_least_recently_used_not_the_oldest() {
        let (first, second, third) = (
            tempfile::tempdir().expect("tempdir a"),
            tempfile::tempdir().expect("tempdir b"),
            tempfile::tempdir().expect("tempdir c"),
        );
        for dir in [&first, &second, &third] {
            one_crate_workspace(dir.path());
        }

        let mut service = SemanticService::with_max_projects(2);
        for dir in [&first, &second] {
            service
                .symbol_search(dir.path(), "target", 8)
                .expect("load project");
        }

        // The point of the test: `first` arrived first but is used last, so it
        // is `second` that must go when `third` needs the room.
        service
            .symbol_search(first.path(), "target", 8)
            .expect("re-query the first project");
        service
            .symbol_search(third.path(), "target", 8)
            .expect("load the third project");

        let mut expected = vec![
            first.path().canonicalize().expect("canonicalize first"),
            third.path().canonicalize().expect("canonicalize third"),
        ];
        expected.sort();
        assert_eq!(
            loaded(&service),
            expected,
            "the cap must keep the two most recently used projects"
        );
    }

    /// A cap of one is the interesting edge: every load evicts everything else,
    /// and the one thing that must survive is the project being asked about.
    #[test]
    fn the_project_being_used_is_never_the_victim() {
        let (first, second) = (
            tempfile::tempdir().expect("tempdir a"),
            tempfile::tempdir().expect("tempdir b"),
        );
        for dir in [&first, &second] {
            one_crate_workspace(dir.path());
        }

        let mut service = SemanticService::with_max_projects(1);
        service
            .symbol_search(first.path(), "target", 8)
            .expect("load the first project");
        let found = service
            .symbol_search(second.path(), "target", 8)
            .expect("load the second project");

        assert!(
            !found.is_empty(),
            "positive control: the surviving project must still answer, got {found:?}"
        );
        assert_eq!(
            loaded(&service),
            vec![second.path().canonicalize().expect("canonicalize second")],
            "the project just queried is the one that stays"
        );
    }

    /// `0` means unlimited, the same as it does for the daemon's other knobs.
    #[test]
    fn a_cap_of_zero_evicts_nothing() {
        let dirs: Vec<_> = (0..3)
            .map(|_| tempfile::tempdir().expect("tempdir"))
            .collect();

        let mut service = SemanticService::with_max_projects(0);
        for dir in &dirs {
            service.insert_test_project_fast(dir.path().to_path_buf());
        }
        let kept = service.projects.keys().next().cloned().expect("a project");
        assert_eq!(service.evict_to_capacity(&kept), 0);
        assert_eq!(
            service.project_count(),
            3,
            "an unlimited cap holds everything"
        );
    }

    /// Two things at once, because the collection can fail in two directions.
    ///
    /// *That it happened*: the whole point is the revision bump — without one,
    /// salsa never reaches `for_each_evicted` and an LRU capacity evicts
    /// nothing. A collection that returned a count without bumping would look
    /// exactly like a working one from the outside, so the revision is read
    /// directly.
    ///
    /// *That it was safe*: the sweep is `unsafe` and process-global. Had it
    /// freed something still in use, the symptom would be a wrong answer or a
    /// crash on the next query — hence the same question on both sides of it.
    #[test]
    fn a_collection_bumps_the_revision_and_keeps_the_answers() {
        use ra_ap_ide_db::base_db::SourceDatabase;

        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        one_crate_workspace(root);

        let mut service = SemanticService::new();
        let before = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("query before collection");
        assert_eq!(
            call_sites(&before),
            1,
            "positive control: the fixture must start with one call site, got {before:?}"
        );

        let canonical = root.canonicalize().expect("canonicalize");
        let revision_before = service.projects[&canonical]
            .host
            .raw_database()
            .nonce_and_revision();

        assert_eq!(
            service.collect_garbage(),
            1,
            "the collection must visit the loaded project"
        );

        let revision_after = service.projects[&canonical]
            .host
            .raw_database()
            .nonce_and_revision();
        assert_ne!(
            revision_before, revision_after,
            "without a revision bump the LRU capacities never evict anything"
        );

        let after = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("query after collection");
        assert_eq!(
            call_sites(&after),
            call_sites(&before),
            "collecting garbage must not change what the analysis knows, got {after:?}"
        );
    }

    /// Does rust-analyzer *leak*, or does the allocator merely keep what it
    /// freed? Both look identical from outside — RSS goes up and stays up — and
    /// they need opposite fixes, so this measures rather than assumes.
    ///
    /// # How it tells them apart
    ///
    /// Each cycle loads the workspace, drops the context, and asks the
    /// allocator to hand memory back. The number that matters is RSS *after*
    /// that release, cycle over cycle:
    ///
    /// - flat across cycles → nothing leaks; the resident memory is
    ///   fragmentation the allocator is sitting on, and the cure is the
    ///   allocator (the `mimalloc` feature), not rust-analyzer;
    /// - rising by roughly the same amount every cycle → a genuine leak, and
    ///   the per-cycle step is its size. rust-analyzer interns symbols in
    ///   process-global tables that are never freed, so a small constant step
    ///   is expected; a step near the cost of a whole load is not.
    ///
    /// The first cycle is excluded from the verdict: it pays one-time costs
    /// (proc-macro server, lazily built tables) that never recur.
    ///
    /// # Running it
    ///
    /// ```text
    /// RMC_RSS_CYCLE_PROJECT=/home/sc/t/bur/rust_app \
    ///   cargo test -p rmc-server --features migraphx -- --ignored --nocapture rss_across
    /// ```
    ///
    /// `RMC_RSS_CYCLE_COUNT` (default 4) and `RMC_RSS_CYCLE_TOLERANCE_MB`
    /// (default 512) tune length and verdict.
    #[test]
    #[ignore = "needs a real workspace in RMC_RSS_CYCLE_PROJECT and minutes to run"]
    fn rss_across_load_clear_cycles_separates_a_leak_from_fragmentation() {
        let Ok(project) = std::env::var("RMC_RSS_CYCLE_PROJECT") else {
            panic!(
                "set RMC_RSS_CYCLE_PROJECT to a cargo workspace root; without one this test \
                 would measure nothing and pass, which is worse than not running"
            );
        };
        let project = PathBuf::from(project);
        let cycles = env_usize("RMC_RSS_CYCLE_COUNT", 4);
        let tolerance_mb = env_usize("RMC_RSS_CYCLE_TOLERANCE_MB", 512) as u64;
        assert!(cycles >= 2, "a verdict needs at least two cycles");

        let mut after_release = Vec::with_capacity(cycles);
        for cycle in 0..cycles {
            let mut service = SemanticService::new();
            // A real query, not a bare load: this is what fills the salsa
            // database in production, and an empty database would understate
            // both fragmentation and any leak.
            let found = service
                .symbol_search(&project, "main", 16)
                .expect("symbol search on the probe workspace");
            let loaded = crate::mcp::memory::rss_kib().expect("RSS readable on linux");

            service.clear_all();
            drop(service);
            let release = crate::mcp::memory::release_and_measure();
            let settled = release.rss_kib_after.expect("RSS readable on linux");
            after_release.push(settled);

            println!(
                "cycle {cycle}: {} symbol(s), loaded {} MB, after clear+release {} MB (released {:?} KiB)",
                found.len(),
                loaded / 1024,
                settled / 1024,
                release.released_kib,
            );
        }

        // Compare the second cycle with the last: the first is warm-up.
        let baseline = after_release[1];
        let final_rss = *after_release.last().expect("cycles >= 2");
        let growth_kib = final_rss.saturating_sub(baseline);
        let per_cycle_kib = growth_kib / (cycles as u64 - 1).max(1);

        println!(
            "verdict: grew {} MB over {} cycles after warm-up ({} MB per cycle); tolerance {} MB",
            growth_kib / 1024,
            cycles - 1,
            per_cycle_kib / 1024,
            tolerance_mb,
        );

        assert!(
            growth_kib / 1024 <= tolerance_mb,
            "RSS after clear+release grew {} MB across {} cycles ({} MB per cycle) — that is a \
             leak, not fragmentation, since every cycle drops its context and trims. Baseline \
             {} MB, final {} MB, all cycles: {:?} MB",
            growth_kib / 1024,
            cycles - 1,
            per_cycle_kib / 1024,
            baseline / 1024,
            final_rss / 1024,
            after_release.iter().map(|kib| kib / 1024).collect::<Vec<_>>(),
        );
    }

    fn env_usize(name: &str, default: usize) -> usize {
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(default)
    }
}

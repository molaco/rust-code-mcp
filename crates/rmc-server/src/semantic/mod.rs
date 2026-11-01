//! Semantic code analysis using rust-analyzer

mod loader;
/// Pricing an LRU capacity at both ends — bytes handed back against the time the
/// next query pays for them. Measurement only, so it ships with the tests.
#[cfg(test)]
mod lru_sweep;
mod position;
mod rename;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{
    Arc, Mutex, MutexGuard, RwLock, RwLockReadGuard, TryLockError,
};
use std::time::SystemTime;

use ra_ap_ide::{Analysis, AnalysisHost};
use ra_ap_ide_db::ChangeWithProcMacros;
use ra_ap_ide_db::base_db::salsa::Cancelled;
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
    /// Shared with the queries running on snapshots of `host`, which read it
    /// (file ids ↔ paths) with the slot let go; a refresh writes it after
    /// `apply_change` has cancelled them. See [`SemanticService::with_project`].
    vfs: Arc<RwLock<Vfs>>,
    load_kind: LoadKind,
    /// The working tree as it was when this context was built or last
    /// refreshed. Without it the cache has no way to notice that the code
    /// moved on — see [`refresh_if_stale`].
    stamps: HashMap<PathBuf, FileStamp>,
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

/// One salsa ingredient's line in a memory report — a query's memo table or an
/// interned/tracked struct's storage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IngredientMemory {
    /// `query` or `struct`.
    pub family: &'static str,
    /// For a query this is the *query* name (`<SelfTy>::<fn>`), which is what
    /// an LRU capacity is addressed by. salsa keeps it in the map key and puts
    /// the result *type* in `debug_name`, so reading the values alone loses the
    /// only label that can be acted on.
    pub name: &'static str,
    /// The result type, beside a query name: two queries can return the same
    /// shape, and the shape is what hints at the heap behind it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub produces: Option<&'static str>,
    pub count: usize,
    pub stack_bytes: usize,
    pub metadata_bytes: usize,
    /// Present only where the query declares a `heap_size` function, and a
    /// lower bound where it does. Absent means "not measured", never "empty".
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heap_bytes: Option<usize>,
    /// Page fill statistics, for the interned families that keep their slots in
    /// pages. Says how much of the reserved space is actually in use.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page: Option<String>,
}

impl IngredientMemory {
    /// What this ingredient is known to cost. Unmeasured heap counts as zero
    /// rather than being guessed at, which is why a report prints this against
    /// RSS instead of presenting it as the whole.
    fn bytes(&self) -> usize {
        self.stack_bytes + self.metadata_bytes + self.heap_bytes.unwrap_or(0)
    }
}

/// What one loaded analysis is holding, ingredient by ingredient.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProjectMemoryReport {
    pub project: String,
    /// Resident set of the whole process — the daemon, not this project alone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rss_kib: Option<u64>,
    pub ingredients: usize,
    /// How many of them report a heap size at all. The rest are counted, not
    /// weighed.
    pub ingredients_reporting_heap: usize,
    pub instances: usize,
    pub accounted_bytes: usize,
    /// Sorted by known bytes, largest first; truncated to the caller's `top`.
    pub rows: Vec<IngredientMemory>,
    /// The whole process, not this project: where every byte it holds goes.
    /// Absent where `/proc` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessPortrait>,
}

/// The daemon's memory as lines that add up to what it holds (`rss + swap`).
///
/// # How the lines are cut
///
/// Top-down, each line a part of the one above it that can be *measured*
/// rather than inferred:
///
/// ```text
/// held = file mappings                       (smaps: Rss − Anonymous)
///      + anonymous, of which
///          outside the allocator             (anonymous + swap − arenas held − mmapped chunks)
///          free in the allocator             (arenas held − in use in arenas)
///          in use, of which
///              outside the Rust heap         (in use − census chunks: C/C++ allocations)
///              allocator overhead            (census chunks − census requested)
///              the Rust heap, requested, of which
///                  salsa, every loaded project   (ingredient rows, as in `rows`)
///                  rust-analyzer's interners     (`intern::interner_memory`)
///                  unattributed                  (the rest, named as such)
/// ```
///
/// The last line is the finding, not a rounding error: it is heap in use whose
/// owner nothing measures yet — salsa memos without a `heap_size`, the search
/// index, the embedder.
///
/// The two lines between "in use" and the Rust heap come from the heap census
/// ([`crate::mcp::heap_census`]): `mallinfo2` counts chunks, every owner below
/// counts requested bytes, and before the census the difference — glibc's
/// headers and rounding, ≈ 10 % of a rust-analyzer heap made of millions of
/// small nodes — sat in "unattributed" as if some owner were unmeasured.
/// Without a census (a binary that does not install it) those two lines are
/// absent and the cut is as before.
///
/// "Arenas held" is the kernel's count over the arena mappings
/// ([`crate::mcp::memory::ArenaResidency`]), not `mallinfo2`'s: glibc keeps
/// pages a trim returned in its own totals, and cutting from those put 6.1 GiB
/// the daemon no longer held into "free" and the same amount, negative, into
/// "outside".
///
/// Without allocator statistics (the `mimalloc` build) the middle splits
/// disappear and the unattributed line is everything anonymous that salsa and
/// the interners do not explain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProcessPortrait {
    pub mappings: crate::mcp::memory::ProcessMappings,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allocator: Option<crate::mcp::memory::AllocatorUsage>,
    /// What of `allocator.arena_bytes` the kernel actually holds; present
    /// exactly when `allocator` is.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arenas: Option<crate::mcp::memory::ArenaResidency>,
    /// The Rust heap as the global allocator counted it; absent in a binary
    /// that does not install the census.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub census: Option<crate::mcp::heap_census::HeapCensus>,
    /// Loaded analyses whose salsa ingredients are in the `salsa` line.
    pub salsa_projects: usize,
    pub lines: Vec<PortraitLine>,
    /// `rss + swap` in bytes; `lines` add up to exactly this.
    pub held_bytes: i64,
    /// Largest first, truncated to the caller's `top`.
    pub interners: Vec<InternerRow>,
    /// Largest first, truncated to the caller's `top`.
    pub largest_mappings: Vec<crate::mcp::memory::MappingUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PortraitLine {
    pub owner: &'static str,
    pub bytes: i64,
    pub what: &'static str,
}

/// One process-global interner of rust-analyzer; see `intern::InternerMemory`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InternerRow {
    pub name: &'static str,
    pub gc: bool,
    pub count: usize,
    pub table_bytes: usize,
    pub value_bytes: usize,
}

/// Owner names of the portrait lines, shared with the test that reads them.
const LINE_FILE: &str = "file_mappings";
const LINE_OUTSIDE: &str = "anonymous_outside_allocator";
const LINE_FREE: &str = "allocator_free";
const LINE_FOREIGN: &str = "outside_rust_heap";
const LINE_OVERHEAD: &str = "allocator_overhead";
const LINE_SALSA: &str = "salsa";
const LINE_INTERNERS: &str = "interners";
const LINE_UNATTRIBUTED: &str = "unattributed";

/// Cut the process's memory into lines; see [`ProcessPortrait`].
///
/// `salsa_bytes` is the sum of [`IngredientMemory::bytes`] over every loaded
/// analysis — the process portrait has to see all of them, not the one the
/// rows describe.
fn process_portrait(
    salsa_projects: usize,
    salsa_bytes: usize,
    top: usize,
) -> Option<ProcessPortrait> {
    use crate::mcp::memory;

    let mappings = memory::smaps_rollup()?;
    let allocator = memory::allocator_usage();
    let census = crate::mcp::heap_census::heap_census();
    let interner_rows = ra_ap_intern::interner_memory();
    let interner_bytes: usize = interner_rows.iter().map(|row| row.bytes()).sum();

    let kib = |kib: u64| (kib * 1024) as i64;
    let held_anonymous = kib(mappings.anonymous_kib + mappings.swap_kib);
    let salsa = salsa_bytes as i64;
    let interners = interner_bytes as i64;

    let mut lines = vec![PortraitLine {
        owner: LINE_FILE,
        bytes: kib(mappings.file_kib()),
        what: "resident file and shared mappings: the binary and its libraries, mapped model \
               and index files",
    }];
    // Read only with allocator statistics: without them there is no in-use
    // figure to subtract from the arenas.
    let arenas = match allocator {
        Some(_) => Some(memory::arena_residency()?),
        None => None,
    };
    let in_use = match (allocator, arenas) {
        (Some(allocator), Some(arenas)) => {
            let arena_held = kib(arenas.held_kib);
            // Chunks with an `mmap` of their own are outside the arenas and are
            // in use for as long as they exist, so resident in full.
            let mmapped = allocator.mmapped_bytes as i64;
            lines.push(PortraitLine {
                owner: LINE_OUTSIDE,
                bytes: held_anonymous - arena_held - mmapped,
                what: "anonymous memory (resident or swapped) outside the allocator's arenas and \
                       chunks: thread stacks, runtimes mapping their own memory",
            });
            lines.push(PortraitLine {
                owner: LINE_FREE,
                bytes: arena_held - (allocator.in_use_bytes as i64 - mmapped),
                what: "resident or swapped arena pages holding no live chunk: fragmentation a \
                       trim has not returned (negative if live chunks hold untouched pages)",
            });
            allocator.in_use_bytes as i64
        }
        _ => held_anonymous,
    };
    // What the Rust heap requested, once the allocator's view is cut down to
    // it; see the census lines in [`ProcessPortrait`].
    let rust_heap = match (census, census.and_then(|census| census.chunk_bytes)) {
        (Some(census), Some(chunks)) if allocator.is_some() => {
            lines.push(PortraitLine {
                owner: LINE_FOREIGN,
                bytes: in_use - chunks,
                what: "heap in use that Rust's allocator did not hand out: C and C++ libraries \
                       (ONNX Runtime, MIGraphX), plus chunks parked in glibc's per-thread caches",
            });
            lines.push(PortraitLine {
                owner: LINE_OVERHEAD,
                bytes: chunks - census.requested_bytes,
                what: "glibc's chunk headers and size rounding over the Rust heap: 8 bytes a \
                       chunk, rounded to 16, at least 32",
            });
            census.requested_bytes
        }
        (Some(census), _) if allocator.is_none() => {
            lines.push(PortraitLine {
                owner: LINE_FOREIGN,
                bytes: in_use - census.requested_bytes,
                what: "anonymous memory beyond what the Rust heap requested: the allocator's \
                       free pages, metadata and rounding, thread stacks, C and C++ allocations",
            });
            census.requested_bytes
        }
        _ => in_use,
    };
    lines.push(PortraitLine {
        owner: LINE_SALSA,
        bytes: salsa,
        what: "salsa ingredients of every loaded analysis: stack, metadata and measured heap",
    });
    lines.push(PortraitLine {
        owner: LINE_INTERNERS,
        bytes: interners,
        what: "rust-analyzer's process-global interners: trait-solver types, Symbol, ModPath",
    });
    lines.push(PortraitLine {
        owner: LINE_UNATTRIBUTED,
        bytes: rust_heap - salsa - interners,
        what: if census.is_some() {
            "Rust heap whose owner is not measured: salsa memos without heap_size, the search \
             index, the graph, the project model"
        } else if allocator.is_some() {
            "heap in use whose owner is not measured: salsa memos without heap_size, the \
             search index, the embedder, the graph"
        } else {
            "anonymous memory salsa and the interners do not explain (no allocator statistics \
             in this build, so fragmentation is in here too)"
        },
    });

    let mut interners: Vec<InternerRow> = interner_rows
        .into_iter()
        .map(|row| InternerRow {
            name: row.name,
            gc: row.gc,
            count: row.count,
            table_bytes: row.table_bytes,
            value_bytes: row.value_bytes,
        })
        .collect();
    if top > 0 {
        interners.truncate(top);
    }

    Some(ProcessPortrait {
        held_bytes: kib(mappings.held_kib()),
        mappings,
        allocator,
        arenas,
        census,
        salsa_projects,
        lines,
        interners,
        largest_mappings: memory::largest_mappings(if top > 0 { top } else { usize::MAX }),
    })
}

/// Flatten salsa's two families — interned/tracked structs and query memo
/// tables — into one list, sorted by what it can see.
///
/// Takes the host by `&mut`: since salsa 0.28.5 measuring memo storage needs
/// exclusive access, and `memory_usage` cancels and waits out every other
/// handle the way a write does. The analysis thread owns the host, so no other
/// handle is alive on it when this runs.
fn ingredient_rows(host: &mut AnalysisHost) -> Vec<IngredientMemory> {
    use ra_ap_ide_db::base_db::salsa;

    // `memory_usage` is implemented on `dyn Database`, not on the trait, so the
    // unsize coercion here is required rather than stylistic.
    let db: &mut dyn salsa::Database = host.raw_database_mut();
    let info = db.memory_usage();

    let mut rows: Vec<IngredientMemory> = info
        .structs
        .iter()
        .map(|ingredient| IngredientMemory {
            family: "struct",
            name: ingredient.debug_name(),
            produces: None,
            count: ingredient.count(),
            stack_bytes: ingredient.size_of_fields(),
            metadata_bytes: ingredient.size_of_metadata(),
            heap_bytes: ingredient.heap_size_of_fields(),
            page: ingredient.page_info().map(|page| format!("{page:?}")),
        })
        .chain(info.queries.iter().map(|(query, ingredient)| IngredientMemory {
            family: "query",
            name: query,
            produces: Some(ingredient.debug_name()),
            count: ingredient.count(),
            stack_bytes: ingredient.size_of_fields(),
            metadata_bytes: ingredient.size_of_metadata(),
            heap_bytes: ingredient.heap_size_of_fields(),
            page: None,
        }))
        .collect();
    rows.sort_by(|a, b| b.bytes().cmp(&a.bytes()).then(b.count.cmp(&a.count)));
    rows
}

/// Push the current contents of `paths` into an already-loaded analysis.
///
/// Fails rather than skips when a file cannot be mapped into the analysis: the
/// caller turns that into a full reload. Silently leaving one file at its old
/// revision would reproduce the very defect this module now guards against,
/// only harder to notice.
/// A context's `Vfs` for reading. A poisoned lock is stepped over: the writer
/// is a refresh, which writes path by path a table that stays consistent at
/// every step.
fn read_vfs(vfs: &RwLock<Vfs>) -> RwLockReadGuard<'_, Vfs> {
    vfs.read().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn shared_vfs(vfs: Vfs) -> Arc<RwLock<Vfs>> {
    Arc::new(RwLock::new(vfs))
}

fn apply_edits(ctx: &mut ProjectContext, paths: &[PathBuf]) -> Result<()> {
    let mut updates = Vec::with_capacity(paths.len());

    // Read everything before mutating anything: a change applied halfway
    // would leave the database describing a mixture of two revisions.
    {
        let vfs = read_vfs(&ctx.vfs);
        for path in paths {
            let vfs_path = VfsPath::new_real_path(path.to_string_lossy().into_owned());
            let (file_id, excluded) = vfs.file_id(&vfs_path).ok_or_else(|| {
                anyhow::anyhow!("{} is not part of the loaded analysis", path.display())
            })?;
            if excluded == FileExcluded::Yes {
                anyhow::bail!("{} is excluded from the loaded analysis", path.display());
            }
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            updates.push((vfs_path, file_id, text));
        }
    }

    let mut change = ChangeWithProcMacros::default();
    for (_, file_id, text) in &updates {
        change.change_file(*file_id, Some(text.clone()));
    }
    // The database first: `apply_change` cancels the queries still running on
    // snapshots of this project and waits for them to drop, so the `Vfs`
    // they read beside their snapshot is free to write a moment later (the
    // slot is held, no new query starts in between).
    ctx.host.apply_change(change);
    let mut vfs = ctx.vfs.write().unwrap_or_else(|poisoned| poisoned.into_inner());
    for (vfs_path, _, text) in updates {
        vfs.set_file_contents(vfs_path, Some(text.into_bytes()));
    }

    Ok(())
}

/// How many project contexts may stay loaded at once. `0` means unlimited.
///
/// Derived rather than guessed. A freshly started daemon costs ~2.3 GB before
/// it loads anything (ONNX runtime, embedding model, GPU probe); one
/// `Fast`-loaded workspace of ~4000 files adds ~3 GB. Three of them come to
/// ~11.3 GB, which is what the watchdog's soft limit was once set to
/// accommodate.
///
/// ⚠ That arithmetic is out of date and is kept only as the record of where
/// the 3 came from: nothing loads `Fast` any more, and one `Full` context of a
/// large workspace alone works at 18–26 GB (2026-09-22), so the soft limit is
/// now sized for ONE such project, not three. The cap still holds for small
/// projects, where three `Full` contexts do fit.
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

/// Service for semantic code queries: one rust-analyzer context per project,
/// each behind a lock of its own.
///
/// # The defect this closes
///
/// The service used to be one `Mutex<SemanticService>` for the whole process,
/// held for the whole of every call — the load of a workspace included. A
/// `Full` load of a large workspace runs for minutes, and a reload follows every
/// file added under it, so a question about a small project queued behind the
/// load of a big one: measured 2026-10-31, `find_definition` on a small project
/// answered after 176.7 s, the time a neighbour's load took.
///
/// # Locks — two levels, taken in this order only
///
/// 1. **A project's slot** ([`ProjectSlot::context`]): held for a load, a
///    refresh, and to take a snapshot (`AnalysisHost::analysis`) — NOT for the
///    query, which runs on the snapshot with the slot let go, so queries on
///    one project run side by side: measured 2026-10-31, a `find_definition`
///    waited 470 s for a `find_references` of the same project when the query
///    held the slot. Two calls that find the project missing are serialized
///    on it — the second waits for the first one's load and then finds the
///    context ready, rather than loading it again.
/// 2. **The registry** ([`Self::registry`]): the map of slots, the use clock and
///    the cap. Held only to look up, insert, touch or remove an entry — never
///    across rust-analyzer work, never while waiting for a slot.
///
/// A slot holder may take the registry; a registry holder never *waits* for a
/// slot (eviction only `try_lock`s one), and no code waits for a second slot
/// while holding one. So the order cannot cycle. The context's `Vfs` sits
/// behind a lock of its own, taken by a query for read beside its snapshot and
/// for write only by a refresh, under the slot, after `apply_change`.
///
/// # A refresh cancels the queries in flight
///
/// A refresh (`apply_change`) or a reload under the slot cancels every
/// snapshot of the project still in use and waits for them to drop (salsa's
/// `Cancelled`): a query that began on files since edited stops and is asked
/// again on the current ones. To keep a project edited faster than a long
/// query runs from starving it, the last of [`QUERY_ATTEMPTS`] keeps the slot
/// for the whole query — no refresh can land under it.
///
/// What is unchanged lives outside: `crate::deep_stack`'s analysis gate still
/// keeps a garbage collection apart from every analysis in flight, since the
/// interner sweep is process-wide whatever lock a context has.
pub(crate) struct SemanticService {
    registry: Mutex<Registry>,
}

/// The map of contexts and what orders their eviction. Lock level 2.
struct Registry {
    projects: HashMap<PathBuf, Entry>,
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
    /// Monotonic tick, handed out on every use. Only its order matters.
    clock: u64,
}

struct Entry {
    slot: Arc<ProjectSlot>,
    /// Value of [`Registry::clock`] when this project was last asked a
    /// question. Orders eviction — see [`Registry::evict_to_capacity`].
    ///
    /// A counter rather than a `SystemTime`: eviction needs the *order* of
    /// uses, and a wall clock can step backwards (NTP, suspend) and reorder
    /// them.
    last_used: u64,
}

/// One project's context and its lock. Lock level 1.
///
/// `Arc` because a call keeps working on the slot it found after letting go of
/// the registry: a project cleared or evicted meanwhile leaves the map at once,
/// and its database goes when the last call on it ends.
struct ProjectSlot {
    /// `None` until the first load finishes, and again after a failed load or a
    /// panic inside one. Held to load, refresh and snapshot the context — not
    /// for the query on the snapshot (see [`SemanticService::with_project`]).
    context: Mutex<Option<ProjectContext>>,
    /// What `context` holds, readable without its lock — so `runtime_status`
    /// can say "loading" rather than wait for the load to end. Written only by
    /// the holder of `context`.
    loaded: AtomicU8,
    /// A panic unwound through a call working on this context. A query runs
    /// on a snapshot with the slot's lock released, so its panic poisons
    /// nothing; this is how [`SemanticService::unload_after_panic`] knows
    /// which contexts a panic interrupted. Set by [`SpoilOnPanic`], never
    /// cleared: a spoiled slot leaves the registry.
    spoiled: AtomicBool,
}

/// Marks its slot spoiled if a panic unwinds past it; see
/// [`ProjectSlot::spoiled`].
struct SpoilOnPanic<'a>(&'a ProjectSlot);

impl Drop for SpoilOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.spoiled.store(true, Ordering::SeqCst);
        }
    }
}

const SLOT_EMPTY: u8 = 0;
const SLOT_FAST: u8 = 1;
const SLOT_FULL: u8 = 2;

impl ProjectSlot {
    fn empty() -> Self {
        Self {
            context: Mutex::new(None),
            loaded: AtomicU8::new(SLOT_EMPTY),
            spoiled: AtomicBool::new(false),
        }
    }

    #[cfg(test)]
    fn holding(context: ProjectContext) -> Self {
        let slot = Self::empty();
        slot.publish(Some(context.load_kind));
        *slot.context.lock().unwrap() = Some(context);
        slot
    }

    /// Record what the slot now holds, for [`SemanticService::status`].
    fn publish(&self, kind: Option<LoadKind>) {
        let value = match kind {
            None => SLOT_EMPTY,
            Some(LoadKind::Fast) => SLOT_FAST,
            Some(LoadKind::Full) => SLOT_FULL,
        };
        self.loaded.store(value, Ordering::SeqCst);
    }

    /// Take the slot's lock. A slot poisoned by a panic is emptied first: what
    /// the panic interrupted — a load half done, a context mid-update — is
    /// unknown, so the caller loads afresh, as after a drift.
    fn lock(&self) -> MutexGuard<'_, Option<ProjectContext>> {
        match self.context.lock() {
            Ok(guard) => guard,
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                *guard = None;
                self.publish(None);
                self.context.clear_poison();
                guard
            }
        }
    }
}

/// What [`SemanticService::unload_after_panic`] found and dropped.
pub(crate) struct AfterPanic {
    /// `"<project>: <what changed>"` for each dropped context whose panic was a
    /// dependency's source changing on disk.
    pub(crate) drifted: Vec<String>,
    pub(crate) dropped: usize,
}

impl Registry {
    /// Drop the least recently used contexts until [`Self::max_projects`] holds,
    /// and hand them back so the caller drops them *after* letting go of the
    /// registry — freeing a large database takes seconds.
    ///
    /// `keep` is never evicted whatever its age: it is the project the caller is
    /// about to answer a question about, and evicting it would mean reloading it
    /// on the very next line. With a cap of 1 that is the whole rule — the
    /// working project stays, everything else goes.
    ///
    /// A context another call is using right now is not a victim either: it was
    /// just asked a question, whatever its clock says, and evicting it would
    /// throw away a load the moment it finishes. If only such contexts are
    /// left, the cap is exceeded until the next use evicts again.
    fn evict_to_capacity(&mut self, keep: &Path) -> Vec<Arc<ProjectSlot>> {
        let mut evicted = Vec::new();
        if self.max_projects == 0 {
            return evicted;
        }

        while self.projects.len() > self.max_projects {
            let victim = self
                .projects
                .iter()
                .filter(|(path, _)| path.as_path() != keep)
                .filter(|(_, entry)| !matches!(entry.slot.context.try_lock(), Err(TryLockError::WouldBlock)))
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(path, _)| path.clone());

            let Some(victim) = victim else { break };

            tracing::info!(
                "unloading {} to stay within {}={} loaded project(s)",
                victim.display(),
                MAX_PROJECTS_ENV,
                self.max_projects
            );
            evicted.push(self.projects.remove(&victim).unwrap().slot);
        }
        evicted
    }

    fn slots(&self) -> Vec<(PathBuf, Arc<ProjectSlot>)> {
        self.projects
            .iter()
            .map(|(path, entry)| (path.clone(), Arc::clone(&entry.slot)))
            .collect()
    }

    /// Remove `path` only if it still maps to `slot`: a clear and a fresh call
    /// may have replaced it while the caller worked.
    fn remove_if_same(&mut self, path: &Path, slot: &ProjectSlot) -> Option<Entry> {
        let same = self
            .projects
            .get(path)
            .is_some_and(|entry| std::ptr::eq(Arc::as_ptr(&entry.slot), slot));
        if same { self.projects.remove(path) } else { None }
    }
}

/// How many times a query is asked before it keeps the slot for its whole run;
/// see "A refresh cancels the queries in flight" on [`SemanticService`].
const QUERY_ATTEMPTS: usize = 3;

/// Whether a query's failure is salsa's cancellation — a refresh landed under
/// its snapshot — carried as an error (`Cancellable` through `?`), not as the
/// unwind [`Cancelled::catch`] stops.
fn is_cancellation(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| cause.is::<Cancelled>())
}

/// Load `canonical` into its slot, or refresh what is there. The caller holds
/// the slot's lock.
fn bring_up_to_date(
    slot: &ProjectSlot,
    context: &mut Option<ProjectContext>,
    canonical: &Path,
    requested: LoadKind,
) -> Result<()> {
    let needs_load = match context {
        Some(ctx) => requested == LoadKind::Full && ctx.load_kind == LoadKind::Fast,
        None => true,
    };
    if needs_load {
        load_into(slot, context, canonical, requested)
    } else {
        // Cached — but "cached" said nothing about "current" until this call
        // existed. See [`refresh_if_stale`].
        refresh_if_stale(slot, context, canonical, requested)
    }
}

fn load_into(
    slot: &ProjectSlot,
    context: &mut Option<ProjectContext>,
    canonical: &Path,
    requested: LoadKind,
) -> Result<()> {
    tracing::info!(
        "Loading {:?} IDE for project: {}",
        requested,
        canonical.display()
    );
    // Its own `mem:` line, so a load is an interval with a project in the log:
    // loads of different projects run side by side since the per-project
    // slots, and the "Loading …" / "IDE loaded successfully" pair no longer
    // says which load ended. `--tool-latency` reads it to tell a query's own
    // load (cost) from another project's (it must not be waited for).
    let mem = rmc_engine::mem_trace::MemSpan::start(format!(
        "analysis load {:?} of {}",
        requested,
        canonical.display()
    ));
    // Stamps are taken BEFORE the load, not after: a file edited while
    // rust-analyzer was reading the workspace must come out stale on the
    // next query, not silently accepted as already analysed.
    let stamps = collect_stamps(canonical);
    // The context being replaced (a reload after a structural change, or
    // a Fast → Full upgrade) goes BEFORE the new one is built. Left in
    // the slot until the new one is stored, it kept a whole database alive
    // beside the one being loaded: on salsa-bur a reload peaked +797 MiB over
    // the 732 MiB held, on a ~49 000-file workspace ≈ 8 → 11 GiB
    // (`reload_peak_holds_one_database`).
    // A failed load then leaves the project unloaded rather than stale;
    // the next call loads it again, which a stale context needed anyway.
    //
    // Dropping alone is half of it: the freed database sits in the arenas
    // of the threads that built it, and the load allocates from the arenas
    // of its own priming threads, so glibc does not reuse it (+597 MiB
    // instead of +797 on salsa-bur). The release hands it back first.
    //
    // Queries may still run on snapshots of the old database, with the slot
    // let go: a snapshot keeps the database alive. Cancelling them first (and
    // waiting for their snapshots to drop) is what makes the drop free it.
    slot.publish(None);
    if let Some(mut previous) = context.take() {
        previous.host.trigger_cancellation();
        drop(previous);
        crate::mcp::memory::release_free_memory();
    }
    let (host, vfs) = match requested {
        LoadKind::Fast => loader::load_project(canonical)?,
        LoadKind::Full => loader::load_project_full(canonical)?,
    };
    *context = Some(ProjectContext {
        host,
        vfs: shared_vfs(vfs),
        load_kind: requested,
        stamps,
    });
    slot.publish(Some(requested));
    mem.finish();
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
fn refresh_if_stale(
    slot: &ProjectSlot,
    context: &mut Option<ProjectContext>,
    canonical: &Path,
    requested: LoadKind,
) -> Result<()> {
    let current = collect_stamps(canonical);
    let ctx = context
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("Project not loaded"))?;

    match classify_staleness(&ctx.stamps, &current) {
        Staleness::Fresh => Ok(()),
        Staleness::StructureChanged => {
            tracing::info!(
                "Project {} gained or lost files since it was analysed; reloading",
                canonical.display()
            );
            load_into(slot, context, canonical, requested)
        }
        Staleness::Edited(paths) => {
            match apply_edits(ctx, &paths).map(|()| ctx.stamps = current) {
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
                    load_into(slot, context, canonical, requested)
                }
            }
        }
    }
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
            registry: Mutex::new(Registry {
                projects: HashMap::new(),
                max_projects,
                clock: 0,
            }),
        }
    }

    /// The registry, stepping over poison: every change under it is a single
    /// map operation, so a panic cannot leave it half-updated in a way a later
    /// call could trip on.
    fn registry(&self) -> MutexGuard<'_, Registry> {
        self.registry.lock().unwrap_or_else(|poisoned| {
            self.registry.clear_poison();
            poisoned.into_inner()
        })
    }

    pub(crate) fn project_count(&self) -> usize {
        self.registry().projects.len()
    }

    pub(crate) fn status(&self) -> SemanticServiceStatus {
        let mut projects = self
            .registry()
            .projects
            .iter()
            .map(|(path, entry)| SemanticProjectStatus {
                path: path.display().to_string(),
                load_kind: match entry.slot.loaded.load(Ordering::SeqCst) {
                    SLOT_FAST => LoadKind::Fast.as_str(),
                    SLOT_FULL => LoadKind::Full.as_str(),
                    _ => "loading",
                }
                .to_string(),
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

    /// Forget every context. A call still working on one keeps it until it
    /// answers; the memory goes when that call ends.
    pub(crate) fn clear_all(&self) -> usize {
        let taken = std::mem::take(&mut self.registry().projects);
        // Counted and dropped after the registry is let go: freeing a large
        // database takes seconds, and `status` must not wait for it.
        let count = taken.len();
        drop(taken);
        count
    }

    /// Clean up after a panic inside semantic work; see `deep_stack::with_semantic`.
    ///
    /// The panic unwound through the slot it happened under and spoiled it
    /// ([`ProjectSlot::spoiled`]; a query on a snapshot holds no lock to
    /// poison) or poisoned it (a load or refresh), so those slots are exactly the
    /// contexts the panic interrupted — and only those are dropped: a panic on
    /// one project no longer costs every other project its load. A slot
    /// another call holds right now is busy, not spoiled, and is left alone;
    /// calls still on a dropped one keep it until they answer.
    ///
    /// A dropped context whose database read a dependency's source back from
    /// disk and found it changed since the load (see `loader::file_text_store`)
    /// is reported as a drift, `"<project>: <what changed>"`: such a context can
    /// answer nothing that touches that file, nothing inside rust-analyzer can
    /// repair an input mid-query, and the cure is the next call loading the
    /// workspace afresh against the files as they are now.
    ///
    /// No slot poisoned means the panic was outside every context, and what it
    /// interrupted is unknown — then every context goes, as before contexts
    /// had locks of their own.
    pub(crate) fn unload_after_panic(&self) -> AfterPanic {
        let slots = self.registry().slots();
        let mut drifted = Vec::new();
        let mut poisoned = Vec::new();
        let drift_of = |context: &Option<ProjectContext>| {
            context
                .as_ref()
                .and_then(|ctx| ctx.host.raw_database().file_text_drift())
        };
        for (path, slot) in slots {
            let spoiled = slot.spoiled.load(Ordering::SeqCst);
            let drift = match slot.context.try_lock() {
                Err(TryLockError::Poisoned(guard)) => Some(drift_of(guard.get_ref())),
                Ok(guard) if spoiled => Some(drift_of(&guard)),
                // Spoiled, and a load is under way on it: what drifted is
                // unknown, the slot goes all the same.
                Err(TryLockError::WouldBlock) if spoiled => Some(None),
                _ => None,
            };
            if let Some(drift) = drift {
                if let Some(drift) = drift {
                    drifted.push(format!("{}: {drift}", path.display()));
                }
                poisoned.push((path, slot));
            }
        }

        if poisoned.is_empty() {
            return AfterPanic {
                drifted,
                dropped: self.clear_all(),
            };
        }
        let removed: Vec<Entry> = {
            let mut registry = self.registry();
            poisoned
                .iter()
                .filter_map(|(path, slot)| registry.remove_if_same(path, slot))
                .collect()
        };
        AfterPanic {
            drifted,
            dropped: removed.len(),
        }
    }

    /// A context built by hand, for a test that needs a database no workspace
    /// on disk would produce. The path is taken as given, not canonicalized.
    #[cfg(test)]
    pub(crate) fn insert_for_test(&self, path: PathBuf, host: AnalysisHost) {
        self.insert_context_for_test(
            path,
            ProjectContext {
                host,
                vfs: shared_vfs(Vfs::default()),
                load_kind: LoadKind::Full,
                stamps: HashMap::new(),
            },
        );
    }

    #[cfg(test)]
    fn insert_context_for_test(&self, path: PathBuf, context: ProjectContext) {
        let mut registry = self.registry();
        registry.clock += 1;
        let last_used = registry.clock;
        registry.projects.insert(
            path,
            Entry {
                slot: Arc::new(ProjectSlot::holding(context)),
                last_used,
            },
        );
    }

    /// Run `work` on the loaded context of `path` (exactly as given), under its
    /// slot lock. Panics if nothing is loaded there.
    #[cfg(test)]
    fn with_context_for_test<T>(
        &self,
        path: &Path,
        work: impl FnOnce(&mut ProjectContext) -> T,
    ) -> T {
        let slot = Arc::clone(
            &self
                .registry()
                .projects
                .get(path)
                .unwrap_or_else(|| panic!("no context is loaded for {}", path.display()))
                .slot,
        );
        let _spoil = SpoilOnPanic(&slot);
        let mut guard = slot.lock();
        work(guard.as_mut().expect("the context is loaded"))
    }

    /// [`Self::with_context_for_test`] for callers outside this module.
    #[cfg(test)]
    pub(crate) fn with_host_for_test<T>(
        &self,
        path: &Path,
        work: impl FnOnce(&mut AnalysisHost) -> T,
    ) -> T {
        self.with_context_for_test(path, |ctx| work(&mut ctx.host))
    }

    /// Hold `path`'s slot for as long as `work` runs, the way a load or a
    /// refresh holds it — for the tests that judge who waits for whom. The
    /// project must be loaded and unchanged on disk, so nothing is loaded here.
    #[cfg(test)]
    pub(crate) fn hold_project_for_test<T>(
        &self,
        path: &Path,
        work: impl FnOnce() -> T,
    ) -> Result<T> {
        let canonical = path.canonicalize()?;
        let slot = self.slot_for(&canonical);
        let _context = self.up_to_date(&canonical, &slot, LoadKind::Full)?;
        Ok(work())
    }

    /// Run `work` the way a query runs — through [`Self::with_project`], on a
    /// snapshot, asked again when cancelled — for the tests that judge who
    /// waits for whom.
    #[cfg(test)]
    pub(crate) fn query_project_for_test<T>(
        &self,
        path: &Path,
        work: impl Fn() -> Result<T>,
    ) -> Result<T> {
        self.with_project(path, LoadKind::Full, |_, _, _| work())
    }

    /// Whether some call holds `path`'s slot right now.
    #[cfg(test)]
    pub(crate) fn slot_is_held_for_test(&self, path: &Path) -> bool {
        let slot = Arc::clone(&self.registry().projects.get(path).expect("loaded").slot);
        matches!(slot.context.try_lock(), Err(TryLockError::WouldBlock))
    }

    /// Projects in the registry, sorted.
    #[cfg(test)]
    pub(crate) fn loaded_paths_for_test(&self) -> Vec<PathBuf> {
        let mut paths: Vec<_> = self.registry().projects.keys().cloned().collect();
        paths.sort();
        paths
    }

    pub(crate) fn clear_project(&self, project_path: &Path) -> usize {
        let canonical =
            std::fs::canonicalize(project_path).unwrap_or_else(|_| project_path.to_path_buf());
        let removed = self.registry().projects.remove(&canonical);
        // Dropped here, after the registry is let go.
        usize::from(removed.is_some())
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
    /// type it has not recorded anywhere yet. That is the gate in
    /// [`crate::deep_stack`], which the caller holds exclusively for this call
    /// (see `RuntimeState::collect_garbage`): every analysis — this service's
    /// and the graph tools', audits' and skeleton builder's, which load
    /// workspaces of their own — runs inside that gate, so none is in flight
    /// here and every slot lock below is free.
    ///
    /// # Cost
    ///
    /// The sweep runs once per host, so N loaded projects pay N sweeps; the
    /// caller keeps this off the hot path (see the watchdog's interval) rather
    /// than the method trying to be clever about it. The bump itself is paid
    /// later, by the next query re-validating its memos.
    ///
    /// Returns the number of contexts collected, for the log line.
    pub(crate) fn collect_garbage(&self) -> usize {
        let slots = self.registry().slots();
        let mut collected = 0;
        for (path, slot) in slots {
            let mut guard = slot.lock();
            if let Some(ctx) = guard.as_mut() {
                tracing::debug!("collecting garbage in {}", path.display());
                ctx.host.trigger_garbage_collection();
                collected += 1;
            }
        }
        collected
    }

    /// Per-ingredient memory of one *already loaded* analysis.
    ///
    /// # Why it refuses to load the project
    ///
    /// The question this answers is "what is this daemon holding", and a daemon
    /// worth asking about has been alive for hours. Loading the project here to
    /// answer would build a fresh database and report *that* — a floor, not a
    /// portrait, and one indistinguishable from the real thing in the output.
    /// So an unloaded path is an error naming what is loaded instead.
    ///
    /// # How to read the numbers
    ///
    /// `heap_bytes` is present only for the queries whose fork declares a
    /// `heap_size` function, and it is a lower bound there (see
    /// `syntax::heap_size`). Everything else reports `count`, `stack_bytes` and
    /// `metadata_bytes` only — enough to say which memos are numerous, never
    /// how many bytes they are worth. `accounted_bytes` against RSS is printed
    /// precisely so the gap cannot be mistaken for a full budget.
    ///
    /// `process` is where that gap goes: the whole daemon cut into lines that
    /// add up to `rss + swap`, salsa of every loaded project being one of them —
    /// see [`ProcessPortrait`].
    ///
    /// # Waits
    ///
    /// Weighing a database needs its slot, so this waits for a load or a query
    /// in progress on *any* loaded project — the portrait has to see all of
    /// them. It is a diagnostic, asked rarely; serving tools never wait on it
    /// for longer than the weighing of one context.
    pub(crate) fn memory_breakdown(
        &self,
        project: &Path,
        top: usize,
    ) -> Result<ProjectMemoryReport> {
        let canonical = project
            .canonicalize()
            .with_context(|| format!("resolving {}", project.display()))?;
        let slots = self.registry().slots();
        let loaded = || -> String {
            let names: Vec<String> =
                slots.iter().map(|(path, _)| path.display().to_string()).collect();
            if names.is_empty() { "nothing".to_owned() } else { names.join(", ") }
        };
        if !slots.iter().any(|(path, _)| *path == canonical) {
            anyhow::bail!(
                "no analysis is loaded for {}; this reports what a live daemon is holding and \
                 will not load a project to answer, because a database built for the question \
                 measures a cold start rather than the daemon. Loaded: {}",
                canonical.display(),
                loaded()
            );
        }
        // Every loaded analysis is weighed, not just the one asked about: the
        // process portrait below has to see all of them, and the rows of the
        // requested one fall out of the same pass. One slot at a time.
        let mut salsa_bytes = 0;
        let mut weighed = 0;
        let mut requested = None;
        for (path, slot) in &slots {
            let mut guard = slot.lock();
            let Some(ctx) = guard.as_mut() else { continue };
            let rows = ingredient_rows(&mut ctx.host);
            weighed += 1;
            salsa_bytes += rows.iter().map(IngredientMemory::bytes).sum::<usize>();
            if *path == canonical {
                requested = Some(rows);
            }
        }
        let Some(mut rows) = requested else {
            anyhow::bail!(
                "the analysis of {} failed to load or was dropped while this waited for it; \
                 nothing to weigh. Loaded: {}",
                canonical.display(),
                loaded()
            );
        };
        let process = process_portrait(weighed, salsa_bytes, top);

        let instances = rows.iter().map(|row| row.count).sum();
        let accounted_bytes = rows.iter().map(IngredientMemory::bytes).sum();
        let ingredients = rows.len();
        let ingredients_reporting_heap = rows.iter().filter(|row| row.heap_bytes.is_some()).count();
        if top > 0 {
            rows.truncate(top);
        }

        Ok(ProjectMemoryReport {
            project: canonical.display().to_string(),
            rss_kib: crate::mcp::memory::rss_kib(),
            ingredients,
            ingredients_reporting_heap,
            instances,
            accounted_bytes,
            rows,
            process,
        })
    }

    /// The slot of `canonical`, made if the project has none yet. A new entry
    /// counts as used now, so a load in progress is not the first victim of
    /// another project's eviction.
    fn slot_for(&self, canonical: &Path) -> Arc<ProjectSlot> {
        let mut registry = self.registry();
        registry.clock += 1;
        let clock = registry.clock;
        let entry = registry
            .projects
            .entry(canonical.to_path_buf())
            .or_insert_with(|| Entry {
                slot: Arc::new(ProjectSlot::empty()),
                last_used: clock,
            });
        Arc::clone(&entry.slot)
    }

    /// Load or refresh `project_path` to `requested`, then run `work` on a
    /// snapshot of it — under that project's slot lock and nothing else, so
    /// the load and the query of one project never hold up another's.
    ///
    /// The slot is held to bring the context up to date and take the snapshot
    /// (`AnalysisHost::analysis`), then let go: `work` runs on the snapshot
    /// beside the other queries of the project. A refresh that lands meanwhile
    /// cancels it (salsa's `Cancelled`, as a panic or as an error); it is asked
    /// again on the refreshed context, and the last of [`QUERY_ATTEMPTS`]
    /// keeps the slot for its whole run.
    ///
    /// The one door every served path and every probe goes through: the lock
    /// order in [`SemanticService`]'s header holds because nothing else takes
    /// a slot for rust-analyzer work.
    fn with_project<T>(
        &self,
        project_path: &Path,
        requested: LoadKind,
        work: impl Fn(&Path, &Analysis, &Vfs) -> Result<T>,
    ) -> Result<T> {
        let canonical = project_path.canonicalize()?;
        let slot = self.slot_for(&canonical);
        let _spoil = SpoilOnPanic(&slot);
        for attempt in 1..=QUERY_ATTEMPTS {
            let context = self.up_to_date(&canonical, &slot, requested)?;
            let ctx = context.as_ref().unwrap();
            let analysis = ctx.host.analysis();
            let vfs = Arc::clone(&ctx.vfs);
            let last = attempt == QUERY_ATTEMPTS;
            // Kept only on the last attempt: then no refresh can cancel it.
            let _held = if last {
                Some(context)
            } else {
                drop(context);
                None
            };
            let outcome = {
                let vfs = read_vfs(&vfs);
                Cancelled::catch(std::panic::AssertUnwindSafe(|| {
                    work(&canonical, &analysis, &vfs)
                }))
            };
            match outcome {
                Ok(Err(error)) if !last && is_cancellation(&error) => {}
                Ok(answer) => return answer,
                Err(_) if !last => {}
                Err(cancelled) => return Err(cancelled.into()),
            }
            tracing::info!(
                "a refresh of {} cancelled a query on it (attempt {attempt} of {QUERY_ATTEMPTS}); asking again",
                canonical.display()
            );
        }
        unreachable!("the last attempt returns")
    }

    /// The slot of `canonical`, locked, holding a context loaded to
    /// `requested` and current with the tree — loaded or refreshed here if it
    /// was not. Orders the use against the cap and evicts.
    fn up_to_date<'s>(
        &self,
        canonical: &Path,
        slot: &'s ProjectSlot,
        requested: LoadKind,
    ) -> Result<MutexGuard<'s, Option<ProjectContext>>> {
        let mut context = slot.lock();
        let outcome = bring_up_to_date(slot, &mut context, canonical, requested);
        let evicted = {
            let mut registry = self.registry();
            match outcome {
                // Only after it succeeded: a load that failed left no context to
                // order, and a query that never happened is not a use.
                Ok(()) => {
                    registry.clock += 1;
                    let clock = registry.clock;
                    match registry.projects.get_mut(canonical) {
                        Some(entry) if std::ptr::eq(Arc::as_ptr(&entry.slot), slot) => {
                            entry.last_used = clock;
                            registry.evict_to_capacity(canonical)
                        }
                        // Cleared while this call worked: it still answers, but
                        // the context goes with it rather than back in the map.
                        _ => Vec::new(),
                    }
                }
                // A failed load leaves the project unloaded rather than stale —
                // and out of the map, so `status` does not show it loading.
                Err(_) => {
                    if context.is_none() {
                        registry.remove_if_same(canonical, slot);
                    }
                    Vec::new()
                }
            }
        };
        // After the registry is let go: freeing a large database takes seconds.
        drop(evicted);
        outcome?;
        Ok(context)
    }

    /// Get or load project with full workspace dependency edges.
    ///
    /// The only entry point left: `find_definition` was the last served path on
    /// a `Fast` context and moved off it, because "which context am I on"
    /// decided whether it could see `#[cfg(test)]` and that depended on call
    /// order. `LoadKind::Fast` stays for the upgrade machinery it is the source
    /// half of — nothing requests it any more.
    #[cfg(test)]
    fn get_or_load_full(&self, project_path: &Path) -> Result<()> {
        self.with_project(project_path, LoadKind::Full, |_, _, _| Ok(()))
    }

    /// Search for symbols by name (for find_definition)
    pub(crate) fn symbol_search(
        &self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
    ) -> Result<Vec<Location>> {
        self.symbol_search_with_exact(project_path, symbol_name, limit, false)
    }

    /// Search for symbols by name with optional full-name filtering.
    ///
    /// # Why this needs the same `Full` context references do
    ///
    /// A declaration lives in exactly one crate, so this one never needed the
    /// dependency edges — and it ran on `Fast` for that reason. What that
    /// overlooked is that `Fast` also has `set_test` off, which takes every
    /// `#[cfg(test)]` module out of the tree: "where is this test declared"
    /// answered *no definition found* for a test that exists.
    ///
    /// And it did so **only sometimes**. `get_or_load` does not downgrade a
    /// context already loaded as `Full`, so after any `find_references` call in
    /// the same session this one silently got the full tree and started finding
    /// what it had just denied. Measured 2026-08-28 on a 4000-file workspace:
    /// the same query, the same session, answered "no definition found" and then
    /// the file and line. An answer that depends on which questions came before
    /// it is worse than a blind spot — a blind spot at least reproduces.
    ///
    /// The load is shared, not doubled: `find_references` pays for the same
    /// upgrade, and whichever runs first pays it once.
    pub(crate) fn symbol_search_with_exact(
        &self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
        exact: bool,
    ) -> Result<Vec<Location>> {
        self.with_project(project_path, LoadKind::Full, |canonical, analysis, vfs| {
            position::symbol_search_with_exact(
                analysis,
                vfs,
                canonical,
                symbol_name,
                limit,
                exact,
            )
        })
    }

    /// The symbol index's own answer, with the source sweep not run.
    ///
    /// Only the survey of "which kinds does the index carry" uses this; the
    /// served path always keeps the sweep behind it. The context is the same
    /// `Full` one the served path loads, so the survey measures the index the
    /// tool actually queries.
    #[cfg(test)]
    pub(crate) fn index_only_search(
        &self,
        project_path: &Path,
        symbol_name: &str,
        limit: usize,
    ) -> Result<Vec<Location>> {
        self.with_project(project_path, LoadKind::Full, |_, analysis, vfs| {
            position::index_only_search(analysis, vfs, symbol_name, limit)
        })
    }

    /// Find all references to symbols matching a name
    /// First finds all symbols matching the name, then finds references for each
    pub(crate) fn find_references_by_name(
        &self,
        project_path: &Path,
        symbol_name: &str,
    ) -> Result<Vec<Location>> {
        self.find_references_by_name_with_exact(project_path, symbol_name, false)
    }

    /// Find all references to symbols matching a name with optional exact filtering.
    ///
    /// # Why this needs a `Full` context
    ///
    /// A `Fast` context is loaded with `no_deps: true`, and `cargo metadata
    /// --no-deps` omits the resolve graph entirely — so rust-analyzer gets the
    /// workspace's packages with **no dependency edges between them**, and a
    /// call from one member crate into another is not a reference it can see.
    /// `set_test` is off there too, which takes every `#[cfg(test)]` call site
    /// out of the module tree along with it.
    ///
    /// Measured on a 4000-file workspace (2026-08-28): a `pub fn` with six call
    /// sites answered three — the cross-crate one and both test ones missing,
    /// with no indication that the answer was partial. "Fewer references than
    /// exist" is the one wrong answer this tool must not give, because it is
    /// read as "this code is dead".
    ///
    /// The cost lands once per project: `get_or_load_full` upgrades a cached
    /// `Fast` context in place rather than holding a second one, and `rename`
    /// already pays the same price for the same reason.
    pub(crate) fn find_references_by_name_with_exact(
        &self,
        project_path: &Path,
        symbol_name: &str,
        exact: bool,
    ) -> Result<Vec<Location>> {
        self.with_project(project_path, LoadKind::Full, |canonical, analysis, vfs| {
            position::find_references_by_name_with_exact(
                analysis,
                vfs,
                canonical,
                symbol_name,
                exact,
            )
        })
    }

    /// Preview rename of a symbol by name. Does not modify any files.
    pub(crate) fn rename_by_name(
        &self,
        project_path: &Path,
        symbol_name: &str,
        new_name: &str,
    ) -> Result<RenamePreview> {
        self.with_project(project_path, LoadKind::Full, |_, analysis, vfs| {
            rename::rename_by_name(analysis, vfs, symbol_name, new_name)
        })
    }

    /// Preview rename of a symbol at a concrete file position. Does not modify any files.
    pub(crate) fn rename_by_position(
        &self,
        project_path: &Path,
        file_path: &Path,
        line: u32,
        column: u32,
        symbol_name: &str,
        new_name: &str,
    ) -> Result<RenamePreview> {
        self.with_project(project_path, LoadKind::Full, |_, analysis, vfs| {
            rename::rename_by_position(
                analysis,
                vfs,
                file_path,
                line,
                column,
                symbol_name,
                new_name,
            )
        })
    }

    #[cfg(test)]
    pub(crate) fn insert_test_project_fast(&self, project_path: PathBuf) {
        let canonical =
            std::fs::canonicalize(&project_path).unwrap_or(project_path);
        self.insert_context_for_test(
            canonical,
            ProjectContext {
                host: AnalysisHost::new(None),
                vfs: shared_vfs(Vfs::default()),
                load_kind: LoadKind::Fast,
                stamps: HashMap::new(),
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// "I am holding a loaded analysis — do not sweep under me."
    ///
    /// This is the *production* gate, not a lock of its own: see
    /// [`crate::deep_stack::test_holding_an_analysis`] for why a second one
    /// beside it was the bug rather than the protection.
    use crate::deep_stack::test_holding_an_analysis as holding_an_analysis;

    /// "I am the sweep."
    use crate::deep_stack::test_sweeping_alone as sweeping_alone;

    #[test]
    fn rename_preview_includes_workspace_reverse_dependencies() {
        let _analysis = holding_an_analysis();
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

        let service = SemanticService::new();
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
        let service = SemanticService::new();
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
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        let lib = one_crate_workspace(root);

        let service = SemanticService::new();
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
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        let lib = one_crate_workspace(root);

        let service = SemanticService::new();
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

    /// Call sites landing in one file, named by the tail of its path.
    fn refs_in(locations: &[Location], file_suffix: &str) -> usize {
        locations
            .iter()
            .filter(|location| {
                location.name == "reference" && location.file_path.ends_with(file_suffix)
            })
            .count()
    }

    /// Two crates and a known answer, built so that each way the search used to
    /// come up short is one countable reference.
    ///
    /// `probed()` is called three times: once in ordinary code, once from a
    /// `#[cfg(test)]` module, once from the other crate. `radius_override` is a
    /// **struct field** — the symbol index does not carry those at all — read
    /// four times across the same three places.
    ///
    /// Nothing here uses `use`: every cross-crate mention is written out as
    /// `oracle_lib::…`, so an import edge cannot pad the counts and make a
    /// missing call site look present.
    fn two_crate_oracle_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[workspace]
members = ["oracle_lib", "oracle_consumer"]
resolver = "2"
"#,
        );
        write_file(
            &root.join("oracle_lib/Cargo.toml"),
            r#"
[package]
name = "oracle_lib"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        );
        write_file(
            &root.join("oracle_lib/src/lib.rs"),
            r#"
pub struct Probe {
    pub radius_override: u32,
}

impl Probe {
    pub fn read_here(&self) -> u32 {
        self.radius_override
    }
}

pub fn probed() {}

pub fn calls_here() {
    probed();
}

/// The other kind `SymbolCollector` drops on the floor, next to struct fields.
pub enum Facing {
    Downhole,
}

#[cfg(test)]
mod tests {
    #[test]
    fn touches_both() {
        let probe = super::Probe { radius_override: 1 };
        assert_eq!(probe.radius_override, 1);
        super::probed();
    }
}
"#,
        );
        write_file(
            &root.join("oracle_consumer/Cargo.toml"),
            r#"
[package]
name = "oracle_consumer"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"

[dependencies]
oracle_lib = { path = "../oracle_lib" }
"#,
        );
        write_file(
            &root.join("oracle_consumer/src/lib.rs"),
            r#"
pub fn read_across(probe: &oracle_lib::Probe) -> u32 {
    probe.radius_override
}

pub fn call_across() {
    oracle_lib::probed();
}
"#,
        );
    }

    /// The regression net for the two ways a `fn` search came up short: a
    /// workspace loaded without dependency edges cannot see the call from the
    /// other crate, and one loaded without `cfg(test)` cannot see the call from
    /// the test module. Both used to be invisible *and* silent — the answer was
    /// a shorter list, not an error.
    #[test]
    fn references_to_a_fn_include_the_other_crate_and_the_test_module() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        two_crate_oracle_workspace(root);

        let service = SemanticService::new();
        let found = service
            .find_references_by_name_with_exact(root, "probed", true)
            .expect("query references");

        assert_eq!(
            refs_in(&found, "oracle_consumer/src/lib.rs"),
            1,
            "the call from the other crate must be visible, got {found:?}"
        );
        assert_eq!(
            refs_in(&found, "oracle_lib/src/lib.rs"),
            2,
            "both the plain call and the #[cfg(test)] one must be visible, got {found:?}"
        );
        assert_eq!(
            call_sites(&found),
            3,
            "three call sites exist and three must be reported, got {found:?}"
        );
    }

    /// A struct field reaches rust-analyzer's symbol index not at all, so the
    /// name resolved to nothing and the tool answered "no references" — the
    /// same words it uses for a field nobody reads. Four reads exist here.
    #[test]
    fn references_to_a_struct_field_are_found_at_all() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        two_crate_oracle_workspace(root);

        let service = SemanticService::new();
        let found = service
            .find_references_by_name_with_exact(root, "radius_override", true)
            .expect("query references");

        assert_eq!(
            refs_in(&found, "oracle_consumer/src/lib.rs"),
            1,
            "the read from the other crate must be visible, got {found:?}"
        );
        assert_eq!(
            refs_in(&found, "oracle_lib/src/lib.rs"),
            3,
            "the impl read plus both test reads must be visible, got {found:?}"
        );
        assert!(
            found.iter().any(|location| location.name != "reference"),
            "the declaration itself must come back alongside the reads, got {found:?}"
        );
    }

    /// The fallback sweeps source text for the identifier, so it must not report
    /// a same-spelled *local* as a field read. Negative control for the sweep:
    /// without the "what did this actually resolve to" check, this reads 2.
    #[test]
    fn a_local_binding_spelled_like_the_field_is_not_counted() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        two_crate_oracle_workspace(root);
        write_file(
            &root.join("oracle_consumer/src/lib.rs"),
            r#"
pub fn read_across(probe: &oracle_lib::Probe) -> u32 {
    probe.radius_override
}

pub fn a_local_of_the_same_name() -> u32 {
    // Same spelling, unrelated binding: not a read of the field.
    let radius_override = 7;
    radius_override
}
"#,
        );

        let service = SemanticService::new();
        let found = service
            .find_references_by_name_with_exact(root, "radius_override", true)
            .expect("query references");

        assert_eq!(
            refs_in(&found, "oracle_consumer/src/lib.rs"),
            1,
            "only the field read counts, not the local spelled like it, got {found:?}"
        );
    }

    /// `find_definition` resolves through the same symbol index that carries no
    /// struct fields, so "where is this field declared" answered *no definition
    /// found* — the words a misspelled name gets, and the words that read as
    /// "there is no such field". The declaration is on line 2 of the lib
    /// (`write_file` trims the fixture's leading newline).
    #[test]
    fn definition_of_a_struct_field_is_found_at_all() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        two_crate_oracle_workspace(root);

        let service = SemanticService::new();
        let found = service
            .symbol_search_with_exact(root, "radius_override", 8, true)
            .expect("query definition");

        assert_eq!(
            found.len(),
            1,
            "the field is declared exactly once, got {found:?}"
        );
        assert!(
            found[0].file_path.ends_with("oracle_lib/src/lib.rs"),
            "the declaring crate's lib must be the answer, got {found:?}"
        );
        assert_eq!(
            found[0].line, 2,
            "the declaration line, not one of the reads, got {found:?}"
        );
    }

    /// Enum variants were read as sharing the field's fate — absent from the
    /// symbol index. They do not: disabling the sweep leaves this test green,
    /// so the index carries them and the index is what answers. Kept as the
    /// regression that says which half of the guess held, and as the net under
    /// the ranking and dedup the field fix put in this path.
    #[test]
    fn definition_of_an_enum_variant_is_found_at_all() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        two_crate_oracle_workspace(root);

        let service = SemanticService::new();
        let found = service
            .symbol_search_with_exact(root, "Downhole", 8, true)
            .expect("query definition");

        assert_eq!(
            found.len(),
            1,
            "the variant is declared exactly once, got {found:?}"
        );
        assert!(
            found[0].file_path.ends_with("oracle_lib/src/lib.rs"),
            "the declaring crate's lib must be the answer, got {found:?}"
        );
    }

    /// One crate declaring one of every kind a caller might reasonably type
    /// into `find_definition`, each under a name used nowhere else.
    ///
    /// Every surveyed candidate lives at module scope and outside
    /// `#[cfg(test)]`, so that none of them can come back absent for the
    /// cfg-gating reason instead of the one under study. The one item that *is*
    /// behind `#[cfg(test)]` is there to test exactly that gating, and the
    /// survey does not ask about it.
    /// A crate whose only declaration of `generated_field` is written by a build
    /// script into `OUT_DIR` and pulled in with `include!` — the shape prost
    /// uses, and the shape a downstream generated-proto crate uses in a large workspace.
    ///
    /// The name also appears inside a string literal in `build.rs`. That is
    /// deliberate: the sweep walks source text, so a fixture where the only
    /// other occurrence is a literal proves it is resolving rather than
    /// grepping.
    fn generated_code_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[package]
name = "generated_probe"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        );
        write_file(
            &root.join("build.rs"),
            r#"
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let out = std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("generated.rs");
    std::fs::write(&out, "pub struct Generated { pub generated_field: u32 }\n").unwrap();
}
"#,
        );
        write_file(
            &root.join("src/lib.rs"),
            r#"
include!(concat!(env!("OUT_DIR"), "/generated.rs"));

pub fn read_generated(value: &Generated) -> u32 {
    value.generated_field
}
"#,
        );
    }

    /// Generated code was invisible: without build scripts there is no `OUT_DIR`
    /// to expand, so the included module is not in the tree and every reference
    /// into it resolves to nothing. Measured on a large workspace before the fix —
    /// seven reads of a proto field counted as zero, in the same words used for
    /// a field nobody reads.
    ///
    /// The declaration's path is asserted too, and that is the non-vacuity
    /// check: it can only sit under `OUT_DIR` if resolution actually went
    /// through the generated file.
    #[test]
    fn references_into_generated_code_are_found() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        generated_code_workspace(root);

        let service = SemanticService::new();
        let found = service
            .find_references_by_name_with_exact(root, "generated_field", true)
            .expect("query references");

        assert_eq!(
            refs_in(&found, "src/lib.rs"),
            1,
            "the read of the generated field must be visible, got {found:?}"
        );
        assert!(
            found.iter().any(|location| {
                location.name != "reference" && location.file_path.ends_with("out/generated.rs")
            }),
            "the declaration must resolve into the build script's output, not to \
             a text match elsewhere, got {found:?}"
        );
    }

    fn kind_survey_workspace(root: &Path) {
        write_file(
            &root.join("Cargo.toml"),
            r#"
[package]
name = "kind_survey"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        );
        write_file(
            &root.join("src/lib.rs"),
            r#"
pub const KIND_CONST: u32 = 1;
pub static KIND_STATIC: u32 = 2;
pub type KindAlias = u32;

pub struct KindStruct {
    pub kind_field: u32,
}

pub struct KindTuple(pub u32);

pub union KindUnion {
    pub kind_union_field: u32,
}

pub enum KindEnum {
    KindVariant,
    KindStructVariant { kind_variant_field: u32 },
}

pub trait KindTrait {
    const KIND_TRAIT_CONST: u32;
    type KindAssocType;
    fn kind_trait_method(&self) -> u32;
}

impl KindStruct {
    pub const KIND_ASSOC_CONST: u32 = 3;

    pub fn kind_impl_method(&self) -> u32 {
        self.kind_field
    }
}

impl KindTrait for KindStruct {
    const KIND_TRAIT_CONST: u32 = 4;
    type KindAssocType = u32;

    fn kind_trait_method(&self) -> u32 {
        self.kind_field
    }
}

pub mod kind_module {
    pub fn kind_module_fn() {}
}

macro_rules! kind_macro {
    () => {
        pub fn kind_generated() {}
    };
}

kind_macro!();

#[cfg(test)]
mod tests {
    #[test]
    fn kind_test_fn() {}
}
"#,
        );
    }

    /// What the symbol index carries and what it does not — measured, one row
    /// per kind, rather than inferred from the two kinds that were looked at.
    ///
    /// This is the mutant "turn the sweep off and ask for the definition" made
    /// permanent: `index_only_search` is the served path with the rescue removed,
    /// so a `false` here is a kind that answers *no definition found* whenever
    /// the sweep does not cover it. The sweep's accept list is `Field | Variant`,
    /// so every `false` row that is neither is a live blind spot, not a covered
    /// one.
    ///
    /// The guess this replaces held for exactly one of its two halves: fields are
    /// absent, enum variants are not.
    #[test]
    fn which_kinds_the_symbol_index_carries() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        kind_survey_workspace(root);

        // Sorted by name so the table reads as a table; the value is "the index
        // answered with at least one exact match".
        let candidates = [
            ("KIND_ASSOC_CONST", "associated const, inherent impl"),
            ("KIND_CONST", "module-scope const"),
            ("KIND_STATIC", "module-scope static"),
            ("KIND_TRAIT_CONST", "associated const, trait"),
            ("KindAlias", "type alias"),
            ("KindAssocType", "associated type"),
            ("KindEnum", "enum"),
            ("KindStruct", "struct"),
            ("KindStructVariant", "struct-shaped enum variant"),
            ("KindTrait", "trait"),
            ("KindTuple", "tuple struct"),
            ("KindUnion", "union"),
            ("KindVariant", "unit enum variant"),
            ("kind_field", "struct field"),
            ("kind_generated", "fn produced by macro expansion"),
            ("kind_impl_method", "inherent method"),
            ("kind_macro", "macro_rules! macro"),
            ("kind_module", "module"),
            ("kind_module_fn", "fn inside a submodule"),
            ("kind_trait_method", "trait method declaration"),
            ("kind_union_field", "union field"),
            ("kind_variant_field", "field of a struct-shaped variant"),
        ];

        let service = SemanticService::new();
        let measured: Vec<(&str, bool)> = candidates
            .iter()
            .map(|(name, _what)| {
                let found = service
                    .index_only_search(root, name, 8)
                    .expect("query the index");
                (*name, !found.is_empty())
            })
            .collect();

        let expected: Vec<(&str, bool)> = vec![
            ("KIND_ASSOC_CONST", true),
            ("KIND_CONST", true),
            ("KIND_STATIC", true),
            ("KIND_TRAIT_CONST", true),
            ("KindAlias", true),
            ("KindAssocType", true),
            ("KindEnum", true),
            ("KindStruct", true),
            ("KindStructVariant", true),
            ("KindTrait", true),
            ("KindTuple", true),
            ("KindUnion", true),
            ("KindVariant", true),
            ("kind_field", false),
            ("kind_generated", true),
            ("kind_impl_method", true),
            ("kind_macro", true),
            ("kind_module", true),
            ("kind_module_fn", true),
            ("kind_trait_method", true),
            ("kind_union_field", false),
            ("kind_variant_field", false),
        ];

        assert_eq!(
            measured, expected,
            "the index's coverage changed — a row flipping to false is a new \
             blind spot, and one flipping to true means the sweep is carrying \
             weight it no longer needs to"
        );
    }

    /// A definition behind `#[cfg(test)]` must be found by the first question
    /// asked, not only by one that follows a `find_references`.
    ///
    /// `find_definition` used to run on a `Fast` context, where `set_test` is
    /// off and a test module is not in the tree — so it answered *no definition
    /// found* for a test that exists. What made that a bug rather than a limit:
    /// `get_or_load` does not downgrade an already-`Full` context, so the very
    /// same query answered correctly once anything else in the session had asked
    /// for references. Measured on a real workspace, both answers minutes apart.
    ///
    /// Hence the shape of this test — one service, nothing asked before, the
    /// query cold. Run it after a reference lookup and it passes either way.
    #[test]
    fn a_definition_behind_cfg_test_is_found_by_the_first_question() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        kind_survey_workspace(root);

        let service = SemanticService::new();
        let found = service
            .symbol_search_with_exact(root, "kind_test_fn", 8, true)
            .expect("query definition");

        assert_eq!(
            found.len(),
            1,
            "the test fn is declared once and must be found on a cold service, \
             got {found:?}"
        );
    }

    /// Every kind the index does not carry must be one the sweep does, or the
    /// tool still answers *no definition found* for something that exists.
    ///
    /// The survey above names three: a struct field, a union field, and a field
    /// of a struct-shaped enum variant. All three are `SymbolKind::Field`, which
    /// is what makes one accept list enough — but "rust-analyzer labels a union
    /// field a field" is an assumption about someone else's code, so it is
    /// checked here rather than reasoned about.
    ///
    /// A tuple struct's field is deliberately absent from both lists: it is
    /// named `0`, so there is no identifier to ask about and no query that could
    /// come up short.
    #[test]
    fn the_sweep_covers_every_kind_the_index_misses() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        kind_survey_workspace(root);

        let service = SemanticService::new();
        for name in ["kind_field", "kind_union_field", "kind_variant_field"] {
            let found = service
                .symbol_search_with_exact(root, name, 8, true)
                .expect("query definition");

            assert_eq!(
                found.len(),
                1,
                "`{name}` is declared exactly once and the served path must say \
                 so, got {found:?}"
            );
        }
    }

    /// Negative control for the definition sweep, the twin of the one guarding
    /// references: a `let radius_override = 7;` declares that exact name too.
    /// Accepting it would answer "the field is declared here" pointing into an
    /// unrelated function body — a wrong location reads worse than none.
    #[test]
    fn a_local_binding_spelled_like_the_field_is_not_a_definition_of_it() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        two_crate_oracle_workspace(root);
        write_file(
            &root.join("oracle_consumer/src/lib.rs"),
            r#"
pub fn a_local_of_the_same_name() -> u32 {
    // Same spelling, unrelated binding: not a declaration of the field.
    let radius_override = 7;
    radius_override
}
"#,
        );

        let service = SemanticService::new();
        let found = service
            .symbol_search_with_exact(root, "radius_override", 8, true)
            .expect("query definition");

        assert_eq!(
            found.len(),
            1,
            "only the field is a definition of this name, got {found:?}"
        );
        assert!(
            found[0].file_path.ends_with("oracle_lib/src/lib.rs"),
            "the local must not be offered as the declaration, got {found:?}"
        );
    }

    /// Untouched code must not pay for the check: same stamps, no reload, and
    /// crucially the same answer.
    #[test]
    fn an_untouched_project_is_not_reloaded() {
        let _analysis = holding_an_analysis();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        one_crate_workspace(root);

        let service = SemanticService::new();
        service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("first query");

        let canonical = root.canonicalize().expect("canonicalize");
        let stamps_before = service.with_context_for_test(&canonical, |ctx| ctx.stamps.clone());

        service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("second query");

        let stamps_after = service.with_context_for_test(&canonical, |ctx| ctx.stamps.clone());
        assert!(
            matches!(classify_staleness(&stamps_before, &stamps_after), Staleness::Fresh),
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
        service.loaded_paths_for_test()
    }

    /// The cap has to evict by *use*, not by arrival. A queue that dropped the
    /// oldest arrival would throw away the project the session actually works
    /// in, which is the one it will ask about next.
    #[test]
    fn a_third_project_evicts_the_least_recently_used_not_the_oldest() {
        let _analysis = holding_an_analysis();
        let (first, second, third) = (
            tempfile::tempdir().expect("tempdir a"),
            tempfile::tempdir().expect("tempdir b"),
            tempfile::tempdir().expect("tempdir c"),
        );
        for dir in [&first, &second, &third] {
            one_crate_workspace(dir.path());
        }

        let service = SemanticService::with_max_projects(2);
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
        let _analysis = holding_an_analysis();
        let (first, second) = (
            tempfile::tempdir().expect("tempdir a"),
            tempfile::tempdir().expect("tempdir b"),
        );
        for dir in [&first, &second] {
            one_crate_workspace(dir.path());
        }

        let service = SemanticService::with_max_projects(1);
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

        let service = SemanticService::with_max_projects(0);
        for dir in &dirs {
            service.insert_test_project_fast(dir.path().to_path_buf());
        }
        let kept = loaded(&service)[0].clone();
        assert!(service.registry().evict_to_capacity(&kept).is_empty());
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
        let _sweep = sweeping_alone();
        use ra_ap_ide_db::base_db::SourceDatabase;

        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        one_crate_workspace(root);

        let service = SemanticService::new();
        let before = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("query before collection");
        assert_eq!(
            call_sites(&before),
            1,
            "positive control: the fixture must start with one call site, got {before:?}"
        );

        let canonical = root.canonicalize().expect("canonicalize");
        let revision_before = service
            .with_context_for_test(&canonical, |ctx| ctx.host.raw_database().nonce_and_revision());

        assert_eq!(
            service.collect_garbage(),
            1,
            "the collection must visit the loaded project"
        );

        let revision_after = service
            .with_context_for_test(&canonical, |ctx| ctx.host.raw_database().nonce_and_revision());
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

    /// The process portrait adds up to what the process holds, and each line it
    /// names is really measured.
    ///
    /// The sum alone proves nothing — `unattributed` is the remainder by
    /// construction, so any wrong line is silently absorbed by it. What can
    /// fail is the other direction: salsa plus the interners must fit inside
    /// the heap in use (double counting, or a byte count of the wrong thing,
    /// overshoots it), and every measured line must be non-zero on a database
    /// that has done real work (a line that stopped measuring reads as zero).
    #[test]
    fn the_process_portrait_adds_up_and_names_its_parts() {
        let _sweep = sweeping_alone();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = workspace.path();
        one_crate_workspace(root);

        let service = SemanticService::new();
        let found = service
            .find_references_by_name_with_exact(root, "target", true)
            .expect("a query that fills the database");
        assert_eq!(
            call_sites(&found),
            1,
            "positive control: the fixture has one call site"
        );

        let report = service
            .memory_breakdown(root, 10)
            .expect("loaded project reports");
        let process = report.process.expect("linux has a portrait");
        let line = |owner: &str| {
            process
                .lines
                .iter()
                .find(|line| line.owner == owner)
                .unwrap_or_else(|| panic!("no `{owner}` line in {:?}", process.lines))
                .bytes
        };

        assert_eq!(
            process.lines.iter().map(|line| line.bytes).sum::<i64>(),
            process.held_bytes,
            "the lines are a partition of rss + swap"
        );
        assert_eq!(process.salsa_projects, 1);
        assert_eq!(
            line(LINE_SALSA),
            report.accounted_bytes as i64,
            "with one project loaded the salsa line is that project's accounted bytes"
        );
        for owner in [LINE_FILE, LINE_SALSA, LINE_INTERNERS, LINE_UNATTRIBUTED] {
            assert!(
                line(owner) > 0,
                "`{owner}` measured nothing: {:?}",
                process.lines
            );
        }
        assert!(
            process
                .interners
                .iter()
                .any(|row| row.name.contains("TyInterned") && row.count > 0),
            "a reference search infers bodies, so the solver's type interner must be populated: \
             {:?}",
            process.interners
        );
        if let Some(allocator) = process.allocator {
            assert!(
                (line(LINE_SALSA) + line(LINE_INTERNERS)) as u64 <= allocator.in_use_bytes,
                "salsa and the interners claim more than the heap has in use — something is \
                 counted twice: {:?} vs {allocator:?}",
                process.lines
            );
            assert!(line(LINE_FREE) >= 0);
            assert!(
                line(LINE_OUTSIDE) >= 0,
                "the arenas held more than the anonymous memory the kernel reports: {:?}",
                process.lines
            );
            let arenas = process
                .arenas
                .expect("allocator statistics come with arena residency");
            // The writable arena mappings cover what glibc took for its arenas:
            // a heap the alignment rule missed would leave its bytes behind, and
            // its resident pages would land in "outside" instead of "free".
            assert!(
                arenas.mapped_kib * 1024 >= allocator.arena_bytes * 9 / 10,
                "arena recognition found {arenas:?} of {allocator:?}"
            );
            assert!(arenas.held_kib > 0 && arenas.held_kib <= arenas.mapped_kib);

            // The census cut: salsa and the interners count requested bytes,
            // so they must fit inside what the Rust heap requested — not merely
            // inside the chunks — and the chunks carry real overhead on top.
            // (`outside_rust_heap` has no sign to check: the census and
            // `mallinfo2` are read a moment apart while other tests allocate.)
            let census = process.census.expect("the test binary installs the census");
            assert!(
                (line(LINE_SALSA) + line(LINE_INTERNERS)) <= census.requested_bytes,
                "salsa and the interners claim more than the Rust heap requested: {:?} vs \
                 {census:?}",
                process.lines
            );
            assert!(
                line(LINE_OVERHEAD) > 0,
                "a heap of rust-analyzer nodes pays glibc's chunk headers: {:?}",
                process.lines
            );
            line(LINE_FOREIGN);
        }
        assert!(!process.largest_mappings.is_empty() && process.largest_mappings.len() <= 10);
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
    /// RMC_RSS_CYCLE_PROJECT=/home/user/project \
    ///   cargo test -p rmc-server --features migraphx -- --ignored --nocapture rss_across
    /// ```
    ///
    /// `RMC_RSS_CYCLE_COUNT` (default 4) and `RMC_RSS_CYCLE_TOLERANCE_MB`
    /// (default 512) tune length and verdict.
    #[test]
    #[ignore = "needs a real workspace in RMC_RSS_CYCLE_PROJECT and minutes to run"]
    fn rss_across_load_clear_cycles_separates_a_leak_from_fragmentation() {
        // Exclusive: this one sweeps too, and it measures RSS — a peer holding
        // a workspace would be counted as this test's own memory.
        let _sweep = sweeping_alone();
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
            let service = SemanticService::new();
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

    /// Where does a loaded rust-analyzer database actually keep its bytes?
    ///
    /// The RSS cycle test above answers "does it leak"; this one answers "what
    /// is it holding". It asks salsa itself — `Database::memory_usage()`, built
    /// because `base-db` enables the `salsa_unstable` feature — for a per
    /// ingredient breakdown, and prints it sorted by bytes.
    ///
    /// # 🚨 What this measurement can and cannot see
    ///
    /// Salsa reports three numbers per ingredient: the number of instances, the
    /// *stack* size of their fields, and its own metadata. Heap behind those
    /// fields is counted only where the query declares `heap_size = <fn>`, and
    /// every heavy memo in rust-analyzer is a `Vec`/`Arc`/`Box` behind a small
    /// struct — so the heap is exactly where the gigabytes are.
    ///
    /// Upstream declares it nowhere (zero occurrences as of the 2026-08-26
    /// tree, which is what made this table explain 4.2% of RSS). Our fork
    /// declares it on the families that were the biggest by count *and* own
    /// buffers: the parse query, bodies with their source maps, the signature
    /// queries and macro expansion. That took the same table to 42.7%. Def
    /// maps, item trees and symbol indices followed, then type inference — the
    /// one capacity the fork chose for itself, and so the one that most needed
    /// bytes to argue from.
    ///
    /// The rest of the rows are still counts and shapes, not bytes, and a
    /// number that is present is a LOWER bound (see `syntax::heap_size`). Read
    /// the printed "accounted for" percentage as the honest measure of how much
    /// of the process this table explains — never as a budget. Turning another
    /// suspect into a number takes one `heap_size = <fn>` attribute on its
    /// query in the fork; picking which suspect is what this output is for.
    ///
    /// # Running it
    ///
    /// ```text
    /// RMC_SALSA_MEMORY_PROJECT=/home/user/project \
    ///   cargo test -p rmc-server --features migraphx -- --ignored --nocapture salsa_ingredient
    /// ```
    ///
    /// `RMC_SALSA_MEMORY_TOP` (default 25, 0 = all) caps how many rows are
    /// printed.
    ///
    /// `RMC_SALSA_MEMORY_PROBE` picks what fills the database first:
    /// `symbols` (default, a symbol search — cheap, and what every earlier run
    /// in this track used) or `references` (chase the uses of
    /// `RMC_SALSA_MEMORY_SYMBOL`, which then has to be set). Only the second
    /// runs type inference, so only under it does the `InferenceResult` row mean
    /// anything.
    /// The probe decides every number in the table: two runs are comparable only
    /// if they probed alike.
    #[test]
    #[ignore = "needs a real workspace in RMC_SALSA_MEMORY_PROJECT and minutes to run"]
    fn salsa_ingredient_memory_breakdown_names_the_suspects() {
        // Exclusive for the same two reasons as the RSS cycle test: it sweeps,
        // and it attributes memory.
        let _sweep = sweeping_alone();
        // Still needed below, where the test asks the database itself which
        // queries have a tunable capacity; the breakdown no longer needs it.
        use ra_ap_ide_db::base_db::salsa;

        let Ok(project) = std::env::var("RMC_SALSA_MEMORY_PROJECT") else {
            panic!(
                "set RMC_SALSA_MEMORY_PROJECT to a cargo workspace root; without one this test \
                 would measure nothing and pass, which is worse than not running"
            );
        };
        let project = PathBuf::from(project);
        // 0 means every row, as `top` does on the `analysis_memory` tool;
        // read literally it printed none.
        let top = match env_usize("RMC_SALSA_MEMORY_TOP", 25) {
            0 => usize::MAX,
            top => top,
        };
        // Which query fills the database before it is weighed. `symbols` is the
        // default because it is what every earlier run in this track used, and
        // the probe decides every number in the table — two runs are comparable
        // only if they probed the same way.
        //
        // `references` exists because `symbols` never runs type inference:
        // measured on a 4000-file workspace, it leaves `InferenceResult` at
        // thirteen memos, and a byte count over thirteen memos is noise. Chasing
        // a name's use sites infers every body that mentions it, which is also
        // what the `find_references` endpoint does in production.
        let probe = std::env::var("RMC_SALSA_MEMORY_PROBE")
            .unwrap_or_else(|_| "symbols".to_string());
        // Deliberately without a default. The name decides both what the probe
        // costs and whether it proves anything, and one guessed here would do
        // neither: `new` looked like the obvious choice and matches up to fifty
        // declarations, each of which sweeps the whole tree for its own uses —
        // measured at 10 GB and still climbing after two minutes on a 4000-file
        // workspace. What works is a name with ONE declaration and many uses.
        let symbol = std::env::var("RMC_SALSA_MEMORY_SYMBOL").unwrap_or_default();

        // The breakdown itself is production code — the `analysis_memory` tool
        // serves the same rows — so this test exercises what ships rather than
        // a copy that could drift from it.
        type Row = IngredientMemory;
        let breakdown = ingredient_rows;

        let service = SemanticService::new();
        // A real query, not a bare load: an empty database has nothing memoized
        // and every row would read as zero.
        let probed = match probe.as_str() {
            "symbols" => {
                let found = service
                    .symbol_search(&project, "main", 16)
                    .expect("symbol search on the probe workspace");
                format!("probe `symbols`: {} symbol(s) named `main`", found.len())
            }
            "references" => {
                assert!(
                    !symbol.is_empty(),
                    "the `references` probe needs RMC_SALSA_MEMORY_SYMBOL: a name with one \
                     declaration and many use sites in the probe workspace. See the comment on \
                     `symbol` for why there is no default"
                );
                let found = service
                    .find_references_by_name_with_exact(&project, &symbol, true)
                    .expect("reference search on the probe workspace");
                format!(
                    "probe `references`: {} location(s) for `{symbol}`",
                    found.len()
                )
            }
            other => panic!(
                "RMC_SALSA_MEMORY_PROBE={other:?} names no probe; use `symbols` (cheap, runs no \
                 type inference) or `references` (infers every body mentioning \
                 RMC_SALSA_MEMORY_SYMBOL, default `new`)"
            ),
        };
        let rss_kib = crate::mcp::memory::rss_kib().expect("RSS readable on linux");

        let canonical = project
            .canonicalize()
            .expect("canonicalize the probe project");
        // Borrowed inline, not held: `collect_garbage` below needs `&mut` on the
        // service, so no borrow of a host may outlive a single measurement.
        let rows = service.with_context_for_test(&canonical, |ctx| breakdown(&mut ctx.host));

        let accounted: usize = rows.iter().map(Row::bytes).sum();
        let with_heap = rows.iter().filter(|row| row.heap_bytes.is_some()).count();
        let instances: usize = rows.iter().map(|row| row.count).sum();

        println!(
            "{probed}; RSS {} MB; {} ingredients, {} instances total",
            rss_kib / 1024,
            rows.len(),
            instances,
        );
        println!(
            "accounted for {} MB = {:.1}% of RSS; {} of {} ingredients report heap size",
            accounted / (1024 * 1024),
            100.0 * accounted as f64 / (rss_kib as f64 * 1024.0),
            with_heap,
            rows.len(),
        );
        println!(
            "{:<8} {:<44} {:>10} {:>10} {:>10} {:>10}  {}",
            "family", "query / struct", "count", "stack KiB", "meta KiB", "heap KiB", "produces"
        );
        for row in rows.iter().take(top) {
            println!(
                "{:<8} {:<44} {:>10} {:>10} {:>10} {:>10}  {}",
                row.family,
                row.name,
                row.count,
                row.stack_bytes / 1024,
                row.metadata_bytes / 1024,
                row.heap_bytes
                    .map_or("-".to_string(), |heap| (heap / 1024).to_string()),
                row.produces.or(row.page.as_deref()).unwrap_or(""),
            );
        }

        assert!(
            !rows.is_empty(),
            "a database that answered a query must have memoized something; an empty breakdown \
             means the measurement, not the database, is broken"
        );

        // The salsa `debug_name` of the type inference query. The spelling is
        // the macro's, lifetime and spaces included, and it cannot be guessed —
        // it is what `Database::lru_capacity_names` prints below.
        const INFER_QUERY: &str = "InferenceResult < 'db >::for_body_";
        let row_of = |rows: &[Row], query: &str| {
            rows.iter()
                .find(|row| row.family == "query" && row.name == query)
                .map(|row| (row.count, row.heap_bytes))
        };
        if probe == "references" {
            // A floor on "did inference run at all", not on "is the sample
            // large": the `symbols` probe leaves this query at thirteen memos,
            // so anything in that neighbourhood means the probe missed its
            // target rather than found a small answer. How big the sample is,
            // the reader judges from the per-memo average printed beside it.
            const INFER_FLOOR: usize = 50;
            let (inferred, infer_heap) = row_of(&rows, INFER_QUERY).unwrap_or((0, None));
            println!(
                "type inference: {inferred} memo(s), heap {} KiB ({} bytes each on average)",
                infer_heap.unwrap_or(0) / 1024,
                infer_heap.unwrap_or(0).checked_div(inferred).unwrap_or(0),
            );
            assert!(
                inferred > INFER_FLOOR,
                "the reference probe inferred only {inferred} bodies (floor {INFER_FLOOR}) — \
                 `{symbol}` is used too little in this workspace to make type inference measurable, \
                 so any heap number for `{INFER_QUERY}` above is noise. Point \
                 RMC_SALSA_MEMORY_SYMBOL at a name called from many bodies"
            );
        }
        // The gap between this table and RSS is the finding, not a defect: it is
        // the heap that salsa cannot see without `heap_size`. If the table ever
        // covers the process, this assert fires and the conclusion above — "read
        // counts, not bytes" — has to be revisited.
        assert!(
            accounted < rss_kib as usize * 1024,
            "the breakdown accounts for {} MB of a {} MB process — salsa is not supposed to be \
             able to see that much without `heap_size` on the queries; re-check the arithmetic \
             before trusting it",
            accounted / (1024 * 1024),
            rss_kib / 1024,
        );

        // ---- the end-to-end positive control for the whole LRU + GC chain ----
        //
        // A capacity is declared on this query (512, restated in
        // `update_base_query_lru_capacities`) but salsa only evicts inside
        // `reset_for_new_revision`, so a database nobody writes to keeps every
        // memo it ever made however small the capacity is. That is the entire
        // reason `collect_garbage` exists; this asserts it in *counts*, which —
        // unlike RSS — cannot be hidden by glibc holding freed pages.
        // Ask the database which queries actually have a tunable capacity rather
        // than trusting the names the fork hardcodes: salsa suffixes the
        // generated ingredient of an *associated* tracked fn with `_`, so a
        // hand-written `Type::method` matches nothing and only logs a warning
        // nobody reads. This line is what makes that visible.
        service.with_context_for_test(&canonical, |ctx| {
            let db: &dyn salsa::Database = ctx.host.raw_database();
            let mut tunable = salsa::Database::lru_capacity_names(db);
            tunable.sort_unstable();
            println!("queries with a tunable LRU capacity: {tunable:?}");
        });
        {
            // And the other half: that rust-analyzer's own configuration reaches
            // those queries. It reports a miss only through `tracing::warn!`, and
            // a warning in a daemon log is not an oracle — this turns the whole
            // set of names into one red assertion.
            let missed = service.with_context_for_test(&canonical, |ctx| {
                ctx.host
                    .raw_database_mut()
                    .update_base_query_lru_capacities(None)
            });
            assert_eq!(
                missed, 0,
                "rust-analyzer asked for {missed} LRU capacities that match no query in this \
                 database; the knob looks alive and retunes nothing (see the printed list of \
                 tunable names just above for what it should have said)"
            );
        }

        // The salsa `debug_name` of a tracked query, which is what the capacity
        // is addressed by. Associated tracked fns carry a trailing `_` — the
        // macro's own name for the generated ingredient — so the readable
        // `Body::with_source_map` matches nothing at all.
        const CAPPED_QUERY: &str = "Body::with_source_map_";
        const DECLARED_CAP: usize = 512;
        let (before, heap_before) = row_of(&rows, CAPPED_QUERY).unwrap_or_else(|| {
            panic!(
                "no query named `{CAPPED_QUERY}` in the breakdown — upstream renamed it, and \
                 `update_base_query_lru_capacities` is now setting a capacity on nothing"
            )
        });
        assert!(
            before > DECLARED_CAP,
            "the probe workspace memoized only {before} bodies, at or under the {DECLARED_CAP} \
             capacity — eviction has nothing to do here, so a pass below would prove nothing; \
             point RMC_SALSA_MEMORY_PROJECT at a bigger workspace"
        );

        service.collect_garbage();

        let after_rows = service.with_context_for_test(&canonical, |ctx| breakdown(&mut ctx.host));
        let (after, heap_after) = row_of(&after_rows, CAPPED_QUERY).unwrap_or((0, None));
        println!(
            "after collect_garbage: `{CAPPED_QUERY}` {before} -> {after} memo slot(s), heap \
             {heap_before:?} -> {heap_after:?} (cap {DECLARED_CAP}); RSS {} MB",
            crate::mcp::memory::rss_kib().unwrap_or(0) / 1024,
        );

        // What the collection actually freed, per query. The table above says
        // where the memory *is*; this says how much of it a collection can take
        // back, which is the other half of arguing about a capacity — and it is
        // not derivable from the first, because only the LRU-capped queries
        // shed anything at all.
        let mut freed: Vec<(&str, usize)> = rows
            .iter()
            .filter_map(|row| {
                let before = row.heap_bytes?;
                let after = row_of(&after_rows, row.name).map(|(_, heap)| heap)??;
                before.checked_sub(after).filter(|freed| *freed > 0).map(|freed| (row.name, freed))
            })
            .collect();
        freed.sort_by_key(|&(_, freed)| std::cmp::Reverse(freed));
        println!("freed by the collection:");
        for (query, freed) in &freed {
            println!("  {:<50} {:>9} KiB", query, freed / 1024);
        }
        if freed.is_empty() {
            println!("  (nothing — every instrumented ingredient kept every byte)");
        }
        // Why the *slot count* cannot be the oracle: evicting a memo sets its
        // value to `None` and keeps the slot (`function/memo.rs`, `map_memo`), so
        // the count is unchanged whether eviction ran or not. What does move is
        // the reported heap, and it is the only allocator-independent signal
        // available — RSS cannot serve, because glibc need not return the pages.
        //
        // The shape of that signal depends on the fork declaring `heap_size` on
        // this query. Before it did, a live memo reported `None` and an evicted
        // one `Some(0)`, so the whole test could say was "at least one value went
        // away". With the option in place the ingredient reports real bytes, and
        // the same question is answered by a strict drop — with the size of the
        // drop printed above, which the old encoding could never show.
        let describe_missing_option = || {
            format!(
                "`{CAPPED_QUERY}` reports no heap size, so this test cannot tell an evicted memo \
                 from a live one. The `heap_size` option on that query is gone — most likely an \
                 upstream rebase dropped it from the rust-analyzer fork (see \
                 `hir-def/src/expr_store/heap_size.rs`)"
            )
        };
        let heap_before = heap_before.unwrap_or_else(|| panic!("{}", describe_missing_option()));
        let heap_after = heap_after.unwrap_or_else(|| panic!("{}", describe_missing_option()));
        assert!(
            heap_after < heap_before,
            "`{CAPPED_QUERY}` holds {after} memo slots against a capacity of {DECLARED_CAP}, and \
             after a garbage collection it still owns every byte it did before ({heap_before}): \
             the capacity is declared but nothing enforces it, which is exactly the defect this \
             whole chain exists to fix"
        );
    }

    /// Where does memory settle after a release, under the allocator this
    /// binary was built with — and what do queries cost there?
    ///
    /// The fragmentation floor of the live daemon (≈ 4.2 GB of resident arena
    /// pages holding no live chunk, left after `malloc_trim`) is holes inside
    /// pages that still hold live objects; no trim returns them. Only the
    /// allocator decides how many such pages there are, so the comparison is
    /// one workload under several allocators: glibc as shipped, glibc with
    /// `MALLOC_ARENA_MAX=<n>` (environment only, no rebuild), and mimalloc
    /// (`--features mimalloc`, which also switches the test binary's global
    /// allocator — see `lib.rs`).
    ///
    /// The workload is the daemon's, in miniature: a reference search loads the
    /// project and infers every body mentioning the load symbol; then, until the
    /// work time is up, rounds of garbage collection (the daemon collects every
    /// five minutes; here every round, to churn the LRU-capped memos faster),
    /// one reference search per work symbol and one symbol search. The portrait
    /// is the shipped one — `memory_breakdown`, as `analysis_memory` serves it —
    /// taken before and after a release at both ends. The number to compare is
    /// `held` after a release: that is the floor the daemon's growth trim
    /// returns to. Latencies are printed per call, because fewer arenas can
    /// queue parallel threads behind one lock.
    ///
    /// Under mimalloc the portrait has no allocator split (no `mallinfo2`), so
    /// fragmentation is inside `unattributed`; `held` stays comparable because
    /// the salsa and interner lines are the same workload's.
    ///
    /// # Running it
    ///
    /// ```text
    /// RMC_ALLOC_FLOOR_PROJECT=/home/user/project \
    /// RMC_ALLOC_FLOOR_SYMBOL=OneDeclManyUses RMC_ALLOC_FLOOR_WORK=A,B,C \
    ///   cargo test --release -p rmc-server --lib [--features mimalloc] \
    ///   -- --ignored --nocapture allocator_floor
    /// ```
    ///
    /// `RMC_ALLOC_FLOOR_WORK_SECS` (default 300) is the length of the work
    /// phase. The symbols have no defaults for the reason given on the salsa
    /// probe above: the name decides what the run costs.
    #[test]
    #[ignore = "needs a real workspace in RMC_ALLOC_FLOOR_PROJECT and minutes to run"]
    fn allocator_floor_after_load_and_work() {
        use std::time::{Duration, Instant};
        // Exclusive: it measures the whole process.
        let _sweep = sweeping_alone();
        let required = |name: &str| {
            std::env::var(name).unwrap_or_else(|_| {
                panic!(
                    "set {name}; without it this probe would measure nothing and pass, which is \
                     worse than not running"
                )
            })
        };
        let project = PathBuf::from(required("RMC_ALLOC_FLOOR_PROJECT"));
        let symbol = required("RMC_ALLOC_FLOOR_SYMBOL");
        let work: Vec<String> = required("RMC_ALLOC_FLOOR_WORK")
            .split(',')
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty())
            .collect();
        assert!(!work.is_empty(), "RMC_ALLOC_FLOOR_WORK names no symbol");
        let work_time = Duration::from_secs(env_usize("RMC_ALLOC_FLOOR_WORK_SECS", 300) as u64);

        let allocator = if cfg!(feature = "mimalloc") { "mimalloc" } else { "glibc" };
        // A run labelled mimalloc that allocates from glibc would compare glibc
        // with itself; ask mimalloc whether a fresh heap block is its own.
        #[cfg(feature = "mimalloc")]
        {
            // Declared by hand for the reason `mi_collect` is in `mcp::memory`:
            // `libmimalloc-sys` binds only the allocation calls, the archive it
            // links has the rest.
            unsafe extern "C" {
                fn mi_is_in_heap_region(p: *const std::ffi::c_void) -> bool;
            }
            let probe = vec![0u8; 4096];
            // SAFETY: only compares the address against mimalloc's regions.
            let owned =
                unsafe { mi_is_in_heap_region(probe.as_ptr().cast::<std::ffi::c_void>()) };
            assert!(owned, "the mimalloc build allocates outside mimalloc's heap");
        }
        println!(
            "allocator {allocator}, MALLOC_ARENA_MAX={}, {}={}",
            std::env::var("MALLOC_ARENA_MAX").unwrap_or_else(|_| "unset".to_owned()),
            loader::PRIME_BATCH_FILES_ENV,
            std::env::var(loader::PRIME_BATCH_FILES_ENV).unwrap_or_else(|_| "unset".to_owned())
        );

        let portrait =
            |service: &mut SemanticService, stage: &str| print_portrait(service, &project, stage);

        let mut service = SemanticService::new();
        let started = Instant::now();
        let found = service
            .find_references_by_name_with_exact(&project, &symbol, true)
            .expect("reference search on the probe workspace");
        println!(
            "load: {} location(s) for `{symbol}` in {:.1} s",
            found.len(),
            started.elapsed().as_secs_f64()
        );
        assert!(!found.is_empty(), "`{symbol}` has no references: the load inferred nothing");
        print_answer(&format!("refs {symbol}"), &found);
        // The load's own peak, apart from the whole run's printed at the end:
        // the priming mode changes this one.
        println!("peak RSS after load {}", vm_hwm());
        portrait(&mut service, "loaded");
        crate::mcp::memory::release_and_measure();
        let loaded_floor = portrait(&mut service, "loaded+release");

        // (operation, milliseconds), in call order.
        let mut latencies: Vec<(String, u128)> = Vec::new();
        let mut rounds = 0;
        let work_started = Instant::now();
        while work_started.elapsed() < work_time {
            service.collect_garbage();
            for name in &work {
                let started = Instant::now();
                let found = service
                    .find_references_by_name_with_exact(&project, name, true)
                    .expect("reference search on the probe workspace");
                assert!(!found.is_empty(), "work symbol `{name}` has no references");
                latencies.push((format!("refs {name}"), started.elapsed().as_millis()));
                if rounds == 0 {
                    print_answer(&format!("refs {name}"), &found);
                }
            }
            let started = Instant::now();
            let found = service
                .symbol_search(&project, "Point", 16)
                .expect("symbol search on the probe workspace");
            latencies.push(("symbols Point".to_owned(), started.elapsed().as_millis()));
            if rounds == 0 {
                print_answer("symbols Point", &found);
            }
            rounds += 1;
        }
        portrait(&mut service, "worked");
        crate::mcp::memory::release_and_measure();
        let worked_floor = portrait(&mut service, "worked+release");

        println!("work: {rounds} round(s) in {:.0} s", work_started.elapsed().as_secs_f64());
        // Per operation: the first call (after the load, cold for that name)
        // apart from the median of the rest, which ran after a collection.
        let mut operations: Vec<&str> = latencies.iter().map(|(op, _)| op.as_str()).collect();
        operations.dedup();
        operations.sort_unstable();
        operations.dedup();
        for op in operations {
            let mut ms: Vec<u128> =
                latencies.iter().filter(|(o, _)| o == op).map(|(_, ms)| *ms).collect();
            let first = ms.remove(0);
            ms.sort_unstable();
            let median = ms.get(ms.len() / 2).copied();
            println!(
                "latency {op:<28} first {first:>7} ms, median of the rest {:>7} ms over {} call(s)",
                median.map_or("-".to_owned(), |ms| ms.to_string()),
                ms.len()
            );
        }
        // The peak is the other end of the trade: an allocator that settles
        // lower but reserves more on the way up needs that headroom on the
        // machine all the same.
        println!(
            "floor: {:.2} GiB after load, {:.2} GiB after work; peak RSS {}",
            gib(loaded_floor),
            gib(worked_floor),
            vm_hwm()
        );
        assert!(rounds > 0, "the work phase ran no round");
    }

    /// How high does RSS go while a project that gained a file is reloaded?
    ///
    /// A file added or removed under a loaded project is a structural change:
    /// `refresh_if_stale` reloads the whole analysis. If the old database is
    /// still in `projects` while the new one is built, the reload's peak is
    /// both of them — on a ~49 000-file workspace the allocator probe's work phase went from
    /// ≈ 8 to ≈ 11 GiB whenever a neighbour's edit added a file mid-run.
    ///
    /// The probe loads the project with the kernel's peak counter reset
    /// (`clear_refs` 5), so the first load's climb is known; then adds a file,
    /// queries again — which reloads — and compares the reload's peak with
    /// that climb. One database at a time keeps the reload at the load's own
    /// peak; two put the whole held database under it.
    ///
    /// The project is MUTATED (a file is added and removed again), so point it
    /// at a scratch copy, never at a tree someone else works in.
    ///
    /// ```text
    /// RMC_RELOAD_PEAK_PROJECT=/scratch/copy-of-salsa RMC_RELOAD_PEAK_SYMBOL=Revision \
    ///   cargo test --release -p rmc-server --lib -- --ignored --nocapture reload_peak
    /// ```
    #[test]
    #[ignore = "mutates the workspace in RMC_RELOAD_PEAK_PROJECT and takes minutes"]
    fn reload_peak_holds_one_database() {
        let _sweep = sweeping_alone();
        let required = |name: &str| {
            std::env::var(name).unwrap_or_else(|_| {
                panic!(
                    "set {name}; without it this probe would measure nothing and pass, which is \
                     worse than not running"
                )
            })
        };
        let project = PathBuf::from(required("RMC_RELOAD_PEAK_PROJECT"));
        let symbol = required("RMC_RELOAD_PEAK_SYMBOL");
        let kib = |field: &str| -> i64 {
            std::fs::read_to_string("/proc/self/status")
                .expect("status readable on linux")
                .lines()
                .find_map(|line| {
                    let value = line.strip_prefix(field)?.split_whitespace().next()?;
                    Some(value.parse::<i64>().expect("a KiB count"))
                })
                .unwrap_or_else(|| panic!("{field} in /proc/self/status"))
        };
        let reset_peak = || std::fs::write("/proc/self/clear_refs", "5").expect("reset VmHWM");

        let service = SemanticService::new();
        reset_peak();
        let started = kib("VmRSS:");
        let first = service
            .find_references_by_name_with_exact(&project, &symbol, true)
            .expect("reference search on the probe workspace");
        assert!(!first.is_empty(), "`{symbol}` has no references: the load inferred nothing");
        // What the first load climbed by from an empty service: the most a
        // reload holding ONE database may climb by from an empty service too.
        let first_rise = kib("VmHWM:") - started;
        crate::mcp::memory::release_and_measure();
        let held = kib("VmRSS:");
        reset_peak();
        let baseline = kib("VmHWM:");

        // A module nobody declares is still a new `.rs` file under the root:
        // the stamps see it and the next query reloads.
        let added = project.join("src/reload_peak_probe.rs");
        std::fs::write(&added, "pub fn reload_peak_probe() {}\n").expect("add a file");
        let second = service.find_references_by_name_with_exact(&project, &symbol, true);
        std::fs::remove_file(&added).expect("remove the added file");
        let second = second.expect("reference search after the reload");
        let peak = kib("VmHWM:");
        println!(
            "first load rose {} MiB from {} MiB; held after load {} MiB; reload peak {} MiB \
             (+{} MiB over held)",
            first_rise / 1024,
            started / 1024,
            held / 1024,
            peak / 1024,
            (peak - baseline) / 1024
        );
        assert_eq!(first.len(), second.len(), "the reload changed the answer");
        // Two databases at once put the whole held one under the new load:
        // the peak is then `held + first_rise`. One at a time it is the load's
        // own climb from what the service keeps without a project (`started`),
        // plus whatever of the old database glibc and the global interners
        // still keep. The line is drawn halfway: half the held database.
        // salsa-bur, 2026-10-22: both alive 1529 MiB, drop only 1348, drop
        // and release 1248, against a line of ≈ 1440.
        let one_database = started + first_rise + (held - started) / 2;
        assert!(
            peak < one_database,
            "the reload peaked at {} MiB, over the {} MiB one database at a time allows (first \
             load +{} MiB from {} MiB, {} MiB held): the old database was still alive while \
             the new one was built",
            peak / 1024,
            one_database / 1024,
            first_rise / 1024,
            started / 1024,
            held / 1024
        );
    }

    /// The process's peak RSS so far, as `/proc/self/status` prints it.
    fn vm_hwm() -> String {
        std::fs::read_to_string("/proc/self/status")
            .expect("status readable on linux")
            .lines()
            .find_map(|line| line.strip_prefix("VmHWM:").map(str::trim).map(str::to_owned))
            .expect("VmHWM in /proc/self/status")
    }

    /// One answer as a count and a fingerprint of its sorted locations, so two
    /// runs of a probe (two allocators, two priming modes) can be checked to
    /// have answered the same, not only to have cost differently.
    fn print_answer(operation: &str, found: &[Location]) {
        use std::hash::{Hash, Hasher};
        let mut keys: Vec<String> = found
            .iter()
            .map(|l| format!("{}:{}:{}:{}", l.file_path.display(), l.line, l.column, l.name))
            .collect();
        keys.sort_unstable();
        // `RMC_ANSWER_DUMP_DIR=<dir>`: the sorted locations themselves, one
        // file per operation, for diffing two runs whose fingerprints differ.
        if let Ok(dir) = std::env::var("RMC_ANSWER_DUMP_DIR") {
            let file = PathBuf::from(dir).join(operation.replace(' ', "_"));
            std::fs::write(&file, keys.join("\n")).expect("write the answer dump");
        }
        // `DefaultHasher::new()` has fixed keys: the same binary prints the
        // same fingerprint for the same answer in every run.
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        keys.hash(&mut hasher);
        println!("answer {operation:<28} {:>6} location(s), fingerprint {:016x}", keys.len(), hasher.finish());
    }

    fn gib(bytes: i64) -> f64 {
        bytes as f64 / (1u64 << 30) as f64
    }

    /// One line per portrait: `held`, then every line of the shipped cut
    /// (`memory_breakdown`, as `analysis_memory` serves it). Returns `held`.
    fn print_portrait(service: &mut SemanticService, project: &Path, stage: &str) -> i64 {
        let report = service
            .memory_breakdown(project, 0)
            .expect("the probe project is loaded");
        let process = report.process.expect("a portrait on linux");
        let lines: Vec<String> = process
            .lines
            .iter()
            .map(|line| format!("{} {:.2}", line.owner, gib(line.bytes)))
            .collect();
        println!(
            "{stage:<16} held {:.2} GiB | {} | anon_huge {:.2}",
            gib(process.held_bytes),
            lines.join(" | "),
            gib(anon_huge_bytes())
        );
        process.held_bytes
    }

    /// Anonymous memory the kernel backs with transparent huge pages, from
    /// `/proc/self/smaps_rollup`. Under THP `always`, khugepaged collapses a
    /// 2 MiB range into one huge page while up to `max_ptes_none` (511) of its
    /// 512 small pages are absent, so a heap whose holes were trimmed can be
    /// resident again in full; this is the number that tells it.
    fn anon_huge_bytes() -> i64 {
        std::fs::read_to_string("/proc/self/smaps_rollup")
            .expect("smaps_rollup readable on linux")
            .lines()
            .find_map(|line| {
                let kib = line.strip_prefix("AnonHugePages:")?.split_whitespace().next()?;
                Some(kib.parse::<i64>().expect("AnonHugePages in KiB") * 1024)
            })
            .expect("AnonHugePages in /proc/self/smaps_rollup")
    }

    /// Who owns the heap the portrait calls `unattributed` — split by stage,
    /// and, under a heap profiler, by allocating stack.
    ///
    /// The stages: `loaded` is right after the workspace load (project model,
    /// VFS, crate graph, salsa inputs and the cache prefill the loader runs),
    /// before any query of ours; `queried` is after one reference search, the
    /// state the earlier probes weighed; `collected` is after a garbage
    /// collection on top of that, the daemon's steady state. The `unattributed` line of each,
    /// against the other, says whether the unowned heap comes with the load or
    /// with the queries.
    ///
    /// `RMC_UNOWNED_EXIT_AFTER=<stage>` ends the process right after that
    /// stage's portrait with `process::exit`, freeing nothing: under
    /// `heaptrack` every live allocation is then reported as leaked, with the
    /// stack that made it, which is the per-owner split the portrait cannot
    /// give. A normal return would drop the database first and leave nothing
    /// to look at.
    ///
    /// # Running it
    ///
    /// ```text
    /// RMC_UNOWNED_PROJECT=/home/user/project RMC_UNOWNED_SYMBOL=OneDeclManyUses \
    /// RMC_UNOWNED_EXIT_AFTER=queried \
    ///   heaptrack -o /scratch/unowned target/release/deps/rmc_server-<hash> \
    ///   --ignored --nocapture --exact semantic::tests::unowned_heap_by_load_stage
    /// ```
    #[test]
    #[ignore = "needs a real workspace in RMC_UNOWNED_PROJECT and minutes to run"]
    fn unowned_heap_by_load_stage() {
        let _sweep = sweeping_alone();
        let required = |name: &str| {
            std::env::var(name).unwrap_or_else(|_| {
                panic!(
                    "set {name}; without it this probe would measure nothing and pass, which is \
                     worse than not running"
                )
            })
        };
        let project = PathBuf::from(required("RMC_UNOWNED_PROJECT"));
        let symbol = required("RMC_UNOWNED_SYMBOL");
        let exit_after = std::env::var("RMC_UNOWNED_EXIT_AFTER").ok();
        let stage_done = |service: &mut SemanticService, stage: &str| {
            print_portrait(service, &project, stage);
            // The rows a heap profile is held against: per ingredient, what it
            // says it owns. One above a MiB, or reporting heap at all, is printed.
            let report = service.memory_breakdown(&project, 0).expect("the probe project is loaded");
            for row in report
                .rows
                .iter()
                .filter(|row| row.heap_bytes.is_some() || row.bytes() > 1 << 20)
            {
                println!(
                    "  {:<8} {:<52} {:>9} stack {:>8} KiB meta {:>8} KiB heap {:>9} KiB",
                    row.family,
                    row.name,
                    row.count,
                    row.stack_bytes / 1024,
                    row.metadata_bytes / 1024,
                    row.heap_bytes.map_or("-".to_owned(), |heap| (heap / 1024).to_string()),
                );
            }
            if exit_after.as_deref() == Some(stage) {
                println!("exiting after `{stage}` without freeing anything");
                std::process::exit(0);
            }
        };

        let mut service = SemanticService::new();
        let started = std::time::Instant::now();
        service.get_or_load_full(&project).expect("load the probe workspace");
        println!("load: {:.1} s", started.elapsed().as_secs_f64());
        stage_done(&mut service, "loaded");

        let started = std::time::Instant::now();
        let found = service
            .find_references_by_name_with_exact(&project, &symbol, true)
            .expect("reference search on the probe workspace");
        println!(
            "query: {} location(s) for `{symbol}` in {:.1} s",
            found.len(),
            started.elapsed().as_secs_f64()
        );
        assert!(!found.is_empty(), "`{symbol}` has no references: the query inferred nothing");
        stage_done(&mut service, "queried");

        // The daemon's steady state: what is left once a collection has
        // evicted the LRU-capped memos — the heap salsa's own bookkeeping is
        // weighed against (`salsa-overhead-census`).
        service.collect_garbage();
        stage_done(&mut service, "collected");
    }

    /// Whose file texts the database holds: library or workspace roots, and
    /// whether the file is a module of any crate at all.
    ///
    /// `FileText` is a salsa input — no LRU, alive as long as the database —
    /// and the loader puts every `.rs` file of every package root into it,
    /// whether a crate's module tree reaches it or not (a dependency's tests,
    /// benches, examples). This splits the bytes by those two axes, which is
    /// what decides the lever: texts no module reaches are only read by
    /// `include!` and friends, library texts never change.
    ///
    /// ```text
    /// RMC_FILE_TEXT_PROJECT=/home/user/project RMC_FILE_TEXT_SYMBOL=OneDeclManyUses \
    ///   cargo test --release -p rmc-server --lib -- --ignored --nocapture file_text_by_root_kind
    /// ```
    #[test]
    #[ignore = "needs a real workspace in RMC_FILE_TEXT_PROJECT and minutes to run"]
    fn file_text_by_root_kind() {
        use ra_ap_ide_db::base_db::SourceDatabase;
        let _sweep = sweeping_alone();
        let required = |name: &str| {
            std::env::var(name).unwrap_or_else(|_| panic!("set {name}: the probe has no default"))
        };
        let project = PathBuf::from(required("RMC_FILE_TEXT_PROJECT"));
        let symbol = required("RMC_FILE_TEXT_SYMBOL");

        let service = SemanticService::new();
        let found = service
            .find_references_by_name_with_exact(&project, &symbol, true)
            .expect("reference search on the probe workspace");
        assert!(!found.is_empty(), "`{symbol}` has no references: the load inferred nothing");

        let canonical = project.canonicalize().expect("canonicalize the probe workspace");
        // (library root, module of some crate) -> (files, text bytes)
        let mut split: std::collections::BTreeMap<(bool, bool), (usize, usize)> =
            Default::default();
        service.with_context_for_test(&canonical, |ctx| {
            let db = ctx.host.raw_database();
            let sema = ra_ap_ide::Semantics::new(db);
            for (file, _path) in read_vfs(&ctx.vfs).iter() {
                let root = db.file_source_root(file).source_root_id(db);
                let library = db.source_root(root).source_root(db).is_library;
                let bytes = db.file_text(file).text(db).len();
                let module = sema.file_to_module_defs(file).next().is_some();
                let entry = split.entry((library, module)).or_default();
                entry.0 += 1;
                entry.1 += bytes;
            }
        });
        for ((library, module), (files, bytes)) in &split {
            println!(
                "{:<9} {:<11} {files:>7} file(s) {:>8.1} MiB",
                if *library { "library" } else { "workspace" },
                if *module { "module" } else { "no module" },
                *bytes as f64 / (1u64 << 20) as f64
            );
        }
        assert!(
            split.keys().any(|&(_, module)| module),
            "no file is a module of any crate: the split measured nothing"
        );
    }

    /// What the crate def maps of a workspace are made of, field by field
    /// (`hir_def::heap_size::DefMapCensus`): the `DefMapPair` row of the
    /// memory report split into the containers that hold it, each with the
    /// bytes `heap_size` charges against what its elements alone take, plus
    /// the item-scope names by origin (own, import, glob) and the heaviest
    /// crates. The question is whether the bytes are a hole of form (spare
    /// capacity, a container that should not exist) or simply the names.
    ///
    /// ```text
    /// RMC_DEF_MAP_PROJECT=/home/user/project \
    ///   cargo test --release -p rmc-server --lib -- --ignored --nocapture def_map_composition
    /// ```
    #[test]
    #[ignore = "needs a real workspace in RMC_DEF_MAP_PROJECT and minutes to run"]
    fn def_map_composition() {
        use ra_ap_hir_def::heap_size::{DefMapCensus, Weight};
        use ra_ap_ide_db::base_db::all_crates;
        let _sweep = sweeping_alone();
        let project = PathBuf::from(
            std::env::var("RMC_DEF_MAP_PROJECT")
                .unwrap_or_else(|_| panic!("set RMC_DEF_MAP_PROJECT: the probe has no default")),
        );

        let service = SemanticService::new();
        let started = std::time::Instant::now();
        service.get_or_load_full(&project).expect("load the probe workspace");
        println!("load: {:.1} s", started.elapsed().as_secs_f64());

        let canonical = project.canonicalize().expect("canonicalize the probe workspace");
        let mut census = DefMapCensus::default();
        let mut per_crate: Vec<(usize, String, usize)> = Vec::new();
        service.with_context_for_test(&canonical, |ctx| {
            let db = ctx.host.raw_database();
            for &krate in all_crates(db).iter() {
                let before = census.bytes();
                let modules = census.modules;
                census.add_crate(db, krate);
                let name = krate
                    .extra_data(db)
                    .display_name
                    .as_ref()
                    .map_or_else(|| format!("{krate:?}"), |name| name.to_string());
                per_crate.push((census.bytes() - before, name, census.modules - modules));
            }
        });
        assert!(
            census.def_maps > 0 && census.bytes() > 0,
            "no def map weighed: the census measured nothing"
        );

        let mib = |bytes: usize| bytes as f64 / (1u64 << 20) as f64;
        let total = census.bytes();
        println!(
            "{} def maps, {} modules, {:.1} MiB charged",
            census.def_maps,
            census.modules,
            mib(total)
        );
        let mut rows: Vec<(&str, Weight)> =
            census.rows.iter().map(|(row, weight)| (*row, *weight)).collect();
        rows.sort_by_key(|(_, weight)| std::cmp::Reverse(weight.bytes));
        println!(
            "{:<42} {:>9} {:>11} {:>10} {:>10} {:>6}",
            "row", "containers", "entries", "MiB", "filled MiB", "fill"
        );
        for (row, weight) in rows {
            if weight.bytes == 0 {
                continue;
            }
            println!(
                "{row:<42} {:>9} {:>11} {:>10.1} {:>10.1} {:>5.0}%",
                weight.containers,
                weight.entries,
                mib(weight.bytes),
                mib(weight.filled_bytes),
                100.0 * weight.filled_bytes as f64 / weight.bytes as f64
            );
        }
        for (origin, entries) in &census.scope_entries_by_origin {
            println!("scope entries {origin:<22} {entries:>11}");
        }
        per_crate.sort_by_key(|(bytes, ..)| std::cmp::Reverse(*bytes));
        for (bytes, name, modules) in per_crate.iter().take(25) {
            println!("crate {name:<40} {modules:>6} module(s) {:>8.1} MiB", mib(*bytes));
        }
    }

    /// What an LRU capacity on `MacroCallId::macro_arg` buys and costs.
    ///
    /// The argument token trees are the second largest salsa line of the live
    /// daemon on a large workspace (0.42 million memos, 0.27 GB of heap) and,
    /// upstream, never evicted. The fork caps them; this prices the cap at
    /// both ends, one capacity per run, set at load with `RMC_LRU_CAPS`
    /// (`MacroCallId::macro_arg_=0` is upstream's unbounded cache):
    ///
    /// * bytes — the `macro_arg` heap after a collection, and the portrait;
    /// * reads — reference searches after a collection, the daemon's steady
    ///   state: an evicted argument is rebuilt only if its expansion was
    ///   evicted too and something asks for it again;
    /// * edits — the part the cap really charges. An evicted argument cannot
    ///   backdate (salsa compares against the old value, and there is none),
    ///   so an edit to a calling file re-expands its calls where the unbounded
    ///   cache stops at the argument. Each file in `RMC_MACRO_ARG_EDIT` gets a
    ///   trailing comment, then the first work search is timed against the
    ///   same search after a plain collection.
    ///
    /// The project is MUTATED (each edit is undone after its query), so point
    /// it at a scratch copy.
    ///
    /// Any other capped query is priced the same way: name it in `RMC_LRU_CAPS`
    /// and list the rows to weigh in `RMC_LRU_PROBE_WEIGH` (comma-separated
    /// salsa names; default the three above). The edit appends a comment at the
    /// end of the file, so it shifts no item — the usual keystroke.
    ///
    /// ```text
    /// RMC_LRU_CAPS=MacroCallId::macro_arg_=0 \
    /// RMC_MACRO_ARG_PROJECT=/scratch/copy RMC_MACRO_ARG_SYMBOL=OneDeclManyUses \
    /// RMC_MACRO_ARG_WORK=A,B RMC_MACRO_ARG_EDIT=src/a.rs,src/b.rs \
    ///   cargo test --release -p rmc-server --lib -- --ignored --nocapture macro_arg_capacity
    /// ```
    #[test]
    #[ignore = "mutates the workspace in RMC_MACRO_ARG_PROJECT and takes minutes"]
    fn macro_arg_capacity_prices_reads_and_edits() {
        use std::time::Instant;
        let _sweep = sweeping_alone();
        let required = |name: &str| {
            std::env::var(name).unwrap_or_else(|_| panic!("set {name}: the probe has no default"))
        };
        let list = |name: &str| -> Vec<String> {
            required(name)
                .split(',')
                .map(|it| it.trim().to_owned())
                .filter(|it| !it.is_empty())
                .collect()
        };
        let project = PathBuf::from(required("RMC_MACRO_ARG_PROJECT"));
        let symbol = required("RMC_MACRO_ARG_SYMBOL");
        let work = list("RMC_MACRO_ARG_WORK");
        let edits = list("RMC_MACRO_ARG_EDIT");
        assert!(!work.is_empty() && !edits.is_empty(), "name work symbols and files to edit");
        let rounds = env_usize("RMC_MACRO_ARG_ROUNDS", 5);
        println!(
            "{}={}",
            loader::QUERY_LRU_CAPS_ENV,
            std::env::var(loader::QUERY_LRU_CAPS_ENV).unwrap_or_else(|_| "unset".to_owned())
        );

        // The rows to weigh at each stage: the macro trio by default, any other
        // capped query (and its `of`) through `RMC_LRU_PROBE_WEIGH`, so the same
        // probe prices the next cap without a copy of itself.
        let weighed: Vec<String> = match std::env::var("RMC_LRU_PROBE_WEIGH") {
            Ok(_) => list("RMC_LRU_PROBE_WEIGH"),
            Err(_) => [
                "MacroCallId::macro_arg_",
                "MacroCallId::parse_macro_expansion_",
                "EditionedFileId::parse_",
            ]
            .map(str::to_owned)
            .to_vec(),
        };
        let weigh = |service: &mut SemanticService, stage: &str| {
            let report = service.memory_breakdown(&project, 0).expect("the probe project is loaded");
            let cells: Vec<String> = weighed
                .iter()
                .map(|query| {
                    let row = report
                        .rows
                        .iter()
                        .find(|row| row.name == *query)
                        .unwrap_or_else(|| panic!("no `{query}` row: the probe loaded nothing"));
                    let heap = row.heap_bytes.unwrap_or_else(|| panic!("`{query}` reports no heap"));
                    format!("{query} {} memos {:.1} MiB", row.count, heap as f64 / (1u64 << 20) as f64)
                })
                .collect();
            println!("{stage:<14} {}", cells.join(" | "));
        };
        let refs = |service: &mut SemanticService, name: &str| {
            let started = Instant::now();
            let found = service
                .find_references_by_name_with_exact(&project, name, true)
                .expect("reference search on the probe workspace");
            assert!(!found.is_empty(), "`{name}` has no references");
            (found, started.elapsed().as_millis())
        };
        let median = |mut ms: Vec<u128>| {
            ms.sort_unstable();
            ms[ms.len() / 2]
        };

        let mut service = SemanticService::new();
        let (found, load_ms) = refs(&mut service, &symbol);
        println!("load + refs {symbol}: {:.1} s, peak RSS {}", load_ms as f64 / 1000.0, vm_hwm());
        print_answer(&format!("refs {symbol}"), &found);
        weigh(&mut service, "loaded");
        service.collect_garbage();
        weigh(&mut service, "collected");
        crate::mcp::memory::release_and_measure();
        print_portrait(&mut service, &project, "collected+rel");

        let mut reads: Vec<Vec<u128>> = vec![Vec::new(); work.len()];
        for round in 0..rounds {
            for (name, ms) in work.iter().zip(reads.iter_mut()) {
                service.collect_garbage();
                let (found, took) = refs(&mut service, name);
                ms.push(took);
                if round == 0 {
                    print_answer(&format!("refs {name}"), &found);
                }
            }
        }
        for (name, ms) in work.iter().zip(&reads) {
            println!("read  refs {name:<24} median {:>6} ms over {} call(s)", median(ms.clone()), ms.len());
        }
        weigh(&mut service, "after reads");

        let mut edited_ms = Vec::new();
        for file in &edits {
            let path = project.join(file);
            let original = std::fs::read_to_string(&path).expect("read the file to edit");
            service.collect_garbage();
            std::fs::write(&path, format!("{original}\n// macro_arg capacity probe\n"))
                .expect("edit the file");
            let (found, took) = refs(&mut service, &work[0]);
            edited_ms.push(took);
            println!("edit  {file:<40} then refs {}: {took:>6} ms", work[0]);
            print_answer(&format!("refs {} after edit", work[0]), &found);
            std::fs::write(&path, original).expect("undo the edit");
            // Settle the undo before the next file, so its query does not pay
            // for two edits.
            refs(&mut service, &work[0]);
        }
        println!(
            "edit  median {:>6} ms against read median {:>6} ms for refs {}",
            median(edited_ms),
            median(reads[0].clone()),
            work[0]
        );
        weigh(&mut service, "after edits");
        service.collect_garbage();
        crate::mcp::memory::release_and_measure();
        print_portrait(&mut service, &project, "end+rel");
        println!("peak RSS {}", vm_hwm());
    }

    fn env_usize(name: &str, default: usize) -> usize {
        std::env::var(name)
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(default)
    }
}

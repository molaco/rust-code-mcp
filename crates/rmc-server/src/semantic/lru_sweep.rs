//! What an LRU capacity costs, measured at both ends.
//!
//! The track that ends here named its lever with numbers: `EditionedFileId::parse`
//! (capacity 128) and `MacroCallId::parse_macro_expansion` (1024) hold 5.5 GB of
//! the 7.1 GB the memory breakdown can see on this workspace. The knob for both
//! is a single number — `RA_LRU_CAP`, which `load_workspace` reads and hands to
//! `RootDatabase::new`, which spreads it as parse = N, macro expansion = 4N, ast
//! id maps = 8N (see `update_base_query_lru_capacities` in the fork).
//!
//! Turning that number down hands memory back and makes the next query rebuild
//! what was thrown away. Neither half is an argument on its own — "it saved two
//! gigabytes" says nothing without "and every query after it re-parsed the
//! workspace" — so this module measures both against the same probe on the same
//! database:
//!
//! * **memory**, as the heap the instrumented queries report before and after a
//!   collection. That is the only allocator-independent signal available: RSS
//!   cannot serve as the oracle, because glibc need not return freed pages, and
//!   the memo *count* cannot either, because evicting a memo blanks its value
//!   and keeps the slot;
//! * **latency**, as the wall time of repeating the probe once the eviction has
//!   happened, beside the time of repeating it again with everything back in
//!   cache. The gap between those two is what the capacity charges per query.
//!
//! # What this measures, and what it does not
//!
//! A steady state, not a peak. Production applies the capacity at load time, so
//! a small capacity there also caps the high-water mark while the workspace is
//! being read; here the database is loaded first and retuned afterwards. The
//! bytes a capacity holds *at rest* are the same either way; the peak is not,
//! and this cannot see it.
//!
//! The capacity is applied through the same call production makes —
//! `update_base_query_lru_capacities(Some(n))`, which is literally what
//! `RootDatabase::new` does with `RA_LRU_CAP` — rather than through a map
//! assembled here, so that what is priced is the knob that would be turned.
//!
//! # Two traps, both found by these tests failing
//!
//! `capacity == 0` **disables** eviction: an unbounded cache, not an empty one.
//! `1` is the smallest cache that still evicts. That sentence is a test below,
//! because it is the one mistake that would make a whole sweep read backwards.
//!
//! Worse, zero does not merely stop evicting — it **forgets the use order**.
//! `Lru::set_capacity(0)` drops the tracking set, and `record_use` records
//! nothing while the capacity is none (salsa's `function/eviction/lru.rs`), so
//! restoring a capacity afterwards does not restore eviction: until fresh
//! queries have recorded uses again, there is nothing to evict and a collection
//! hands back nothing. A sweep that walked capacities on one database without
//! knowing this measured a knob that had quietly turned itself off — which is
//! how the first run of these tests failed.
//!
//! That damage outlives the row it happens in: every capacity priced after a
//! zero can only evict what has been *used since*, which on this workload is
//! almost nothing (see the cost end below), so those rows read as "the knob
//! stopped working". A sweep therefore refuses a zero in its list — see
//! [`sweep_capacities`] — and the zero case is measured on its own instead.
//!
//! # Where the cost actually falls
//!
//! Not on repeats, which is where this module first looked for it. Measured:
//! evicting 12.2 MB of parse trees down to one and then repeating the very same
//! reference search costs *nothing* and brings *nothing* back — both read paths
//! of this daemon are answered out of memos above the parse query, and
//! re-validating those against unchanged files never asks for a tree again. What
//! pays is a **change**: after one edited file, 0.77 MB of the 12.2 came back.
//! So a smaller parse capacity charges the daemon on the queries that follow an
//! edit, and only for what the edit invalidated.

use std::path::Path;
use std::time::{Duration, Instant};

use super::{IngredientMemory, SemanticService, ingredient_rows};

/// The salsa `debug_name` of the parse query, which is what a capacity is
/// addressed by and what the rows below are keyed on.
///
/// The trailing underscore is the salsa macro's own name for the ingredient of
/// an *associated* tracked fn. It cannot be guessed and must not be tidied away:
/// the readable `EditionedFileId::parse` matches nothing, which is exactly how
/// this knob spent five sessions looking alive while retuning nothing.
const PARSE_QUERY: &str = "EditionedFileId::parse_";

/// The macro expansion query — the other half of the lever, and the larger one
/// on a real workspace (3.26 GB against 2.26 GB when last measured).
const MACRO_QUERY: &str = "MacroCallId::parse_macro_expansion_";

/// What fills the database before it is weighed, and what is repeated to price
/// the rebuild.
///
/// The probe decides every number a sweep prints: two runs are comparable only
/// if they probed alike.
/// # Neither probe is cheap any more
///
/// Earlier sessions of this track called `symbols` the cheap one and measured it
/// at 818 MB and 15 s on a 4000-file workspace. That was a `Fast` context, and it
/// is gone: since 2026-08-28 `symbol_search` loads the same `Full` context
/// references do, because a `Fast` one has `set_test` off and answered *no
/// definition found* for anything behind `#[cfg(test)]`. Measured after that
/// change, a `symbols` sweep on a large workspace passed **13.2 GB** and was still
/// climbing — on a machine whose swap was already full it had to be killed.
///
/// Two consequences worth carrying: run either probe only on a quiet machine
/// with room to spare, and do not compare a number taken today against one from
/// sessions six to nine — the context under them differs.
pub(super) enum Probe {
    /// A symbol search. Runs no type inference; that is now the *only* thing
    /// that makes it lighter than the other.
    Symbols { name: String },
    /// Chase every use of a name. The only probe that infers types, and the same
    /// path `find_references` serves in production.
    References { name: String },
}

impl Probe {
    /// Read the probe out of the environment, sharing the variables the memory
    /// breakdown test already documents.
    ///
    /// There is deliberately no default symbol for `references`: the name decides
    /// both what the probe costs and whether it proves anything. `new` was the
    /// obvious guess and matches up to fifty declarations, each sweeping the tree
    /// for its own uses — measured at 10 GB and still climbing after two minutes.
    fn from_env() -> Self {
        let kind = std::env::var("RMC_SALSA_MEMORY_PROBE").unwrap_or_else(|_| "symbols".to_string());
        let name = std::env::var("RMC_SALSA_MEMORY_SYMBOL").unwrap_or_default();
        match kind.as_str() {
            "symbols" => Probe::Symbols {
                name: if name.is_empty() { "main".to_string() } else { name },
            },
            "references" => {
                assert!(
                    !name.is_empty(),
                    "the `references` probe needs RMC_SALSA_MEMORY_SYMBOL: a name with one \
                     declaration and many use sites in the probe workspace"
                );
                Probe::References { name }
            }
            other => panic!(
                "RMC_SALSA_MEMORY_PROBE={other:?} names no probe; use `symbols` (cheap, runs no \
                 type inference) or `references` (infers every body mentioning \
                 RMC_SALSA_MEMORY_SYMBOL)"
            ),
        }
    }

    /// How many answers it found. The number itself is not the point — that it
    /// stays the same across capacities is: a cache knob that changes an answer
    /// is not a knob, it is a bug.
    fn run(&self, service: &mut SemanticService, project: &Path) -> usize {
        match self {
            Probe::Symbols { name } => service
                .symbol_search(project, name, 16)
                .expect("symbol search on the probe workspace")
                .len(),
            Probe::References { name } => service
                .find_references_by_name_with_exact(project, name, true)
                .expect("reference search on the probe workspace")
                .len(),
        }
    }

    fn describe(&self) -> String {
        match self {
            Probe::Symbols { name } => format!("symbols `{name}`"),
            Probe::References { name } => format!("references of `{name}`"),
        }
    }
}

/// One capacity, priced.
pub(super) struct Priced {
    /// `None` is the capacity a daemon runs at today — whatever
    /// `DEFAULT_PARSE_LRU_CAP` is — rather than a missing measurement.
    pub capacity: Option<u16>,
    /// Memo slots of the parse query before the collection. Guards against a
    /// vacuous pass: a workspace that memoized no more than the capacity gives
    /// eviction nothing to do, and a green run there would prove nothing.
    pub parse_memos: usize,
    /// Parse-query heap before the collection, after it, and after the probe was
    /// repeated. The third one is what says the rebuild the latency prices
    /// actually happened.
    pub parse_heap_before: usize,
    pub parse_heap_after: usize,
    pub parse_heap_rebuilt: usize,
    pub macro_heap_after: usize,
    /// Everything the collection handed back, per query, largest first. Only
    /// LRU-capped queries appear: the rest keep every byte whatever the capacity.
    pub freed: Vec<(&'static str, usize)>,
    /// What the whole breakdown can see after the collection. Not a budget — see
    /// `memory_breakdown` for why the gap to RSS is the finding, not a defect.
    pub accounted_after: usize,
    /// The probe run under this capacity *before* the collection — the row's own
    /// reference point: the same query, on the same database, with nothing
    /// thrown away yet.
    ///
    /// No assertion judges it; it is a measurement, and the module says so
    /// rather than implying coverage it does not have. It was written believing
    /// it also re-recorded the uses eviction sorts by, which a mutation
    /// disproved — a repeat fetches no parse memos, so it records none.
    pub settle: Duration,
    /// First repeat of the probe, paying for whatever was evicted…
    pub rebuild: Duration,
    /// …and the one after it, with the caches full again. The difference is the
    /// capacity's price per query.
    pub warm: Duration,
    pub answers: usize,
    pub rss_kib_after: u64,
}

impl Priced {
    /// Bytes the collection handed back at this capacity.
    pub(super) fn freed_total(&self) -> usize {
        self.freed.iter().map(|&(_, bytes)| bytes).sum()
    }

    fn capacity_label(&self) -> String {
        self.capacity.map_or_else(
            || format!("default({})", ra_ap_ide_db::base_db::DEFAULT_PARSE_LRU_CAP),
            |cap| cap.to_string(),
        )
    }
}

/// Slots and heap of one query in a breakdown, `(0, 0)` when it is absent.
///
/// A query with no `heap_size` reports `None`, and that is counted as zero here
/// rather than guessed at — the callers below only ever ask about the two
/// queries the fork instruments.
fn query_bytes(rows: &[IngredientMemory], query: &str) -> (usize, usize) {
    rows.iter()
        .find(|row| row.family == "query" && row.name == query)
        .map_or((0, 0), |row| (row.count, row.heap_bytes.unwrap_or(0)))
}

fn rows_of(service: &mut SemanticService, canonical: &Path) -> Vec<IngredientMemory> {
    service.with_context_for_test(canonical, |ctx| ingredient_rows(&mut ctx.host))
}

/// The capacities a sweep will walk, led by `None` — the capacity the daemon
/// runs at today, which is the row every decision is measured against.
///
/// Rejects a zero rather than pricing it. Zero does not evict, and worse, it
/// clears the recorded use order (module docs), so every row after it can only
/// evict what has been used since and reads as a knob that stopped working. A
/// sweep that quietly printed those rows would be worse than one that refuses.
pub(super) fn sweep_capacities(spec: &str) -> Vec<Option<u16>> {
    let mut capacities = vec![None];
    for value in spec.split(',').map(str::trim).filter(|value| !value.is_empty()) {
        let capacity: u16 = value.parse().unwrap_or_else(|_| {
            panic!("RMC_LRU_SWEEP holds {value:?}, which is not a capacity; use whole numbers")
        });
        assert!(
            capacity > 0,
            "RMC_LRU_SWEEP asks for a capacity of 0. Zero DISABLES eviction rather than emptying \
             the cache, and it also drops salsa's record of which memos were used, so every row \
             priced after it would understate what a capacity frees. The smallest cache that \
             still evicts is 1; the zero case has its own test"
        );
        capacities.push(Some(capacity));
    }
    assert!(
        capacities.len() > 1,
        "RMC_LRU_SWEEP named no capacity to price, so the sweep would report only the default \
         row and prove nothing about the knob"
    );
    capacities
}

/// Set the base capacity on a loaded analysis, exactly as `RootDatabase::new`
/// does with `RA_LRU_CAP`. `None` means the default the daemon runs at.
///
/// Panics rather than returns on a name that matched nothing: a sweep that
/// quietly priced a capacity it never applied would be worse than no sweep, and
/// the count of unapplied names is the one thing that can tell the difference.
pub(super) fn apply_capacity(
    service: &mut SemanticService,
    canonical: &Path,
    capacity: Option<u16>,
) {
    let missed = service.with_context_for_test(canonical, |ctx| {
        ctx.host
            .raw_database_mut()
            .update_base_query_lru_capacities(capacity)
    });
    assert_eq!(
        missed, 0,
        "{missed} of the capacities rust-analyzer sets match no query in this database, so this \
         row would price a capacity that was never applied; the names are salsa `debug_name`s and \
         an upstream rename is the usual cause"
    );
}

/// Live parse-tree bytes and memo slots, the pair every assertion here is about.
pub(super) fn parse_bytes(service: &mut SemanticService, canonical: &Path) -> (usize, usize) {
    query_bytes(&rows_of(service, canonical), PARSE_QUERY)
}

/// Apply one capacity to an already-loaded analysis, collect, and time what the
/// queries around the collection cost.
///
/// The probe runs three times: once under the new capacity before the collection
/// (the row's reference time), once immediately after it, and once more with the
/// caches full again. Three rather than two so that every row can be read on its
/// own instead of against the row above it.
pub(super) fn price_capacity(
    service: &mut SemanticService,
    project: &Path,
    capacity: Option<u16>,
    probe: &Probe,
) -> Priced {
    let canonical = project
        .canonicalize()
        .expect("canonicalize the probe project");

    apply_capacity(service, &canonical, capacity);

    let started = Instant::now();
    let settled_answers = probe.run(service, project);
    let settle = started.elapsed();

    let before = rows_of(service, &canonical);
    let (parse_memos, parse_heap_before) = query_bytes(&before, PARSE_QUERY);

    // The bump. Capacities mean nothing without it: salsa evicts inside
    // `reset_for_new_revision`, so a database nobody writes to keeps every memo
    // it ever made, however small the capacity.
    service.collect_garbage();

    let after = rows_of(service, &canonical);
    let (_, parse_heap_after) = query_bytes(&after, PARSE_QUERY);
    let (_, macro_heap_after) = query_bytes(&after, MACRO_QUERY);
    let accounted_after = after.iter().map(IngredientMemory::bytes).sum();

    let mut freed: Vec<(&'static str, usize)> = before
        .iter()
        .filter_map(|row| {
            let was = row.heap_bytes?;
            let (_, now) = query_bytes(&after, row.name);
            was.checked_sub(now).filter(|freed| *freed > 0).map(|freed| (row.name, freed))
        })
        .collect();
    freed.sort_by_key(|&(_, bytes)| std::cmp::Reverse(bytes));

    // The cost end. The first repeat pays for whatever the collection threw
    // away; the second finds it all back in cache, which is what makes the
    // first number readable as a price rather than as the cost of the query.
    let started = Instant::now();
    let answers = probe.run(service, project);
    let rebuild = started.elapsed();
    let rebuilt = rows_of(service, &canonical);
    let (_, parse_heap_rebuilt) = query_bytes(&rebuilt, PARSE_QUERY);

    let started = Instant::now();
    let warm_answers = probe.run(service, project);
    let warm = started.elapsed();

    assert_eq!(
        (settled_answers, answers),
        (warm_answers, warm_answers),
        "the same probe answered {settled_answers}, then {answers}, then {warm_answers} on the \
         same unedited workspace, with a collection in between; an LRU capacity may cost time, \
         never an answer"
    );

    Priced {
        capacity,
        settle,
        parse_memos,
        parse_heap_before,
        parse_heap_after,
        parse_heap_rebuilt,
        macro_heap_after,
        freed,
        accounted_after,
        rebuild,
        warm,
        answers,
        rss_kib_after: crate::mcp::memory::rss_kib().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{Priced, Probe, price_capacity};
    use crate::deep_stack::test_sweeping_alone as sweeping_alone;
    use crate::semantic::SemanticService;

    /// A workspace of many small files, so that the parse query memoizes more
    /// than one entry and a capacity of one has something to throw away.
    ///
    /// Twelve rather than two: the assertions below are about a cache that
    /// evicts, and a cache holding a single entry cannot demonstrate one.
    fn many_file_workspace(root: &Path) -> PathBuf {
        fs::create_dir_all(root.join("src")).expect("create src");
        fs::write(
            root.join("Cargo.toml"),
            r#"
[package]
name = "lru_probe"
version = "0.1.0"
edition = "2021"

[lib]
path = "src/lib.rs"
"#,
        )
        .expect("write manifest");

        let mut lib = String::new();
        for module in 0..12 {
            lib.push_str(&format!("pub mod part{module};\n"));
            fs::write(
                root.join(format!("src/part{module}.rs")),
                format!(
                    "pub fn probe_target{module}() -> u32 {{\n    {module}\n}}\n\n\
                     pub struct Carrier{module} {{\n    pub value: u32,\n}}\n\n\
                     impl Carrier{module} {{\n    pub fn probe_target_method{module}(&self) -> u32 {{\n        \
                     self.value + probe_target{module}()\n    }}\n}}\n"
                ),
            )
            .expect("write module");
        }
        fs::write(root.join("src/lib.rs"), lib).expect("write lib");
        root.to_path_buf()
    }

    /// Change one file of the fixture, leaving `probe_target0` and its use site
    /// alone: what the tests want from an edit is the invalidation, not a
    /// different answer.
    fn edit_a_file(root: &Path) {
        fs::write(
            root.join("src/part3.rs"),
            "pub fn probe_target3() -> u32 {\n    3\n}\n\npub fn added_by_the_edit() {}\n",
        )
        .expect("edit a fixture file");
    }

    /// Load the fixture and answer one query, so that the database has something
    /// memoized before any capacity is priced.
    fn probed_service(root: &Path) -> (SemanticService, Probe) {
        let probe = Probe::Symbols { name: "probe_target".to_string() };
        let mut service = SemanticService::new();
        let found = probe.run(&mut service, root);
        assert!(
            found > 0,
            "the fixture query found nothing, so nothing below is measuring a populated database"
        );
        (service, probe)
    }

    fn priced(service: &mut SemanticService, root: &Path, probe: &Probe, cap: Option<u16>) -> Priced {
        let priced = price_capacity(service, root, cap, probe);
        assert!(
            priced.parse_memos > cap.unwrap_or(0) as usize,
            "the fixture memoized {} parse entries against a capacity of {cap:?}, so eviction had \
             nothing to do and a pass here would prove nothing",
            priced.parse_memos
        );
        priced
    }

    /// The sweep's list, and the one value it must refuse. Cheap — no workspace
    /// — because what it guards is a reading error, not a rust-analyzer
    /// behaviour: a zero row poisons every row printed after it.
    #[test]
    fn a_sweep_list_leads_with_the_default_and_refuses_a_zero() {
        assert_eq!(
            super::sweep_capacities("128, 8,1"),
            vec![None, Some(128), Some(8), Some(1)],
            "the default capacity must lead the list: it is the row the others are read against"
        );

        let zero = std::panic::catch_unwind(|| super::sweep_capacities("32,0,8"));
        assert!(
            zero.is_err(),
            "a sweep accepted a capacity of 0. It evicts nothing and clears the recorded use \
             order, so the `8` row after it would have measured a knob that had turned itself off"
        );
        let empty = std::panic::catch_unwind(|| super::sweep_capacities(" , "));
        assert!(
            empty.is_err(),
            "a sweep with no capacity in its list would print one default row and read as a \
             finished measurement"
        );
    }

    /// The memory end, and the trap in its units, judged as a pair on one
    /// database — which is the only way to say that the difference came from the
    /// capacity rather than from the workspace.
    #[test]
    fn a_capacity_of_one_hands_bytes_back_and_a_capacity_of_zero_hands_back_nothing() {
        let _sweep = sweeping_alone();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = many_file_workspace(workspace.path());
        let (mut service, probe) = probed_service(&root);

        let evicting = priced(&mut service, &root, &probe, Some(1));
        assert!(
            evicting.parse_heap_after < evicting.parse_heap_before,
            "a capacity of one kept every one of {} parse bytes across a collection ({} memo \
             slots): the capacity is declared and nothing enforces it",
            evicting.parse_heap_before,
            evicting.parse_memos
        );
        assert!(
            evicting.freed_total() > 0,
            "the collection reported no query as having handed anything back, while the parse \
             query's own heap fell — the per-query accounting and the totals disagree"
        );

        // Zero disables eviction rather than emptying the cache. If this ever
        // starts freeing bytes, every sweep printed with a zero row has been
        // reading backwards.
        //
        // The edit is what makes this half mean anything, and it was added
        // because a mutation proved the half vacuous without it: repeating a
        // query does NOT refill the parse cache (see
        // `a_repeat_after_eviction_pays_nothing_...`), so after the eviction
        // above there was nothing left for a zero to wrongly throw away, and a
        // zero behaving exactly like a one would still have passed.
        edit_a_file(&root);
        probe.run(&mut service, &root);
        let disabled = priced(&mut service, &root, &probe, Some(0));
        assert!(
            disabled.parse_heap_before > evicting.parse_heap_after,
            "the edit brought no parse trees back ({} bytes, against {} left by the eviction), so \
             a zero capacity has nothing it could wrongly evict and the check below is empty",
            disabled.parse_heap_before,
            evicting.parse_heap_after
        );
        assert!(
            disabled.parse_heap_after >= disabled.parse_heap_before,
            "a capacity of zero freed {} bytes of parse trees; zero DISABLES eviction — the \
             smallest cache that still evicts is one",
            disabled.parse_heap_before.saturating_sub(disabled.parse_heap_after)
        );
    }

    /// Zero does not pause eviction, it **forgets**: the tracking set is dropped
    /// and nothing is recorded while the capacity is none, so a capacity handed
    /// back afterwards evicts nothing until fresh queries have run.
    ///
    /// This is not a curiosity. It is the difference between "set RA_LRU_CAP=0 to
    /// compare" and "measure a knob that has quietly turned itself off" — the
    /// first version of the test above walked capacities on one database in that
    /// order and read a working eviction as a broken one.
    #[test]
    fn a_capacity_of_zero_forgets_the_use_order_it_does_not_merely_pause_eviction() {
        let _sweep = sweeping_alone();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = many_file_workspace(workspace.path());
        let (mut service, probe) = probed_service(&root);
        let canonical = root.canonicalize().expect("canonicalize the fixture");

        // Zero, and no query after it: the set of recorded uses is now empty.
        super::apply_capacity(&mut service, &canonical, Some(0));
        // Restore a capacity that evicts — but still ask nothing.
        super::apply_capacity(&mut service, &canonical, Some(1));
        let (memos, heap_before) = super::parse_bytes(&mut service, &canonical);
        assert!(
            memos > 1,
            "the fixture holds {memos} parse memo slot(s); with nothing above the capacity this \
             proves nothing either way"
        );
        service.collect_garbage();
        let (_, heap_after) = super::parse_bytes(&mut service, &canonical);
        assert_eq!(
            heap_after, heap_before,
            "a capacity of one evicted {} bytes from a use set that a preceding zero had cleared; \
             if salsa ever starts keeping the order across a zero, the sweep may stop exercising \
             each capacity before collecting — and the module docs saying it must are wrong",
            heap_before.saturating_sub(heap_after)
        );

        // And the other half: once something makes the queries fetch parse trees
        // again, the restored capacity evicts. Without this the test would also
        // pass on a capacity that stayed broken forever.
        //
        // An *edit*, not a repeat: a repeated query is answered out of the memos
        // above the parse query and never fetches one, so it records no uses (see
        // `a_repeat_after_eviction_pays_nothing_...` for that measured on its
        // own). What is evicted below is therefore only what the edit touched —
        // everything memoized before the zero stays untracked and unreachable
        // for eviction, which is the shape of the damage a zero does.
        edit_a_file(&root);
        probe.run(&mut service, &root);
        let recovered = priced(&mut service, &root, &probe, Some(1));
        assert!(
            recovered.parse_heap_after < recovered.parse_heap_before,
            "after fresh uses were recorded under the restored capacity, a collection still \
             freed nothing — the capacity did not come back at all, which is a different defect \
             from forgetting"
        );
    }

    /// The cost end, and it is not where it was expected to be.
    ///
    /// The sweep was written assuming that a query repeated after an eviction
    /// pays to rebuild what was thrown away. Measured on this fixture, it pays
    /// **nothing**: eviction takes 12.2 MB of parse trees down to one, and the
    /// same reference search — the `find_references` production serves — answers
    /// again with the table still at one. Both of this daemon's read paths are
    /// answered out of memos that sit *above* the parse query (the symbol index,
    /// item trees, def maps), and re-validating those against an unchanged file
    /// never asks for the tree back.
    ///
    /// What does pay is a **change**. The edit below brings 0.77 MB of the 12.2
    /// back — the file that moved plus what its invalidation forced to recompute
    /// — and that is where a smaller parse capacity would charge the daemon.
    ///
    /// So the pair below is the finding and its positive control: the equality
    /// is the claim ("a repeat is free"), and the edit is what makes the equality
    /// mean something rather than being satisfied by a measurement that can see
    /// nothing at all.
    #[test]
    fn a_repeat_after_eviction_pays_nothing_and_an_edit_is_what_pays() {
        let _sweep = sweeping_alone();
        let workspace = tempfile::tempdir().expect("create workspace tempdir");
        let root = many_file_workspace(workspace.path());
        let (mut service, _) = probed_service(&root);
        let canonical = root.canonicalize().expect("canonicalize the fixture");
        let probe = Probe::References { name: "probe_target0".to_string() };

        // The answer to hold every later one against, taken from a plain run
        // rather than from a priced row: this test is about cost, and a second
        // priced capacity would cost three more probe runs to learn nothing.
        let baseline_answers = probe.run(&mut service, &root);
        let evicting = priced(&mut service, &root, &probe, Some(1));
        assert!(
            evicting.parse_heap_after < evicting.parse_heap_before / 2,
            "eviction at a capacity of one left {} of {} parse bytes; the rest of this test is \
             about what a *thorough* eviction costs, and there was not one",
            evicting.parse_heap_after,
            evicting.parse_heap_before
        );
        assert_eq!(
            evicting.parse_heap_rebuilt, evicting.parse_heap_after,
            "repeating the probe after the eviction brought parse trees back, so a repeat is no \
             longer free and the cost model in these docs is out of date: a smaller capacity now \
             charges every query, not only the ones that follow a change"
        );
        assert_eq!(
            baseline_answers, evicting.answers,
            "the probe found {} answers at the default capacity and {} at a capacity of one; \
             evicting a cache must cost time, never answers",
            baseline_answers, evicting.answers
        );

        // The positive control: a change *is* paid for. Without this, a
        // measurement that reported the same number forever would satisfy the
        // equality above and read as "repeats are free".
        let (_, before_edit) = super::parse_bytes(&mut service, &canonical);
        edit_a_file(&root);
        let after_edit_answers = probe.run(&mut service, &root);
        let (_, after_edit) = super::parse_bytes(&mut service, &canonical);
        assert!(
            after_edit > before_edit,
            "an edited file did not bring a single parse byte back ({before_edit} both before and \
             after), so this test cannot see a rebuild at all and its equality above proves \
             nothing"
        );
        assert_eq!(
            baseline_answers, after_edit_answers,
            "the edit was meant to change what the workspace costs, not what it answers"
        );
    }

    /// The sweep itself: one row per capacity, on a real workspace, priced at
    /// both ends.
    ///
    /// # Running it
    ///
    /// ```text
    /// RMC_SALSA_MEMORY_PROJECT=/home/user/project \
    ///   RMC_LRU_SWEEP=128,32,8,1 \
    ///   cargo test -p rmc-server --features migraphx -- --ignored --nocapture lru_capacity_sweep
    /// ```
    ///
    /// `RMC_SALSA_MEMORY_PROBE` / `RMC_SALSA_MEMORY_SYMBOL` choose what fills the
    /// database, exactly as for the breakdown test. Read the note on [`Probe`]
    /// before starting one: both probes now load a `Full` context, `references`
    /// wants about 12.5 GB and `symbols` was measured past 13.2 GB, and this
    /// sweep then keeps that database alive across every capacity in the list.
    /// It needs a quiet machine with room to spare — three attempts in this
    /// track have been killed by a desktop that had none.
    ///
    /// # Reading the table
    ///
    /// `freed` is what a collection at that capacity handed back; `rebuild` is
    /// the probe repeated immediately afterwards, `warm` the same probe once the
    /// caches are full again. `rebuild - warm` is the price the capacity charges
    /// the next query. Both ends have to be read together: this is a trade, and
    /// a row is not an argument for anything on its own.
    #[test]
    #[ignore = "needs a real workspace in RMC_SALSA_MEMORY_PROJECT and a quiet machine"]
    fn lru_capacity_sweep_prices_memory_against_latency() {
        let _sweep = sweeping_alone();

        let Ok(project) = std::env::var("RMC_SALSA_MEMORY_PROJECT") else {
            panic!(
                "set RMC_SALSA_MEMORY_PROJECT to a cargo workspace root; without one this test \
                 would measure nothing and pass, which is worse than not running"
            );
        };
        let project = PathBuf::from(project);
        let probe = Probe::from_env();
        // The default list brackets today's capacity (128) on the way down to the
        // smallest cache that still evicts.
        let capacities = super::sweep_capacities(
            &std::env::var("RMC_LRU_SWEEP").unwrap_or_else(|_| "128,32,8,1".to_string()),
        );

        let mut service = SemanticService::new();
        let started = std::time::Instant::now();
        let found = probe.run(&mut service, &project);
        println!(
            "probe {}: {found} answer(s), first run {:.1}s (load included)",
            probe.describe(),
            started.elapsed().as_secs_f64(),
        );

        let mut rows = Vec::new();
        for capacity in capacities {
            let priced = price_capacity(&mut service, &project, capacity, &probe);
            println!(
                "{:>12} parse {:>8} MB -> {:>8} MB   macro {:>8} MB   freed {:>8} MB   \
                 accounted {:>6} MB   settled {:>7.2}s   rebuild {:>7.2}s   warm {:>7.2}s   \
                 RSS {:>6} MB",
                priced.capacity_label(),
                priced.parse_heap_before / (1024 * 1024),
                priced.parse_heap_after / (1024 * 1024),
                priced.macro_heap_after / (1024 * 1024),
                priced.freed_total() / (1024 * 1024),
                priced.accounted_after / (1024 * 1024),
                priced.settle.as_secs_f64(),
                priced.rebuild.as_secs_f64(),
                priced.warm.as_secs_f64(),
                priced.rss_kib_after / 1024,
            );
            for (query, bytes) in priced.freed.iter().take(6) {
                println!("             freed {query:<44} {:>9} KiB", bytes / 1024);
            }
            rows.push(priced);
        }

        // Anti-vacuum, and the reason this test is worth running at all: if the
        // smallest capacity in the sweep freed nothing, the whole table prices a
        // knob that does not turn, and every number above is noise.
        let smallest = rows
            .iter()
            .filter(|row| row.capacity.is_some_and(|cap| cap > 0))
            .min_by_key(|row| row.capacity)
            .expect("the sweep list must contain at least one evicting capacity");
        assert!(
            smallest.freed_total() > 0,
            "a capacity of {:?} freed nothing on a workspace holding {} parse memo slots — the \
             capacity is not reaching the queries, and nothing above prices anything",
            smallest.capacity,
            smallest.parse_memos,
        );
    }
}

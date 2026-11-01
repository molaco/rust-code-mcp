//! Running rust-analyzer work on a stack deep enough for it.
//!
//! Work that loads a workspace through rust-analyzer walks HIR/AST trees
//! recursively — the graph tools building a hypergraph, the audits, and the
//! interner sweep inside a garbage collection alike. That recursion is bounded
//! by the *shape of the analyzed source*, not by anything we control, and it
//! does not fit in the 2 MiB stack a tokio blocking-pool thread gets by
//! default: building the hypergraph for this very workspace aborted the
//! process with
//!
//! ```text
//! thread 'tokio-rt-worker' has overflowed its stack
//! fatal runtime error: stack overflow, aborting
//! ```
//!
//! A stack overflow is an `abort`, not a `panic` — it takes the whole MCP
//! server down rather than failing one tool call, so this is not something a
//! caller can guard against. The work therefore carries its own stack instead
//! of depending on whoever spawned it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};

use rmcp::ErrorData as McpError;

use crate::semantic::SemanticService;

/// Who may be inside rust-analyzer at the same time.
///
/// Ordinary analysis takes it shared; a garbage collection takes it
/// exclusively, because its interner sweep is process-global and `unsafe`.
///
/// # The defect this closes
///
/// `hir::collect_ty_garbage` marks live types by refcount, so anything another
/// `AnalysisHost` *stores* survives it. What it cannot see is a type held by a
/// query still in flight — computed, not yet recorded anywhere. The collection
/// was documented as safe because "every path into the semantic service goes
/// through one mutex", and that much is true; what it missed is that the
/// semantic service is not the only rust-analyzer in this process. The graph
/// tools, the five audits and the skeleton builder each load a workspace of
/// their own, outside that mutex, and the watchdog fires a collection on a
/// timer — so a hypergraph build and a sweep could, and did, overlap.
///
/// The symptom is not a wrong answer: it is `SIGSEGV`, observed in a parallel
/// test run of this crate (the same suite is green run single-threaded, and
/// green again on a rerun, which is what a race looks like).
///
/// Making the gate live at this door rather than at the call sites is the point
/// — [`run_analysis`] is already the one place all such work passes through, so
/// a new tool gets the guarantee without knowing it exists.
///
/// # What it costs
///
/// Analyses still run concurrently with each other. A collection runs only at a
/// moment when none is inside — it does not queue for the gate (see
/// [`try_run_exclusive_analysis`]), because a queued writer would stop every new
/// analysis behind a load in progress — and the ones arriving during the sweep
/// wait for it. The sweep is short and fires on a timer
/// (`RMC_GC_INTERVAL_SECS`, 300s by default), so the added latency is bounded by
/// one sweep — against a crash that takes the whole daemon with it.
static ANALYSIS_GATE: RwLock<()> = RwLock::new(());

/// Enter the gate shared. A poisoned gate is stepped over rather than
/// propagated: it means some earlier analysis panicked, which says nothing
/// about whether it is safe to run this one, and refusing every analysis
/// afterwards would turn one failed tool call into a dead server.
fn enter_shared() -> RwLockReadGuard<'static, ()> {
    ANALYSIS_GATE.read().unwrap_or_else(|error| error.into_inner())
}

/// Exclusive entry that waits until it gets the gate — tests only; the served
/// collection skips instead (see [`try_run_exclusive_analysis`]).
///
/// It polls rather than calling `write()`, for the same reason the served path
/// does not wait: a writer parked in `write()` parks every reader arriving after
/// it, and the tests that judge "an analysis is not held up" would then fail on
/// whichever sweep test happened to be waiting beside them.
#[cfg(test)]
fn enter_exclusive() -> RwLockWriteGuard<'static, ()> {
    loop {
        match ANALYSIS_GATE.try_write() {
            Ok(guard) => return guard,
            Err(std::sync::TryLockError::Poisoned(poisoned)) => return poisoned.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
        }
    }
}

/// The same gate, for tests that drive a `SemanticService` directly instead of
/// through [`run_analysis`].
///
/// Those tests hold a loaded analysis for their whole body and the collection
/// tests call `collect_garbage` without going through
/// [`try_run_exclusive_analysis`] — so neither side takes this gate on its own, and
/// the sweep is free to land inside someone else's analysis. It does not fail an
/// assertion when it does; it takes a SIGSEGV and the whole test binary with it,
/// reported as "the suite crashed" with no clue which pair collided.
///
/// There used to be a second `RwLock` inside `semantic::tests` doing this job.
/// Two locks that must agree and cannot see each other agreed only by luck:
/// the heaviest analysis in the suite lives in `tools::graph::tests`, on the
/// other side of the divide, and the crash arrived the day something shifted the
/// schedule. One gate, the real one, or none.
#[cfg(test)]
pub(crate) fn test_holding_an_analysis() -> RwLockReadGuard<'static, ()> {
    enter_shared()
}

/// "I am the sweep" — see [`test_holding_an_analysis`].
#[cfg(test)]
pub(crate) fn test_sweeping_alone() -> RwLockWriteGuard<'static, ()> {
    enter_exclusive()
}

/// Set once, when the daemon retires; never cleared, because a retired process
/// does not come back — it drains and exits.
///
/// # The defect this closes
///
/// Retiring hands the socket to a successor and unloads this process's analyses,
/// but the sessions still attached keep asking — and every question used to load
/// a `Full` workspace again. Measured 2026-09-22: a daemon retired at 28.8 GB
/// reloaded a large workspace five times over its grace period (each time back to
/// 15–19 GB, each time unloaded on the next cooldown) while the successor beside
/// it grew its own copy. Retirement meant to free memory ended up holding two
/// analyses on one machine for half an hour, and the grace period then dropped
/// the three sessions anyway.
///
/// So a retired daemon admits no new rust-analyzer work at all. The sessions
/// draining on it still get everything that does not load an analysis (search,
/// the persisted graph queries, status); what they lose is only what they would
/// have lost at the deadline, and they are told how to get it back.
///
/// # Why process-global
///
/// For the same reason [`ANALYSIS_GATE`] is: [`run_analysis`] is the one door
/// every loading path passes through — the semantic service, the graph builder,
/// the audits, the skeleton — and most of them are handed no runtime state to
/// carry a flag in. Retirement is a property of the process, not of any one
/// service inside it. Tests do not touch this static; they drive
/// [`run_admitted`] with a flag of their own, so one test retiring cannot refuse
/// another test's analysis running beside it.
static RETIRED: AtomicBool = AtomicBool::new(false);

/// Stop admitting rust-analyzer work in this process. See [`RETIRED`].
///
/// Garbage collection is not affected: it goes through
/// [`try_run_exclusive_analysis`], never loads anything, and only gives memory
/// back — which is the one thing a retired daemon should still do.
pub(crate) fn refuse_new_analyses() {
    RETIRED.store(true, Ordering::SeqCst);
}

/// Either kind of gate guard, so one spawn path can hold whichever it took.
/// Held, never read.
enum GateGuard {
    Shared(#[allow(dead_code)] RwLockReadGuard<'static, ()>),
    Exclusive(#[allow(dead_code)] RwLockWriteGuard<'static, ()>),
}

/// Stack for the analysis thread.
///
/// Measured, not guessed: the default 2 MiB aborts on this workspace, 32 MiB
/// completes it. 64 MiB doubles the headroom that measurement gives us, and
/// costs only address space — thread stacks are mapped lazily, so the pages a
/// shallower walk never touches are never backed by memory.
const ANALYSIS_STACK_BYTES: usize = 64 * 1024 * 1024;

/// Run blocking rust-analyzer work on a dedicated thread with a deep stack, and
/// await its result.
///
/// Replaces `tokio::task::spawn_blocking` for this kind of work. The blocking
/// pool is otherwise the right tool — the point of the swap is solely the stack
/// size, which the pool does not let us set per task.
///
/// Refused once the process has retired — see [`RETIRED`].
pub(crate) async fn run_analysis<T, F>(what: &'static str, work: F) -> Result<T, McpError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    run_admitted(&RETIRED, what, work).await
}

/// [`run_analysis`] with the retirement flag passed in, so tests can retire a
/// flag of their own instead of the process's.
///
/// The check sits before the spawn, not inside the thread: a refused call must
/// cost nothing, and least of all a 64 MiB stack reservation and a wait at the
/// gate behind a garbage collection.
async fn run_admitted<T, F>(
    retired: &AtomicBool,
    what: &'static str,
    work: F,
) -> Result<T, McpError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    if retired.load(Ordering::SeqCst) {
        return Err(McpError::internal_error(retired_refusal(what), None));
    }
    // Shared entry waits for the gate, so it always runs.
    Ok(spawn_gated(what, work, false).await?.unwrap())
}

/// What a draining session is told instead of an answer.
///
/// It has to say two things the caller cannot find out otherwise: that this is
/// not a failure of the question (retrying it here will never work), and what
/// does work. The proxy between session and daemon is byte-level and does not
/// reconnect on its own, so the cure is on the session's side.
fn retired_refusal(what: &str) -> String {
    format!(
        "{what}: this rust-code-mcp daemon has retired (its RSS went over RMC_RSS_HARD_MB) and \
         no longer loads rust-analyzer; a fresh daemon is already serving new sessions. \
         Reconnect this session's MCP server (in Claude Code: /mcp, then reconnect \
         rust-code-mcp) to reach it. Tools that do not load an analysis — search, the \
         persisted graph queries, status — still work here until then."
    )
}

/// Run rust-analyzer work that must be the *only* such work in the process —
/// if it can be, right now. `Ok(None)` means analyses were inside the gate and
/// the work did not run; the caller tries again later.
///
/// The one caller is the garbage collection; see [`ANALYSIS_GATE`] for why it
/// cannot share the process with an analysis in flight.
///
/// # Why it does not wait for the gate
///
/// `std`'s `RwLock` prefers writers: once a writer waits, new readers wait
/// behind it. A collection queued for the gate while a large workspace was
/// loading — minutes, and the timer fires every five — therefore stopped every
/// *other* analysis, on any project, for the rest of that load, and the
/// watchdog that awaits the collection stopped with them. A sweep that is
/// skipped when the process is busy costs a few minutes of memory held a
/// little longer; one that queues costs every reader its answer.
pub(crate) async fn try_run_exclusive_analysis<T, F>(
    what: &'static str,
    work: F,
) -> Result<Option<T>, McpError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    spawn_gated(what, work, true).await
}

/// Enter the gate for one piece of work: shared waits for a collection inside;
/// exclusive does not wait at all (see [`try_run_exclusive_analysis`]) and
/// returns `None` when analyses are inside.
fn enter_gate(exclusive: bool) -> Option<GateGuard> {
    if !exclusive {
        return Some(GateGuard::Shared(enter_shared()));
    }
    match ANALYSIS_GATE.try_write() {
        Ok(guard) => Some(GateGuard::Exclusive(guard)),
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            Some(GateGuard::Exclusive(poisoned.into_inner()))
        }
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

async fn spawn_gated<T, F>(
    what: &'static str,
    work: F,
    exclusive: bool,
) -> Result<Option<T>, McpError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("rmc-analysis".to_string())
        .stack_size(ANALYSIS_STACK_BYTES)
        .spawn(move || {
            // Taken here rather than before the spawn: waiting for the gate is
            // the analysis thread's business, and doing it in the caller would
            // block a runtime worker on exactly what this module exists to keep
            // off one.
            let Some(_guard) = enter_gate(exclusive) else {
                let _ = tx.send(Ok(None));
                return;
            };
            // Caught for its text only: the thread ends either way, and the
            // caller used to get "panicked" with no hint of what. Unwind
            // safety is what it was — `work` owns its inputs, and a mutex it
            // held while panicking is poisoned by the unwind, before this.
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                .map(Some)
                .map_err(|payload| crate::panic_boundary::panic_message(payload.as_ref()));
            // A send error means the caller went away; nothing to report to.
            let _ = tx.send(outcome);
        })
        .map_err(|error| {
            McpError::internal_error(
                format!("{what}: failed to spawn analysis thread: {error}"),
                None,
            )
        })?;

    // The sender is dropped without sending only if the thread died outside
    // the catch — the gate wait or the send itself.
    match rx.await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(message)) => Err(McpError::internal_error(
            format!("{what}: analysis thread panicked: {message}"),
            None,
        )),
        Err(_) => Err(McpError::internal_error(format!("{what}: analysis thread panicked"), None)),
    }
}

/// Run work against the shared [`SemanticService`] on the analysis thread.
///
/// Every semantic call loads or queries a rust-analyzer workspace, so every one
/// of them belongs on [`run_analysis`]'s stack for the reason in this module's
/// header. This is the single door to the service from an async context so that
/// the choice is made once rather than at each call site: taking the mutex
/// inline in an `async fn` gets the 2 MiB worker stack *and* blocks the runtime
/// for the duration of the analysis, and both are easy to reintroduce by
/// accident when the lock is one `.lock()` away.
///
/// The closure receives the service and reports its own failures, so a caller
/// keeps its own error wording rather than inheriting one from here. The
/// service locks per project inside (see [`SemanticService`]): this door holds
/// no lock of its own, so work on one project never queues behind another's.
///
/// One panic is caught here and turned into a cure: a dependency's source
/// found changed on disk since its project was loaded. It is a fact about the
/// files, not a bug — the context is unloaded and loads again on the next
/// call — so it comes back as an error saying so.
///
/// Every other panic is caught as well, and costs the contexts it interrupted
/// (`SemanticService::unload_after_panic`): what it left half done is unknown,
/// so they load afresh on the next call, as after a drift. Uncaught, it once
/// poisoned the process-wide service mutex for the life of the daemon — every
/// semantic tool after it failed on the lock, and so did `clear_runtime`, the
/// one cure a session could reach.
pub(crate) async fn with_semantic<T, F>(
    semantic: &Arc<SemanticService>,
    what: &'static str,
    work: F,
) -> Result<T, McpError>
where
    F: FnOnce(&SemanticService) -> Result<T, McpError> + Send + 'static,
    T: Send + 'static,
{
    let semantic = Arc::clone(semantic);
    run_analysis(what, move || {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&semantic))) {
            Ok(outcome) => outcome,
            Err(panic) => {
                let after = semantic.unload_after_panic();
                if !after.drifted.is_empty() {
                    return Err(McpError::internal_error(
                        drift_refusal(what, &after.drifted),
                        None,
                    ));
                }
                let message = crate::panic_boundary::panic_message(panic.as_ref());
                tracing::error!(
                    target: "panic",
                    "{what} panicked inside the semantic service ({message}); dropped {} loaded context(s)",
                    after.dropped
                );
                Err(McpError::internal_error(
                    panic_refusal(what, &message, after.dropped),
                    None,
                ))
            }
        }
    })
    .await?
}

/// What a call whose analysis panicked for any other reason is told.
fn panic_refusal(what: &str, message: &str, dropped: usize) -> String {
    format!(
        "{what}: rust-analyzer work panicked ({message}); {dropped} loaded project(s) it \
         interrupted were unloaded and load afresh on the next call — retrying is safe, and a \
         repeat of the same panic is a bug worth reporting with the daemon log's backtrace"
    )
}

/// What a call that hit a changed dependency source is told.
fn drift_refusal(what: &str, unloaded: &[String]) -> String {
    format!(
        "{what}: a dependency's source changed on disk since the project was loaded ({}); \
         the project was unloaded and loads afresh on the next call — retry it",
        unloaded.join("; ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// The gate, judged by the two things that matter about it: a collection
    /// must not *begin* while an analysis is inside — and trying to begin must
    /// not hold up the next analysis.
    ///
    /// The shared side reports that it entered and then waits to be released;
    /// the collection stamps a flag the moment it starts. While the shared side
    /// is held, a collection is attempted, and then a second analysis must run
    /// to the end within a bound.
    ///
    /// The flag check is one-sided, which is what makes it an oracle rather
    /// than a race: with the gate in place the flag can never be set before the
    /// release however long we wait, because a write lock cannot be taken while
    /// a read guard is held. The second analysis is the oracle for not queueing:
    /// a collection *waiting* for the gate is a waiting writer, and `std`'s
    /// writer-preferring `RwLock` parks every reader that arrives after it — so
    /// with a blocking `write()` the second analysis times out (mutant checked
    /// 2026-10-31). The 150 ms before it gives the collection's thread time to
    /// reach the gate; the bound decides only how quickly a broken
    /// implementation is caught.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_collection_neither_overlaps_an_analysis_nor_queues_ahead_of_the_next() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc;
        use std::time::Duration;

        static EXCLUSIVE_STARTED: AtomicBool = AtomicBool::new(false);
        EXCLUSIVE_STARTED.store(false, Ordering::SeqCst);

        // Entry is reported over a *tokio* channel and awaited, not received
        // with a blocking `recv` on a runtime thread. The release channel stays
        // a blocking one, because the side that waits on it is the analysis
        // thread, which is allowed to block and whose blocking is the point.
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let shared = tokio::spawn(run_analysis("shared", move || {
            entered_tx.send(()).expect("report entry");
            // Held until the test says otherwise: this is the window during
            // which a collection must not run.
            release_rx.recv().expect("wait for release");
            EXCLUSIVE_STARTED.load(Ordering::SeqCst)
        }));

        entered_rx.await.expect("shared work entered the gate");

        let collection = tokio::spawn(try_run_exclusive_analysis("exclusive", || {
            EXCLUSIVE_STARTED.store(true, Ordering::SeqCst);
        }));
        tokio::time::sleep(Duration::from_millis(150)).await;

        let second = tokio::time::timeout(
            Duration::from_secs(5),
            run_analysis("second", || 6 * 7),
        )
        .await;

        release_tx.send(()).expect("release the shared work");
        let started_before_release = shared.await.expect("shared task").expect("shared analysis");

        assert!(
            !started_before_release,
            "the collection ran while an analysis was still inside rust-analyzer — that overlap \
             is the one the interner sweep cannot survive, and it shows up as a SIGSEGV rather \
             than as a failed call"
        );
        assert_eq!(
            second.expect(
                "an analysis arriving after a collection attempt waited for the analysis already \
                 inside — the collection queued for the gate, and a queued writer parks every \
                 reader behind it: one project's load holds up every other project"
            )
            .expect("second analysis"),
            42
        );
        let skipped = collection.await.expect("collection task").expect("collection");
        assert!(
            skipped.is_none(),
            "the collection reported running while an analysis held the gate"
        );

        // Positive control: once the gate is free the same attempt runs.
        // Retried, because other tests in this binary hold the gate too.
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let ran = try_run_exclusive_analysis("exclusive", || {
                EXCLUSIVE_STARTED.store(true, Ordering::SeqCst);
            })
            .await
            .expect("collection");
            if ran.is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "positive control: the collection never found the gate free, so the assertions \
                 above passed without proving anything"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(EXCLUSIVE_STARTED.load(Ordering::SeqCst));
    }

    /// A retired process must not start the work at all — refusing only the
    /// answer after a `Full` load had already run would keep exactly the memory
    /// retirement exists to release. So the oracle is whether the closure ran,
    /// not what came back.
    #[tokio::test]
    async fn a_retired_process_does_not_start_the_work() {
        let retired = AtomicBool::new(true);
        let started = Arc::new(AtomicBool::new(false));
        let stamp = Arc::clone(&started);

        let outcome = run_admitted(&retired, "find_references", move || {
            stamp.store(true, Ordering::SeqCst);
        })
        .await;

        let error = outcome.expect_err("a retired process must refuse the analysis");
        assert!(
            !started.load(Ordering::SeqCst),
            "the work ran before being refused — the load happened anyway"
        );
        assert!(
            error.message.starts_with("find_references: ") && error.message.contains("retired"),
            "the refusal must name the tool and say why, so it does not read as a failure of \
             the question: {}",
            error.message
        );
    }

    /// Positive control for the test above, through the same door: with the
    /// flag down the identical closure runs. Without it, "the work never ran"
    /// could just as well mean the door never runs anything.
    #[tokio::test]
    async fn a_serving_process_runs_the_same_work() {
        let retired = AtomicBool::new(false);
        let started = Arc::new(AtomicBool::new(false));
        let stamp = Arc::clone(&started);

        run_admitted(&retired, "find_references", move || {
            stamp.store(true, Ordering::SeqCst);
        })
        .await
        .expect("a process that has not retired admits the analysis");

        assert!(started.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_result_travels_back_from_the_analysis_thread() {
        let value = run_analysis("test", || 6 * 7)
            .await
            .expect("analysis result");
        assert_eq!(value, 42);
    }

    #[tokio::test]
    async fn recursion_that_overflows_a_default_thread_completes_here() {
        // Positive control for the stack size itself rather than for the
        // plumbing: this frame chain needs far more than the 2 MiB a tokio
        // blocking thread would hand us, so the test fails by aborting if
        // `stack_size` ever stops being applied.
        fn burn(depth: usize, sink: &mut u64) -> u64 {
            // A big live frame, kept from being optimized away by feeding it
            // into the running sum.
            let block = [0xABu8; 8192];
            *sink = sink.wrapping_add(block[depth % block.len()] as u64);
            if depth == 0 {
                *sink
            } else {
                burn(depth - 1, sink)
            }
        }

        let mut sink = 0;
        // ~1000 frames × 8 KiB ≈ 8 MiB of live frames.
        let total = run_analysis("test", move || burn(1000, &mut sink))
            .await
            .expect("deep recursion completes");
        assert!(total > 0, "the recursion must actually have run");
    }

    fn test_semantic() -> Arc<SemanticService> {
        Arc::new(SemanticService::new())
    }

    #[tokio::test]
    async fn semantic_work_leaves_the_runtime_worker() {
        // The regression this guards is a call site taking the mutex inline in
        // an `async fn`: that runs rust-analyzer on the worker's 2 MiB stack,
        // which is what aborted the whole server. The thread name is how that
        // choice becomes observable from the closure.
        let semantic = test_semantic();
        let thread_name = with_semantic(&semantic, "test", |_| {
            Ok(std::thread::current().name().map(str::to_string))
        })
        .await
        .expect("semantic work runs");
        assert_eq!(thread_name.as_deref(), Some("rmc-analysis"));
    }

    #[tokio::test]
    async fn semantic_work_gets_the_deep_stack_too() {
        // Positive control end to end: the same frame chain that a worker
        // thread cannot hold must complete through the semantic door.
        fn burn(depth: usize, sink: &mut u64) -> u64 {
            let block = [0xCDu8; 8192];
            *sink = sink.wrapping_add(block[depth % block.len()] as u64);
            if depth == 0 {
                *sink
            } else {
                burn(depth - 1, sink)
            }
        }

        let semantic = test_semantic();
        let total = with_semantic(&semantic, "test", |_| {
            let mut sink = 0;
            Ok(burn(1000, &mut sink))
        })
        .await
        .expect("deep recursion completes");
        assert!(total > 0, "the recursion must actually have run");
    }

    /// Two loaded projects, each an empty directory with a hand-built context,
    /// so a query on either runs the served path (stat the tree, find it
    /// unchanged, query the database) without loading a workspace.
    fn two_loaded_projects(semantic: &SemanticService) -> (tempfile::TempDir, PathBuf, PathBuf) {
        use ra_ap_ide::AnalysisHost;

        let dir = tempfile::tempdir().expect("tempdir");
        let mut paths = Vec::new();
        for name in ["a", "b"] {
            let path = dir.path().join(name);
            std::fs::create_dir_all(&path).unwrap();
            let path = path.canonicalize().unwrap();
            semantic.insert_for_test(path.clone(), AnalysisHost::default());
            paths.push(path);
        }
        let b = paths.pop().unwrap();
        let a = paths.pop().unwrap();
        (dir, a, b)
    }

    /// The defect of 2026-10-31: `find_definition` on a small project answered
    /// after 176.7 s — the time a large neighbour's load took — because every
    /// semantic call held one process-wide mutex for its whole run, the load
    /// included.
    ///
    /// Project A's slot is held by a call that waits to be released (it holds
    /// it exactly as a load does: the slot lock spans the load, the refresh and
    /// the query alike). While it is held, `find_definition`'s path on project B
    /// must answer within a bound; `runtime_status`'s view of the service must
    /// too, and must show both projects.
    ///
    /// Mutant (checked 2026-10-31): `with_semantic` taking one service-wide
    /// lock around the closure again → B times out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn work_on_one_project_does_not_hold_another() {
        use std::time::Duration;

        let semantic = test_semantic();
        let (_dir, a, b) = two_loaded_projects(&semantic);

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let held = Arc::clone(&semantic);
        let holder = tokio::spawn(async move {
            with_semantic(&held, "load_a", move |service| {
                service
                    .hold_project_for_test(&a, || {
                        entered_tx.send(()).expect("report entry");
                        release_rx.recv().expect("wait for release");
                    })
                    .map_err(|error| McpError::internal_error(error.to_string(), None))
            })
            .await
        });
        entered_rx.await.expect("A's slot is held");

        let answered = tokio::time::timeout(
            Duration::from_secs(5),
            with_semantic(&semantic, "find_definition", move |service| {
                service
                    .symbol_search_with_exact(&b, "anything", 10, false)
                    .map_err(|error| McpError::internal_error(error.to_string(), None))
            }),
        )
        .await;
        let status = semantic.status();
        release_tx.send(()).expect("release A");
        holder.await.expect("holder task").expect("holder work");

        answered
            .expect(
                "find_definition on project B waited for work on project A — one project's load \
                 holds every other project again",
            )
            .expect("find_definition on B");
        assert_eq!(status.project_count, 2, "status while A is busy: {status:?}");
    }

    /// The defect of 2026-10-31, inside one project: a `find_definition`
    /// waited 470 s for a `find_references` of the same project, because the
    /// query held the project's slot for its whole run.
    ///
    /// A query on A blocks until released; another query on A must answer
    /// within a bound — it takes its own snapshot while the first runs on its
    /// own.
    ///
    /// Mutant (checked 2026-10-31): `with_project` keeping the slot on every
    /// attempt (as on the last) → the second query times out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn queries_on_one_project_do_not_wait_for_each_other() {
        use std::time::Duration;

        let semantic = test_semantic();
        let (_dir, a, _b) = two_loaded_projects(&semantic);

        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let held = Arc::clone(&semantic);
        let held_a = a.clone();
        let holder = tokio::spawn(async move {
            with_semantic(&held, "find_references", move |service| {
                let entered = std::sync::Mutex::new(Some(entered_tx));
                let release = std::sync::Mutex::new(release_rx);
                service
                    .query_project_for_test(&held_a, || {
                        entered.lock().unwrap().take().unwrap().send(()).unwrap();
                        release.lock().unwrap().recv().unwrap();
                        Ok(())
                    })
                    .map_err(|error| McpError::internal_error(error.to_string(), None))
            })
            .await
        });
        entered_rx.await.expect("a query on A is running");

        let answered = tokio::time::timeout(
            Duration::from_secs(5),
            with_semantic(&semantic, "find_definition", {
                let a = a.clone();
                move |service| {
                    service
                        .symbol_search_with_exact(&a, "anything", 10, false)
                        .map_err(|error| McpError::internal_error(error.to_string(), None))
                }
            }),
        )
        .await;
        release_tx.send(()).expect("release the first query");
        holder.await.expect("holder task").expect("holder work");

        answered
            .expect("a query on project A waited for another query on A")
            .expect("find_definition on A");
    }

    /// A query cancelled by a refresh (salsa's `Cancelled`, as an unwind or as
    /// an error through `?`) is asked again, and the last attempt keeps the
    /// slot so no refresh can cancel it: a project edited faster than a long
    /// query runs still gets an answer. Mutant: treating `Cancelled` as the
    /// answer (no retry) → the first attempt's cancellation is returned.
    #[tokio::test]
    async fn a_cancelled_query_is_asked_again_and_the_last_attempt_holds_the_slot() {
        use ra_ap_ide_db::base_db::salsa::Cancelled;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let semantic = test_semantic();
        let (_dir, a, _b) = two_loaded_projects(&semantic);

        let answer = with_semantic(&semantic, "find_references", move |service| {
            let attempts = AtomicUsize::new(0);
            service
                .query_project_for_test(&a, || {
                    let attempt = attempts.fetch_add(1, Ordering::SeqCst) + 1;
                    let slot_held = service.slot_is_held_for_test(&a);
                    match attempt {
                        1 => std::panic::resume_unwind(Box::new(Cancelled::PendingWrite)),
                        2 => Err(anyhow::Error::new(Cancelled::PendingWrite)
                            .context(format!("slot held: {slot_held}"))),
                        _ => Ok((attempt, slot_held)),
                    }
                })
                .map_err(|error| McpError::internal_error(error.to_string(), None))
        })
        .await
        .expect("the third attempt answers");
        assert_eq!(answer, (3, true), "attempts, and whether the last held the slot");
    }

    /// A panic inside a query — on a snapshot, with the slot let go, so no
    /// lock is poisoned — still unloads exactly the project it happened on.
    /// Mutant: no `SpoilOnPanic` in `with_project` → the panic is "outside
    /// every context" and both projects are dropped.
    #[tokio::test]
    async fn a_panic_inside_a_query_unloads_only_its_project() {
        let semantic = test_semantic();
        let (_dir, a, b) = two_loaded_projects(&semantic);

        let outcome: Result<(), McpError> = with_semantic(&semantic, "find_references", {
            let a = a.clone();
            move |service| {
                service
                    .query_project_for_test(&a, || -> anyhow::Result<()> {
                        panic!("boom in a query")
                    })
                    .map_err(|error| McpError::internal_error(error.to_string(), None))
            }
        })
        .await;
        let error = outcome.expect_err("a panic must surface as an error");
        assert!(error.message.contains("1 loaded"), "{}", error.message);
        assert_eq!(semantic.loaded_paths_for_test(), vec![b]);
    }

    /// The same two projects, one of them in a call that panics: the panicking
    /// project is unloaded, the other stays loaded and keeps answering. Before
    /// contexts had locks of their own, every panic cost every project its
    /// load — minutes each for a large workspace.
    #[tokio::test]
    async fn a_panic_on_one_project_keeps_the_other_loaded() {
        let semantic = test_semantic();
        let (_dir, a, b) = two_loaded_projects(&semantic);

        let outcome: Result<(), McpError> = with_semantic(&semantic, "find_references", {
            let a = a.clone();
            move |service| {
                service
                    .hold_project_for_test(&a, || panic!("boom in rust-analyzer"))
                    .map_err(|error| McpError::internal_error(error.to_string(), None))
            }
        })
        .await;
        let error = outcome.expect_err("a panic must surface as an error");
        assert!(
            error.message.contains("boom in rust-analyzer") && error.message.contains("1 loaded"),
            "the error names the panic and that one project was unloaded: {}",
            error.message
        );

        assert_eq!(semantic.loaded_paths_for_test(), vec![b.clone()]);
        with_semantic(&semantic, "find_definition", move |service| {
            service
                .symbol_search_with_exact(&b, "anything", 10, false)
                .map_err(|error| McpError::internal_error(error.to_string(), None))
        })
        .await
        .expect("the other project still answers");
    }

    #[tokio::test]
    async fn a_panicking_analysis_is_reported_rather_than_hanging() {
        let outcome: Result<(), McpError> =
            run_analysis("test", || panic!("boom in analysis")).await;
        let error = outcome.expect_err("a panic must surface as an error");
        assert!(
            error.message.contains("panicked") && error.message.contains("boom in analysis"),
            "the error names the panic, not just that one happened: {}",
            error.message
        );
    }

    /// A panic that is not a drift: the call gets an error naming it, every
    /// loaded context is gone (what the panic interrupted is unknown), and —
    /// the point — the next call is served, not refused on a poisoned lock.
    /// Before the catch covered every panic, the second call here failed with
    /// "Failed to acquire lock" and so would every later one.
    #[tokio::test]
    async fn any_panic_in_semantic_work_keeps_the_service() {
        use ra_ap_ide::AnalysisHost;

        let semantic = test_semantic();
        let dir = tempfile::tempdir().expect("tempdir");
        semantic.insert_for_test(dir.path().join("project"), AnalysisHost::default());

        // Outside every project's slot: what it interrupted is unknown, so
        // every context goes.
        let outcome: Result<(), McpError> =
            with_semantic(&semantic, "find_references", |_| panic!("boom in rust-analyzer")).await;
        let error = outcome.expect_err("a panic must surface as an error");
        assert!(
            error.message.starts_with("find_references: ")
                && error.message.contains("boom in rust-analyzer")
                && error.message.contains("unloaded"),
            "the error names the tool, the panic and what happened to the loaded projects: {}",
            error.message
        );

        let left = with_semantic(&semantic, "count", |service| Ok(service.project_count())).await;
        assert_eq!(left.expect("the service survives the panic"), 0, "nothing loaded is kept");
    }

    /// A dependency source changed behind a loaded context: the call that
    /// reads it is told why and to retry, the context is gone (so the retry
    /// loads afresh), and the service still answers — the mutex is not
    /// poisoned. Without the catch in `with_semantic` the first call reports a
    /// bare "panicked" and every later one "Failed to acquire lock".
    #[tokio::test]
    async fn a_changed_dependency_source_unloads_its_project_and_keeps_the_service() {
        use ra_ap_ide::AnalysisHost;
        use ra_ap_ide_db::RootDatabase;
        use ra_ap_ide_db::base_db::file_text_store::FileTextStore;
        use ra_ap_ide_db::base_db::{
            AbsPathBuf, FileChange, FileId, FileSet, SourceDatabase, SourceRoot, VfsPath,
        };

        let dir = tempfile::tempdir().expect("tempdir");
        let abs = |path: &std::path::Path| AbsPathBuf::try_from(path.to_str().unwrap()).unwrap();
        let registry = dir.path().join("registry");
        let source = registry.join("dep/src/lib.rs");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        let text = "pub fn dep() -> u8 { 1 }\n";
        std::fs::write(&source, text).unwrap();
        std::fs::create_dir_all(dir.path().join("copies")).unwrap();

        let mut db = RootDatabase::new(None);
        db.set_file_text_store(FileTextStore {
            read_back_dirs: vec![abs(&registry)],
            copy_dir: abs(&dir.path().join("copies")),
        });
        let mut library = FileSet::default();
        library.insert(FileId::from_raw(0), VfsPath::from(abs(&source)));
        let mut change = FileChange::default();
        change.set_roots(vec![SourceRoot::new_library(library)]);
        change.change_file(FileId::from_raw(0), Some(text.to_owned()));
        change.apply(&mut db);
        std::fs::write(&source, text.replace('1', "2")).unwrap();

        let semantic = test_semantic();
        let project = dir.path().join("project");
        semantic.insert_for_test(project.clone(), AnalysisHost::with_database(db));

        let read = with_semantic(&semantic, "read", move |service| {
            Ok(service.with_host_for_test(&project, |host| {
                let db = host.raw_database();
                db.file_text(FileId::from_raw(0)).text(db).len()
            }))
        })
        .await;
        let error = read.expect_err("a changed source must not be read as the old one");
        assert!(
            error.message.contains("changed on disk")
                && error.message.contains(source.to_str().unwrap())
                && error.message.contains("retry"),
            "the refusal says what changed and what to do: {}",
            error.message
        );

        let left = with_semantic(&semantic, "count", |service| Ok(service.project_count())).await;
        assert_eq!(
            left.expect("the service survives the drift"),
            0,
            "the drifted project is unloaded"
        );
    }
}

//! Running rust-analyzer work on a stack deep enough for it.
//!
//! Work that loads a workspace through rust-analyzer walks HIR/AST trees
//! recursively — the graph tools building a hypergraph, the audits, the
//! skeleton builder alike. That recursion is bounded by the *shape of the
//! analyzed source*, not by anything we control, and it does not fit in the
//! 2 MiB stack a tokio blocking-pool thread gets by default: building the
//! hypergraph for this very workspace aborted the process with
//!
//! ```text
//! thread 'tokio-rt-worker' has overflowed its stack
//! fatal runtime error: stack overflow, aborting
//! ```
//!
//! A stack overflow is an `abort`, not a `panic` — it takes the whole MCP
//! server down rather than failing one tool call, so this is not something a
//! caller can guard against, and on a server shared by several sessions it ends
//! all of them at once. The work therefore carries its own stack instead of
//! depending on whoever spawned it.

use std::sync::{Arc, Mutex};

use rmcp::ErrorData as McpError;

use crate::semantic::SemanticService;

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
pub(crate) async fn run_analysis<T, F>(what: &'static str, work: F) -> Result<T, McpError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("rmc-analysis".to_string())
        .stack_size(ANALYSIS_STACK_BYTES)
        .spawn(move || {
            // A send error means the caller went away; nothing to report to.
            let _ = tx.send(work());
        })
        .map_err(|error| {
            McpError::internal_error(
                format!("{what}: failed to spawn analysis thread: {error}"),
                None,
            )
        })?;

    // The sender is dropped without sending only if the thread panicked, which
    // is the same failure `spawn_blocking` reported as a join error.
    rx.await
        .map_err(|_| McpError::internal_error(format!("{what}: analysis thread panicked"), None))
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
/// The closure receives the locked service and reports its own failures, so a
/// caller keeps its own error wording rather than inheriting one from here. A
/// poisoned mutex is the only failure this adds.
pub(crate) async fn with_semantic<T, F>(
    semantic: &Arc<Mutex<SemanticService>>,
    what: &'static str,
    work: F,
) -> Result<T, McpError>
where
    F: FnOnce(&mut SemanticService) -> Result<T, McpError> + Send + 'static,
    T: Send + 'static,
{
    let semantic = Arc::clone(semantic);
    run_analysis(what, move || {
        let mut service = semantic.lock().map_err(|error| {
            McpError::internal_error(format!("Failed to acquire lock: {}", error), None)
        })?;
        work(&mut service)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn test_semantic() -> Arc<Mutex<SemanticService>> {
        Arc::new(Mutex::new(SemanticService::new()))
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

    #[tokio::test]
    async fn a_poisoned_semantic_mutex_is_reported_rather_than_panicking() {
        let semantic = test_semantic();
        let poisoner = Arc::clone(&semantic);
        // Poison the mutex the way a panicking analysis would.
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.lock().expect("fresh mutex");
            panic!("poison the semantic mutex");
        })
        .join();

        let outcome: Result<(), McpError> = with_semantic(&semantic, "test", |_| Ok(())).await;
        let error = outcome.expect_err("a poisoned mutex must surface as an error");
        assert!(
            error.message.contains("Failed to acquire lock"),
            "unexpected error message: {}",
            error.message
        );
    }

    #[tokio::test]
    async fn a_panicking_analysis_is_reported_rather_than_hanging() {
        let outcome: Result<(), McpError> =
            run_analysis("test", || panic!("boom in analysis")).await;
        let error = outcome.expect_err("a panic must surface as an error");
        assert!(
            error.message.contains("panicked"),
            "unexpected error message: {}",
            error.message
        );
    }
}

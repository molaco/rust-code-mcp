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

use rmcp::ErrorData as McpError;

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

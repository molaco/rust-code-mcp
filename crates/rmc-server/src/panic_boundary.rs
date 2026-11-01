//! Where a panic stops in this server, and what it turns into there.
//!
//! A Rust panic that nobody catches inside the daemon does not crash it; it
//! ends the task or thread it ran on, and what the client sees depends on who
//! spawned that. rmcp answers each request from a `tokio::spawn` of its own and
//! sends the response from inside that task — so a tool that panicked on the
//! async side sent nothing at all, and the client waited for its own timeout
//! with no hint of why. The same unwind out of the background sync loop ended
//! the loop for the rest of the daemon's life, silently.
//!
//! The doors that decide this are few, and each catches here:
//!
//! - [`answer_or_error`] — every tool call (`ServerHandler::call_tool`): the
//!   panic becomes a JSON-RPC error carrying its message.
//! - `deep_stack` — the analysis thread: the same, and the semantic service
//!   drops its contexts instead of poisoning its mutex.
//! - the sync loop — one directory's panic is that directory's failed sync.
//!
//! None of this reaches a C++ `abort` (MIGraphX, HIP) or a stack overflow:
//! those are not unwinds, and they still end the process.

use std::any::Any;
use std::future::Future;
use std::panic::AssertUnwindSafe;

use futures::FutureExt;

/// The text a panic was raised with: `panic!` with a literal carries a
/// `&str`, with format arguments a `String`. Anything else — a payload from
/// `resume_unwind`, such as salsa's `Cancelled` — has no text of its own.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic payload without a message".to_owned()
    }
}

/// Await `work`, turning a panic out of it into `Err` with the panic's text.
///
/// `AssertUnwindSafe`: what `work` borrows is either the server's shared
/// state, whose mutexes poison on their own, or the call's own arguments, which
/// end with it — the same exposure a panic had when it ended the task outright.
pub async fn catch_panic<T>(work: impl Future<Output = T>) -> Result<T, String> {
    AssertUnwindSafe(work)
        .catch_unwind()
        .await
        .map_err(|payload| panic_message(payload.as_ref()))
}

/// Await one tool call; a panic in it becomes the call's error.
pub(crate) async fn answer_or_error<T>(
    tool: &str,
    work: impl Future<Output = Result<T, rmcp::ErrorData>>,
) -> Result<T, rmcp::ErrorData> {
    match catch_panic(work).await {
        Ok(answer) => answer,
        Err(message) => {
            tracing::error!(target: "panic", "tool {tool} panicked: {message}");
            Err(rmcp::ErrorData::internal_error(
                format!(
                    "{tool}: the tool panicked ({message}); the daemon is still serving — see its log for the backtrace"
                ),
                None,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_panic_comes_back_as_its_text() {
        let literal = catch_panic(async { panic!("literal") }).await;
        assert_eq!(literal.map(|()| ()), Err("literal".to_owned()));
        let formatted = catch_panic(async { panic!("formatted {}", 7) }).await;
        assert_eq!(formatted.map(|()| ()), Err("formatted 7".to_owned()));
        assert_eq!(catch_panic(async { 42 }).await, Ok(42));
    }

    #[tokio::test]
    async fn a_resumed_payload_without_text_is_still_caught() {
        let caught = catch_panic(async { std::panic::resume_unwind(Box::new(17_u32)) }).await;
        assert_eq!(
            caught.map(|()| ()),
            Err("a panic payload without a message".to_owned())
        );
    }
}

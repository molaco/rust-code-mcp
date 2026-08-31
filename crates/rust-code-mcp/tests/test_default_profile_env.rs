//! Startup refusal for `RMC_EMBEDDING_PROFILE`, on the real binary: the
//! contract is one stderr line and exit code 2, at every log level.
//!
//! `RUST_LOG=error` is the case that matters. The check used to sit inside a
//! `tracing::info!` argument, and `tracing` evaluates arguments only when the
//! callsite level is enabled, so a quiet log level started the server with an
//! unusable profile. The profile rules themselves are unit-tested in
//! `rmc_server::mcp::defaults`.

use std::process::{Command, Stdio};

#[test]
fn unknown_profile_name_exits_with_code_two_even_when_logging_is_quiet() {
    let output = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"))
        .env("RMC_EMBEDDING_PROFILE", "local-cpu-small-typo")
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .expect("the server binary runs");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(output.status.code(), Some(2), "stderr was: {stderr}");
    assert!(
        stderr.contains("local-cpu-small-typo") && !stderr.contains("panicked"),
        "the message must name the value and must not be a panic, stderr was: {stderr}"
    );
}

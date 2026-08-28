//! The oracle for the shared daemon: however many sessions attach to a project,
//! the analysis lives in ONE process.
//!
//! What is checked is not "a daemon came up" but the sharing itself:
//! `runtime_status` reports the pid of the process that actually served the call.
//! Two clients, one pid — the state is shared; two different pids — every session
//! is loading its own copy of the rust-analyzer context again, which is precisely
//! the regression the daemon exists to prevent.
//!
//! The positive control sits right next to it: with `RMC_DAEMON=0` the pid must
//! equal the client's own. Without it the first test would only prove that two
//! calls returned the same number, without distinguishing that from "both were
//! served somewhere else entirely".

#![cfg(unix)]

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// An MCP client: the binary plus a pipe to it.
struct Session {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<std::io::Result<String>>,
    next_id: u64,
}

impl Session {
    /// `socket_dir` is per test: otherwise a run would attach to the daemon of a
    /// live working session and assert about someone else's process.
    fn start(socket_dir: &Path, shared: bool) -> Result<Self> {
        Self::start_in(socket_dir, shared, None)
    }

    /// As [`Session::start`], with the client's working directory chosen by the
    /// caller — the axis `two_clients_from_different_directories_share_one_process`
    /// is about.
    fn start_in(socket_dir: &Path, shared: bool, cwd: Option<&Path>) -> Result<Self> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"));
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        command
            .env("RUST_LOG", "error")
            .env("RMC_DAEMON_DIR", socket_dir)
            // The daemon must not outlive the run: the idle tick is 15s, so an
            // orphan exits on its own even if the test fails before the kill.
            .env("RMC_DAEMON_IDLE_SECS", "5")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if shared {
            command.env_remove("RMC_DAEMON");
        } else {
            command.env("RMC_DAEMON", "0");
        }

        let mut child = command.spawn().context("failed to spawn the MCP client")?;
        let stdout = child.stdout.take().context("child stdout was not piped")?;
        let stdin = child.stdin.take().context("child stdin was not piped")?;

        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });

        let mut session = Self {
            child,
            stdin,
            rx,
            next_id: 1,
        };
        session.handshake()?;
        Ok(session)
    }

    fn handshake(&mut self) -> Result<()> {
        let id = self.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "daemon-sharing-test", "version": "0.0.0" }
            }),
        )?;
        let response = self.read_response(id, Duration::from_secs(120))?;
        if response.get("error").is_some() {
            return Err(anyhow!("initialize failed: {response}"));
        }
        self.notify("notifications/initialized", json!({}))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        Ok(id)
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({ "jsonrpc": "2.0", "method": method, "params": params }))
    }

    fn send(&mut self, message: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &message)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    fn read_response(&self, id: u64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(anyhow!("timed out waiting for response id {id}"));
            }
            let line = match self.rx.recv_timeout(remaining) {
                Ok(line) => line?,
                Err(RecvTimeoutError::Timeout) => {
                    return Err(anyhow!("timed out waiting for response id {id}"));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("server closed stdout before response id {id}"));
                }
            };
            let value: Value = serde_json::from_str(&line)
                .with_context(|| format!("stdout contained a non-JSON-RPC line: {line:?}"))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) {
                return Ok(value);
            }
        }
    }

    /// The pid of the process that ACTUALLY serves this session's calls.
    fn serving_pid(&mut self) -> Result<u32> {
        let id = self.request(
            "tools/call",
            json!({ "name": "runtime_status", "arguments": {} }),
        )?;
        let response = self.read_response(id, Duration::from_secs(120))?;
        if response.get("error").is_some() {
            return Err(anyhow!("runtime_status failed: {response}"));
        }
        let text = response
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("no status text in the response: {response}"))?;
        let status: Value = serde_json::from_str(text)?;
        status
            .pointer("/process/pid")
            .and_then(Value::as_u64)
            .map(|pid| pid as u32)
            .ok_or_else(|| anyhow!("no process.pid in the status: {text}"))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn kill_pid(pid: u32) {
    let _ = Command::new("kill").arg(pid.to_string()).status();
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> Option<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() {
            return Some(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    None
}

#[test]
fn two_clients_share_one_server_process() -> Result<()> {
    let socket_dir = TempDir::new()?;

    let mut first = Session::start(socket_dir.path(), true)?;
    let first_pid = first.serving_pid()?;
    let mut second = Session::start(socket_dir.path(), true)?;
    let second_pid = second.serving_pid()?;

    assert_eq!(
        first_pid, second_pid,
        "two sessions were served by different processes, so each holds its own copy of the analysis"
    );
    assert_ne!(
        first_pid,
        first.child.id(),
        "the call was served by the client itself, so no daemon came up and nothing is shared"
    );
    assert_ne!(second_pid, second.child.id());

    drop(first);
    drop(second);
    kill_pid(first_pid);
    Ok(())
}

/// The same oracle along the axis that used to split the fleet: two sessions
/// started in DIFFERENT directories must still be served by one process.
///
/// The daemon key included the working directory until this test existed. It
/// bought no isolation — every tool takes its `directory` as a parameter, so the
/// cwd never chose the project — and it cost, measured on one machine, eleven
/// processes holding 12.5 GB, several of them analysing the same repository.
///
/// Mutation that must fail it: put the cwd back into `key_from_parts`, and the
/// two pids diverge.
#[test]
fn two_clients_from_different_directories_share_one_process() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let first_cwd = TempDir::new()?;
    let second_cwd = TempDir::new()?;

    let mut first = Session::start_in(socket_dir.path(), true, Some(first_cwd.path()))?;
    let first_pid = first.serving_pid()?;
    let mut second = Session::start_in(socket_dir.path(), true, Some(second_cwd.path()))?;
    let second_pid = second.serving_pid()?;

    assert_eq!(
        first_pid, second_pid,
        "sessions started in {} and {} were served by different processes — the daemon key is \
         splitting on the working directory again",
        first_cwd.path().display(),
        second_cwd.path().display()
    );
    // Same positive control as the sibling test: without it, "one pid" would not
    // rule out both calls having been served in-process by two clients that
    // happen to be compared wrongly.
    assert_ne!(
        first_pid,
        first.child.id(),
        "the call was served by the client itself, so no daemon came up and nothing is shared"
    );

    drop(first);
    drop(second);
    kill_pid(first_pid);
    Ok(())
}

/// A killed daemon must remove its own socket file.
///
/// Clients survive a stale socket — they clear it and start a new daemon. But
/// while the file is there, `--print-socket` plus `ls` point at an address where
/// nobody listens, so diagnostics lie exactly when someone comes to read them.
#[test]
fn killed_daemon_removes_its_socket() -> Result<()> {
    let dir = TempDir::new()?;
    let socket = dir.path().join("probe.sock");

    let mut daemon = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"))
        .arg("--daemon")
        .arg("--socket")
        .arg(&socket)
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;

    wait_until(Duration::from_secs(60), || socket.exists())
        .ok_or_else(|| anyhow!("the daemon never bound its socket"))?;

    kill_pid(daemon.id());
    let gone = wait_until(Duration::from_secs(30), || !socket.exists());
    let _ = daemon.wait();
    gone.ok_or_else(|| anyhow!("SIGTERM left a stale {}", socket.display()))?;
    Ok(())
}

/// Positive control: the opt-out must restore the previous behaviour.
#[test]
fn opt_out_serves_in_process() -> Result<()> {
    let socket_dir = TempDir::new()?;

    let mut session = Session::start(socket_dir.path(), false)?;
    let serving_pid = session.serving_pid()?;

    assert_eq!(
        serving_pid,
        session.child.id(),
        "with RMC_DAEMON=0 the client process itself must serve the session"
    );
    Ok(())
}

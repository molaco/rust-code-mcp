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
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Cut the developer's own MCP configuration out of a spawned server.
///
/// These tests assert about the transport, and every child inherits the
/// environment of whoever runs them — which on a working machine is the live
/// server's configuration. `RMC_EMBEDDING_PROFILE=local-gpu-bge` with
/// `RMC_EP_CENSUS=1` is the usual pair, and it makes the test binary (built
/// without `--features migraphx`) REFUSE to start, exactly as designed: the
/// census exists to fail rather than quietly compute on the CPU. The result was
/// every daemon test failing with `server closed stdout` on a machine where the
/// daemon itself works fine, and passing in CI — a verdict that says more about
/// the shell than about the code.
///
/// Removed rather than pinned to a value: naming a profile here would make the
/// tests depend on which backends this build happens to carry.
fn hermetic_env(command: &mut Command) {
    command.env_remove("RMC_EMBEDDING_PROFILE");
    command.env_remove("RMC_EP_CENSUS");
}

/// An MCP client: the binary plus a pipe to it — or, for a test about the
/// daemon itself, a socket straight to the daemon with no client in between.
struct Session {
    child: Option<Child>,
    stdin: Box<dyn Write + Send>,
    rx: Receiver<std::io::Result<String>>,
    next_id: u64,
}

/// Lines from `source`, on a channel, read by a thread of their own.
fn line_channel(source: impl std::io::Read + Send + 'static) -> Receiver<std::io::Result<String>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(source).lines() {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
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
        Self::start_with(socket_dir, shared, cwd, &[])
    }

    /// As [`Session::start_in`], plus environment for the daemon this client
    /// spawns — a daemon inherits its first client's environment, which is how
    /// a test sets a watchdog threshold. Every `RMC_*` variable is part of the
    /// daemon key, so a per-test `socket_dir` is not optional here either.
    fn start_with(
        socket_dir: &Path,
        shared: bool,
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<Self> {
        Self::start_from(
            Path::new(env!("CARGO_BIN_EXE_rust-code-mcp")),
            &[],
            socket_dir,
            shared,
            cwd,
            env,
        )
    }

    /// As [`Session::start_with`], from a binary at `exe` and with `args` —
    /// the tests that replace the binary under a running session.
    fn start_from(
        exe: &Path,
        args: &[&std::ffi::OsStr],
        socket_dir: &Path,
        shared: bool,
        cwd: Option<&Path>,
        env: &[(&str, &str)],
    ) -> Result<Self> {
        let mut command = Command::new(exe);
        command.args(args);
        for (key, value) in env {
            command.env(key, value);
        }
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
        hermetic_env(&mut command);
        if shared {
            command.env_remove("RMC_DAEMON");
        } else {
            command.env("RMC_DAEMON", "0");
        }

        let mut child = command.spawn().context("failed to spawn the MCP client")?;
        let stdout = child.stdout.take().context("child stdout was not piped")?;
        let stdin = child.stdin.take().context("child stdin was not piped")?;

        let mut session = Self {
            child: Some(child),
            stdin: Box::new(stdin),
            rx: line_channel(stdout),
            next_id: 1,
        };
        session.handshake()?;
        Ok(session)
    }

    /// A session on the daemon's socket itself, without the client relay: what
    /// the daemon does to a connection, unmasked by a client that would move it
    /// elsewhere.
    fn over_socket(socket: &Path) -> Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(socket)
            .with_context(|| format!("connect {}", socket.display()))?;
        let mut session = Self {
            child: None,
            rx: line_channel(stream.try_clone()?),
            stdin: Box::new(stream),
            next_id: 1,
        };
        session.handshake()?;
        Ok(session)
    }

    /// Pid of the client process, when there is one.
    fn client_pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
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

    /// Call a tool and return the whole JSON-RPC response, error or not.
    ///
    /// The timeout covers a cold `Full` load of a one-file crate, sysroot
    /// included, which is the positive control's cost.
    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        let id = self.request(
            "tools/call",
            json!({ "name": name, "arguments": arguments }),
        )?;
        self.read_response(id, Duration::from_secs(300))
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
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
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
        Some(first_pid),
        first.client_pid(),
        "the call was served by the client itself, so no daemon came up and nothing is shared"
    );
    assert_ne!(Some(second_pid), second.client_pid());

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
        Some(first_pid),
        first.client_pid(),
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

    let mut command = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"));
    command
        .arg("--daemon")
        .arg("--socket")
        .arg(&socket)
        .env("RUST_LOG", "error")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hermetic_env(&mut command);
    let mut daemon = command.spawn()?;

    wait_until(Duration::from_secs(60), || socket.exists())
        .ok_or_else(|| anyhow!("the daemon never bound its socket"))?;

    kill_pid(daemon.id());
    let gone = wait_until(Duration::from_secs(30), || !socket.exists());
    let _ = daemon.wait();
    gone.ok_or_else(|| anyhow!("SIGTERM left a stale {}", socket.display()))?;
    Ok(())
}

/// One session's worth of JSON-RPC in a file: handshake, then one call.
///
/// A file rather than a pipe the test closes itself, because the case under
/// test is exactly "stdin reaches EOF while the server still owes an answer" —
/// the shape of every one-shot invocation a host makes.
fn piped_script(dir: &Path) -> Result<std::path::PathBuf> {
    let path = dir.join("session.jsonl");
    let mut file = std::fs::File::create(&path)?;
    for message in [
        json!({
            "jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "piped-session-test", "version": "0.0.0" }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} }),
        json!({
            "jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": { "name": "runtime_status", "arguments": {} }
        }),
    ] {
        writeln!(file, "{message}")?;
    }
    file.flush()?;
    Ok(path)
}

/// Ids of the responses a piped session got back, in order.
fn piped_response_ids(socket_dir: &Path, shared: bool) -> Result<Vec<u64>> {
    let script = piped_script(socket_dir)?;
    let mut command = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"));
    command
        .env("RUST_LOG", "error")
        .env("RMC_DAEMON_DIR", socket_dir)
        .env("RMC_DAEMON_IDLE_SECS", "5")
        .stdin(std::fs::File::open(&script)?)
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    hermetic_env(&mut command);
    if shared {
        command.env_remove("RMC_DAEMON");
    } else {
        command.env("RMC_DAEMON", "0");
    }

    let output = command.output()?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        // A response, not a notification: it carries an id and an outcome.
        .filter(|value| value.get("result").is_some() || value.get("error").is_some())
        .filter_map(|value| value.get("id").and_then(Value::as_u64))
        .collect())
}

/// A one-shot session must not lose the replies it was owed.
///
/// The proxy used to end the moment stdin reached EOF, which is the moment the
/// last request has just been sent — so the answers still travelling from the
/// daemon were dropped, and a piped invocation returned NOTHING while the same
/// input with `RMC_DAEMON=0` answered normally. The daemon looked broken to
/// every non-interactive caller and fine to every editor.
///
/// The oracle is the script itself: every request in it is owed an answer.
/// It used to be the in-process path, and that path is no oracle — rmcp ends a
/// service the moment its input ends and drops the answers still being
/// computed, so `RMC_DAEMON=0` answers this script with `[1]`, the `tools/call`
/// lost; the comparison could only pass while the daemon lost the same answer.
/// Now the client passes the EOF on only after the last answer.
///
/// This test judges the outcome. Which mechanism delivers it is judged by
/// `relay::tests::a_one_shot_session_gets_every_answer`: a relay that
/// half-closed at once would still get here, by losing the connection and
/// replaying the call — the long way round.
#[test]
fn a_piped_session_keeps_its_replies() -> Result<()> {
    let shared_dir = TempDir::new()?;

    let through_daemon = piped_response_ids(shared_dir.path(), true)?;

    // The piped session leaves its daemon behind and its replies carry no pid,
    // so ask an ordinary session which process is listening, and stop it.
    let mut probe = Session::start(shared_dir.path(), true)?;
    let daemon_pid = probe.serving_pid()?;
    drop(probe);
    kill_pid(daemon_pid);

    assert_eq!(
        through_daemon,
        vec![1, 2],
        "the daemon dropped replies owed to a one-shot session"
    );
    Ok(())
}

/// A file that is not a socket must survive a client that finds it in the way.
///
/// `--socket` names any path, so a typo used to be enough: the client found
/// something at that address, could not connect to it, and deleted it as a
/// stale socket. The file it deletes belongs to whoever mistyped the path.
#[test]
fn a_file_that_is_not_a_socket_is_left_alone() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let not_a_socket = socket_dir.path().join("precious.sock");
    std::fs::write(&not_a_socket, b"someone else's data")?;

    let mut command = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"));
    command
        .arg("--socket")
        .arg(&not_a_socket)
        .env("RUST_LOG", "error")
        .env("RMC_DAEMON_DIR", socket_dir.path())
        .env("RMC_DAEMON_IDLE_SECS", "5")
        .env_remove("RMC_DAEMON")
        // Empty stdin: the client falls back in-process and the server ends at
        // once, which is all this test needs — the decision about the file is
        // taken before any session starts.
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hermetic_env(&mut command);
    command.status()?;

    let survived = std::fs::read(&not_a_socket).context("the client deleted a file it did not own")?;
    assert_eq!(survived, b"someone else's data");
    Ok(())
}

/// A one-file crate for a real rust-analyzer load: small enough that the
/// positive control costs seconds, real enough that a load actually happens.
fn probe_crate() -> Result<TempDir> {
    let dir = TempDir::new()?;
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"retire_probe\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
    )?;
    std::fs::create_dir(dir.path().join("src"))?;
    std::fs::write(dir.path().join("src/lib.rs"), "pub fn probe_target() {}\n")?;
    Ok(dir)
}

fn find_probe_target(session: &mut Session, project: &Path) -> Result<Value> {
    session.call_tool(
        "find_definition",
        json!({
            "symbol_name": "probe_target",
            "directory": project.to_str().ok_or_else(|| anyhow!("non-UTF-8 temp path"))?,
            "exact": true
        }),
    )
}


/// A daemon started on `socket` with `env`, once it listens.
fn start_daemon(socket: &Path, env: &[(&str, &str)]) -> Result<Child> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"));
    command
        .arg("--daemon")
        .arg("--socket")
        .arg(socket)
        .envs(env.iter().copied())
        .env("RUST_LOG", "error")
        .env("RMC_DAEMON_IDLE_SECS", "5")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hermetic_env(&mut command);
    let mut daemon = command.spawn()?;
    if wait_until(Duration::from_secs(60), || socket.exists()).is_none() {
        let _ = daemon.kill();
        return Err(anyhow!("the daemon never bound {}", socket.display()));
    }
    Ok(daemon)
}

/// Whether `pid` has exited: gone from /proc, or a zombie nobody reaped yet —
/// a daemon's parent is the client that spawned it, and a test's client lives on.
fn process_gone(pid: u32) -> bool {
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Err(_) => true,
        Ok(stat) => stat
            .rsplit_once(") ")
            .is_some_and(|(_, rest)| rest.starts_with('Z')),
    }
}

/// A retired daemon must refuse analysis instead of loading it again.
///
/// Measured 2026-09-22 on a live machine: a daemon retired at 28.8 GB still had
/// three sessions attached, and each of their questions loaded a `Full` context
/// back — five reloads to 15–19 GB over the grace period, next to a successor
/// growing its own copy. Retirement meant to release memory held two analyses.
///
/// The case is driven for real rather than through `decide_watchdog_action`:
/// `RMC_RSS_HARD_MB=1` makes the very first watchdog tick retire the daemon,
/// with this client still attached — the draining session of the incident.
///
/// Straight on the daemon's socket, not through a client: a client moves its
/// session off a retired daemon (`relay`), and then this call would be
/// answered by the successor — which is the point of the client, and not what
/// this test is about.
///
/// Mutation that must fail it: drop the `refuse_new_analyses()` call on the
/// retire branch, and the call below loads the crate and finds the symbol.
#[test]
fn a_retired_daemon_refuses_analysis_instead_of_loading_it() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let socket = socket_dir.path().join("retiring.sock");
    let project = probe_crate()?;

    let mut daemon = start_daemon(&socket, &[("RMC_RSS_HARD_MB", "1")])?;
    let mut session = Session::over_socket(&socket)?;

    // The watchdog ticks every 15 s; the unlinked socket is the retirement.
    wait_until(Duration::from_secs(60), || !socket.exists())
        .ok_or_else(|| anyhow!("the daemon never retired, so there is nothing to judge"))?;

    let response = find_probe_target(&mut session, project.path())?;
    drop(session);
    let _ = daemon.kill();
    let _ = daemon.wait();

    let message = response
        .pointer("/error/message")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            anyhow!("a retired daemon answered the analysis instead of refusing it: {response}")
        })?;
    assert!(
        message.contains("has retired") && message.contains("reconnect"),
        "the refusal must say why and what to do, or the session reads it as a broken tool: \
         {message}"
    );
    Ok(())
}

/// Positive control for the test above: the same call, on the same crate,
/// against a daemon that has NOT retired, finds the symbol. Without it "the
/// call was refused" could be any failure to load a crate in a temp directory.
#[test]
fn a_serving_daemon_answers_the_same_analysis() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let socket = socket_dir.path().join("serving.sock");
    let project = probe_crate()?;

    let mut daemon = start_daemon(&socket, &[])?;
    let mut session = Session::over_socket(&socket)?;
    let response = find_probe_target(&mut session, project.path())?;
    drop(session);
    let _ = daemon.kill();
    let _ = daemon.wait();

    let text = response
        .pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("the analysis failed on a daemon that never retired: {response}"))?;
    assert!(
        text.contains("probe_target") && text.starts_with("Found"),
        "the control must actually load the crate and find the symbol: {text}"
    );
    Ok(())
}

/// Positive control: the opt-out must restore the previous behaviour.
#[test]
fn opt_out_serves_in_process() -> Result<()> {
    let socket_dir = TempDir::new()?;

    let mut session = Session::start(socket_dir.path(), false)?;
    let serving_pid = session.serving_pid()?;

    assert_eq!(
        Some(serving_pid),
        session.client_pid(),
        "with RMC_DAEMON=0 the client process itself must serve the session"
    );
    Ok(())
}

/// Ask `session` for its serving pid until it is not `old`, for up to `timeout`.
fn wait_for_new_pid(session: &mut Session, old: u32, timeout: Duration) -> Result<u32> {
    let deadline = Instant::now() + timeout;
    loop {
        let pid = session.serving_pid()?;
        if pid != old {
            return Ok(pid);
        }
        if Instant::now() >= deadline {
            return Err(anyhow!("still served by {old} after {timeout:?}"));
        }
        thread::sleep(Duration::from_secs(1));
    }
}

/// A daemon that dies under a session must not take the session's tools with
/// it: the client starts a successor, replays the handshake, and the next call
/// is answered there — no error, a different pid.
///
/// SIGKILL, the OOM killer's signal and the shape of a GPU abort: the socket
/// file stays behind and nothing gets to say goodbye.
///
/// Mutation that must fail it: end the relay on a lost connection instead of
/// dialing again, and the call below finds stdout closed.
#[test]
fn a_session_survives_its_daemon_being_killed() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let mut session = Session::start(socket_dir.path(), true)?;
    let first = session.serving_pid()?;

    let _ = Command::new("kill").arg("-KILL").arg(first.to_string()).status();
    wait_until(Duration::from_secs(10), || process_gone(first))
        .ok_or_else(|| anyhow!("SIGKILL did not end {first}"))?;

    let second = session.serving_pid()?;
    drop(session);
    kill_pid(second);
    assert_ne!(second, first, "a killed daemon cannot have answered");
    Ok(())
}

/// The binary under test, linked into a directory of its own — the launch
/// path a test can replace. A hard link, so staging costs no copy; the
/// directory sits in the target dir because a hard link cannot cross into
/// the tmpfs that holds ordinary temp directories.
fn staged_binary() -> Result<(TempDir, std::path::PathBuf)> {
    let dir = tempfile::Builder::new()
        .prefix("rebuilt-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))?;
    let exe = dir.path().join("rust-code-mcp");
    std::fs::hard_link(env!("CARGO_BIN_EXE_rust-code-mcp"), &exe)?;
    Ok((dir, exe))
}

/// A rebuild as the deploy does it: a new file renamed over the launch path.
/// A copy, because only a new file has a new identity (size, mtime).
fn replace_binary(exe: &Path) -> Result<()> {
    let next = exe.with_extension("next");
    std::fs::copy(env!("CARGO_BIN_EXE_rust-code-mcp"), &next)?;
    std::fs::rename(&next, exe)?;
    Ok(())
}

/// A replaced binary retires its daemon, and every session moves to a daemon of
/// the new binary between requests, without losing its tools; the old daemon
/// then exits on its own instead of living for as long as its longest session.
///
/// Two sessions, one per way a client finds its daemon: `--socket` (the path
/// stays, only the daemon's retirement can tell the client to move) and the
/// derived key (the path itself changes with the binary). Measured before
/// this existed: two daemons of a deleted binary held ≈ 30 GB, one of them
/// with twelve sessions attached since the morning.
///
/// Mutation that must fail it: skip the retirement on `Launch::replacement`
/// in the daemon, and the `--socket` session is still served by its old pid.
#[test]
fn a_replaced_binary_moves_its_sessions_and_retires_its_daemons() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let (_staged, exe) = staged_binary()?;
    let socket = socket_dir.path().join("named.sock");
    let named_args = [std::ffi::OsStr::new("--socket"), socket.as_os_str()];

    let mut named = Session::start_from(&exe, &named_args, socket_dir.path(), true, None, &[])?;
    let mut keyed = Session::start_from(&exe, &[], socket_dir.path(), true, None, &[])?;
    let named_old = named.serving_pid()?;
    let keyed_old = keyed.serving_pid()?;
    assert_ne!(named_old, keyed_old, "two sockets, two daemons");

    replace_binary(&exe)?;

    // Daemon tick (15 s) to retire, client tick (15 s) to move, then a start.
    let budget = Duration::from_secs(120);
    let named_new = wait_for_new_pid(&mut named, named_old, budget);
    let keyed_new = wait_for_new_pid(&mut keyed, keyed_old, budget);
    // Both old daemons are left without clients: they exit on their next tick.
    let named_exited = wait_until(Duration::from_secs(60), || process_gone(named_old));
    let keyed_exited = wait_until(Duration::from_secs(60), || process_gone(keyed_old));

    let successors = [&named_new, &keyed_new].map(|pid| pid.as_ref().ok().copied());
    for pid in [Some(named_old), Some(keyed_old)]
        .into_iter()
        .chain(successors)
        .flatten()
    {
        kill_pid(pid);
    }
    drop(named);
    drop(keyed);

    named_new.context("the --socket session never left the replaced binary's daemon")?;
    keyed_new.context("the keyed session never moved to the new binary's daemon")?;
    named_exited.ok_or_else(|| anyhow!("the --socket session's old daemon {named_old} never exited"))?;
    keyed_exited.ok_or_else(|| anyhow!("the keyed session's old daemon {keyed_old} never exited"))?;
    Ok(())
}

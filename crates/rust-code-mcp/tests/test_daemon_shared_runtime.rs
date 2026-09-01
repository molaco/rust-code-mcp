//! The oracle for the shared daemon: however many sessions attach to a project,
//! the analysis lives in ONE process.
//!
//! The serving pid is that oracle. `runtime_status` reports the pid of the
//! process that actually answered, so two sessions on one pid share their state,
//! while two pids mean each loaded its own copy of the rust-analyzer context —
//! the regression the daemon exists to prevent. `RMC_DAEMON=0` is the positive
//! control: there the pid must equal the client's own, without which "one pid
//! twice" would not be distinguishable from "both served somewhere else".
//!
//! Every test uses a socket directory of its own. The default one belongs to
//! whatever session the developer is running, and a test that attached to it
//! would assert about someone else's process and then kill it.

#![cfg(unix)]

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// The env variable that picks the automatic embedding profile, and a value it
/// must never accept. Spelled out rather than imported: the variable is the
/// contract a host sets, not an internal name.
const PROFILE_ENV: &str = "RMC_EMBEDDING_PROFILE";
const BAD_PROFILE: &str = "local-cpu-small-typo";

/// Where a test's daemon logs go, under its socket directory so one temp dir
/// removes both. Only here: in production the log directory is on disk and the
/// socket directory is a tmpfs, and keeping them apart is the point.
const LOG_SUBDIR: &str = "logs";

/// A client to start: the binary as every test here needs it, plus the parts of
/// the environment that take part in the daemon key.
#[derive(Clone, Default)]
struct Client { cwd: Option<PathBuf>, env: Vec<(String, String)> }

impl Client {
    /// Sharing is the default, and the absence of `RMC_DAEMON` is what selects it.
    fn shared() -> Self { Self::default() }
    fn in_process() -> Self { Self::default().env("RMC_DAEMON", "0") }
    fn cwd(mut self, dir: impl Into<PathBuf>) -> Self { self.cwd = Some(dir.into()); self }

    fn env(mut self, name: &str, value: impl Into<String>) -> Self {
        self.env.push((name.to_string(), value.into()));
        self
    }

    /// Piped stdio, the given socket directory, and an idle timeout short enough
    /// that an orphaned daemon exits on its own if a test fails before the kill.
    fn command(&self, socket_dir: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"));
        command.env("RUST_LOG", "error").env("RMC_DAEMON_DIR", socket_dir)
            .env("RMC_DAEMON_LOG_DIR", socket_dir.join(LOG_SUBDIR))
            .env("RMC_DAEMON_IDLE_SECS", "5").env_remove("RMC_DAEMON")
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        if let Some(cwd) = &self.cwd { command.current_dir(cwd); }
        for (name, value) in &self.env { command.env(name, value); }
        command
    }

    /// Spawn the client and take it through the MCP handshake.
    fn start(&self, socket_dir: &Path) -> Result<Session> {
        let mut child = self.command(socket_dir).spawn().context("cannot spawn the client")?;
        let stdout = child.stdout.take().context("child stdout was not piped")?;
        let stdin = child.stdin.take().context("child stdin was not piped")?;
        // Reading on a thread is what makes a timeout possible: a server that
        // never answers must fail the test rather than block the run for ever.
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || BufReader::new(stdout).lines().try_for_each(|l| tx.send(l)).ok());

        let mut session = Session { child, stdin, rx, next_id: 1 };
        let response = session.call("initialize", initialize_params())?;
        if response.get("error").is_some() {
            return Err(anyhow!("initialize failed: {response}"));
        }
        session.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} }))?;
        Ok(session)
    }
}

fn initialize_params() -> Value {
    json!({ "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": { "name": "daemon-sharing-test", "version": "0.0.0" } })
}

/// A live MCP client: the binary plus a pipe to it.
struct Session { child: Child, stdin: ChildStdin, rx: Receiver<std::io::Result<String>>, next_id: u64 }

impl Session {
    fn send(&mut self, message: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &message)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }

    /// One request, and the reply to that request. A cold daemon loads the
    /// analysis before it answers, hence the long wait.
    fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            let line = self.rx.recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .map_err(|e| anyhow!("no reply to request {id}: {e}"))??;
            let value: Value = serde_json::from_str(&line)
                .with_context(|| format!("stdout carried a non-JSON-RPC line: {line:?}"))?;
            if value.get("id").and_then(Value::as_u64) == Some(id) { return Ok(value); }
        }
    }

    /// The pid of the process that ACTUALLY serves this session's calls. An error
    /// reply carries no status text, so it fails here with the reply quoted.
    fn serving_pid(&mut self) -> Result<u32> {
        let response = self.call("tools/call", json!({ "name": "runtime_status", "arguments": {} }))?;
        let text = response.pointer("/result/content/0/text").and_then(Value::as_str)
            .ok_or_else(|| anyhow!("no status text in the response: {response}"))?;
        serde_json::from_str::<Value>(text)?.pointer("/process/pid").and_then(Value::as_u64)
            .map(|pid| pid as u32)
            .ok_or_else(|| anyhow!("no process.pid in the status: {text}"))
    }
}

impl Drop for Session {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}

fn signal_pid(pid: u32, signal: &str) {
    let _ = Command::new("kill").arg(signal).arg(pid.to_string()).status();
}

fn wait_until(timeout: Duration, mut done: impl FnMut() -> bool) -> Option<()> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if done() { return Some(()); }
        thread::sleep(Duration::from_millis(20));
    }
    None
}

/// A daemon of its own, listening: the tests below assert about the socket it
/// bound, so a spawn that never bound is an error, not a failed assertion.
fn bound_daemon(socket: &Path) -> Result<Child> {
    let mut daemon = Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"))
        .arg("--daemon").arg("--socket").arg(socket)
        .env("RUST_LOG", "error").env("RMC_DAEMON_IDLE_SECS", "5")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn()?;
    if wait_until(Duration::from_secs(60), || socket.exists()).is_none() {
        let _ = daemon.kill();
        return Err(anyhow!("the daemon never bound {}", socket.display()));
    }
    Ok(daemon)
}

#[test]
fn the_serving_pid_says_which_process_answers() -> Result<()> {
    // The three relations a case can expect, spelled as they read in a failure.
    let one_daemon = "served by one process";
    let different_daemons = "served by different processes";
    let the_client_itself = "served by the client process itself";

    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)
        .expect("the crate manifest lives two levels below the workspace root");
    let member = root.join("crates").join("rust-code-mcp");
    // Two index roots, and two library paths that only append one of them, so
    // the values differ while the loader still finds the libraries it needs.
    let (a, b) = (TempDir::new()?, TempDir::new()?);
    let (root_a, root_b) = (a.path().display().to_string(), b.path().display().to_string());
    let libs = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
    let (libs_a, libs_b) = (format!("{libs}:{root_a}"), format!("{libs}:{root_b}"));
    let split_on = |name: &str, first: &str, second: &str| {
        vec![Client::shared().env(name, first), Client::shared().env(name, second)]
    };

    // The socket directory is a private temporary one unless a case names its
    // own; `/proc` refuses `mkdir`, so no socket directory can appear under it.
    let cases: Vec<(&str, Option<&Path>, Vec<Client>, &str)> = vec![
        ("two sessions of one project", None,
         vec![Client::shared(), Client::shared()], one_daemon),
        ("two directories of one repository", None,
         vec![Client::shared().cwd(root), Client::shared().cwd(&member)], one_daemon),
        ("two sessions differing in OPENROUTER_API_KEY", None,
         split_on("OPENROUTER_API_KEY", "AAA", "BBB"), different_daemons),
        ("two sessions differing in LD_LIBRARY_PATH", None,
         split_on("LD_LIBRARY_PATH", &libs_a, &libs_b), different_daemons),
        ("two sessions differing in XDG_DATA_HOME", None,
         split_on("XDG_DATA_HOME", &root_a, &root_b), different_daemons),
        ("a session with RMC_DAEMON=0", None,
         vec![Client::in_process()], the_client_itself),
        ("a session whose socket directory cannot be used",
         Some(Path::new("/proc/self/cannot-create-here")),
         vec![Client::shared()], the_client_itself),
    ];

    for (name, named_dir, clients, expected) in cases {
        let private = TempDir::new()?;
        let socket_dir = named_dir.unwrap_or(private.path());
        // Both pid sets are read while every session of the case is attached.
        let mut sessions = Vec::new();
        let (mut own, mut serving) = (Vec::new(), Vec::new());
        for client in &clients {
            let mut session = client.start(socket_dir)?;
            own.push(session.child.id());
            serving.push(session.serving_pid()?);
            sessions.push(session);
        }
        drop(sessions);

        let served = if own == serving {
            the_client_itself
        } else if serving.iter().collect::<HashSet<_>>().len() == 1 {
            one_daemon
        } else {
            different_daemons
        };
        for pid in serving.iter().copied().filter(|pid| !own.contains(pid)) {
            signal_pid(pid, "-TERM");
        }
        assert_eq!(served, expected,
            "{name}: {served}, not {expected}; serving pids {serving:?}, own pids {own:?}");
    }
    Ok(())
}

/// The pid says one process answers; this says WHERE it answers from. A daemon
/// left in the directory of whichever client started it would resolve a relative
/// `directory` argument differently for every client of the same daemon.
// procfs is the only portable-enough way to read another process's working
// directory; the behaviour under test is not Linux-specific.
#[cfg(target_os = "linux")]
#[test]
fn a_daemon_works_in_the_project_root_not_in_its_client() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(2)
        .expect("the crate manifest lives two levels below the workspace root");
    let member = root.join("crates").join("rmc-server");
    let socket_dir = TempDir::new()?;
    let mut session = Client::shared().cwd(&member).start(socket_dir.path())?;
    let serving_pid = session.serving_pid()?;
    assert_ne!(serving_pid, session.child.id(), "no daemon came up, so it has no cwd to read");

    let serving_cwd = fs::read_link(format!("/proc/{serving_pid}/cwd"))?;
    drop(session);
    signal_pid(serving_pid, "-TERM");
    assert_eq!(serving_cwd, fs::canonicalize(root)?,
        "the daemon works in {}, so a relative directory means that instead of the project root",
        serving_cwd.display());
    Ok(())
}

/// One non-interactive session, the shape a host uses for a single call: the
/// whole exchange is already waiting and stdin is at EOF from the first read, so
/// every reply still owed has to arrive before the process ends. Returns the ids
/// of the responses in arrival order.
fn piped_response_ids(client: &Client, socket_dir: &Path) -> Result<Vec<u64>> {
    let dir = TempDir::new()?;
    let script = dir.path().join("session.jsonl");
    let mut file = fs::File::create(&script)?;
    for message in [
        json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": initialize_params() }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized", "params": {} }),
        json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/call",
                "params": { "name": "runtime_status", "arguments": {} } }),
    ] {
        serde_json::to_writer(&mut file, &message)?;
        file.write_all(b"\n")?;
    }
    drop(file);

    let output = client.command(socket_dir).stdin(fs::File::open(&script)?).output()?;
    Ok(String::from_utf8_lossy(&output.stdout).lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        // A response, not a notification: it carries an id and an outcome.
        .filter(|value| value.get("result").is_some() || value.get("error").is_some())
        .filter_map(|value| value.get("id").and_then(Value::as_u64))
        .collect())
}

/// The oracle is the in-process path rather than a fixed list: whatever that one
/// answers for this input, the daemon must answer too.
#[test]
fn a_piped_session_keeps_its_replies() -> Result<()> {
    let shared_dir = TempDir::new()?;
    let direct_dir = TempDir::new()?;
    let through_daemon = piped_response_ids(&Client::shared(), shared_dir.path())?;
    let in_process = piped_response_ids(&Client::in_process(), direct_dir.path())?;

    // The piped session leaves its daemon behind, and its own replies carry no
    // pid, so ask an ordinary session which process is listening and stop it.
    let mut probe = Client::shared().start(shared_dir.path())?;
    let daemon_pid = probe.serving_pid()?;
    drop(probe);
    signal_pid(daemon_pid, "-TERM");

    assert_eq!(through_daemon, in_process,
        "the daemon dropped replies that the in-process server delivered for the same input");
    Ok(())
}

/// A server that died under a live session is a crash, not a shutdown: an
/// in-process retry would read a stdin that is already half consumed.
#[test]
fn a_killed_daemon_does_not_become_an_in_process_server() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let mut session = Client::shared().start(socket_dir.path())?;
    let serving_pid = session.serving_pid()?;
    assert_ne!(serving_pid, session.child.id(), "no daemon came up, so there is nothing to kill");

    signal_pid(serving_pid, "-KILL");
    wait_until(Duration::from_secs(60), || matches!(session.child.try_wait(), Ok(Some(_))))
        .ok_or_else(|| anyhow!("the client did not exit after its daemon was killed"))?;
    let status = session.child.wait()?;
    assert!(!status.success(),
        "a daemon killed mid-session was reported as a clean shutdown: {status:?}");
    Ok(())
}

/// Clients survive a stale socket, but while the file is there `--print-socket`
/// plus `ls` point at an address where nobody listens.
#[test]
fn killed_daemon_removes_its_socket() -> Result<()> {
    let dir = TempDir::new()?;
    let socket = dir.path().join("probe.sock");
    let mut daemon = bound_daemon(&socket)?;

    signal_pid(daemon.id(), "-TERM");
    let gone = wait_until(Duration::from_secs(30), || !socket.exists());
    let _ = daemon.wait();
    gone.ok_or_else(|| anyhow!("SIGTERM left a stale {}", socket.display()))?;
    Ok(())
}

/// The socket directory may be one the user named, so the daemon must not
/// tighten a directory it did not create.
#[test]
fn an_existing_socket_directory_keeps_its_mode() -> Result<()> {
    let dir = TempDir::new()?;
    let socket_dir = dir.path().join("sockets");
    fs::create_dir(&socket_dir)?;
    fs::set_permissions(&socket_dir, fs::Permissions::from_mode(0o755))?;

    let mut daemon = bound_daemon(&socket_dir.join("probe.sock"))
        .context("the daemon refused to bind inside an existing 0o755 directory")?;
    let mode = fs::metadata(&socket_dir)?.permissions().mode() & 0o777;
    signal_pid(daemon.id(), "-TERM");
    let _ = wait_until(Duration::from_secs(30), || matches!(daemon.try_wait(), Ok(Some(_))));
    let _ = daemon.wait();

    assert_eq!(mode, 0o755, "the daemon changed the mode of a directory it did not create");
    Ok(())
}

/// The socket carries the whole session and the log carries what the daemon saw.
/// The log lives in the log directory, never beside the socket: the runtime
/// directory is a small tmpfs, and one daemon log once filled all of it.
#[test]
fn the_files_a_session_leaves_are_owner_only() -> Result<()> {
    let socket_dir = TempDir::new()?;
    let mut session = Client::shared().start(socket_dir.path())?;
    let serving_pid = session.serving_pid()?;
    assert_ne!(serving_pid, session.child.id(),
        "no daemon came up, so no socket, lock or log was written");

    let listing = |dir: &Path| -> Result<Vec<(String, u32)>> {
        let mut files = Vec::new();
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
            files.push((path.to_string_lossy().into_owned(), mode));
        }
        Ok(files)
    };
    let log_dir = socket_dir.path().join(LOG_SUBDIR);
    let beside_socket = listing(socket_dir.path())?;
    let logs = listing(&log_dir)?;
    let log_dir_mode = fs::metadata(&log_dir)?.permissions().mode() & 0o777;
    drop(session);
    signal_pid(serving_pid, "-TERM");

    for (files, suffix) in [(&beside_socket, ".sock"), (&beside_socket, ".lock"), (&logs, ".log")] {
        let found: Vec<_> = files.iter().filter(|(name, _)| name.ends_with(suffix)).collect();
        assert_eq!(found.len(), 1, "expected one {suffix}, found {files:?}");
        let (name, mode) = found[0];
        assert_eq!(*mode, 0o600,
            "{name} is 0o{mode:o}, so another account on this machine reaches the session");
    }
    assert!(!beside_socket.iter().any(|(name, _)| name.ends_with(".log")),
        "a log beside the socket is a log on the runtime tmpfs: {beside_socket:?}");
    assert_eq!(log_dir_mode, 0o700, "the log directory the daemon creates is owner-only");
    Ok(())
}

/// `--help` and `--print-socket` are how a host reads back a configuration it
/// got wrong, so they must answer for the very value it came to inspect, while
/// every mode that starts a server must refuse it.
#[test]
fn the_mode_decides_whether_a_bad_profile_is_refused() -> Result<()> {
    let anything: fn(&str) -> bool = |_| true;
    let names_the_value: fn(&str) -> bool = |text| text.contains(BAD_PROFILE);
    // (arguments, exit code, what stdout must show, what stderr must show)
    let cases: [(&[&str], i32, fn(&str) -> bool, fn(&str) -> bool); 4] = [
        (&[], 2, anything, names_the_value),
        (&["--in-process"], 2, anything, names_the_value),
        (&["--help"], 0, |text| text.contains("--print-socket"), anything),
        (&["--print-socket"], 0, |text| text.trim().ends_with(".sock"), anything),
    ];

    for (args, code, stdout_ok, stderr_ok) in cases {
        let socket_dir = TempDir::new()?;
        let output = Client::shared().env(PROFILE_ENV, BAD_PROFILE).command(socket_dir.path())
            .args(args).stdin(Stdio::null()).stderr(Stdio::piped()).output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        assert_eq!(output.status.code(), Some(code),
            "{args:?} with a bad {PROFILE_ENV} must exit {code}; stderr was: {stderr}");
        assert!(stderr_ok(&stderr), "{args:?} hid the rejected value; stderr was: {stderr}");
        assert!(stdout_ok(&stdout), "{args:?} did not print what the host came to read: {stdout}");
    }
    Ok(())
}

//! One server per project: a unix-socket daemon plus a thin proxy client.
//!
//! # Why
//!
//! The stdio transport is 1:1 with its client by construction — one pipe, one
//! process. Every editor window or agent session therefore spawned its own
//! `rust-code-mcp`, and with it another `SemanticService` (the loaded
//! rust-analyzer context for the workspace, ~2 GB) and another ONNX/GPU context.
//! Measured on one developer machine: six live servers, 8.9 GB, all of them
//! analyzing the same repository.
//!
//! The runtime state was already shareable and already partitioned by project:
//! `RuntimeState` is a bundle of `Arc`s, `SemanticService` caches contexts in a
//! `HashMap<PathBuf, ProjectContext>`, and locking is per workspace
//! (`WorkspaceLockRegistry`) rather than global. The only missing piece was a
//! transport that accepts more than one client.
//!
//! That is what this module adds: the daemon listens on a unix socket and serves
//! each connection with its own `SearchToolRouter` on top of one shared
//! `RuntimeState`. The client is the same binary with no arguments: it pumps
//! stdin/stdout into the socket, and spawns the daemon if none is listening.
//!
//! # The socket key is more than the project
//!
//! It covers the working directory, the binary's size and mtime, and the env
//! vars that change what the server computes. Otherwise a rebuilt binary — or a
//! different configuration — would silently attach to a daemon that answers
//! differently, and that reads as "the server is lying", not as "we connected to
//! the wrong one".
//!
//! # A failing daemon never leaves a client without a server
//!
//! Any failure along connect / spawn / wait returns `Ok(false)` from
//! [`run_client`], and the caller serves the session in-process exactly as it did
//! before this module existed. The daemon is a memory optimisation, not a new
//! point of failure.

use fs2::FileExt;
use rmc_server::mcp::{BACKGROUND_SYNC_ENV, RuntimeState, ServerRuntime};
use rmc_server::tools::SearchTool;
use rmcp::ServiceExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncWriteExt, copy};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Opt out of the whole scheme: `RMC_DAEMON=0` (`off`/`false`/`no`) keeps the
/// server inside the client process, as it was before.
pub const DAEMON_ENV: &str = "RMC_DAEMON";
/// Directory holding sockets and daemon logs. Defaults to
/// `$XDG_RUNTIME_DIR/rust-code-mcp`.
pub const DAEMON_DIR_ENV: &str = "RMC_DAEMON_DIR";
/// How long a daemon stays alive with no clients, in seconds. `0` means forever.
pub const IDLE_ENV: &str = "RMC_DAEMON_IDLE_SECS";

/// Env vars that change what the server computes, and therefore which daemon a
/// client belongs to. Extend this list whenever a new behaviour-changing knob is
/// added, or clients configured differently will end up sharing one server.
const KEYED_ENV: [&str; 1] = [BACKGROUND_SYNC_ENV];

/// Half an hour: long enough to survive a pause between questions in a session,
/// short enough that a closed editor does not hold gigabytes until end of day.
const DEFAULT_IDLE_SECS: u64 = 1800;
/// Idle-check interval, and therefore the upper bound on how late the daemon
/// exits after its last client leaves.
const IDLE_TICK: Duration = Duration::from_secs(15);
/// Upper bound on waiting for a daemon to come up. Deliberately generous, since
/// startup may include model initialisation. The wait is not blind: if the
/// process dies earlier, its exit status ends the wait instead of the timeout.
const SPAWN_WAIT: Duration = Duration::from_secs(90);
const SPAWN_POLL: Duration = Duration::from_millis(50);

/// How this process was started. Resolved *before* the expensive startup: a
/// client needs neither a `ServerRuntime` nor a background sync task — it is a pipe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Server inside this process over stdio — the behaviour before the daemon.
    InProcess,
    /// Daemon: listens on a socket, serves many clients from one `RuntimeState`.
    Daemon { socket: PathBuf, idle: Duration },
    /// Client: stdin/stdout ↔ socket, spawning the daemon when needed.
    Client { socket: PathBuf },
    /// `--print-socket`: print the resolved socket path and exit (diagnostics).
    PrintSocket { socket: PathBuf },
    /// `--help`.
    Help,
}

/// Parse arguments and env. `args` excludes the program name.
pub fn resolve_mode(args: &[String]) -> Result<Mode, BoxError> {
    let mut socket: Option<PathBuf> = None;
    let mut idle: Option<Duration> = None;
    let mut explicit: Option<&str> = None;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--help" | "-h" => return Ok(Mode::Help),
            "--daemon" | "--client" | "--in-process" | "--print-socket" => {
                if let Some(prev) = explicit {
                    return Err(format!("modes {prev} and {arg} are mutually exclusive").into());
                }
                explicit = Some(match arg.as_str() {
                    "--daemon" => "--daemon",
                    "--client" => "--client",
                    "--in-process" => "--in-process",
                    _ => "--print-socket",
                });
            }
            "--socket" => {
                let value = it
                    .next()
                    .ok_or_else(|| BoxError::from("--socket requires a path"))?;
                socket = Some(PathBuf::from(value));
            }
            "--idle-secs" => {
                let value = it
                    .next()
                    .ok_or_else(|| BoxError::from("--idle-secs requires a number"))?;
                idle = Some(Duration::from_secs(value.parse::<u64>()?));
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }

    let socket = match socket {
        Some(path) => path,
        None => default_socket_path()?,
    };
    let idle = idle.unwrap_or_else(idle_from_env);

    Ok(match explicit {
        Some("--daemon") => Mode::Daemon { socket, idle },
        Some("--client") => Mode::Client { socket },
        Some("--in-process") => Mode::InProcess,
        Some("--print-socket") => Mode::PrintSocket { socket },
        _ if daemon_disabled() => Mode::InProcess,
        _ => Mode::Client { socket },
    })
}

pub const USAGE: &str = "\
rust-code-mcp — an MCP server for Rust codebases.

With no arguments: a client of this project's shared daemon, which is started
on demand.

  --client            the same, explicitly
  --daemon            become the daemon: listen on a socket, serve many clients
  --in-process        run the server in this process over stdio (previous behaviour)
  --print-socket      print this project's socket path and exit
  --socket <PATH>     use this socket instead of the one derived from the project
  --idle-secs <N>     daemon exits after N seconds with no clients (0 = never)

Env: RMC_DAEMON=0 forces in-process; RMC_DAEMON_DIR sets the socket directory;
     RMC_DAEMON_IDLE_SECS is the same as --idle-secs.
";

fn daemon_disabled() -> bool {
    match std::env::var(DAEMON_ENV) {
        Ok(value) => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        ),
        Err(_) => false,
    }
}

fn idle_from_env() -> Duration {
    let secs = std::env::var(IDLE_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_SECS);
    Duration::from_secs(secs)
}

/// Where sockets live. `$XDG_RUNTIME_DIR` is preferred: it is private, on tmpfs,
/// and cleaned out at logout together with any orphaned sockets.
fn socket_dir() -> Result<PathBuf, BoxError> {
    if let Ok(dir) = std::env::var(DAEMON_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(runtime_dir) = std::env::var("XDG_RUNTIME_DIR") {
        if !runtime_dir.is_empty() {
            return Ok(PathBuf::from(runtime_dir).join("rust-code-mcp"));
        }
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "shared".to_string());
    Ok(std::env::temp_dir().join(format!("rust-code-mcp-{user}")))
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    // The socket is an entry point into analysing someone's code: owner only.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// The daemon key: the project plus everything that changes what answers mean.
///
/// The binary contributes its size and mtime rather than a content hash: a
/// rebuild must produce a *new* daemon (otherwise a client built from new code
/// would be served by the old server), and reading tens of megabytes on every
/// startup to establish that is not worth it.
fn workspace_key() -> Result<String, BoxError> {
    let cwd = std::env::current_dir()?;
    let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);

    let exe = std::env::current_exe()?;
    let exe_meta = fs::metadata(&exe).ok();
    let exe_len = exe_meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let exe_mtime = exe_meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    let env: Vec<(&str, String)> = KEYED_ENV
        .iter()
        .map(|key| {
            (
                *key,
                std::env::var(key).unwrap_or_else(|_| "<unset>".to_string()),
            )
        })
        .collect();

    Ok(key_from_parts(&cwd, &exe, exe_len, exe_mtime, &env))
}

/// The pure part of the key: everything that matters arrives as an argument.
///
/// Split out of [`workspace_key`] for testability rather than tidiness: checking
/// "the key changes with configuration" through `set_var` means mutating global
/// env in parallel with other tests, which fails for reasons unrelated to keys.
fn key_from_parts(
    cwd: &Path,
    exe: &Path,
    exe_len: u64,
    exe_mtime: u128,
    env: &[(&str, String)],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(cwd.as_os_str().as_encoded_bytes());
    hasher.update([0]);
    hasher.update(exe.as_os_str().as_encoded_bytes());
    hasher.update(exe_len.to_le_bytes());
    hasher.update(exe_mtime.to_le_bytes());
    for (key, value) in env {
        hasher.update([0]);
        hasher.update(key.as_bytes());
        hasher.update(b"=");
        hasher.update(value.as_bytes());
    }

    let digest = hasher.finalize();
    digest[..8].iter().map(|b| format!("{b:02x}")).collect()
}

pub fn default_socket_path() -> Result<PathBuf, BoxError> {
    Ok(socket_dir()?.join(format!("{}.sock", workspace_key()?)))
}

fn lock_path(socket: &Path) -> PathBuf {
    socket.with_extension("lock")
}

fn log_path(socket: &Path) -> PathBuf {
    socket.with_extension("log")
}

/// A file lock around "check / clear a stale socket / spawn / wait".
///
/// Without it, two sessions starting at the same moment both find no socket and
/// both spawn a daemon — which is exactly the duplicated memory this module
/// exists to remove.
struct SpawnLock {
    file: File,
}

impl SpawnLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(path)?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }
}

impl Drop for SpawnLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

async fn try_connect(socket: &Path) -> Option<UnixStream> {
    UnixStream::connect(socket).await.ok()
}

/// Client: serve this session through the shared daemon.
///
/// `Ok(true)` — the session ran through the daemon and finished. `Ok(false)` —
/// no daemon could be reached or started, and the caller must serve the session
/// itself, in-process.
pub async fn run_client(socket: &Path) -> Result<bool, BoxError> {
    if let Some(stream) = try_connect(socket).await {
        tracing::info!("connected to shared daemon at {}", socket.display());
        proxy(stream).await?;
        return Ok(true);
    }

    if let Some(parent) = socket.parent() {
        if let Err(e) = ensure_dir(parent) {
            tracing::warn!("socket dir {} unusable: {e}", parent.display());
            return Ok(false);
        }
    }

    let lock = match SpawnLock::acquire(&lock_path(socket)) {
        Ok(lock) => lock,
        Err(e) => {
            tracing::warn!("spawn lock unavailable: {e}; serving in-process");
            return Ok(false);
        }
    };

    // Re-check under the lock: a daemon may have come up while we waited for it.
    let stream = match try_connect(socket).await {
        Some(stream) => Some(stream),
        None => {
            // The socket file exists but refuses connections, so the daemon died
            // without cleaning up. Remove it ourselves: binding over a live file
            // fails with EADDRINUSE.
            if socket.exists() {
                let _ = fs::remove_file(socket);
            }
            match spawn_daemon(socket) {
                Ok(child) => wait_for_daemon(socket, child).await,
                Err(e) => {
                    tracing::warn!("failed to spawn daemon: {e}; serving in-process");
                    None
                }
            }
        }
    };
    drop(lock);

    match stream {
        Some(stream) => {
            proxy(stream).await?;
            Ok(true)
        }
        None => Ok(false),
    }
}

fn spawn_daemon(socket: &Path) -> io::Result<Child> {
    let exe = std::env::current_exe()?;
    let log = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log_path(socket))?;

    let mut cmd = Command::new(exe);
    cmd.arg("--daemon")
        .arg("--socket")
        .arg(socket)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        // The daemon's stderr goes to a file next to the socket: otherwise the
        // diagnostics of a shared process die with the session that spawned it.
        .stderr(Stdio::from(log))
        // Its own process group, so Ctrl-C in one client's session does not take
        // down a server other sessions are using.
        .process_group(0);
    cmd.spawn()
}

/// Wait for the daemon to bind. Ends early if the process dies, so a failed
/// startup costs a moment rather than the full timeout.
async fn wait_for_daemon(socket: &Path, mut child: Child) -> Option<UnixStream> {
    let deadline = tokio::time::Instant::now() + SPAWN_WAIT;
    loop {
        if let Some(stream) = try_connect(socket).await {
            return Some(stream);
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                tracing::warn!(
                    "daemon exited before accepting connections ({status}); see {}",
                    log_path(socket).display()
                );
                return None;
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("cannot poll daemon process: {e}");
                return None;
            }
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("daemon did not come up in {:?}", SPAWN_WAIT);
            let _ = child.kill();
            return None;
        }
        tokio::time::sleep(SPAWN_POLL).await;
    }
}

/// Pump stdin/stdout ↔ socket.
///
/// `select`, not `join`: the daemon is the side that closes the connection, and
/// waiting for EOF on stdin after that would hang — it may never arrive.
async fn proxy(stream: UnixStream) -> io::Result<()> {
    let (mut from_daemon, mut to_daemon) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();

    let upstream = async {
        copy(&mut stdin, &mut to_daemon).await?;
        to_daemon.shutdown().await
    };
    let downstream = async {
        copy(&mut from_daemon, &mut stdout).await?;
        stdout.flush().await
    };

    tokio::select! {
        result = upstream => result,
        result = downstream => result,
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Daemon: listen on the socket, serve every connection from one `RuntimeState`.
pub async fn run_daemon(
    socket: &Path,
    idle: Duration,
    runtime: &ServerRuntime,
) -> Result<(), BoxError> {
    if let Some(parent) = socket.parent() {
        ensure_dir(parent)?;
    }
    let listener = UnixListener::bind(socket).map_err(|e| {
        BoxError::from(format!(
            "cannot bind {}: {e} (is a live daemon already holding it?)",
            socket.display()
        ))
    })?;
    tracing::info!(
        "daemon listening on {} (idle timeout {:?})",
        socket.display(),
        idle
    );

    let live = Arc::new(AtomicUsize::new(0));
    let idle_since = Arc::new(AtomicI64::new(now_secs()));

    // Signals must reach the same exit path as an idle timeout. A daemon killed
    // outright leaves its socket file behind; clients survive that (they clear it
    // and start a new one), but `--print-socket` plus `ls` then point at an
    // address where nobody listens — diagnostics lying exactly when consulted.
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => Some(accepted),
            _ = tokio::time::sleep(IDLE_TICK) => None,
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM, shutting down");
                break;
            }
            _ = sigint.recv() => {
                tracing::info!("SIGINT, shutting down");
                break;
            }
        };

        match accepted {
            Some(Ok((stream, _addr))) => {
                let state = runtime.state();
                let live = Arc::clone(&live);
                let idle_since = Arc::clone(&idle_since);
                live.fetch_add(1, Ordering::SeqCst);
                tracing::info!("client connected ({} live)", live.load(Ordering::SeqCst));
                tokio::spawn(async move {
                    if let Err(e) = serve_connection(stream, state).await {
                        tracing::warn!("connection ended with error: {e}");
                    }
                    // The idle countdown starts when the *last* client leaves.
                    if live.fetch_sub(1, Ordering::SeqCst) == 1 {
                        idle_since.store(now_secs(), Ordering::SeqCst);
                    }
                });
            }
            Some(Err(e)) => {
                tracing::error!("accept failed: {e}");
                break;
            }
            None => {}
        }

        if !idle.is_zero()
            && live.load(Ordering::SeqCst) == 0
            && now_secs() - idle_since.load(Ordering::SeqCst) >= idle.as_secs() as i64
        {
            tracing::info!("no clients for {:?}, shutting down", idle);
            break;
        }
    }

    // Clean up, so the next client does not find a file, get refused, and spend a
    // round trip clearing a stale socket.
    let _ = fs::remove_file(socket);
    Ok(())
}

async fn serve_connection(stream: UnixStream, state: RuntimeState) -> Result<(), BoxError> {
    let (read_half, write_half) = stream.into_split();
    let service = SearchTool::with_runtime_state(state)
        .serve((read_half, write_half))
        .await?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode_of(args: &[&str]) -> Mode {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        resolve_mode(&owned).expect("mode")
    }

    #[test]
    fn explicit_socket_wins_over_computed_key() {
        let mode = mode_of(&["--daemon", "--socket", "/tmp/x.sock", "--idle-secs", "5"]);
        assert_eq!(
            mode,
            Mode::Daemon {
                socket: PathBuf::from("/tmp/x.sock"),
                idle: Duration::from_secs(5),
            }
        );
    }

    #[test]
    fn in_process_is_explicit_opt_out() {
        assert_eq!(
            mode_of(&["--in-process", "--socket", "/tmp/x.sock"]),
            Mode::InProcess
        );
    }

    #[test]
    fn two_modes_at_once_are_rejected() {
        let owned: Vec<String> = ["--daemon", "--client"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(resolve_mode(&owned).is_err());
    }

    #[test]
    fn unknown_argument_is_rejected() {
        let owned = vec!["--socks".to_string()];
        assert!(resolve_mode(&owned).is_err());
    }

    fn key(cwd: &str, exe: &str, len: u64, mtime: u128, sync: &str) -> String {
        key_from_parts(
            Path::new(cwd),
            Path::new(exe),
            len,
            mtime,
            &[(BACKGROUND_SYNC_ENV, sync.to_string())],
        )
    }

    #[test]
    fn key_is_stable_for_same_inputs() {
        assert_eq!(
            key("/repo", "/bin/mcp", 10, 20, "1"),
            key("/repo", "/bin/mcp", 10, 20, "1")
        );
    }

    /// Configuration must split daemons: a server started with different
    /// behaviour-changing env does not answer what the new client is asking for.
    #[test]
    fn key_depends_on_keyed_env() {
        assert_ne!(
            key("/repo", "/bin/mcp", 10, 20, "1"),
            key("/repo", "/bin/mcp", 10, 20, "0")
        );
    }

    /// Different projects, different daemons — otherwise "one per project" turns
    /// into "one for everything".
    #[test]
    fn key_depends_on_project() {
        assert_ne!(
            key("/repo-a", "/bin/mcp", 10, 20, "1"),
            key("/repo-b", "/bin/mcp", 10, 20, "1")
        );
    }

    /// A rebuilt binary must get a new socket, or a client built from new code is
    /// silently served by the old server.
    #[test]
    fn key_depends_on_binary_identity() {
        assert_ne!(
            key("/repo", "/bin/mcp", 10, 20, "1"),
            key("/repo", "/bin/mcp", 10, 21, "1"),
            "a different binary mtime means a different daemon"
        );
        assert_ne!(
            key("/repo", "/bin/mcp", 10, 20, "1"),
            key("/repo", "/bin/mcp", 11, 20, "1"),
            "a different binary size means a different daemon"
        );
    }
}

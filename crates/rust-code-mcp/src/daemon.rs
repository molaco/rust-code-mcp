//! One server per project: a unix-socket daemon plus a thin proxy client.
//!
//! The daemon serves every connection with its own router on top of one shared
//! `RuntimeState`, so a repository is analysed once instead of once per editor
//! window. The client is this same binary with no arguments: it pumps
//! stdin/stdout into the socket and spawns the daemon when nothing is listening.
//! [`workspace_key`] decides which daemon a client belongs to.

use fs2::FileExt;
use rmc_server::mcp::{RuntimeState, ServerRuntime};
use rmc_server::tools::SearchTool;
use rmcp::ServiceExt;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt, copy};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// `RMC_DAEMON=0` (`off`/`false`/`no`) keeps the server inside the client.
pub const DAEMON_ENV: &str = "RMC_DAEMON";
/// Sockets and locks. Defaults to `$XDG_RUNTIME_DIR/rust-code-mcp`.
pub const DAEMON_DIR_ENV: &str = "RMC_DAEMON_DIR";
/// Daemon logs. Defaults to `$XDG_STATE_HOME/rust-code-mcp/logs`: on disk, never
/// beside the socket. `XDG_RUNTIME_DIR` is a tmpfs of a few gigabytes meant for
/// sockets, and one daemon log filled the whole of it (3.3 GB, 2026-09-02).
pub const DAEMON_LOG_DIR_ENV: &str = "RMC_DAEMON_LOG_DIR";
/// Seconds a daemon stays alive with no clients. `0` means forever.
pub const IDLE_ENV: &str = "RMC_DAEMON_IDLE_SECS";

/// A prefix policy rather than a list: every knob this server reads is
/// namespaced, and a list falls behind the code, so clients configured
/// differently would share one server.
const KEYED_ENV_PREFIXES: [&str; 2] = ["RMC_", "RUST_CODE_MCP_"];

/// Keyed with no shared prefix: whose account pays for a request, and which CUDA
/// and ONNX libraries the daemon links when it starts.
const KEYED_ENV_EXTRA: [&str; 2] = ["OPENROUTER_API_KEY", "LD_LIBRARY_PATH"];

/// Keyed prefix, excluded anyway: these four select which daemon a client talks
/// to, or where it writes its log, rather than what it answers, and keying them
/// would split one daemon in two.
const UNKEYED_ENV: [&str; 4] = [DAEMON_ENV, DAEMON_DIR_ENV, IDLE_ENV, DAEMON_LOG_DIR_ENV];

/// Long enough to survive a pause between questions, short enough that a closed
/// editor does not hold gigabytes all day.
const DEFAULT_IDLE_SECS: u64 = 1800;
/// Idle-check interval, so also how late the daemon may exit after its last client.
const IDLE_TICK: Duration = Duration::from_secs(15);
/// Generous, since startup may include model initialisation. Not blind: a
/// process that dies earlier ends the wait sooner.
const SPAWN_WAIT: Duration = Duration::from_secs(90);
const SPAWN_POLL: Duration = Duration::from_millis(50);
/// How long the client drains the socket after stdin reached EOF. A healthy
/// session ends the drain by itself; this bound only stops a client waiting
/// forever behind a daemon that never closes, and reaching it is a failure, not
/// a shutdown: replies the host awaits never came.
const DRAIN_AFTER_EOF: Duration = Duration::from_secs(60);

/// How this process was started. Resolved *before* the expensive startup: a
/// client needs neither a `ServerRuntime` nor a background sync task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Server inside this process over stdio — the behaviour before the daemon.
    InProcess,
    /// Listens on a socket, serves many clients from one `RuntimeState`.
    Daemon { socket: PathBuf, idle: Duration },
    /// stdin/stdout ↔ socket, spawning the daemon when needed.
    ///
    /// `idle` holds `--idle-secs` only when the command line passed it, so a
    /// client-spawned daemon honours the flag; the environment default stays a
    /// daemon-side concern, since the daemon inherits the variable itself.
    Client {
        socket: PathBuf,
        idle: Option<Duration>,
    },
    /// `--print-socket`: print the resolved socket path and exit.
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
            "--daemon" | "--in-process" | "--print-socket" => {
                if let Some(prev) = explicit {
                    return Err(format!("modes {prev} and {arg} are mutually exclusive").into());
                }
                explicit = Some(arg.as_str());
            }
            "--socket" => socket = Some(PathBuf::from(it.next().ok_or("--socket needs a path")?)),
            "--idle-secs" => {
                let value = it.next().ok_or("--idle-secs needs a number")?;
                idle = Some(Duration::from_secs(value.parse::<u64>()?));
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
    }

    let socket = match socket {
        Some(path) => path,
        None => default_socket_path()?,
    };
    // The daemon needs a number and falls back to the environment default; the
    // client passes on only what the command line asked for.
    Ok(match explicit {
        Some("--daemon") => Mode::Daemon {
            socket,
            idle: idle.unwrap_or_else(idle_from_env),
        },
        Some("--in-process") => Mode::InProcess,
        Some("--print-socket") => Mode::PrintSocket { socket },
        _ if daemon_disabled() => Mode::InProcess,
        _ => Mode::Client { socket, idle },
    })
}

pub const USAGE: &str = "\
rust-code-mcp — an MCP server for Rust codebases.

With no arguments: a client of this project's shared daemon, started on demand.

  --daemon            become the daemon: listen on a socket, serve many clients
  --in-process        run the server in this process over stdio
  --print-socket      print this project's socket path and exit
  --socket <PATH>     use this socket instead of the one derived from the project
  --idle-secs <N>     daemon exits after N seconds with no clients (0 = never);
                      a client passes the flag on to the daemon it starts

Env: RMC_DAEMON=0 makes in-process the default mode; RMC_DAEMON_DIR sets the
     socket directory; RMC_DAEMON_LOG_DIR sets the log directory;
     RMC_DAEMON_IDLE_SECS is the same as --idle-secs.
";

fn daemon_disabled() -> bool {
    let Ok(value) = std::env::var(DAEMON_ENV) else {
        return false;
    };
    let value = value.trim().to_ascii_lowercase();
    matches!(value.as_str(), "0" | "off" | "false" | "no")
}

fn idle_from_env() -> Duration {
    let secs = std::env::var(IDLE_ENV)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_SECS);
    Duration::from_secs(secs)
}

/// `$XDG_RUNTIME_DIR` is preferred: private, on tmpfs, and cleaned out at logout
/// together with any orphaned sockets.
fn socket_dir() -> Result<PathBuf, BoxError> {
    if let Ok(dir) = std::env::var(DAEMON_DIR_ENV) {
        return Ok(PathBuf::from(dir));
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return Ok(PathBuf::from(dir).join("rust-code-mcp"));
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "shared".to_string());
    Ok(std::env::temp_dir().join(format!("rust-code-mcp-{user}")))
}

/// `geteuid` rather than the owner of `/proc/self`, which is Linux only.
fn process_uid() -> u32 {
    // SAFETY: no arguments, process state the kernel always has, cannot fail.
    unsafe { libc::geteuid() }
}

/// A refusal naming the path, so the warning the client logs before it falls
/// back in-process says what to fix.
fn refuse_dir(dir: &Path, reason: &str) -> io::Error {
    io::Error::other(format!("directory {}: {reason}", dir.display()))
}

/// The socket or log directory: created `0o700`, because the socket is an entry
/// point into analysing someone's code and the log carries its paths — and
/// otherwise checked, never chmodded. It may be a directory the user named
/// through `--socket`, and turning someone's project directory into `0o700`
/// behind their back is a change nobody asked for.
fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    // `symlink_metadata`: a symlink here decides where the socket really lands,
    // so it has to be seen rather than followed.
    let meta = match fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == ErrorKind::NotFound => {
            fs::create_dir_all(dir)?;
            return fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        Err(e) => return Err(refuse_dir(dir, &format!("cannot be inspected: {e}"))),
    };
    let uid = process_uid();
    if meta.file_type().is_symlink() {
        return Err(refuse_dir(dir, "is a symlink"));
    }
    if !meta.is_dir() {
        return Err(refuse_dir(dir, "is not a directory"));
    }
    if meta.uid() != uid {
        let owner = format!("is owned by uid {}, not by uid {uid}", meta.uid());
        return Err(refuse_dir(dir, &owner));
    }
    Ok(())
}

/// The directory that identifies the project, so two sessions started from
/// different subdirectories of one repository reach one daemon.
///
/// Walks up from `start`, stopping at a repository boundary — a directory holding
/// `.git` or `.jj`, that directory included. Within that range the nearest
/// `[workspace]` manifest wins, then the nearest `Cargo.toml`, then `start`.
///
/// Nearest, not highest: one stray `[workspace]` above two checkouts would
/// otherwise merge them into a single daemon, and one project's answers would
/// come from another project's server. Nearest is also Cargo's own rule.
fn project_root(start: &Path) -> PathBuf {
    let mut manifest: Option<PathBuf> = None;
    for dir in start.ancestors() {
        let toml = dir.join("Cargo.toml");
        if toml.is_file() {
            if manifest.is_none() {
                manifest = Some(dir.to_path_buf());
            }
            if let Ok(text) = fs::read_to_string(&toml)
                && text.lines().any(|line| line.trim().starts_with("[workspace]"))
            {
                return dir.to_path_buf();
            }
        }
        // Inclusive: this directory has just been inspected. `.git` is a file in
        // a worktree and a directory otherwise, so presence is the test.
        if dir.join(".git").exists() || dir.join(".jj").exists() {
            break;
        }
    }
    manifest.unwrap_or_else(|| start.to_path_buf())
}

/// The keyed variables of `env`, sorted by the raw bytes of the name so the key
/// does not depend on the order the operating system hands the environment over.
///
/// Bytes, never text: nothing is decoded and nothing is skipped. An entry
/// dropped for not decoding still changed what the daemon does, so two shells
/// with different non-UTF-8 `LD_LIBRARY_PATH` values got one key and therefore
/// one daemon — the collision this key exists to prevent. Raw bytes also keep
/// every value out of the code, so no secret can reach a log from here.
fn keyed_env_pairs(env: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    let keyed = |name: &OsString| {
        let name = name.as_bytes();
        !UNKEYED_ENV.iter().any(|unkeyed| name == unkeyed.as_bytes())
            && (KEYED_ENV_PREFIXES.iter().any(|p| name.starts_with(p.as_bytes()))
                || KEYED_ENV_EXTRA.iter().any(|extra| name == extra.as_bytes()))
    };
    let mut pairs: Vec<(OsString, OsString)> = env.filter(|(name, _)| keyed(name)).collect();
    pairs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    pairs
}

/// The daemon key: the project plus everything that changes what answers mean.
///
/// The binary contributes size and mtime rather than a content hash: a rebuild
/// must produce a *new* daemon, or a client built from new code would be served
/// by the old server, and hashing tens of megabytes on every startup to
/// establish that is not worth it.
///
/// The index root contributes the path it resolves to, not the variables that
/// resolve it: `data_dir()` reads `XDG_DATA_HOME`, then `HOME`, neither of which
/// carries a keyed prefix, so two clients pointing at different index roots used
/// to meet in one daemon and read and write the root of whichever client spawned
/// it. Hashing the outcome also survives a later change to that chain.
fn workspace_key() -> Result<String, BoxError> {
    let cwd = std::env::current_dir()?;
    let cwd = fs::canonicalize(&cwd).unwrap_or(cwd);
    let project = project_root(&cwd);
    let data_dir = rmc_server::mcp::project_paths::data_dir();

    let exe = std::env::current_exe()?;
    let exe_meta = fs::metadata(&exe).ok();
    let exe_len = exe_meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let exe_mtime = exe_meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0);

    // `vars_os`, not `vars`, which panics on a non-UTF-8 name or value anywhere
    // in this process: this runs before the in-process fallback exists, so that
    // panic would kill the session instead of degrading it.
    let env = keyed_env_pairs(std::env::vars_os());
    let key = key_from_parts(&project, &data_dir, &exe, exe_len, exe_mtime, &env);

    // Names, never values: the keyed set includes OPENROUTER_API_KEY, and a
    // secret written to a log stays there.
    let names: Vec<_> = env.iter().map(|(name, _)| name.to_string_lossy()).collect();
    tracing::debug!(
        "daemon key {key} for project {}, index root {}, keyed env: {}",
        project.display(),
        data_dir.display(),
        names.join(", ")
    );
    Ok(key)
}

/// The pure part of the key, hashing `env` in the order given. Split out for
/// testability: checking "the key changes with configuration" through `set_var`
/// means mutating global env in parallel with other tests, which fails for
/// unrelated reasons.
fn key_from_parts(
    project: &Path,
    data_dir: &Path,
    exe: &Path,
    exe_len: u64,
    exe_mtime: u128,
    env: &[(OsString, OsString)],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(project.as_os_str().as_encoded_bytes());
    hasher.update([0]);
    hasher.update(data_dir.as_os_str().as_encoded_bytes());
    hasher.update([0]);
    hasher.update(exe.as_os_str().as_encoded_bytes());
    hasher.update(exe_len.to_le_bytes());
    hasher.update(exe_mtime.to_le_bytes());
    for (name, value) in env {
        hasher.update([0]);
        hasher.update(name.as_bytes());
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

/// Where daemon logs go: [`DAEMON_LOG_DIR_ENV`], else the XDG state directory,
/// else next to the index. Never the socket directory: that is a tmpfs of a few
/// gigabytes meant for sockets, and a log there is bounded by the machine's RAM
/// rather than by anything this code does.
fn log_dir() -> PathBuf {
    if let Ok(dir) = std::env::var(DAEMON_LOG_DIR_ENV) {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new()
        .and_then(|base| base.state_dir().map(|state| state.join("rust-code-mcp").join("logs")))
        .unwrap_or_else(|| rmc_server::mcp::project_paths::data_dir().join("logs"))
}

/// `<dir>/<stem>-<hash>.log`: the socket's stem for the eye, eight hex digits
/// of the whole path so that two sockets sharing a stem — `--socket` names are
/// the user's to choose — never share a log. Pure, no environment: the spawner
/// opening stderr and the daemon opening its writer must agree on the name.
fn log_path(dir: &Path, socket: &Path) -> PathBuf {
    let stem = socket.file_stem().unwrap_or_default().to_string_lossy();
    let digest = Sha256::digest(socket.as_os_str().as_encoded_bytes());
    let suffix: String = digest[..4].iter().map(|b| format!("{b:02x}")).collect();
    dir.join(format!("{stem}-{suffix}.log"))
}

/// The most a daemon log may hold. Truncation, not rotation: one bound, one file
/// to read, and no generations to expire.
const LOG_CAP: u64 = 32 * 1024 * 1024;

/// The daemon's tracing writer: an owner-only append file, truncated in place
/// once a write would take it past [`LOG_CAP`], and every error swallowed.
///
/// The bound is enforced on the write path, not at spawn: the daemon that filled
/// a 3.2 GB tmpfs was one long-lived process, which a check at the next spawn
/// would never have reached. Errors are swallowed because a log that cannot be
/// written is a log lost, never a request failed: that same full disk made
/// `tracing-subscriber` report the failed write through `eprintln!`, whose own
/// failure panicked a request's blocking task.
///
/// The daemon's raw stderr — panic messages, which do not go through tracing —
/// is the same file, opened `O_APPEND` by the spawner. Both descriptors write at
/// the end of the file, so both follow a truncation; and the bound is measured
/// on the file rather than counted here, so what stderr wrote counts at the next
/// event. One `fstat` per event, which is cheap next to formatting it.
pub struct CappedLog {
    file: Option<File>,
}

/// Everything that can fail, so that `write` has one place to give up.
fn append_bounded(file: &mut File, buf: &[u8]) -> io::Result<()> {
    if file.metadata()?.len() + buf.len() as u64 > LOG_CAP {
        file.set_len(0)?;
    }
    file.write_all(buf)
}

impl Write for CappedLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // An event larger than the whole cap is cut, not skipped: its head is
        // worth more than nothing, and the bound holds either way.
        let cut = &buf[..buf.len().min(LOG_CAP as usize)];
        // A full or vanished disk: stop. The next spawn opens a new file.
        if let Some(file) = self.file.as_mut()
            && append_bounded(file, cut).is_err()
        {
            self.file = None;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The tracing writer for the daemon serving `socket`. `Mutex<W: Write>` is a
/// `MakeWriter` already. A directory or file that cannot be opened leaves the
/// daemon running without a log: the same trade as a failed write.
pub fn daemon_log_writer(socket: &Path) -> std::sync::Mutex<CappedLog> {
    let dir = log_dir();
    let file = ensure_private_dir(&dir)
        .and_then(|()| {
            open_owner_only(&log_path(&dir, socket), OpenOptions::new().create(true).append(true))
        })
        .ok();
    std::sync::Mutex::new(CappedLog { file })
}

/// Open `path` through `options` as an owner-only regular file.
///
/// `OpenOptions::mode` applies only when the open creates the file, so a lock or
/// a log left by an earlier build keeps the mode it was made with: the mode has
/// to be set as well as requested. Type check and chmod both go through the open
/// handle, never the path, so a file swapped in after the open is not the one
/// they act on. Neither failure is swallowed: a lock the client cannot secure
/// becomes an in-process session, and a daemon that cannot secure its own log
/// has no business starting.
fn open_owner_only(path: &Path, options: &mut OpenOptions) -> io::Result<File> {
    let file = options.mode(0o600).open(path)?;
    if !file.metadata()?.file_type().is_file() {
        return Err(io::Error::other(format!("{} is not a regular file", path.display())));
    }
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    Ok(file)
}

/// A file lock around "check / clear a stale socket / spawn / wait". Without it
/// two sessions starting at the same moment both find no socket and both spawn a
/// daemon — the duplicated memory this module exists to remove.
struct SpawnLock {
    file: File,
}

impl SpawnLock {
    fn acquire(path: &Path) -> io::Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true).truncate(false);
        let file = open_owner_only(path, &mut options)?;
        file.lock_exclusive()?;
        Ok(Self { file })
    }
}

impl Drop for SpawnLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.file);
    }
}

/// Connect to `socket`, retrying every [`SPAWN_POLL`] while `patience` lasts.
///
/// One connect error proves nothing: a full listen backlog reports
/// `ConnectionRefused` exactly like a socket nobody answers, and a daemon still
/// starting has not bound its path yet. `child`, when given, ends the wait as
/// soon as that process dies, so a failed startup costs a moment rather than the
/// whole timeout — `Err(None)`, already logged. `Err(Some(e))` is the last
/// connect error, whose kind the caller may act on.
async fn poll_connect(
    socket: &Path,
    patience: Duration,
    mut child: Option<&mut Child>,
) -> Result<UnixStream, Option<io::Error>> {
    let deadline = tokio::time::Instant::now() + patience;
    loop {
        let error = match UnixStream::connect(socket).await {
            Ok(stream) => return Ok(stream),
            Err(e) => e,
        };
        if let Some(child) = child.as_mut() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let log = log_path(&log_dir(), socket);
                    tracing::warn!("daemon exited before accepting ({status}); see {}", log.display());
                    return Err(None);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("cannot poll daemon process: {e}");
                    return Err(None);
                }
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(Some(error));
        }
        tokio::time::sleep(SPAWN_POLL).await;
    }
}

/// Client: serve this session through the shared daemon.
///
/// Three outcomes, and the caller must treat them differently:
///
/// - `Ok(true)` — the daemon handled the session and it ended normally.
/// - `Ok(false)` — nothing was exchanged; the caller may serve the session
///   in-process, exactly as it did before this module existed.
/// - `Err(_)` — bytes were exchanged and the session ended badly. The caller
///   must exit rather than serve the session in-process: stdin is already partly
///   consumed, so an in-process server would answer a truncated stream.
///
/// `idle` is this client's `--idle-secs`, passed on to a daemon this call starts.
pub async fn run_client(socket: &Path, idle: Option<Duration>) -> Result<bool, BoxError> {
    // The directory is checked before the first connect, not only on the spawn
    // path: a socket that answers proves a daemon is there, not that the
    // directory holding it is trustworthy, so a live socket in a symlinked or
    // foreign-owned directory used to skip the guard entirely. One
    // `symlink_metadata` per startup is nothing beside a connect.
    if let Some(parent) = socket.parent()
        && let Err(e) = ensure_private_dir(parent)
    {
        tracing::warn!("socket dir {} unusable: {e}", parent.display());
        return Ok(false);
    }

    if let Ok(stream) = UnixStream::connect(socket).await {
        tracing::info!("connected to shared daemon at {}", socket.display());
        return proxy(stream).await;
    }

    let lock = match SpawnLock::acquire(&lock_path(socket)) {
        Ok(lock) => lock,
        Err(e) => {
            tracing::warn!("spawn lock unavailable: {e}; serving in-process");
            return Ok(false);
        }
    };

    // Probe again under the lock: a daemon may have come up while we waited for
    // it. Only `NotFound` and `ConnectionRefused` prove that nothing is
    // listening; every other kind — `EACCES`, `EMFILE` — says this client cannot
    // reach the socket, which is no reason to touch it.
    let stream = match poll_connect(socket, SPAWN_POLL, None).await {
        Ok(stream) => Some(stream),
        Err(Some(e)) if !matches!(e.kind(), ErrorKind::NotFound | ErrorKind::ConnectionRefused) => {
            let path = socket.display();
            tracing::warn!("cannot reach {path}: {e}; leaving it in place, serving in-process");
            return Ok(false);
        }
        Err(_) => {
            // A socket nobody answers is the corpse of a daemon that died
            // without cleaning up: remove it, because binding over an existing
            // file fails with EADDRINUSE. A socket only — `--socket` can name
            // anything, and this must never delete someone else's file.
            match fs::symlink_metadata(socket) {
                Ok(meta) if meta.file_type().is_socket() => {
                    if let Err(e) = fs::remove_file(socket) {
                        let path = socket.display();
                        tracing::warn!("cannot remove stale {path}: {e}; serving in-process");
                        return Ok(false);
                    }
                }
                Ok(_) => {
                    let path = socket.display();
                    tracing::warn!("{path} exists and is not a socket; serving in-process");
                    return Ok(false);
                }
                // Nothing there: the ordinary first-start case.
                Err(_) => {}
            }
            match spawn_daemon(socket, idle, &log_dir()) {
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
        Some(stream) => proxy(stream).await,
        None => Ok(false),
    }
}

/// Start the daemon for `socket`, forwarding `--idle-secs` when given one. Its
/// stderr goes to the log under `logs`, on disk.
fn spawn_daemon(socket: &Path, idle: Option<Duration>, logs: &Path) -> io::Result<Child> {
    let exe = std::env::current_exe()?;
    ensure_private_dir(logs)?;
    let log_file = log_path(logs, socket);
    // Append, never truncate: the log of the daemon that just died is the only
    // record of why this spawn is happening. The daemon bounds the file itself;
    // see [`CappedLog`].
    let mut log = open_owner_only(&log_file, OpenOptions::new().create(true).append(true))?;
    // One header per spawn keeps consecutive lifetimes in one file apart. Unix
    // seconds, because this crate carries no date formatter.
    let pid = std::process::id();
    writeln!(log, "--- daemon spawned by pid {pid} at unix {} ---", now_secs())?;

    let mut cmd = Command::new(exe);
    cmd.arg("--daemon").arg("--socket").arg(socket);
    if let Some(idle) = idle {
        cmd.arg("--idle-secs").arg(idle.as_secs().to_string());
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        // To a file on disk: otherwise the diagnostics of a shared process die
        // with the session that spawned it.
        .stderr(Stdio::from(log))
        // Its own process group, so Ctrl-C in one client's session does not take
        // down a server other sessions are using.
        .process_group(0);
    cmd.spawn()
}

/// Wait for the daemon to bind, killing it if it never does.
async fn wait_for_daemon(socket: &Path, mut child: Child) -> Option<UnixStream> {
    match poll_connect(socket, SPAWN_WAIT, Some(&mut child)).await {
        Ok(stream) => return Some(stream),
        // A dead process is already logged; a timeout is not.
        Err(Some(e)) => tracing::warn!("daemon did not come up in {SPAWN_WAIT:?}: {e}"),
        Err(None) => {}
    }
    let _ = child.kill();
    None
}

/// Pump stdin/stdout ↔ socket for one session, as one of [`run_client`]'s three
/// outcomes.
///
/// Half-close, not cancel: at stdin EOF the write half is shut down, so the
/// daemon sees EOF, finishes the answers it still owes, and closes. Those replies
/// arrive between the half-close and the close, which is why the drain continues
/// afterwards; ending the session at stdin EOF dropped every reply still in
/// flight, and a piped one-shot session returned nothing at all.
///
/// The socket reaching EOF while stdin is still open is the opposite case: that
/// daemon died mid-session, a failure rather than a shutdown.
async fn proxy(stream: UnixStream) -> Result<bool, BoxError> {
    let (mut from_daemon, mut to_daemon) = stream.into_split();
    let mut stdout = tokio::io::stdout();
    // Whether stdin gave up a byte is the whole difference between the two
    // failure outcomes, and it must be readable while the copy still runs. A
    // flag, not a count: nothing needs the total.
    let consumed = AtomicBool::new(false);

    let mut upstream = pin!(async {
        let mut stdin = tokio::io::stdin();
        let mut head = [0u8; 8 * 1024];
        let n = stdin.read(&mut head).await?;
        if n > 0 {
            consumed.store(true, Ordering::Relaxed);
            to_daemon.write_all(&head[..n]).await?;
            copy(&mut stdin, &mut to_daemon).await?;
        }
        to_daemon.shutdown().await
    });
    let mut downstream = pin!(async {
        copy(&mut from_daemon, &mut stdout).await?;
        stdout.flush().await
    });

    // Bytes already read from stdin are gone from the pipe, so an in-process
    // server started afterwards would answer a truncated stream; a session that
    // never started can still be served locally.
    let ended = |reason: BoxError| -> Result<bool, BoxError> {
        if consumed.load(Ordering::Relaxed) {
            return Err(reason);
        }
        tracing::warn!("daemon session ended with nothing exchanged: {reason}; serving in-process");
        Ok(false)
    };

    tokio::select! {
        result = &mut upstream => {
            if let Err(e) = result {
                return ended(e.into());
            }
        }
        // Nobody is left to answer the requests already sent, and the host waits
        // for them: a clean exit here would report a crash as a shutdown.
        result = &mut downstream => {
            return ended(match result {
                Ok(()) => "daemon closed the connection while stdin was still open".into(),
                Err(e) => format!("daemon connection failed mid-session: {e}").into(),
            });
        }
    }

    match tokio::time::timeout(DRAIN_AFTER_EOF, &mut downstream).await {
        Ok(Ok(())) => Ok(true),
        Ok(Err(e)) => ended(e.into()),
        // Not `ended`: stdin has run to EOF and everything it held went to the
        // daemon, so no in-process server can serve this session even when not a
        // byte moved.
        Err(_elapsed) => Err(format!(
            "daemon held the connection open for longer than DRAIN_AFTER_EOF \
             ({DRAIN_AFTER_EOF:?}) after stdin closed; replies may have been lost"
        )
        .into()),
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
    // The working directory becomes the project root this daemon is keyed on, so
    // its identity and its idea of `"."` agree: every client of one daemon
    // resolves a relative path the same way. Inheriting the first client's cwd
    // made that answer depend on who happened to start the server, which no
    // caller can see or predict. One difference remains: a client whose own cwd
    // is a subdirectory of the project sees `"."` mean the project root through
    // the daemon and that subdirectory in-process.
    //
    // The socket is resolved against the old cwd first, since `--socket` may
    // name a relative path and the client resolved it against its own.
    let cwd = std::env::current_dir()?;
    let socket = &cwd.join(socket);
    let root = project_root(&cwd);
    std::env::set_current_dir(&root)?;
    tracing::info!("daemon working directory {}", root.display());

    if let Some(parent) = socket.parent() {
        ensure_private_dir(parent)?;
    }
    let path = socket.display();
    let listener = UnixListener::bind(socket)
        .map_err(|e| format!("cannot bind {path}: {e} (is a live daemon holding it?)"))?;
    // `bind` leaves the socket at `0777 & ~umask`, and Linux enforces the
    // permissions of a socket file on connect. This is the layer under the
    // `0700` directory, which an existing directory never gets.
    fs::set_permissions(socket, fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("cannot set {path} to 0600: {e}"))?;
    tracing::info!("daemon listening on {path} (idle {idle:?})");

    let live = Arc::new(AtomicUsize::new(0));
    let idle_since = Arc::new(AtomicI64::new(now_secs()));

    // Signals reach the same exit path as an idle timeout. A daemon killed
    // outright leaves its socket behind; clients survive that, but
    // `--print-socket` plus `ls` then point at an address where nobody listens.
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
                    // Before the count, never after: the loop must not see zero
                    // clients beside the timestamp of the first connect, or a
                    // long session ends in an immediate exit and the next client
                    // spawns the daemon again.
                    idle_since.store(now_secs(), Ordering::SeqCst);
                    live.fetch_sub(1, Ordering::SeqCst);
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
            tracing::info!("no clients for {idle:?}, shutting down");
            break;
        }
    }

    // So the next client does not find a file, get refused, and spend a round
    // trip clearing a stale socket.
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
    use rmc_server::mcp::BACKGROUND_SYNC_ENV;
    use std::os::unix::ffi::OsStringExt;

    /// The inputs of [`key_from_parts`], so a case can name the field it changes.
    struct Parts {
        project: PathBuf,
        data_dir: PathBuf,
        exe: PathBuf,
        len: u64,
        mtime: u128,
        env: Vec<(OsString, OsString)>,
    }

    impl Parts {
        fn base() -> Self {
            Self {
                project: PathBuf::from("/repo"),
                data_dir: PathBuf::from("/data"),
                exe: PathBuf::from("/bin/mcp"),
                len: 10,
                mtime: 20,
                env: pairs(&[(BACKGROUND_SYNC_ENV, "1")]),
            }
        }

        fn key(&self) -> String {
            let Self { len, mtime, .. } = *self;
            let Self { project, data_dir, exe, env, .. } = self;
            key_from_parts(project, data_dir, exe, len, mtime, env)
        }
    }

    /// Through the keyed policy, so a case about a variable exercises the filter
    /// as well as the hash.
    fn pairs(env: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        let owned = env.iter();
        keyed_env_pairs(owned.map(|(n, v)| (OsString::from(*n), OsString::from(*v))))
    }

    /// Everything the key covers must split daemons, and nothing else may: a
    /// configuration that changes what answers mean and still shares a socket is
    /// a server answering with someone else's settings.
    #[test]
    fn the_key_covers_every_field_and_nothing_else() {
        let base = Parts::base();
        let sync = |value| pairs(&[(BACKGROUND_SYNC_ENV, value)]);
        // Two values that do not decode are two cases, because the bytes decide.
        let raw = |value: &[u8]| {
            let name = OsString::from("LD_LIBRARY_PATH");
            let value = OsString::from_vec(value.to_vec());
            let env = keyed_env_pairs(std::iter::once((name, value)));
            Parts { env, ..Parts::base() }
        };
        let changes: [(&str, Parts); 8] = [
            ("the project root", Parts { project: "/other".into(), ..Parts::base() }),
            ("the index root", Parts { data_dir: "/data/two".into(), ..Parts::base() }),
            ("the binary size", Parts { len: 11, ..Parts::base() }),
            ("the binary mtime", Parts { mtime: 21, ..Parts::base() }),
            ("a keyed value", Parts { env: sync("0"), ..Parts::base() }),
            ("a second keyed name", Parts {
                env: pairs(&[(BACKGROUND_SYNC_ENV, "1"), ("RMC_EMBEDDING_PROFILE", "fast")]),
                ..Parts::base()
            }),
            ("a value that does not decode", raw(b"\xff\xfe")),
            ("another value that does not decode", raw(b"\xfe\xff")),
        ];
        let mut keys: Vec<(&str, String)> = vec![("the base configuration", base.key())];
        for (what, parts) in &changes {
            keys.push((what, parts.key()));
        }
        for (index, (one, one_key)) in keys.iter().enumerate() {
            for (other, other_key) in &keys[index + 1..] {
                assert_ne!(one_key, other_key, "{one} and {other} must not share a key");
            }
        }

        let knobs: Vec<(&str, &str)> = UNKEYED_ENV.iter().map(|name| (*name, "5")).collect();
        let mut with_knobs = sync("1");
        with_knobs.extend(pairs(&knobs));
        let invariants: [(&str, Parts); 2] = [
            ("the same inputs twice", Parts::base()),
            ("an unkeyed transport knob", Parts { env: with_knobs, ..Parts::base() }),
        ];
        for (what, parts) in &invariants {
            assert_eq!(parts.key(), base.key(), "{what} must not change the daemon key");
        }
    }

    /// A name that does not decode can still carry a keyed prefix, so the policy
    /// reads bytes; the sort is by those bytes, so two clients configured alike
    /// agree on the key whatever order their environment arrives in.
    #[test]
    fn keyed_env_pairs_filters_and_sorts_by_raw_bytes() {
        let env = [
            OsString::from("RMC_B"),
            OsString::from_vec(b"RMC_\xff".to_vec()),
            OsString::from("RMC_A"),
            OsString::from(DAEMON_DIR_ENV),
            OsString::from("HOME"),
            OsString::from("OPENROUTER_API_KEY"),
        ];
        let kept = keyed_env_pairs(env.iter().map(|name| (name.clone(), OsString::from("v"))));
        let names: Vec<&[u8]> = kept.iter().map(|(name, _)| name.as_bytes()).collect();
        let expected: Vec<&[u8]> = vec![b"OPENROUTER_API_KEY", b"RMC_A", b"RMC_B", b"RMC_\xff"];
        assert_eq!(names, expected, "keyed names must come back sorted by raw bytes");
    }

    fn mode_of(args: &[&str]) -> Mode {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        resolve_mode(&owned).expect("mode")
    }

    /// The command line decides the mode before the expensive startup, so a
    /// misread flag costs a whole session. `--idle-secs` must survive the hop
    /// through a client, since the daemon it spawns is what honours it; without
    /// the flag a client forwards nothing, so that daemon applies its own default.
    #[test]
    fn resolve_mode_maps_arguments_to_modes() {
        let sock = || PathBuf::from("/tmp/x.sock");
        let accepted: [(&str, &[&str], Mode); 7] = [
            ("--daemon with a socket and an idle timeout",
             &["--daemon", "--socket", "/tmp/x.sock", "--idle-secs", "5"],
             Mode::Daemon { socket: sock(), idle: Duration::from_secs(5) }),
            ("--in-process is an explicit opt out",
             &["--in-process", "--socket", "/tmp/x.sock"], Mode::InProcess),
            ("--print-socket resolves the socket and nothing else",
             &["--print-socket", "--socket", "/tmp/x.sock"], Mode::PrintSocket { socket: sock() }),
            ("--help", &["--help"], Mode::Help),
            ("-h", &["-h"], Mode::Help),
            ("no mode is a client, carrying the idle flag it was given",
             &["--socket", "/tmp/x.sock", "--idle-secs", "7"],
             Mode::Client { socket: sock(), idle: Some(Duration::from_secs(7)) }),
            ("a client with no idle flag carries none",
             &["--socket", "/tmp/x.sock"], Mode::Client { socket: sock(), idle: None }),
        ];
        for (what, args, expected) in accepted {
            assert_eq!(mode_of(args), expected, "{what}");
        }

        let rejected: [(&str, &[&str]); 5] = [
            ("two modes at once", &["--daemon", "--in-process"]),
            ("an unknown argument", &["--socks"]),
            ("--socket without a path", &["--socket"]),
            ("--idle-secs without a number", &["--idle-secs"]),
            ("--idle-secs with something that is not one", &["--idle-secs", "soon"]),
        ];
        for (what, args) in rejected {
            let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            assert!(resolve_mode(&owned).is_err(), "{what} must be rejected");
        }
    }

    /// One repository, one daemon: the nearest workspace root wins over a member
    /// manifest below it and over a stray workspace manifest above it, and the
    /// walk never leaves the repository it started in.
    #[test]
    fn project_root_is_the_nearest_workspace_root_inside_the_repository() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let outer = fs::canonicalize(tmp.path()).expect("canonical temp dir");
        // Directory, its manifest, and its repository marker if it is a boundary.
        // The manifest above every checkout must swallow none of them.
        let tree: [(&str, &str, Option<&str>); 5] = [
            ("", "[workspace]\nmembers = [\"repo\", \"plain\", \"ws\"]\n", None),
            ("repo", "[workspace]\nmembers = [\"crates/a\"]\n", Some(".git")),
            ("repo/crates/a", "[package]\nname = \"a\"\n", None),
            ("plain", "[package]\nname = \"plain\"\n", Some(".git")),
            ("ws", "[workspace]\nmembers = []\n", Some(".jj")),
        ];
        for (dir, manifest, marker) in tree {
            let dir = outer.join(dir);
            fs::create_dir_all(dir.join("src")).expect("temp tree");
            fs::write(dir.join("Cargo.toml"), manifest).expect("manifest");
            if let Some(marker) = marker {
                fs::create_dir_all(dir.join(marker)).expect("repository marker");
            }
        }

        // `plain` declares no workspace: the answer is its own manifest, never
        // the one above the boundary. `ws` is a boundary and is still inspected,
        // so its own `[workspace]` wins.
        for (start, expected) in [
            ("repo", "repo"),
            ("repo/crates/a", "repo"),
            ("repo/crates/a/src", "repo"),
            ("plain/src", "plain"),
            ("ws/src", "ws"),
        ] {
            assert_eq!(
                project_root(&outer.join(start)),
                outer.join(expected),
                "{start} must resolve to {expected}"
            );
        }
    }

    fn permission_bits(path: &Path) -> u32 {
        fs::metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    /// The lock and the log are owner-only even when they already exist:
    /// `OpenOptions::mode` applies only to a file the open creates, so a lock left
    /// by an earlier build kept whatever mode it was made with. They usually sit
    /// in a `0700` directory, but `--socket` can put them anywhere, and the log
    /// carries the paths of the code being analysed. Anything that is not a
    /// regular file is refused instead: locking and chmodding a directory or a
    /// device says nothing about who reads it.
    #[test]
    fn the_lock_and_the_log_are_owner_only() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let socket = tmp.path().join("probe.sock");
        let lock_file = lock_path(&socket);
        fs::write(&lock_file, b"").expect("pre-existing lock");
        fs::set_permissions(&lock_file, fs::Permissions::from_mode(0o644)).expect("chmod");

        let lock = SpawnLock::acquire(&lock_file).expect("lock");
        assert_eq!(
            permission_bits(&lock_file),
            0o600,
            "a lock file left by an earlier build must be tightened, not trusted"
        );
        drop(lock);

        assert!(
            SpawnLock::acquire(tmp.path()).is_err(),
            "a directory is not a regular file and must be refused"
        );

        // The log and its header are written before the child starts, so the
        // child is of no interest beyond being reaped. It is this test binary,
        // which rejects `--daemon` and exits at once.
        let logs = tempfile::TempDir::new().expect("log dir");
        let mut child = spawn_daemon(&socket, None, logs.path()).expect("spawn");
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            permission_bits(&log_path(logs.path(), &socket)),
            0o600,
            "the daemon log is owner-only"
        );
        let in_socket_dir: Vec<_> = fs::read_dir(tmp.path())
            .expect("socket dir")
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "log"))
            .collect();
        assert!(
            in_socket_dir.is_empty(),
            "the socket directory holds sockets and locks, never a log: {in_socket_dir:?}"
        );
    }

    /// The bound holds on the write path, while the daemon runs: 100 MiB written
    /// never leaves more than `LOG_CAP` on disk, and the file keeps being written
    /// after each truncation.
    #[test]
    fn the_capped_log_never_grows_past_the_cap() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let path = tmp.path().join("probe.log");
        let file =
            open_owner_only(&path, OpenOptions::new().create(true).append(true)).expect("open");
        let mut log = CappedLog { file: Some(file) };
        let chunk = [b'x'; 4096];
        let mut largest = 0;
        for _ in 0..(100 * 1024 * 1024 / chunk.len()) {
            assert_eq!(log.write(&chunk).expect("a write never fails"), chunk.len());
            largest = largest.max(fs::metadata(&path).expect("metadata").len());
        }
        assert!(largest <= LOG_CAP, "the log reached {largest} bytes; the cap is {LOG_CAP}");
        let len = fs::metadata(&path).expect("metadata").len();
        assert!(len > 0, "the log must still be written after a truncation");
    }

    /// One event larger than the cap is cut to it, and bytes another descriptor
    /// appends — the daemon's raw stderr — count towards the bound at the next
    /// event, because the bound is read from the file rather than counted.
    #[test]
    fn the_cap_holds_for_one_huge_write_and_for_the_raw_stderr() {
        let tmp = tempfile::TempDir::new().expect("temp dir");
        let path = tmp.path().join("probe.log");
        let open = || {
            open_owner_only(&path, OpenOptions::new().create(true).append(true)).expect("open")
        };
        let len = || fs::metadata(&path).expect("metadata").len();
        let mut log = CappedLog { file: Some(open()) };

        let huge = vec![b'x'; LOG_CAP as usize + 1];
        assert_eq!(log.write(&huge).expect("never an error"), huge.len());
        assert_eq!(len(), LOG_CAP, "an event past the cap is cut to it");

        let mut stderr = open();
        stderr.write_all(&[b'e'; 1000]).expect("the raw stderr appends");
        assert_eq!(log.write(b"tracing").expect("never an error"), 7);
        assert_eq!(len(), 7, "the next event sees the file past the cap and truncates");
    }

    /// `--socket` names are the user's to choose, so two sockets may share a
    /// stem — in different directories, or in one directory under different
    /// extensions — and must not share a log. One socket always maps to one
    /// name, or the spawner's stderr and the daemon's writer would land in
    /// different files.
    #[test]
    fn log_names_do_not_collide() {
        let logs = Path::new("/logs");
        let sockets = ["/a/foo.sock", "/b/foo.sock", "/a/foo.socket", "/a/foo"];
        let names: Vec<PathBuf> =
            sockets.iter().map(|socket| log_path(logs, Path::new(socket))).collect();
        for (i, name) in names.iter().enumerate() {
            assert_eq!(name, &log_path(logs, Path::new(sockets[i])), "one socket, one name");
            let file = name.file_name().expect("name").to_string_lossy();
            assert!(
                file.starts_with("foo-") && file.ends_with(".log"),
                "stem kept, suffix added: {file}"
            );
            for other in &names[i + 1..] {
                assert_ne!(name, other, "same stem, different sockets, different logs");
            }
        }
    }

    /// A write that fails is dropped, not reported: the disk that filled was the
    /// log's own, and reporting that through stderr panicked a request.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_capped_log_swallows_write_errors() {
        let full = OpenOptions::new().write(true).open("/dev/full").expect("/dev/full");
        let mut log = CappedLog { file: Some(full) };
        for _ in 0..3 {
            assert_eq!(log.write(b"dropped").expect("never an error"), 7);
            assert!(log.flush().is_ok());
        }
        assert!(log.file.is_none(), "after the first failure the file is closed");
    }

    /// Created private, and otherwise left exactly as it is: the directory may be
    /// one the user named through `--socket`, and a symlink there decides where
    /// the socket really lands.
    #[test]
    fn socket_dir_is_created_private_and_otherwise_untouched() {
        let tmp = tempfile::TempDir::new().expect("temp dir");

        let created = tmp.path().join("nested").join("sockets");
        ensure_private_dir(&created).expect("create");
        assert_eq!(permission_bits(&created), 0o700, "a created directory is owner-only");

        let existing = tmp.path().join("existing");
        fs::create_dir(&existing).expect("create dir");
        fs::set_permissions(&existing, fs::Permissions::from_mode(0o755)).expect("chmod");
        ensure_private_dir(&existing).expect("a directory this user owns is accepted");
        assert_eq!(
            permission_bits(&existing),
            0o755,
            "an existing directory must keep the mode it had"
        );

        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&existing, &link).expect("symlink");
        let error = ensure_private_dir(&link).expect_err("a symlink must be refused");
        assert!(
            error.to_string().contains(&link.display().to_string()),
            "the refusal must name the path: {error}"
        );
    }
}

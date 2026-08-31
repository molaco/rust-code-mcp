//! One server per user: a unix-socket daemon plus a thin proxy client.
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
//! # The socket key is the configuration, not the directory
//!
//! It covers the binary's size and mtime and the env vars that change what the
//! server computes. Otherwise a rebuilt binary — or a different configuration —
//! would silently attach to a daemon that answers differently, and that reads as
//! "the server is lying", not as "we connected to the wrong one".
//!
//! It deliberately does *not* cover the working directory. It used to, and the
//! result was one daemon per directory a session happened to start in: measured
//! on this machine, 11 processes holding 12.5 GB between them, several of them
//! analysing the same repository. The cwd never selected the project in the
//! first place — every tool takes its `directory` as a parameter — so keying on
//! it bought no isolation, only duplication.
//!
//! The consequence is that the daemon does not share a working directory with
//! the session asking, and cannot resolve a relative path on its behalf. Path
//! parameters are therefore required to be absolute
//! (`rmc_server::tools::paths::require_absolute`), and the daemon is spawned in
//! `/` so that anything slipping past that gate fails loudly instead of quietly
//! answering about the wrong tree.
//!
//! # A failing daemon never leaves a client without a server
//!
//! Any failure along connect / spawn / wait returns `Ok(false)` from
//! [`run_client`], and the caller serves the session in-process exactly as it did
//! before this module existed. The daemon is a memory optimisation, not a new
//! point of failure.

use fs2::FileExt;
use rmc_server::mcp::{
    BACKGROUND_SYNC_ENV, EMBEDDING_PROFILE_ENV, RuntimeClearRequest, RuntimeClearScope,
    RuntimeState, ServerRuntime, mem_available_kib, rss_kib,
};
use rmc_server::tools::SearchTool;
use rmcp::ServiceExt;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
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
/// RSS above which the daemon unloads its rust-analyzer contexts, in MB.
/// `0` disables the check.
pub const RSS_SOFT_ENV: &str = "RMC_RSS_SOFT_MB";
/// RSS above which unloading is judged hopeless and the daemon retires itself,
/// in MB. `0` disables the check.
pub const RSS_HARD_ENV: &str = "RMC_RSS_HARD_MB";
/// Minimum gap between two unloads, in seconds — see [`WatchdogAction`].
pub const RSS_COOLDOWN_ENV: &str = "RMC_RSS_COOLDOWN_SECS";
/// Machine-wide `MemAvailable` floor, in MB: below it the daemon unloads its
/// contexts even when its own RSS is fine. `0` disables the check.
pub const MIN_AVAILABLE_ENV: &str = "RMC_MIN_AVAILABLE_MB";
/// How long a retired daemon waits for its clients to leave before exiting
/// anyway, in seconds. `0` means wait forever — see [`retire_grace_expired`].
pub const RETIRE_GRACE_ENV: &str = "RMC_RETIRE_GRACE_SECS";
/// How often the daemon collects garbage in its loaded analyses, in seconds.
/// `0` disables it — see [`should_collect_garbage`].
pub const GC_INTERVAL_ENV: &str = "RMC_GC_INTERVAL_SECS";

/// Env vars that change what the server computes, and therefore which daemon a
/// client belongs to. Extend this list whenever a new behaviour-changing knob is
/// added, or clients configured differently will end up sharing one server.
const KEYED_ENV: [&str; 2] = [EMBEDDING_PROFILE_ENV, BACKGROUND_SYNC_ENV];

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
  --print-socket      print this configuration's socket path and exit
  --socket <PATH>     use this socket instead of the one derived from the
                      binary and the environment
  --idle-secs <N>     daemon exits after N seconds with no clients (0 = never)

Env: RMC_DAEMON=0 forces in-process; RMC_DAEMON_DIR sets the socket directory;
     RMC_DAEMON_IDLE_SECS is the same as --idle-secs.

One daemon serves EVERY working directory: the socket is keyed by this binary
and the environment above, not by the directory a session starts in. Path
parameters must therefore be absolute — the daemon shares no working directory
with the session asking, and will refuse a relative path rather than resolve it
against a directory nobody chose.

Memory watchdog (daemon only; 0 disables a threshold):
  RMC_RSS_SOFT_MB=12288      unload the analysis contexts above this RSS
  RMC_RSS_HARD_MB=20480      above this, retire: stop taking new clients, let
                             the current ones finish, exit — a successor is
                             started on demand, so no session is cut off
  RMC_RSS_COOLDOWN_SECS=300  minimum gap between two unloads
  RMC_MIN_AVAILABLE_MB=6144  unload when the MACHINE has less than this free,
                             whatever this daemon's own RSS is — the only
                             reading that sees pressure it did not cause
  RMC_RETIRE_GRACE_SECS=1800 a retired daemon exits after this even with
                             clients still attached (those connections drop)
  RMC_GC_INTERVAL_SECS=300   how often loaded analyses are garbage-collected;
                             this is also what makes salsa's LRU capacities
                             evict anything at all
  RMC_MAX_PROJECTS=3         how many analysis contexts stay loaded; past the
                             cap the least recently used one is dropped
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

/// What the memory watchdog decided to do this tick.
///
/// # Why a daemon needs one at all
///
/// Before the daemon, a leaking analysis was bounded by the session: the
/// process died with its client. The daemon deliberately outlives clients, so
/// nothing bounds it any more — `SemanticService` holds its contexts in a
/// `HashMap` with no TTL and no eviction, and a workspace stays loaded until
/// someone calls `clear_runtime` by hand. Which is to say: until an operator
/// notices, which is exactly what "it just grows" means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchdogAction {
    /// Below the thresholds, or too soon after the last unload.
    None,
    /// Drop the loaded rust-analyzer contexts and return what the allocator
    /// will give back. Costs the next semantic query a reload (~1.5 s for a
    /// `Fast` load of a 4000-file workspace), which is why it is rate-limited.
    Unload,
    /// Unloading cannot fix this one: retire the daemon.
    ///
    /// Retiring is *not* killing the connections. The socket file is unlinked,
    /// so new clients no longer find this daemon and start a fresh one, while
    /// the clients already attached finish normally and the process exits when
    /// the last of them leaves. A daemon that instead exited outright would
    /// hand every attached session the `Connection closed` error that this
    /// whole mechanism exists to prevent — a self-inflicted version of the
    /// failure we are trying to avoid.
    ///
    /// The waiting is bounded, and it has to be — see [`retire_grace_expired`].
    Retire,
}

/// Thirty minutes, deliberately the same number as [`DEFAULT_IDLE_SECS`].
///
/// The policy it states is easy to hold in the head: *no daemon outlives the
/// idle timeout once it has been retired*, whatever its clients are doing. An
/// idle daemon already exits after half an hour of silence; a retired one is
/// strictly worse than idle — it is over the hard memory limit, its socket is
/// gone, and every new client goes to a successor — so it has no business
/// living longer than that.
const DEFAULT_RETIRE_GRACE_SECS: u64 = 1800;

/// Has a retired daemon waited long enough that it should exit even though
/// clients are still attached?
///
/// # Why the wait needs an end at all
///
/// [`WatchdogAction::Retire`] hands the socket to a successor and then waits for
/// `live == 0`. That wait is unbounded, and its length is decided by something
/// the daemon does not control: how long a client session lasts. A Claude Code
/// session runs for hours, so the mechanism meant to *free* memory produced its
/// opposite — measured 2026-08-27, a retired daemon sat on **24.2 GB** next to a
/// live successor, and the two together were what exhausted this machine's swap.
///
/// # The price, stated plainly
///
/// Exiting on the deadline breaks the connections still attached. The proxy is
/// byte-level and remembers no handshake, so a client whose daemon vanishes does
/// not silently reattach to the successor — that session loses its MCP tools
/// until it restarts them. That is the trade this knob makes: one session's
/// tools against gigabytes held away from the whole machine. It only ever
/// applies past the hard RSS limit, which is why the deadline can be generous.
///
/// `grace` of zero disables the deadline and restores the original
/// wait-forever behaviour.
pub fn retire_grace_expired(retiring_for: Option<Duration>, grace: Duration) -> bool {
    if grace.is_zero() {
        return false;
    }
    retiring_for.is_some_and(|elapsed| elapsed >= grace)
}

/// Thresholds for [`decide_watchdog_action`], in MB. `0` disables a threshold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogLimits {
    pub soft_mb: u64,
    pub hard_mb: u64,
    /// Machine-wide floor on `MemAvailable`. Below it the daemon unloads even
    /// though its own RSS is fine — see [`DEFAULT_MIN_AVAILABLE_MB`].
    pub min_available_mb: u64,
    pub cooldown: Duration,
}

/// Eight gigabytes, and the number comes from a measurement rather than a guess.
///
/// A freshly started daemon that has done *nothing* — no project loaded, no
/// query answered — already sits at **2.3 GB**: the ONNX runtime, the embedding
/// model and the GPU probe are paid for at startup. One `Fast`-loaded workspace
/// of ~4000 files adds ~3 GB, which is why a healthy daemon serving one such
/// workspace measures ~5.3 GB.
///
/// So the ordinary working point is above 5 GB, and the first draft of this
/// constant — 4 GB, picked from the workspace cost alone and forgetting the
/// fixed 2.3 GB floor — sat *below* normal. It would have unloaded
/// rust-analyzer every cooldown on a perfectly healthy daemon: a memory guard
/// that does nothing but make every query reload the workspace.
///
/// It was 8192 while each daemon served one working directory. One daemon now
/// serves all of them, so the same arithmetic is run for the three projects
/// `RMC_MAX_PROJECTS` allows: 2.3 GB fixed + 3 × ~3 GB ≈ 11.3 GB. Note what the
/// change is *not*: one daemon at 12 GB is less than the 12.5 GB that eleven of
/// them held between them on the machine that prompted this, and unlike that
/// fleet it is a number something actually enforces.
const DEFAULT_RSS_SOFT_MB: u64 = 12288;
/// Twenty gigabytes: past this, unloading has already been tried and the memory
/// is stuck in the allocator, so only a fresh process gets it back. Kept at a
/// ratio to the soft limit rather than at a fixed distance — the escalation
/// wants room for a peak above the working point, not a constant.
const DEFAULT_RSS_HARD_MB: u64 = 20480;
/// Five minutes between unloads. Without a floor the watchdog would unload on
/// every tick — RSS usually does *not* drop after an unload (see
/// `rmc_server::mcp::memory`), so the trigger stays true and the next tick would
/// fire again, reloading the workspace on every query and turning a memory
/// guard into a performance disaster.
const DEFAULT_RSS_COOLDOWN_SECS: u64 = 300;

/// Six gigabytes of `MemAvailable`, and the number is set by what else is on the
/// machine rather than by what the daemon costs.
///
/// The other three thresholds ask "is this process too big?", which a daemon can
/// answer while the machine around it goes to swap — its own RSS is nobody's
/// idea of the whole picture. This one asks "does the machine still have room?",
/// and it is the only reading that sees pressure the daemon did not cause: a
/// workspace build, a second analyser, the application under development.
///
/// Six is chosen to fire *before* the OOM machinery does. A typical `earlyoom`
/// on a 61 GB desktop warns near 5 GB and starts killing near 2.4 GB; a floor
/// below that would only ever unload after something had already been killed,
/// which is not a guard. Higher than ~8 GB and an ordinary `cargo` build would
/// evict the analysis every time, making every following query reload it.
///
/// `0` disables the check — including on any platform where `MemAvailable`
/// cannot be read, since an unknown reading must not act like a violated floor.
const DEFAULT_MIN_AVAILABLE_MB: u64 = 6144;

impl WatchdogLimits {
    pub fn from_env() -> Self {
        Self {
            soft_mb: env_u64(RSS_SOFT_ENV, DEFAULT_RSS_SOFT_MB),
            hard_mb: env_u64(RSS_HARD_ENV, DEFAULT_RSS_HARD_MB),
            min_available_mb: env_u64(MIN_AVAILABLE_ENV, DEFAULT_MIN_AVAILABLE_MB),
            cooldown: Duration::from_secs(env_u64(RSS_COOLDOWN_ENV, DEFAULT_RSS_COOLDOWN_SECS)),
        }
    }
}

/// Five minutes — deliberately the same number as [`DEFAULT_RSS_COOLDOWN_SECS`],
/// because it buys the same thing at the same price.
///
/// A collection makes the next query in each project slower (its memos have to
/// be re-validated against a new revision) in exchange for memory. The cooldown
/// is already this daemon's statement of how often it is willing to make that
/// trade; a garbage collection is a far cheaper version of it than an unload, so
/// there is no case for pacing it more eagerly than the expensive one.
const DEFAULT_GC_INTERVAL_SECS: u64 = 300;

/// Is it time to collect garbage in the loaded analyses?
///
/// # Why the daemon needs a timer for this at all
///
/// salsa evicts an LRU-capped query's memos only while bumping the revision, and
/// a revision bumps when a file changes. rust-analyzer inside an editor gets
/// those from typing; a project loaded into this daemon and then left alone gets
/// none, so its LRU capacity — whatever it is set to — never evicts anything.
/// The measured 14.6 GB daemon was mostly exactly that: contexts for directories
/// no one had touched for hours. This timer is where the bumps come from
/// instead, which is what makes the capacities in the rust-analyzer fork mean
/// something here.
///
/// `interval` of zero disables collection.
///
/// The elapsed time is measured from daemon startup, so the first collection
/// happens one interval in rather than on the first tick — before that there is
/// usually nothing loaded to collect.
pub fn should_collect_garbage(since_last_gc: Duration, interval: Duration) -> bool {
    if interval.is_zero() {
        return false;
    }
    since_last_gc >= interval
}

/// The retire deadline, read from the environment.
///
/// Kept out of [`WatchdogLimits`] on purpose: those three numbers are the inputs
/// of [`decide_watchdog_action`], and this one is not — it governs what happens
/// *after* that decision has already been made.
fn retire_grace_from_env() -> Duration {
    Duration::from_secs(env_u64(RETIRE_GRACE_ENV, DEFAULT_RETIRE_GRACE_SECS))
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

/// Pure decision, so the policy can be judged by a test instead of by watching a
/// daemon for an afternoon.
///
/// `since_unload` is `None` when nothing has been unloaded yet — the cooldown
/// cannot bar the first attempt.
///
/// `available_mb` is `None` when the machine's free memory could not be read.
/// That is treated as "the floor is not violated": an unreadable number must not
/// act like an alarming one, or every non-Linux daemon would unload forever.
pub fn decide_watchdog_action(
    rss_mb: u64,
    available_mb: Option<u64>,
    limits: WatchdogLimits,
    since_unload: Option<Duration>,
    retiring: bool,
) -> WatchdogAction {
    // Hard first: once memory is this high the unload has demonstrably not
    // helped, and checking soft first would spend another cooldown finding out.
    //
    // A daemon that already retired may not retire again: the socket it would
    // unlink by then belongs to its successor. Only *this* branch is barred to
    // it, though — everything below is not only still allowed but is where it
    // pays best. A retired daemon is past the hard limit by definition, serves
    // no new clients, and holds its contexts purely for the sessions still
    // draining; unloading touches no socket at all. Barring the whole function
    // instead was the 2026-08-29 defect: RSS 21.6 GB sat untouched for the full
    // `RMC_RETIRE_GRACE_SECS` next to a successor loading its own analysis, and
    // between them they took this machine's 16 GB of swap to zero. Garbage
    // collection kept running throughout and kept reporting success — it
    // reclaims salsa memos, not the database — so the logs read "collected
    // garbage" every five minutes while nothing came back.
    if !retiring && limits.hard_mb > 0 && rss_mb >= limits.hard_mb {
        return WatchdogAction::Retire;
    }

    let over_own_limit = limits.soft_mb > 0 && rss_mb >= limits.soft_mb;
    // The machine's floor gets its own reason to unload rather than being folded
    // into the soft limit: the pressure it reports is usually not ours, and a
    // daemon at 3 GB next to a build that took the machine to swap is exactly
    // the case the RSS thresholds cannot see.
    //
    // It stops at `Unload`, never `Retire`. Retiring unlinks the socket and puts
    // a deadline on live sessions to answer memory pressure the daemon may not
    // even have caused, and the pressure usually passes with the build.
    let machine_is_short = limits.min_available_mb > 0
        && available_mb.is_some_and(|available| available < limits.min_available_mb);

    if !over_own_limit && !machine_is_short {
        return WatchdogAction::None;
    }

    match since_unload {
        Some(elapsed) if elapsed < limits.cooldown => WatchdogAction::None,
        _ => WatchdogAction::Unload,
    }
}

/// Where sockets live. `$XDG_RUNTIME_DIR` is preferred: it is private, on tmpfs,
/// and cleaned out at logout together with any orphaned sockets.
fn socket_dir() -> Result<PathBuf, BoxError> {
    let probed = probe_runtime_dir();
    Ok(resolve_socket_dir(
        std::env::var(DAEMON_DIR_ENV).ok().as_deref(),
        std::env::var("XDG_RUNTIME_DIR").ok().as_deref(),
        probed.as_deref(),
        std::env::var("USER").ok().as_deref(),
        &std::env::temp_dir(),
    ))
}

/// `/run/user/<uid>`, if it exists — the directory `XDG_RUNTIME_DIR` would name.
///
/// Read from the filesystem rather than assumed: the uid comes from
/// `/proc/self`, and the directory has to be there. A session started outside a
/// login shell (a hook, a cron job, an editor spawned from a display manager)
/// often has no `XDG_RUNTIME_DIR` at all, and without this it would compute the
/// same key as its neighbours but look for it in `/tmp` — not find the running
/// daemon, and start a second one. Both halves of that split were live on this
/// machine at once, on two keys.
fn probe_runtime_dir() -> Option<PathBuf> {
    let uid = fs::metadata("/proc/self").ok()?.uid();
    let dir = PathBuf::from(format!("/run/user/{uid}"));
    dir.is_dir().then_some(dir)
}

/// The pure part of [`socket_dir`], for the same reason as [`key_from_parts`]:
/// deciding this from tests otherwise means mutating global env in parallel
/// with other tests.
fn resolve_socket_dir(
    env_dir: Option<&str>,
    xdg: Option<&str>,
    probed_runtime: Option<&Path>,
    user: Option<&str>,
    temp_dir: &Path,
) -> PathBuf {
    if let Some(dir) = env_dir.filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    if let Some(runtime_dir) = xdg.filter(|d| !d.is_empty()) {
        return PathBuf::from(runtime_dir).join("rust-code-mcp");
    }
    if let Some(runtime_dir) = probed_runtime {
        return runtime_dir.join("rust-code-mcp");
    }
    let user = user.filter(|u| !u.is_empty()).unwrap_or("shared");
    temp_dir.join(format!("rust-code-mcp-{user}"))
}

fn ensure_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    // The socket is an entry point into analysing someone's code: owner only.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

/// The daemon key: everything that changes what an answer means.
///
/// The binary contributes its size and mtime rather than a content hash: a
/// rebuild must produce a *new* daemon (otherwise a client built from new code
/// would be served by the old server), and reading tens of megabytes on every
/// startup to establish that is not worth it.
///
/// The working directory is deliberately absent — see the module docs. Adding
/// anything here splits the fleet, so a new part earns its place only by
/// changing what the server *computes*.
fn daemon_key() -> Result<String, BoxError> {
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

    Ok(key_from_parts(&exe, exe_len, exe_mtime, &env))
}

/// The pure part of the key: everything that matters arrives as an argument.
///
/// Split out of [`workspace_key`] for testability rather than tidiness: checking
/// "the key changes with configuration" through `set_var` means mutating global
/// env in parallel with other tests, which fails for reasons unrelated to keys.
fn key_from_parts(exe: &Path, exe_len: u64, exe_mtime: u128, env: &[(&str, String)]) -> String {
    let mut hasher = Sha256::new();
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
    Ok(socket_dir()?.join(format!("{}.sock", daemon_key()?)))
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

/// Size at which a daemon log is rolled aside on the next spawn. Generous on
/// purpose: an indexing run writes hundreds of kilobytes in minutes, and a log
/// that rolls mid-investigation is barely better than one that is truncated.
const LOG_ROTATE_BYTES: u64 = 32 * 1024 * 1024;

/// Roll the current log aside if it has grown past [`LOG_ROTATE_BYTES`].
///
/// Exactly one generation is kept (`<key>.log.1`). Failures are deliberately
/// ignored: losing the previous log is not a reason to fail to start a daemon.
fn rotate_log_if_large(path: &Path) {
    let too_big = fs::metadata(path).is_ok_and(|meta| meta.len() >= LOG_ROTATE_BYTES);
    if too_big {
        let _ = fs::rename(path, path.with_extension("log.1"));
    }
}

/// Open the log a spawned daemon writes its stderr into.
///
/// Append, never truncate. A successor is spawned precisely when its
/// predecessor is in trouble — over the hard memory limit, or dead — and
/// truncating here destroyed that predecessor's log at the exact moment someone
/// was about to read it. Measured 2026-08-27: a retired daemon holding 24.2 GB
/// whose entire history had been overwritten by the successor that replaced it,
/// so *why* it grew could not be reconstructed at all.
///
/// Daemons are told apart inside the shared file by the pid on their
/// `daemon listening on …` line.
fn open_daemon_log(socket: &Path) -> io::Result<File> {
    let path = log_path(socket);
    rotate_log_if_large(&path);
    OpenOptions::new().create(true).append(true).open(path)
}

fn spawn_daemon(socket: &Path) -> io::Result<Child> {
    let exe = std::env::current_exe()?;
    let log = open_daemon_log(socket)?;

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
        .process_group(0)
        // The root, not the spawning session's directory. One daemon serves
        // every working directory, so inheriting one client's cwd would make it
        // the accidental base for any relative path that slipped past the
        // absolute-path gate — answering about the wrong tree instead of
        // failing. `/` holds no Cargo project, so such a path fails loudly.
        .current_dir("/");
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
    // The pid is what makes a shared, appended log readable: successive daemons
    // write into the same file, and this line is the boundary between them.
    tracing::info!(
        "daemon listening on {} (pid {}, idle timeout {:?})",
        socket.display(),
        std::process::id(),
        idle
    );

    let live = Arc::new(AtomicUsize::new(0));
    let idle_since = Arc::new(AtomicI64::new(now_secs()));

    let limits = WatchdogLimits::from_env();
    tracing::info!(
        "memory watchdog: unload above {} MB, retire above {} MB, at most one unload per {:?} ({}/{}/{} to change; 0 disables)",
        limits.soft_mb,
        limits.hard_mb,
        limits.cooldown,
        RSS_SOFT_ENV,
        RSS_HARD_ENV,
        RSS_COOLDOWN_ENV,
    );
    // Announced separately because it answers a different question — the
    // machine's free memory, not this process's size — and because a knob the
    // daemon never names is a knob nobody knows to reach for. It also reports
    // what it can actually see: on a platform without /proc the floor is dead,
    // and that must be visible at startup rather than inferred from silence.
    tracing::info!(
        "machine memory floor: unload when less than {} MB is available, now {:?} MB ({} to change; 0 disables)",
        limits.min_available_mb,
        mem_available_kib().map(|kib| kib / 1024),
        MIN_AVAILABLE_ENV,
    );
    let retire_grace = retire_grace_from_env();
    tracing::info!(
        "retire deadline: a retired daemon exits after {:?} even with clients attached ({} to change; 0 waits forever)",
        retire_grace,
        RETIRE_GRACE_ENV,
    );
    let gc_interval = Duration::from_secs(env_u64(GC_INTERVAL_ENV, DEFAULT_GC_INTERVAL_SECS));
    tracing::info!(
        "garbage collection: every {:?} in every loaded analysis ({} to change; 0 disables)",
        gc_interval,
        GC_INTERVAL_ENV,
    );
    let mut last_gc = SystemTime::now();
    let mut last_unload: Option<SystemTime> = None;
    // Once retiring, the socket file is gone and belongs to whoever binds it
    // next — this flag keeps the exit path from deleting a successor's socket.
    // `retiring_since` is what bounds the wait; see `retire_grace_expired`.
    let mut retiring = false;
    let mut retiring_since: Option<SystemTime> = None;

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

        // Memory watchdog. Runs on the same tick as the idle check, so it costs
        // one `/proc/self/status` and one `/proc/meminfo` read every 15 s and
        // nothing else.
        if let Some(rss_mb) = rss_kib().map(|kib| kib / 1024) {
            let available_mb = mem_available_kib().map(|kib| kib / 1024);
            let since_unload = last_unload.and_then(|at| at.elapsed().ok());
            match decide_watchdog_action(rss_mb, available_mb, limits, since_unload, retiring) {
                WatchdogAction::None => {}
                WatchdogAction::Unload => {
                    // Which of the two reasons fired is worth naming: "RSS 3 GB,
                    // unloading" would read as a bug in the threshold rather
                    // than as the machine-wide floor doing its job.
                    // `retiring` is named too: a retired daemon unloading at 21 GB
                    // reads as a broken soft limit unless the line says which
                    // daemon it came from, and both daemons write to one log file.
                    tracing::warn!(
                        "unloading analysis contexts{}: RSS {} MB (soft limit {} MB), \
                         machine has {:?} MB available (floor {} MB)",
                        if retiring { " (retired, draining)" } else { "" },
                        rss_mb,
                        limits.soft_mb,
                        available_mb,
                        limits.min_available_mb
                    );
                    let report = runtime
                        .state()
                        .clear(RuntimeClearRequest {
                            scope: RuntimeClearScope::SemanticOnly,
                            workspace: None,
                        })
                        .await;
                    last_unload = Some(SystemTime::now());
                    // Report what the release actually achieved, not that it
                    // ran. On glibc this line is routinely "0 MB released",
                    // and that fact belongs in the log rather than in a later
                    // investigation.
                    tracing::warn!(
                        "unloaded {} project(s); RSS {:?} -> {:?} KiB, {:?} KiB released",
                        report.semantic_projects_cleared,
                        report.memory.rss_kib_before,
                        report.memory.rss_kib_after,
                        report.memory.released_kib,
                    );
                }
                WatchdogAction::Retire => {
                    tracing::warn!(
                        "RSS {} MB is over the {} MB hard limit; retiring: new clients will start a fresh daemon, current ones finish here",
                        rss_mb,
                        limits.hard_mb
                    );
                    // Unlinking is what makes this graceful: the address stops
                    // resolving to us, so the next client spawns a successor,
                    // while the connections already open keep working.
                    let _ = fs::remove_file(socket);
                    retiring = true;
                    retiring_since = Some(SystemTime::now());
                }
            }
        }

        // Garbage collection, on its own timer rather than on the watchdog's
        // thresholds: its job is to keep the working set from growing in the
        // first place, so waiting for the soft limit would be waiting for the
        // problem it prevents. Kept running while retiring too — a retired
        // daemon can sit here for the whole grace period holding what pushed it
        // over the hard limit.
        if should_collect_garbage(last_gc.elapsed().unwrap_or_default(), gc_interval) {
            let collected = runtime.state().collect_garbage().await;
            last_gc = SystemTime::now();
            if collected > 0 {
                tracing::info!("collected garbage in {} loaded project(s)", collected);
            }
        }

        if retiring && live.load(Ordering::SeqCst) == 0 {
            tracing::info!("last client left after retiring; shutting down");
            break;
        }

        // The deadline. Without it this daemon waits for clients that outlive
        // it by hours, holding whatever pushed it past the hard limit in the
        // first place.
        if retiring
            && retire_grace_expired(
                retiring_since.and_then(|at| at.elapsed().ok()),
                retire_grace,
            )
        {
            tracing::warn!(
                "retired {:?} ago and {} client(s) are still attached; exiting anyway on the {} deadline — those connections will drop",
                retire_grace,
                live.load(Ordering::SeqCst),
                RETIRE_GRACE_ENV,
            );
            break;
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
    // round trip clearing a stale socket. Skipped when retiring, where the file
    // was already unlinked and any file at that path now belongs to a successor.
    if !retiring {
        let _ = fs::remove_file(socket);
    }
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
mod watchdog_tests {
    use super::*;

    const LIMITS: WatchdogLimits = WatchdogLimits {
        soft_mb: 4096,
        hard_mb: 10240,
        min_available_mb: 6144,
        cooldown: Duration::from_secs(300),
    };

    /// A machine with room to spare, so a case about RSS is only about RSS.
    const ROOMY: Option<u64> = Some(40_000);

    #[test]
    fn ordinary_memory_use_is_left_alone() {
        assert_eq!(
            decide_watchdog_action(3000, ROOMY, LIMITS, None, false),
            WatchdogAction::None
        );
    }

    #[test]
    fn crossing_the_soft_limit_unloads() {
        assert_eq!(
            decide_watchdog_action(5000, ROOMY, LIMITS, None, false),
            WatchdogAction::Unload
        );
    }

    /// The trigger normally stays true after an unload, because RSS does not
    /// drop. Without the cooldown the watchdog would unload on every tick and
    /// every semantic query would pay for a fresh workspace load.
    #[test]
    fn a_second_unload_waits_for_the_cooldown() {
        assert_eq!(
            decide_watchdog_action(5000, ROOMY, LIMITS, Some(Duration::from_secs(60)), false),
            WatchdogAction::None
        );
        assert_eq!(
            decide_watchdog_action(5000, ROOMY, LIMITS, Some(Duration::from_secs(600)), false),
            WatchdogAction::Unload
        );
    }

    /// Past the hard limit the cooldown must not delay retirement: unloading has
    /// already been tried and the memory is stuck in the allocator.
    #[test]
    fn the_hard_limit_outranks_the_cooldown() {
        assert_eq!(
            decide_watchdog_action(11000, ROOMY, LIMITS, Some(Duration::from_secs(1)), false),
            WatchdogAction::Retire
        );
    }

    /// Retiring twice would unlink a socket that by then belongs to the
    /// successor daemon.
    #[test]
    fn a_retiring_daemon_never_retires_again() {
        assert_ne!(
            decide_watchdog_action(11000, ROOMY, LIMITS, None, true),
            WatchdogAction::Retire
        );
    }

    /// And it is the one daemon that most needs to unload. It is past the hard
    /// limit by definition, no new client will ever reach it, and its contexts
    /// serve only the sessions still draining — while a successor is already
    /// loading an analysis of its own beside it. On 2026-08-29 this returned
    /// `None` and a retired daemon held 21.6 GB for the full grace period,
    /// taking the machine's swap to zero.
    #[test]
    fn a_retiring_daemon_still_unloads() {
        assert_eq!(
            decide_watchdog_action(11000, ROOMY, LIMITS, None, true),
            WatchdogAction::Unload
        );
    }

    /// The rate limit is not lifted by retiring: an unload that just ran freed
    /// what it was going to free, and repeating it every tick would only spend
    /// the reload cost again on the sessions that are draining.
    #[test]
    fn a_retiring_daemon_still_respects_the_cooldown() {
        assert_eq!(
            decide_watchdog_action(11000, ROOMY, LIMITS, Some(Duration::from_secs(60)), true),
            WatchdogAction::None
        );
        assert_eq!(
            decide_watchdog_action(11000, ROOMY, LIMITS, Some(Duration::from_secs(600)), true),
            WatchdogAction::Unload
        );
    }

    /// Retiring is not itself a reason to unload — the thresholds still decide.
    /// A daemon retired by the hard limit and since brought back under the soft
    /// one by its own unload has nothing left to do but drain.
    #[test]
    fn a_retiring_daemon_below_the_limits_does_nothing() {
        assert_eq!(
            decide_watchdog_action(3000, ROOMY, LIMITS, None, true),
            WatchdogAction::None
        );
    }

    #[test]
    fn zero_disables_a_threshold() {
        let no_soft = WatchdogLimits {
            soft_mb: 0,
            ..LIMITS
        };
        assert_eq!(
            decide_watchdog_action(9000, ROOMY, no_soft, None, false),
            WatchdogAction::None,
            "soft_mb = 0 must not unload"
        );

        let no_hard = WatchdogLimits {
            hard_mb: 0,
            ..LIMITS
        };
        assert_eq!(
            decide_watchdog_action(99000, ROOMY, no_hard, None, false),
            WatchdogAction::Unload,
            "hard_mb = 0 must never retire, however high RSS goes"
        );

        let disabled = WatchdogLimits {
            soft_mb: 0,
            hard_mb: 0,
            ..LIMITS
        };
        assert_eq!(
            decide_watchdog_action(99000, ROOMY, disabled, None, false),
            WatchdogAction::None
        );
    }

    /// The case none of the RSS thresholds can see: this daemon is small, and
    /// the machine is in trouble anyway — a build, a second analyser, the
    /// application under development. The caches belong to whoever needs the
    /// memory more.
    #[test]
    fn a_short_machine_unloads_even_a_small_daemon() {
        assert_eq!(
            decide_watchdog_action(3000, Some(2000), LIMITS, None, false),
            WatchdogAction::Unload,
            "3 GB of RSS is fine; 2 GB left on the machine is not"
        );
    }

    /// It unloads and stops there. Retiring unlinks the socket and puts a
    /// deadline on live sessions — too much for pressure that is usually
    /// somebody else's and passes with the build that caused it.
    #[test]
    fn a_short_machine_never_retires() {
        assert_eq!(
            decide_watchdog_action(3000, Some(0), LIMITS, None, false),
            WatchdogAction::Unload,
            "even at zero available memory the floor may not retire the daemon"
        );
    }

    /// The floor is rate-limited like the soft limit, and for the same reason:
    /// the memory usually does not come back to the machine on the next tick.
    #[test]
    fn the_floor_respects_the_cooldown() {
        assert_eq!(
            decide_watchdog_action(3000, Some(2000), LIMITS, Some(Duration::from_secs(60)), false),
            WatchdogAction::None
        );
    }

    #[test]
    fn a_roomy_machine_and_a_small_daemon_are_left_alone() {
        assert_eq!(
            decide_watchdog_action(3000, Some(6144), LIMITS, None, false),
            WatchdogAction::None,
            "exactly at the floor is not below it"
        );
    }

    /// Two ways for the floor to be silent, and both must be: `0` is the opt-out,
    /// and `None` is "the reading could not be taken". An unknown number that
    /// acted like an alarming one would unload forever on any platform without
    /// `/proc`.
    #[test]
    fn an_unknown_or_disabled_floor_never_fires() {
        assert_eq!(
            decide_watchdog_action(3000, None, LIMITS, None, false),
            WatchdogAction::None,
            "unreadable available memory must not read as pressure"
        );

        let no_floor = WatchdogLimits {
            min_available_mb: 0,
            ..LIMITS
        };
        assert_eq!(
            decide_watchdog_action(3000, Some(1), no_floor, None, false),
            WatchdogAction::None,
            "min_available_mb = 0 disables the check"
        );
    }

    /// A healthy daemon serving one large workspace, measured live: 2.3 GB of
    /// fixed startup cost (ONNX runtime, embedding model, GPU probe) plus ~3 GB
    /// for a `Fast` load of ~4000 files.
    const MEASURED_HEALTHY_MB: u64 = 5300;

    /// The defect this closes: the first `DEFAULT_RSS_SOFT_MB` was 4096, chosen
    /// from the workspace cost alone and forgetting the fixed startup floor —
    /// below the normal working point, so a healthy daemon would have unloaded
    /// rust-analyzer on every cooldown forever. A default that fires during
    /// ordinary work is worse than no watchdog at all.
    #[test]
    fn defaults_do_not_fire_on_a_healthy_daemon() {
        let defaults = WatchdogLimits {
            soft_mb: DEFAULT_RSS_SOFT_MB,
            hard_mb: DEFAULT_RSS_HARD_MB,
            min_available_mb: DEFAULT_MIN_AVAILABLE_MB,
            cooldown: Duration::from_secs(DEFAULT_RSS_COOLDOWN_SECS),
        };

        assert_eq!(
            decide_watchdog_action(MEASURED_HEALTHY_MB, ROOMY, defaults, None, false),
            WatchdogAction::None,
            "a daemon at the measured healthy working point ({MEASURED_HEALTHY_MB} MB) must be \
             left alone; soft limit is {DEFAULT_RSS_SOFT_MB} MB"
        );
        assert!(
            DEFAULT_RSS_HARD_MB > DEFAULT_RSS_SOFT_MB,
            "retiring before unloading has even been tried inverts the escalation"
        );
    }

    /// The machine-wide floor has to sit above where an OOM killer starts
    /// choosing victims, or it only ever fires after something has been killed —
    /// which is not a guard. `earlyoom -m 8,4` on a 61 GB desktop kills near
    /// 2.4 GB; a floor at or below that would be decoration.
    #[test]
    fn the_machine_floor_leaves_room_before_the_oom_killer() {
        const TYPICAL_OOM_KILL_MB: u64 = 2440;

        assert!(
            DEFAULT_MIN_AVAILABLE_MB > TYPICAL_OOM_KILL_MB,
            "a floor at {DEFAULT_MIN_AVAILABLE_MB} MB must leave room above the \
             ~{TYPICAL_OOM_KILL_MB} MB where the OOM killer starts"
        );
    }

    /// Exactly at the limit counts as over it — a threshold that only fires
    /// strictly above leaves a value that reads as tripped but does nothing.
    #[test]
    fn thresholds_are_inclusive() {
        assert_eq!(
            decide_watchdog_action(4096, ROOMY, LIMITS, None, false),
            WatchdogAction::Unload
        );
        assert_eq!(
            decide_watchdog_action(10240, ROOMY, LIMITS, None, false),
            WatchdogAction::Retire
        );
    }

    const GRACE: Duration = Duration::from_secs(1800);

    /// A daemon that has not retired has no deadline to expire, however long it
    /// has been running.
    #[test]
    fn a_daemon_that_never_retired_has_no_deadline() {
        assert!(!retire_grace_expired(None, GRACE));
    }

    /// The waiting period is real: a daemon that retired a moment ago must give
    /// its clients their chance to finish, not drop them on the same tick.
    #[test]
    fn a_freshly_retired_daemon_keeps_waiting() {
        assert!(!retire_grace_expired(Some(Duration::from_secs(60)), GRACE));
        assert!(!retire_grace_expired(
            Some(GRACE - Duration::from_secs(1)),
            GRACE
        ));
    }

    /// And it ends. This is the whole point of the knob: on 2026-08-27 a retired
    /// daemon held 24.2 GB waiting for a session that ran for hours.
    #[test]
    fn the_deadline_eventually_fires() {
        assert!(retire_grace_expired(Some(GRACE), GRACE));
        assert!(retire_grace_expired(Some(Duration::from_secs(7200)), GRACE));
    }

    /// Zero restores the original behaviour — wait for the last client, however
    /// long that takes. Without this the knob could not be turned off, and a
    /// daemon on a machine with memory to spare would drop sessions for nothing.
    #[test]
    fn zero_grace_waits_forever() {
        let forever = Duration::ZERO;
        assert!(!retire_grace_expired(
            Some(Duration::from_secs(86400)),
            forever
        ));
        assert!(!retire_grace_expired(None, forever));
    }

    /// The deadline is only useful strictly *inside* the idle timeout: a retired
    /// daemon is worse than an idle one, so it must not be allowed to outlive
    /// one. Equal is the deliberate choice — this guards against someone raising
    /// the default past it.
    #[test]
    fn the_default_deadline_does_not_exceed_the_idle_timeout() {
        assert!(
            DEFAULT_RETIRE_GRACE_SECS <= DEFAULT_IDLE_SECS,
            "a retired daemon outliving the idle timeout defeats the deadline: an idle daemon \
             would already have exited by then"
        );
    }

    const GC_EVERY: Duration = Duration::from_secs(300);

    /// The interval is a floor, so the tick that lands before it must not
    /// collect: the daemon ticks every 15 s, and collecting on each of them
    /// would re-validate every project's memos four times a minute.
    #[test]
    fn a_tick_inside_the_interval_does_not_collect() {
        assert!(!should_collect_garbage(Duration::ZERO, GC_EVERY));
        assert!(!should_collect_garbage(Duration::from_secs(299), GC_EVERY));
    }

    #[test]
    fn the_interval_eventually_fires() {
        assert!(should_collect_garbage(GC_EVERY, GC_EVERY));
        assert!(should_collect_garbage(Duration::from_secs(3600), GC_EVERY));
    }

    /// Zero turns collection off. Needed because the collection is not free —
    /// a daemon on a machine with memory to spare can decline to pay for it.
    #[test]
    fn a_zero_interval_never_collects() {
        assert!(!should_collect_garbage(
            Duration::from_secs(86400),
            Duration::ZERO
        ));
    }

    /// Collecting garbage and unloading contexts buy the same thing — memory in
    /// exchange for a slower next query — and the collection is by far the
    /// cheaper of the two. Pacing it more eagerly than the expensive one would
    /// be backwards, and this is what says so out loud.
    #[test]
    fn collection_is_not_paced_more_eagerly_than_an_unload() {
        assert!(
            DEFAULT_GC_INTERVAL_SECS >= DEFAULT_RSS_COOLDOWN_SECS,
            "a collection running more often than the unload cooldown would spend the cheap \
             mechanism harder than the expensive one"
        );
    }
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

    fn key(exe: &str, len: u64, mtime: u128, sync: &str) -> String {
        key_from_parts(
            Path::new(exe),
            len,
            mtime,
            &[(BACKGROUND_SYNC_ENV, sync.to_string())],
        )
    }

    #[test]
    fn key_is_stable_for_same_inputs() {
        assert_eq!(key("/bin/mcp", 10, 20, "1"), key("/bin/mcp", 10, 20, "1"));
    }

    /// Configuration must split daemons: a server started with different
    /// behaviour-changing env does not answer what the new client is asking for.
    #[test]
    fn key_depends_on_keyed_env() {
        assert_ne!(key("/bin/mcp", 10, 20, "1"), key("/bin/mcp", 10, 20, "0"));
    }

    /// The inverse of the old `key_depends_on_project`, and the whole point of
    /// this change: sessions started in different directories must land on ONE
    /// daemon. Keying on the cwd bought no isolation — every tool takes its
    /// `directory` as a parameter — and cost 11 processes holding 12.5 GB.
    ///
    /// Judged on the real `daemon_key()`, not on `key_from_parts`: the pure
    /// helper no longer takes a cwd at all, so asking it would prove nothing.
    /// The process-wide cwd is restored, and no other test in this binary reads
    /// it.
    #[test]
    fn key_does_not_depend_on_the_working_directory() {
        let original = std::env::current_dir().expect("a working directory");

        std::env::set_current_dir("/tmp").expect("chdir /tmp");
        let from_tmp = daemon_key().expect("a key");

        std::env::set_current_dir("/").expect("chdir /");
        let from_root = daemon_key().expect("a key");

        std::env::set_current_dir(&original).expect("chdir back");

        assert_eq!(
            from_tmp, from_root,
            "one daemon serves every working directory"
        );
    }

    #[test]
    fn an_explicit_directory_wins() {
        assert_eq!(
            resolve_socket_dir(
                Some("/explicit"),
                Some("/run/user/1000"),
                Some(Path::new("/run/user/1000")),
                Some("sc"),
                Path::new("/tmp")
            ),
            PathBuf::from("/explicit")
        );
    }

    #[test]
    fn the_runtime_dir_is_preferred_when_the_environment_names_it() {
        assert_eq!(
            resolve_socket_dir(
                None,
                Some("/run/user/1000"),
                None,
                Some("sc"),
                Path::new("/tmp")
            ),
            PathBuf::from("/run/user/1000/rust-code-mcp")
        );
    }

    /// The split this closes. A session started outside a login shell has no
    /// `XDG_RUNTIME_DIR`, computes the same key as its neighbours, and used to
    /// look for the socket in `/tmp` — where it found nothing and started a
    /// second daemon. Both halves of that were live on this machine at once, on
    /// two separate keys.
    #[test]
    fn a_missing_xdg_variable_still_finds_the_runtime_directory() {
        assert_eq!(
            resolve_socket_dir(
                None,
                None,
                Some(Path::new("/run/user/1000")),
                Some("sc"),
                Path::new("/tmp")
            ),
            PathBuf::from("/run/user/1000/rust-code-mcp"),
            "an unset XDG_RUNTIME_DIR must not send this client to a different directory \
             than its neighbours"
        );
        assert_eq!(
            resolve_socket_dir(
                Some(""),
                Some(""),
                Some(Path::new("/run/user/1000")),
                Some("sc"),
                Path::new("/tmp")
            ),
            PathBuf::from("/run/user/1000/rust-code-mcp"),
            "empty is as unset as unset"
        );
    }

    /// With no runtime directory at all — a container, a non-systemd host — the
    /// old per-user temp directory is still the answer.
    #[test]
    fn without_a_runtime_directory_it_falls_back_to_temp() {
        assert_eq!(
            resolve_socket_dir(None, None, None, Some("sc"), Path::new("/tmp")),
            PathBuf::from("/tmp/rust-code-mcp-sc")
        );
        assert_eq!(
            resolve_socket_dir(None, None, None, None, Path::new("/tmp")),
            PathBuf::from("/tmp/rust-code-mcp-shared")
        );
    }

    /// A rebuilt binary must get a new socket, or a client built from new code is
    /// silently served by the old server.
    #[test]
    fn key_depends_on_binary_identity() {
        assert_ne!(
            key("/bin/mcp", 10, 20, "1"),
            key("/bin/mcp", 10, 21, "1"),
            "a different binary mtime means a different daemon"
        );
        assert_ne!(
            key("/bin/mcp", 10, 20, "1"),
            key("/bin/mcp", 11, 20, "1"),
            "a different binary size means a different daemon"
        );
    }

    /// The regression this exists for: a successor must not erase the log of the
    /// daemon it is replacing. That log is the only account of *why* the
    /// predecessor grew, and it was being destroyed at the one moment it
    /// mattered.
    #[test]
    fn a_successor_appends_to_the_log_instead_of_erasing_it() {
        use std::io::Write as _;

        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("key.sock");
        let predecessor = "predecessor: RSS 24000 MB is over the hard limit\n";
        std::fs::write(log_path(&socket), predecessor).expect("seed the predecessor's log");

        let mut log = open_daemon_log(&socket).expect("successor opens the log");
        write!(log, "successor: daemon listening (pid 2)\n").expect("successor writes");

        let contents = std::fs::read_to_string(log_path(&socket)).expect("read back");
        assert!(
            contents.starts_with(predecessor),
            "the predecessor's log must survive its successor's start, got {contents:?}"
        );
        assert!(
            contents.contains("successor:"),
            "the successor's own lines must be there too, got {contents:?}"
        );
    }

    /// Appending forever is its own failure mode, so one generation is kept.
    /// Sparse allocation keeps this test from writing 32 MB to disk.
    #[test]
    fn an_oversized_log_is_rolled_aside_rather_than_grown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("key.sock");
        let log = log_path(&socket);

        let file = File::create(&log).expect("create the oversized log");
        file.set_len(LOG_ROTATE_BYTES).expect("grow it sparsely");
        drop(file);

        open_daemon_log(&socket).expect("open after rotation");

        assert_eq!(
            std::fs::metadata(&log).map(|meta| meta.len()).unwrap_or(0),
            0,
            "the fresh log must start empty once the old one was rolled aside"
        );
        assert_eq!(
            std::fs::metadata(log.with_extension("log.1"))
                .expect("the rolled-aside generation must exist")
                .len(),
            LOG_ROTATE_BYTES,
            "the previous generation must be kept intact, not discarded"
        );
    }

    /// A log under the limit is left exactly where it is — rotating on every
    /// spawn would reintroduce the very loss this pair of tests guards.
    #[test]
    fn a_small_log_is_not_rotated() {
        let dir = tempfile::tempdir().expect("tempdir");
        let socket = dir.path().join("key.sock");
        std::fs::write(log_path(&socket), "small\n").expect("seed");

        open_daemon_log(&socket).expect("open");

        assert!(
            !log_path(&socket).with_extension("log.1").exists(),
            "nothing should have been rolled aside"
        );
    }
}

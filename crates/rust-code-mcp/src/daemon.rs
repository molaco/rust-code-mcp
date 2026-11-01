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
//!
//! # A replaced binary retires its daemon
//!
//! The key covers the binary, so a rebuild starts a new daemon for new
//! sessions — and the old one used to live on for as long as any session it
//! had was open (2026-10-21: ≈ 30 GB held by two daemons of a deleted binary).
//! Now both sides notice the replacement: the daemon compares the file at its
//! launch path with the image it runs ([`Launch::replacement`]) and retires,
//! and the client moves its session to the daemon of the current binary
//! between requests (`relay`). A lost daemon — crash, OOM killer, deadline —
//! is replaced the same way, without the session losing its tools.

mod relay;

use fs2::FileExt;
use rmc_server::mcp::{
    BACKGROUND_SYNC_ENV, EMBEDDING_PROFILE_ENV, EP_CENSUS_ENV, RuntimeClearRequest,
    RuntimeClearScope, RuntimeState, RuntimeTaskReport, ServerRuntime, gpu_vram_budget,
    mem_available_kib, release_free_memory, release_idle_gpu_sessions, rss_kib,
};
use rmc_server::tools::SearchTool;
use rmcp::ServiceExt;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::{AsFd, AsRawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::net::{UnixListener, UnixStream};
use tokio::signal::unix::{SignalKind, signal};

pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Opt out of the whole scheme: `RMC_DAEMON=0` (`off`/`false`/`no`) keeps the
/// server inside the client process, as it was before.
pub const DAEMON_ENV: &str = "RMC_DAEMON";
/// Directory holding sockets, and then daemon logs too. Defaults to
/// `$XDG_RUNTIME_DIR/rust-code-mcp` for sockets and
/// `$XDG_STATE_HOME/rust-code-mcp` for logs (see [`log_path`]).
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
/// How long a shared GPU embedding session may sit unused before it is
/// released, in seconds. `0` keeps it for the life of the daemon.
pub const GPU_IDLE_ENV: &str = "RMC_GPU_IDLE_SECS";

/// Fifteen minutes. A session holds ≈ 1.4 GB of VRAM (`local-gpu-bge`,
/// measured by the ledger) and the background sync runs whenever files
/// change, so in an active day the session is busy every few minutes and
/// stays; overnight, or for a daemon whose clients only read, the card gets
/// it back. Rebuilding it reloads the compiled program from the MIGraphX
/// kernel cache, seconds rather than the minute of a cold compile.
const DEFAULT_GPU_IDLE_SECS: u64 = 900;

/// A prefix policy rather than a list, because a list falls behind the code.
///
/// Every knob this server reads is namespaced, and the hand-kept list this
/// replaced named three variables while the code read far more: the five
/// `RUST_CODE_MCP_OPENROUTER_*` settings decide WHICH endpoint answers, and none
/// of them was keyed — two sessions pointing at different endpoints shared one
/// daemon, whose answers came from whichever endpoint the first client had.
const KEYED_ENV_PREFIXES: [&str; 2] = ["RMC_", "RUST_CODE_MCP_"];

/// Keyed without a shared prefix: whose account pays for a request, and which
/// ONNX/CUDA libraries the daemon links when it starts.
const KEYED_ENV_EXTRA: [&str; 2] = ["OPENROUTER_API_KEY", "LD_LIBRARY_PATH"];

/// Keyed prefix, excluded anyway: none of these changes what the server
/// *computes*, so keying them splits the fleet and buys nothing back.
///
/// Two kinds live here. The first three are transport — they select WHICH daemon
/// a client talks to, and keying them would split one daemon in two.
///
/// [`EP_CENSUS_ENV`] is the second kind: a startup diagnostic. It probes which
/// execution provider actually runs the graph and prints the finding
/// (`probe_ep_census_on_startup`), and decides nothing else — the answer is a
/// property of the machine and the build, not of the session that asked.
///
/// It was keyed until a measurement said what that costs. This machine sets it
/// to `0` for one repository and leaves eighteen others defaulting to `1`, and
/// the split does not even need two Claude Code sessions to bite: an editor
/// extension that does not read Claude Code's settings expanded the default and
/// ran a SECOND daemon of its own — 1.6 GB of fixed startup (ONNX runtime,
/// embedding model, GPU probe) beside a live one, and two memory watchdogs that
/// cannot see each other's RSS. Measured 2026-09-09, while the keyed daemon was
/// on its way to 30 GB; the second one was unloading on the machine-wide
/// availability floor because of a pile it had no part in.
///
/// The price of unkeying it, stated plainly: a client that asks for a census
/// while a daemon is already up gets none, because the probe runs once at
/// startup. Restarting the daemon is what re-runs it.
const UNKEYED_ENV: [&str; 4] = [DAEMON_ENV, DAEMON_DIR_ENV, IDLE_ENV, EP_CENSUS_ENV];

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
/// How long the client keeps draining the socket after stdin reached EOF.
///
/// A healthy session ends the drain by itself: the daemon sees the half-close,
/// finishes the answers it still owes, and closes. This bound only stops a
/// client from waiting forever behind a daemon that never closes — and reaching
/// it is a failure, not a shutdown, because replies the host is waiting for
/// never arrived.
const DRAIN_AFTER_EOF: Duration = Duration::from_secs(60);
/// How often a client checks whether its daemon was superseded — the same
/// cadence as the daemon's own tick, so a replaced binary empties its daemon
/// within two ticks of the last request in flight.
const CLIENT_TICK: Duration = IDLE_TICK;
/// How long a client waits before trying again to reach a successor it could
/// not start — a binary still being written, say. The old daemon keeps
/// answering meanwhile.
const MOVE_RETRY: Duration = Duration::from_secs(60);

/// How this process was started. Resolved *before* the expensive startup: a
/// client needs neither a `ServerRuntime` nor a background sync task — it is a pipe.
#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    /// Server inside this process over stdio — the behaviour before the daemon.
    InProcess,
    /// Daemon: listens on a socket, serves many clients from one `RuntimeState`.
    Daemon { socket: PathBuf, idle: Duration },
    /// Client: stdin/stdout ↔ socket, spawning the daemon when needed.
    ///
    /// `idle` carries `--idle-secs` only when the command line actually passed
    /// it, so a daemon this client starts honours the flag. The environment
    /// default stays a daemon-side concern: the spawned process inherits the
    /// variable itself, and forwarding it here would turn "unset" into an
    /// explicit argument.
    ///
    /// `follow_key` — the socket was derived from the key rather than named by
    /// `--socket`, so a rebuilt binary means a different socket and the session
    /// moves there.
    Client {
        socket: PathBuf,
        idle: Option<Duration>,
        follow_key: bool,
    },
    /// `--print-socket`: print the resolved socket path and exit (diagnostics).
    PrintSocket { socket: PathBuf },
    /// `--bm25-stats <DIR>`: print the state of the BM25 index in `DIR` as
    /// JSON and exit (diagnostics; safe on a live daemon's index).
    Bm25Stats { index_dir: PathBuf },
    /// `--tool-latency <LOG>... [--since T] [--until T]`: judge every reading
    /// tool call in daemon logs against the work that overlapped it, print
    /// the report and exit 1 when a call was blocked (see `tool_latency`).
    ToolLatency {
        logs: Vec<PathBuf>,
        since: Option<f64>,
        until: Option<f64>,
    },
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
            // Answers from its argument alone, like `--help`: no socket, no
            // server, so it neither needs nor combines with the other modes.
            "--bm25-stats" => {
                let value = it
                    .next()
                    .ok_or_else(|| BoxError::from("--bm25-stats requires an index directory"))?;
                return Ok(Mode::Bm25Stats {
                    index_dir: PathBuf::from(value),
                });
            }
            // Reads log files only, like `--bm25-stats`; everything after it
            // is its own: log paths and the window bounds.
            "--tool-latency" => return parse_tool_latency(it.as_slice()),
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

    let follow_key = socket.is_none();
    let socket = match socket {
        Some(path) => path,
        None => default_socket_path()?,
    };
    Ok(match explicit {
        Some("--daemon") => Mode::Daemon {
            socket,
            idle: idle.unwrap_or_else(idle_from_env),
        },
        Some("--in-process") => Mode::InProcess,
        Some("--print-socket") => Mode::PrintSocket { socket },
        None if daemon_disabled() => Mode::InProcess,
        // `--client`, or nothing said.
        Some(_) | None => Mode::Client {
            socket,
            idle,
            follow_key,
        },
    })
}

/// The arguments after `--tool-latency`: one or more logs, `--since T` and
/// `--until T` (UTC, `2026-10-31T15:27:00Z`) bounding the calls judged.
fn parse_tool_latency(args: &[String]) -> Result<Mode, BoxError> {
    let mut logs = Vec::new();
    let mut since = None;
    let mut until = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--since" | "--until" => {
                let value = it
                    .next()
                    .ok_or_else(|| BoxError::from(format!("{arg} requires a UTC time")))?;
                let at = crate::tool_latency::parse_timestamp(value).ok_or_else(|| {
                    BoxError::from(format!(
                        "{arg} {value}: expected a UTC time like 2026-10-31T15:27:00Z"
                    ))
                })?;
                if arg == "--since" {
                    since = Some(at);
                } else {
                    until = Some(at);
                }
            }
            other if other.starts_with("--") => {
                return Err(format!("--tool-latency: unknown argument {other}").into());
            }
            log => logs.push(PathBuf::from(log)),
        }
    }
    if logs.is_empty() {
        return Err("--tool-latency requires at least one daemon log".into());
    }
    Ok(Mode::ToolLatency { logs, since, until })
}

pub const USAGE: &str = "\
rust-code-mcp — an MCP server for Rust codebases.

With no arguments: a client of this project's shared daemon, which is started
on demand.

  --client            the same, explicitly
  --daemon            become the daemon: listen on a socket, serve many clients
  --in-process        run the server in this process over stdio (previous behaviour)
  --print-socket      print this configuration's socket path and exit
  --bm25-stats <DIR>  print the BM25 index in DIR (segments, deleted documents,
                      bytes per file kind) as JSON and exit; reads meta.json
                      and file sizes only, safe on a running daemon's index
  --tool-latency <LOG>... [--since T] [--until T]
                      judge every reading tool call in daemon logs against the
                      work that overlapped it (syncs, maintenance, other
                      projects' loads, other calls); exit 1 when one was
                      blocked. T is UTC, e.g. 2026-10-31T15:27:00Z
  --socket <PATH>     use this socket instead of the one derived from the
                      binary and the environment
  --idle-secs <N>     daemon exits after N seconds with no clients (0 = never);
                      a client passes it on to a daemon it starts

Env: RMC_DAEMON=0 forces in-process; RMC_DAEMON_DIR sets the socket directory;
     RMC_DAEMON_IDLE_SECS is the same as --idle-secs.

     RMC_BUILD_SCRIPTS=0 loads a workspace without running its build scripts.
     They run by default because code included from OUT_DIR (prost messages,
     generated tables) is otherwise absent from the tree, and references into
     it come back as an empty list rather than an error. The cost is one cargo
     invocation per project load, in the workspace's own target directory.

One daemon serves EVERY working directory: the socket is keyed by this binary
and the environment above, not by the directory a session starts in. Path
parameters must therefore be absolute — the daemon shares no working directory
with the session asking, and will refuse a relative path rather than resolve it
against a directory nobody chose.

Memory watchdog (daemon only; 0 disables a threshold):
  RMC_RSS_SOFT_MB=24576      unload the analysis contexts above this RSS
  RMC_RSS_HARD_MB=28672      above this, retire: stop taking new clients,
                             unload contexts, let the current clients finish;
                             a successor is started on demand
  RMC_RSS_COOLDOWN_SECS=300  minimum gap between two unloads
  RMC_MIN_AVAILABLE_MB=6144  unload when the MACHINE has less than this free,
                             whatever this daemon's own RSS is — the only
                             reading that sees pressure it did not cause
  RMC_RETIRE_GRACE_SECS=1800 a retired daemon exits after this even with
                             clients still attached (those connections drop)
  RMC_GC_INTERVAL_SECS=300   how often loaded analyses are garbage-collected;
                             this is also what makes salsa's LRU capacities
                             evict anything at all
  RMC_TRIM_GROWTH_MB=2048    return freed heap to the kernel (malloc_trim)
                             whenever RSS grows this much above what the last
                             trim left, at most every 5 minutes
  RMC_MAX_PROJECTS=3         how many analysis contexts stay loaded; past the
                             cap the least recently used one is dropped

GPU embedding (one shared session per embedding backend; runtime_status.gpu):
  RMC_GPU_IDLE_SECS=900      release a session unused this long (0 never);
                             a retired daemon releases its idle ones at once
  RMC_VRAM_BUDGET_MB=<half of the card>
                             refuse to build another session past this much
                             VRAM held by the daemon (0 disables)

Index durability (0 disables the guard):
  RMC_MIN_INDEX_FREE_MB=2048 refuse persistent index writes below this much
                             free disk space; failed files remain queued in
                             the Merkle snapshot for the next sync
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
    /// Retire the daemon and immediately unload its analysis contexts.
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
    RetireAndUnload,
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
/// [`WatchdogAction::RetireAndUnload`] hands the socket to a successor and then waits for
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

/// How long a shared GPU session may sit unused before this tick hands it
/// back to the card; `None` keeps every session.
///
/// A retired daemon lets go of every idle session at once — its successor is
/// building its own on the same card — but only once its background sync has
/// stopped. Retiring asks the sync to stop, and a pass already in flight runs
/// to its end: between two files of that pass the session is idle for a tick,
/// and dropping it there rebuilt it for the next file. Seen live 2026-10-29:
/// a retired daemon finishing a full re-embed created 78 sessions in 30 min,
/// each a MIGraphX program load, while holding the same VRAM almost all the
/// time. `gpu_idle` of zero means keep, as it does outside retirement.
pub fn gpu_session_max_idle(
    retiring: bool,
    background_sync_running: bool,
    gpu_idle: Duration,
) -> Option<Duration> {
    if retiring && !background_sync_running {
        Some(Duration::ZERO)
    } else {
        (!gpu_idle.is_zero()).then_some(gpu_idle)
    }
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
/// of ~4000 files adds ~3 GB, which is why a healthy daemon serving
/// a large workspace measures ~5.3 GB.
///
/// So the ordinary working point is above 5 GB, and the first draft of this
/// constant — 4 GB, picked from the workspace cost alone and forgetting the
/// fixed 2.3 GB floor — sat *below* normal. It would have unloaded
/// rust-analyzer every cooldown on a perfectly healthy daemon: a memory guard
/// that does nothing but make every query reload the workspace.
///
/// It was 8192 while each daemon served one working directory, then 12288 for
/// three projects: 2.3 GB fixed + 3 × ~3 GB ≈ 11.3 GB.
///
/// # Why 24 GB now — the same mistake as the 4096, one layer up
///
/// Both earlier numbers priced a project at its `Fast` load (~3 GB). Nothing
/// loads `Fast` any more: every semantic call goes through `get_or_load_full`,
/// because which context answered decided whether `#[cfg(test)]` code was
/// visible. A `Full` context of a large workspace works at **18–26 GB** (measured
/// 2026-09-22 over twenty load/unload cycles), so one project costs more than
/// the whole three-project budget, and 12288 sat below the working point
/// exactly as the first 4096 had: the daemon unloaded on every cooldown and the
/// next question from any attached session loaded `Full` again — thirty-eight
/// loads in the log since rotation, each 20 s, five minutes under swap, and not
/// a gigabyte of the peak saved.
///
/// 24 GB lets that working point stand and still unloads the upper end of it
/// (26 GB was measured too) and anything that grows past it; the hard limit
/// stays 4 GB above. Pressure from *outside* the daemon was never this
/// threshold's job — that is `DEFAULT_MIN_AVAILABLE_MB`, which fires whatever
/// the daemon's own size.
///
/// ⚠ Changed as a default rather than set in the environment on purpose: every
/// `RMC_*` variable is part of the daemon key, so setting it in one client's
/// environment starts a second daemon beside the first — the doubling this
/// whole watchdog exists to prevent.
const DEFAULT_RSS_SOFT_MB: u64 = 24576;
/// Twenty-eight gigabytes: past this, unloading has already been tried and the
/// memory is stuck in the allocator, so only a fresh process gets it back.
///
/// "Stuck in the allocator" is checked, not assumed: [`watchdog_tick`] trims
/// before this limit is consulted, so retiring needs RSS that a trim could not
/// return. Until 2026-09-23 the raw reading decided, and freed-but-untrimmed
/// heap alone retired the daemon about once an hour.
///
/// The previous 20 GiB default sat inside the measured 15--22 GiB working range
/// of one `Full` rust-analyzer context and retired a healthy daemon three times
/// in two hours. `Full` is required for complete cross-crate and `#[cfg(test)]`
/// reference answers, so the hard limit must leave that normal peak alone. The
/// 24 GiB soft limit still unloads first; 28 GiB keeps an escalation
/// gap while the machine-wide availability floor remains the final guard when
/// other processes consume the RAM.
const DEFAULT_RSS_HARD_MB: u64 = 28672;
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

/// How far RSS must move from the last logged value before the watchdog tick
/// logs it again.
///
/// The per-operation `mem:` lines (`rmc_engine::mem_trace`) say what each call
/// left behind; this line is the timeline between them — including growth no
/// instrumented operation owns. A gigabyte keeps it to a few lines an hour on
/// a quiet daemon: the tick runs every 15 s, and logging every reading would
/// bury everything else.
const RSS_LOG_STEP_MB: u64 = 1024;

/// Whether this tick's RSS is far enough from the last logged one to log.
/// The first reading is always logged — it is the baseline.
pub fn rss_moved_enough(last_logged_mb: Option<u64>, now_mb: u64) -> bool {
    last_logged_mb.is_none_or(|last| now_mb.abs_diff(last) >= RSS_LOG_STEP_MB)
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
        return WatchdogAction::RetireAndUnload;
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

/// One reading of the two numbers the watchdog decides on, in MB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemoryReading {
    pub rss_mb: u64,
    pub available_mb: Option<u64>,
}

impl MemoryReading {
    /// `None` where RSS cannot be read — the watchdog then does nothing, as it
    /// always has on such platforms.
    fn now() -> Option<Self> {
        rss_kib().map(|kib| Self {
            rss_mb: kib / 1024,
            available_mb: mem_available_kib().map(|kib| kib / 1024),
        })
    }
}

/// One minute between allocator trims made to settle a decision.
///
/// A trim over a fragmented 25 GB heap took 11.6 s on the live daemon
/// (2026-09-23, `unloaded 0 project(s)` at 01:20:59 → 01:21:11) and stalls
/// allocating threads while it walks their arenas. RSS that stays over a limit
/// after one trim would otherwise buy another on every 15 s tick. Within the
/// minute the watchdog decides on the raw reading, exactly as before trims
/// existed: memory that came back that fast is not free heap.
const TRIM_COOLDOWN: Duration = Duration::from_secs(60);

/// What one watchdog tick decided, and the trim that settled it, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchdogTick {
    pub action: WatchdogAction,
    /// `(before, after)` when the tick trimmed the allocator before deciding.
    pub trimmed: Option<(MemoryReading, MemoryReading)>,
}

/// [`decide_watchdog_action`], but with the allocator trimmed before any
/// threshold is allowed to act.
///
/// # Why RSS alone is the wrong input
///
/// RSS counts heap that the program has already freed and glibc has not yet
/// handed back. On this daemon that is not a rounding error. Measured
/// 2026-09-22/23 in `6244fe736b7a5d99.log`: five unloads found **no project
/// loaded at all** (`unloaded 0 project(s)`; four in retired daemons, one in a
/// serving one) at 17476, 19824, 20838, 24767, 24772 MB, and `malloc_trim`
/// alone took them to 4941, 4621, 5004, 8555, 8206 MB. Background indexing and
/// the transient allocations of a `Full` load leave that much behind.
///
/// Deciding on the raw number made both thresholds fire on memory nobody
/// holds: the soft one unloaded a live `Full` context that the next question
/// reloaded, and the hard one retired the daemon — every hour or so, each time
/// dropping the sessions still attached when the grace deadline came. A trim
/// costs seconds; a retirement costs every attached session its tools.
///
/// So when the raw reading asks for action, trim first and decide again on
/// what is left. The trim is not attempted on a healthy tick (nothing to
/// settle) nor within [`TRIM_COOLDOWN`] of the previous one.
pub fn watchdog_tick(
    reading: MemoryReading,
    limits: WatchdogLimits,
    since_unload: Option<Duration>,
    since_trim: Option<Duration>,
    retiring: bool,
    trim: impl FnOnce() -> MemoryReading,
) -> WatchdogTick {
    let raw = decide_watchdog_action(
        reading.rss_mb,
        reading.available_mb,
        limits,
        since_unload,
        retiring,
    );
    let trim_is_due = since_trim.is_none_or(|elapsed| elapsed >= TRIM_COOLDOWN);
    if raw == WatchdogAction::None || !trim_is_due {
        return WatchdogTick {
            action: raw,
            trimmed: None,
        };
    }
    let after = trim();
    WatchdogTick {
        action: decide_watchdog_action(
            after.rss_mb,
            after.available_mb,
            limits,
            since_unload,
            retiring,
        ),
        trimmed: Some((reading, after)),
    }
}

/// Two gigabytes of RSS growth above what the last trim left, and the number is
/// read off the two daemons of 2026-10-21 rather than picked.
///
/// The pressure trim in [`watchdog_tick`] only runs once a limit is crossed,
/// and between crossings RSS is mostly heap the program already freed. The
/// daemon serving twelve sessions trimmed 24.9 → 7.7 GB at 19:03, was back at
/// 14.6 GB nine minutes later and at ~20 GB for the next hour with no trim in
/// sight: its limits sit at 24/28 GB, and the machine floor of 6 GB available
/// fires only after the rest of the machine has gone to swap. Its successor,
/// serving one large workspace, trimmed 15.3 → 9.2 GB at the floor. Both times the
/// trim returned more than the analysis itself holds in salsa (1.9 GB).
///
/// Two gigabytes is a handful of heavy tool calls' worth of freed heap — well
/// above the noise of indexing a few files — and under a tenth of what was
/// being held back. A trim that finds real growth instead of free heap
/// returns little, its result becomes the new baseline, and the next one waits
/// for another two gigabytes: the trigger cannot fire on memory that is
/// genuinely in use.
const DEFAULT_TRIM_GROWTH_MB: u64 = 2048;

/// RSS growth above the last trim's result that buys a trim on its own, in MB.
/// `0` disables it — see [`growth_trim_is_due`].
pub const TRIM_GROWTH_ENV: &str = "RMC_TRIM_GROWTH_MB";

/// Five minutes between growth trims, the GC interval's pace for the same
/// reason: each trim stalls the threads allocating in an arena while it walks
/// that arena (11.6 s over a 25 GB heap, see [`TRIM_COOLDOWN`]), and the accept
/// loop waits for it. At this pace a trim costs at most a few percent of one
/// core, and growth faster than two gigabytes in five minutes is a load, which
/// the next tick after it settles anyway.
const GROWTH_TRIM_COOLDOWN: Duration = Duration::from_secs(300);

/// Whether RSS has grown enough above the last trim's result to trim for that
/// alone, without any limit having been crossed.
///
/// `baseline_mb` is RSS right after the last trim, or at startup before the
/// first. `since_trim` covers trims of either kind: a pressure trim a minute
/// ago has just done this one's work.
pub fn growth_trim_is_due(
    rss_mb: u64,
    baseline_mb: u64,
    growth_mb: u64,
    since_trim: Option<Duration>,
) -> bool {
    if growth_mb == 0 {
        return false;
    }
    if since_trim.is_some_and(|elapsed| elapsed < GROWTH_TRIM_COOLDOWN) {
        return false;
    }
    rss_mb >= baseline_mb.saturating_add(growth_mb)
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

/// `geteuid`, rather than the owner of `/proc/self`, which is Linux only.
fn process_uid() -> u32 {
    // SAFETY: no arguments, reads process state the kernel always has, cannot fail.
    unsafe { libc::geteuid() }
}

/// A refusal that names the path, so the warning logged before the in-process
/// fallback says what to fix.
fn refuse_dir(dir: &Path, reason: &str) -> io::Error {
    io::Error::other(format!("directory {}: {reason}", dir.display()))
}

/// The socket directory: created `0o700` — the socket is an entry point into
/// analysing someone's code — and otherwise CHECKED, never chmodded.
///
/// It may be a directory the user named through `--socket` or `RMC_DAEMON_DIR`,
/// and turning someone's own directory into `0o700` behind their back is a
/// change nobody asked for: `--daemon --socket <dir>/mcp.sock` used to take a
/// perfectly ordinary `0o755` directory down to owner-only as a side effect.
///
/// `symlink_metadata`, because a symlink here decides where the socket really
/// lands: it has to be seen rather than followed.
fn ensure_dir(dir: &Path) -> io::Result<()> {
    let meta = match fs::symlink_metadata(dir) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(dir)?;
            return fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
        }
        Err(e) => return Err(refuse_dir(dir, &format!("cannot be inspected: {e}"))),
    };
    if meta.file_type().is_symlink() {
        return Err(refuse_dir(dir, "is a symlink"));
    }
    if !meta.is_dir() {
        return Err(refuse_dir(dir, "is not a directory"));
    }
    let uid = process_uid();
    if meta.uid() != uid {
        return Err(refuse_dir(
            dir,
            &format!("is owned by uid {}, not by uid {uid}", meta.uid()),
        ));
    }
    Ok(())
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
    let exe = &launch()?.exe;
    // Unreadable (mid-swap, deleted) keys as zeros, as it always did: the key
    // is then one no daemon of a real binary has, and spawning from the path
    // fails loudly instead of attaching to the wrong server.
    let identity = BinaryIdentity::at(exe).unwrap_or_default();

    // `vars_os`, not `vars`: the latter panics on a non-UTF-8 name or value
    // ANYWHERE in this process. This runs before the in-process fallback exists,
    // so that panic would kill the session instead of degrading it.
    let env = keyed_env_pairs(std::env::vars_os());
    let data_dir = rmc_server::mcp::project_paths::data_dir();
    let key = key_from_parts(&data_dir, exe, identity.len, identity.mtime_ns, &env);

    // Names, never values: the keyed set now includes OPENROUTER_API_KEY, and a
    // secret written to a log stays in that log.
    let names: Vec<_> = env.iter().map(|(name, _)| name.to_string_lossy()).collect();
    tracing::debug!(
        "daemon key {key} for index root {}, keyed env: {}",
        data_dir.display(),
        names.join(", ")
    );
    Ok(key)
}

/// The keyed variables of `env`, sorted by the raw bytes of the name so that the
/// key does not depend on the order in which the operating system hands the
/// environment over.
///
/// Bytes, never text: nothing is decoded and nothing is skipped. A variable
/// dropped for not decoding still changed what the daemon does, so two shells
/// with different non-UTF-8 `LD_LIBRARY_PATH` values would get one key and
/// therefore one daemon — the very collision this key exists to prevent.
fn keyed_env_pairs(env: impl Iterator<Item = (OsString, OsString)>) -> Vec<(OsString, OsString)> {
    let keyed = |name: &OsString| {
        let name = name.as_bytes();
        !UNKEYED_ENV.iter().any(|unkeyed| name == unkeyed.as_bytes())
            && (KEYED_ENV_PREFIXES
                .iter()
                .any(|prefix| name.starts_with(prefix.as_bytes()))
                || KEYED_ENV_EXTRA.iter().any(|extra| name == extra.as_bytes()))
    };
    let mut pairs: Vec<(OsString, OsString)> = env.filter(|(name, _)| keyed(name)).collect();
    pairs.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    pairs
}

/// The pure part of the key: everything that matters arrives as an argument.
///
/// Split out of [`daemon_key`] for testability rather than tidiness: checking
/// "the key changes with configuration" through `set_var` means mutating global
/// env in parallel with other tests, which fails for reasons unrelated to keys.
///
/// `data_dir` is the index root, hashed as the path it resolves to rather than
/// as the variables that resolve it: that chain (`XDG_DATA_HOME`, then `HOME`)
/// carries no keyed prefix, so two clients pointing at different index roots
/// used to meet in one daemon and then read and write the root of whichever
/// client happened to spawn it. Hashing the outcome also survives a later change
/// to that chain.
fn key_from_parts(
    data_dir: &Path,
    exe: &Path,
    exe_len: u64,
    exe_mtime: u128,
    env: &[(OsString, OsString)],
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data_dir.as_os_str().as_encoded_bytes());
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

/// The two parts of the key that name the binary: its size and mtime.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BinaryIdentity {
    pub len: u64,
    pub mtime_ns: u128,
}

impl BinaryIdentity {
    fn at(path: &Path) -> Option<Self> {
        let meta = fs::metadata(path).ok()?;
        let mtime_ns = meta
            .modified()
            .ok()?
            .duration_since(UNIX_EPOCH)
            .ok()?
            .as_nanos();
        Some(Self {
            len: meta.len(),
            mtime_ns,
        })
    }
}

/// Where this process was started from, and what it runs.
///
/// Captured once, at the first call — `main` makes that call before anything
/// else — because both halves drift. After `mv new rust-code-mcp` the path
/// names the new binary while this process still runs the old image, and
/// `current_exe()` then names the old image's new name (`….prev`, or
/// `… (deleted)`). The path is what a client spawns successors from and keys
/// sockets by; the image is what a daemon compares that path against.
#[derive(Debug)]
pub struct Launch {
    exe: PathBuf,
    running: Option<BinaryIdentity>,
}

static LAUNCH: OnceLock<Launch> = OnceLock::new();

pub fn launch() -> io::Result<&'static Launch> {
    if let Some(launch) = LAUNCH.get() {
        return Ok(launch);
    }
    let exe = std::env::current_exe()?;
    // `/proc/self/exe` resolves to the inode being executed, however it has
    // been renamed or unlinked since; without procfs, the path as it is now.
    let running =
        BinaryIdentity::at(Path::new("/proc/self/exe")).or_else(|| BinaryIdentity::at(&exe));
    Ok(LAUNCH.get_or_init(|| Launch { exe, running }))
}

impl Launch {
    /// The binary now at the launch path, when it is readable and is not the
    /// one this process runs. `None` while the path is missing: `mv a b; mv c a`
    /// passes through that state, and retiring there would retire every daemon
    /// for a binary that is about to come back.
    pub fn replacement(&self) -> Option<BinaryIdentity> {
        let running = self.running?;
        BinaryIdentity::at(&self.exe).filter(|now| *now != running)
    }
}

/// `(dev, ino)` of the socket file at `path`: whether the address a client
/// connected through still belongs to the daemon it reached. A retired daemon
/// unlinks its socket, and a successor binds a new one.
fn socket_file_id(path: &Path) -> Option<(u64, u64)> {
    fs::symlink_metadata(path)
        .ok()
        .filter(|meta| meta.file_type().is_socket())
        .map(|meta| (meta.dev(), meta.ino()))
}

fn lock_path(socket: &Path) -> PathBuf {
    socket.with_extension("lock")
}

/// The log a daemon on `socket` writes its stderr into.
///
/// The default socket lives on tmpfs (`$XDG_RUNTIME_DIR`), and its log used to
/// sit next to it — so a reboot erased the only account of what the daemon
/// was doing before it. Measured 2026-10-29: a planned reboot mid-soak took
/// the GPU-session ledger with it, and a 4 GB step in card memory could no
/// longer be attributed. The log of a default socket therefore goes to the
/// persistent state directory, under the same key.
///
/// A socket somewhere else (`--socket`, `RMC_DAEMON_DIR`) keeps its log beside
/// it: the user named that place, and two unrelated `--socket <dir>/x.sock`
/// must not share one `x.log` in the state directory.
fn log_path(socket: &Path) -> PathBuf {
    let default_socket = std::env::var_os(DAEMON_DIR_ENV).is_none_or(|d| d.is_empty())
        && socket_dir().is_ok_and(|dir| socket.parent() == Some(dir.as_path()));
    let state_dir = resolve_state_dir(
        std::env::var("XDG_STATE_HOME").ok().as_deref(),
        std::env::var("HOME").ok().as_deref(),
    );
    resolve_log_path(socket, default_socket, state_dir.as_deref())
}

/// `$XDG_STATE_HOME/rust-code-mcp`, else `$HOME/.local/state/rust-code-mcp`
/// (the XDG default); `None` without either — then the log stays by the socket.
fn resolve_state_dir(xdg_state: Option<&str>, home: Option<&str>) -> Option<PathBuf> {
    if let Some(dir) = xdg_state.filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir).join("rust-code-mcp"));
    }
    home.filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".local/state/rust-code-mcp"))
}

/// The pure part of [`log_path`], for the same reason as [`resolve_socket_dir`].
fn resolve_log_path(socket: &Path, default_socket: bool, state_dir: Option<&Path>) -> PathBuf {
    match (default_socket, state_dir, socket.file_name()) {
        (true, Some(dir), Some(name)) => dir.join(name).with_extension("log"),
        _ => socket.with_extension("log"),
    }
}

/// Logs untouched this long are deleted on the next spawn. On tmpfs a reboot
/// did this; on disk every rebuild leaves a log under a new key, and nothing
/// else would ever remove them. Two weeks covers any investigation that still
/// needs a dead daemon's log.
const LOG_KEEP: Duration = Duration::from_secs(14 * 24 * 3600);

/// Remove `*.log` / `*.log.1` in `dir` last written more than `keep` ago.
/// Failures are ignored, as in [`rotate_log_if_large`].
fn prune_old_logs(dir: &Path, keep: Duration) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_log = path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.ends_with(".log") || n.ends_with(".log.1"));
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .is_ok_and(|mtime| mtime.elapsed().is_ok_and(|age| age > keep));
        if is_log && stale {
            let _ = fs::remove_file(path);
        }
    }
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
/// `idle` is this client's `--idle-secs`, passed on to a daemon it starts.
/// `follow_key` — see [`Mode::Client`].
pub async fn run_client(
    socket: &Path,
    idle: Option<Duration>,
    follow_key: bool,
) -> Result<bool, BoxError> {
    let mut dialer = SocketDialer {
        socket: socket.to_path_buf(),
        idle,
        follow_key,
        connected: None,
        retry_after: None,
    };
    let Some(stream) = relay::Dialer::dial(&mut dialer).await else {
        return Ok(false);
    };
    // The verdict is the relay's, not ours: a session where nothing was
    // exchanged can still be served in-process, and discarding that here
    // would report an empty session as a completed one.
    relay::relay(
        tokio::io::stdin(),
        tokio::io::stdout(),
        stream,
        &mut dialer,
        CLIENT_TICK,
        DRAIN_AFTER_EOF,
    )
    .await
}

/// The socket this client reached, as it was when it connected.
struct Connected {
    path: PathBuf,
    file: Option<(u64, u64)>,
}

/// The real [`relay::Dialer`]: the socket of this configuration, its daemon
/// started on demand.
struct SocketDialer {
    /// `--socket`, or the key's path at startup.
    socket: PathBuf,
    idle: Option<Duration>,
    follow_key: bool,
    connected: Option<Connected>,
    /// Set after a failed dial: no move is attempted before it.
    retry_after: Option<Instant>,
}

impl SocketDialer {
    /// The socket a new connection should go to. With `follow_key` it is
    /// recomputed, because a rebuilt binary has a different key; `None` while
    /// the binary is unreadable — mid-swap, nothing to follow yet.
    fn target(&self) -> Option<PathBuf> {
        if !self.follow_key {
            return Some(self.socket.clone());
        }
        let exe = &launch().ok()?.exe;
        BinaryIdentity::at(exe)?;
        default_socket_path().ok()
    }
}

impl relay::Dialer for SocketDialer {
    async fn dial(&mut self) -> Option<UnixStream> {
        let socket = self.target().unwrap_or_else(|| self.socket.clone());
        let stream = connect_or_spawn(&socket, self.idle).await;
        match &stream {
            Some(_) => {
                self.retry_after = None;
                self.connected = Some(Connected {
                    file: socket_file_id(&socket),
                    path: socket,
                });
            }
            None => self.retry_after = Some(Instant::now() + MOVE_RETRY),
        }
        stream
    }

    fn superseded(&mut self) -> bool {
        if self.retry_after.is_some_and(|at| Instant::now() < at) {
            return false;
        }
        let Some(connected) = &self.connected else {
            return false;
        };
        if self.target().is_some_and(|target| target != connected.path) {
            return true;
        }
        socket_file_id(&connected.path) != connected.file
    }
}

/// Reach the daemon listening on `socket`, starting one when nobody does.
async fn connect_or_spawn(socket: &Path, idle: Option<Duration>) -> Option<UnixStream> {
    if let Some(stream) = try_connect(socket).await {
        tracing::info!("connected to shared daemon at {}", socket.display());
        return Some(stream);
    }

    if let Some(parent) = socket.parent() {
        if let Err(e) = ensure_dir(parent) {
            tracing::warn!("socket dir {} unusable: {e}", parent.display());
            return None;
        }
    }

    let lock = match SpawnLock::acquire(&lock_path(socket)) {
        Ok(lock) => lock,
        Err(e) => {
            tracing::warn!("spawn lock unavailable: {e}");
            return None;
        }
    };

    // Re-check under the lock: a daemon may have come up while we waited for it.
    let stream = match try_connect(socket).await {
        Some(stream) => Some(stream),
        None => {
            // Nobody answers, so this is the corpse of a daemon that died
            // without cleaning up: remove it, because binding over an existing
            // file fails with EADDRINUSE.
            //
            // A socket only. `--socket` can name anything, and a mistyped path
            // must never make this delete someone else's file — the check is
            // what separates "clear a stale address" from "rm whatever is
            // there". `symlink_metadata`, so a symlink is judged as a symlink
            // rather than as whatever it points at.
            match fs::symlink_metadata(socket) {
                Ok(meta) if meta.file_type().is_socket() => {
                    if let Err(e) = fs::remove_file(socket) {
                        tracing::warn!("cannot remove stale {}: {e}", socket.display());
                        return None;
                    }
                }
                Ok(_) => {
                    tracing::warn!("{} exists and is not a socket", socket.display());
                    return None;
                }
                // Nothing there: the ordinary first-start case.
                Err(_) => {}
            }
            match spawn_daemon(socket, idle) {
                Ok(child) => wait_for_daemon(socket, child).await,
                Err(e) => {
                    tracing::warn!("failed to spawn daemon: {e}");
                    None
                }
            }
        }
    };
    drop(lock);
    stream
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
    open_log_at(&log_path(socket))
}

/// [`open_daemon_log`] for an already resolved path: creates its directory
/// (the state directory may not exist yet), prunes old logs there, rotates.
fn open_log_at(path: &Path) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        ensure_dir(dir)?;
        prune_old_logs(dir, LOG_KEEP);
    }
    rotate_log_if_large(path);
    OpenOptions::new().create(true).append(true).open(path)
}

/// Move this daemon's stderr from the legacy log beside the socket to
/// [`log_path`], when the two differ.
///
/// Whoever spawns a daemon opens its log, and that is a client — usually one
/// of a session started before the binary was replaced, running the old code
/// that still put the log on tmpfs. Measured 2026-10-29: the first daemon of
/// the build that moved logs to disk was spawned by such a client and logged
/// to `/run/user` anyway. So the daemon checks for itself.
///
/// Only a stderr that IS the legacy file is moved: a daemon run by hand in a
/// terminal keeps its terminal. Failures leave stderr where it was — a log on
/// tmpfs is still better than none.
fn adopt_disk_log(socket: &Path) {
    let legacy = socket.with_extension("log");
    let target = log_path(socket);
    if target == legacy {
        return;
    }
    let Ok(legacy_meta) = fs::metadata(&legacy) else {
        return;
    };
    let stderr = io::stderr();
    let Ok(stderr_meta) = stderr
        .as_fd()
        .try_clone_to_owned()
        .map(File::from)
        .and_then(|f| f.metadata())
    else {
        return;
    };
    if (stderr_meta.dev(), stderr_meta.ino()) != (legacy_meta.dev(), legacy_meta.ino()) {
        return;
    }
    let Ok(log) = open_log_at(&target) else {
        return;
    };
    tracing::info!(
        "pid {}: log continues in {}",
        std::process::id(),
        target.display()
    );
    // SAFETY: both descriptors are open for the duration of the call; dup2
    // atomically replaces fd 2, so concurrent writers see either file.
    unsafe {
        libc::dup2(log.as_raw_fd(), libc::STDERR_FILENO);
    }
}

/// Start the daemon for `socket`, forwarding `--idle-secs` when the client was
/// given one. Without that forwarding the flag was parsed and dropped: only the
/// environment variable reached the daemon, by inheritance, so `--idle-secs 5`
/// through the ordinary client path did nothing at all.
///
/// From the launch path, not `current_exe()`: a client moving its session off
/// a replaced binary must start the binary that is there NOW, and
/// `current_exe()` by then names the old image (`….prev`, `… (deleted)`).
fn spawn_daemon(socket: &Path, idle: Option<Duration>) -> io::Result<Child> {
    let exe = &launch()?.exe;
    let log = open_daemon_log(socket)?;

    let mut cmd = Command::new(exe);
    cmd.arg("--daemon").arg("--socket").arg(socket);
    if let Some(idle) = idle {
        cmd.arg("--idle-secs").arg(idle.as_secs().to_string());
    }
    cmd.stdin(Stdio::null())
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

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

async fn unload_analysis_contexts(
    runtime: &ServerRuntime,
    retiring: bool,
    rss_mb: u64,
    available_mb: Option<u64>,
    limits: WatchdogLimits,
) {
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
    tracing::warn!(
        "unloaded {} project(s); RSS {:?} -> {:?} KiB, {:?} KiB released",
        report.semantic_projects_cleared,
        report.memory.rss_kib_before,
        report.memory.rss_kib_after,
        report.memory.released_kib,
    );
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
    adopt_disk_log(socket);
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
    let trim_growth_mb = env_u64(TRIM_GROWTH_ENV, DEFAULT_TRIM_GROWTH_MB);
    tracing::info!(
        "allocator trim: whenever RSS grows {} MB above what the last trim left, at most every {:?} ({} to change; 0 disables)",
        trim_growth_mb,
        GROWTH_TRIM_COOLDOWN,
        TRIM_GROWTH_ENV,
    );
    let gpu_idle = Duration::from_secs(env_u64(GPU_IDLE_ENV, DEFAULT_GPU_IDLE_SECS));
    tracing::info!(
        "GPU embedding sessions: released after {:?} unused ({} to change; 0 keeps them), VRAM budget {:?} ({})",
        gpu_idle,
        GPU_IDLE_ENV,
        gpu_vram_budget(),
        "RMC_VRAM_BUDGET_MB",
    );
    let mut last_gc = SystemTime::now();
    let mut last_unload: Option<SystemTime> = None;
    let mut last_trim: Option<SystemTime> = None;
    // RSS right after the last trim of either kind; startup's before the first.
    let mut trim_baseline_mb = MemoryReading::now().map_or(0, |reading| reading.rss_mb);
    // Once retiring, the socket file is gone and belongs to whoever binds it
    // next — this flag keeps the exit path from deleting a successor's socket.
    // `retiring_since` is what bounds the wait; see `retire_grace_expired`.
    let mut retiring = false;
    let mut retiring_since: Option<SystemTime> = None;
    let mut last_logged_rss_mb: Option<u64> = None;

    // Signals must reach the same exit path as an idle timeout. A daemon killed
    // outright leaves its socket file behind; clients survive that (they clear it
    // and start a new one), but `--print-socket` plus `ls` then point at an
    // address where nobody listens — diagnostics lying exactly when consulted.
    record_sigterm_sender();
    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;

    let launch = launch()?;
    tracing::info!(
        "binary watch: {} (running {:?}); a different binary there retires this daemon",
        launch.exe.display(),
        launch.running,
    );

    loop {
        let accepted = tokio::select! {
            accepted = listener.accept() => Some(accepted),
            _ = tokio::time::sleep(IDLE_TICK) => None,
            _ = sigterm.recv() => {
                tracing::info!("SIGTERM from {}, shutting down", describe_sigterm_sender());
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
                    // The mark BEFORE the count, never after. The idle loop reads
                    // the counter and the mark separately, so publishing zero
                    // first leaves a window where it sees "no clients, idle since
                    // the first connect" — a long session then ends in an
                    // immediate exit, and the next client pays to start the
                    // daemon again. Storing on every departure rather than only
                    // the last is harmless: the mark is read only at zero, and
                    // "since the last client left" is what it means.
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

        // Memory watchdog. Runs on the same tick as the idle check, so it costs
        // one `/proc/self/status` and one `/proc/meminfo` read every 15 s and
        // nothing else.
        if let Some(reading) = MemoryReading::now() {
            let since_unload = last_unload.and_then(|at| at.elapsed().ok());
            let since_trim = last_trim.and_then(|at| at.elapsed().ok());
            // Blocking pool: the trim, when the tick asks for one, walks every
            // arena and took 11.6 s on a 25 GB heap. The accept loop waits for it
            // the same way it already waits for an unload.
            let tick = tokio::task::spawn_blocking(move || {
                watchdog_tick(reading, limits, since_unload, since_trim, retiring, || {
                    release_free_memory();
                    // Readable a moment ago; if it no longer is, deciding on the
                    // old reading reproduces the pre-trim behaviour exactly.
                    MemoryReading::now().unwrap_or(reading)
                })
            })
            .await
            .expect("watchdog tick is pure apart from the trim and cannot panic");
            let MemoryReading {
                rss_mb,
                available_mb,
            } = match tick.trimmed {
                Some((before, after)) => {
                    last_trim = Some(SystemTime::now());
                    trim_baseline_mb = after.rss_mb;
                    tracing::info!(
                        "trimmed the allocator before acting on the memory limits: RSS {} -> {} MB; decided {:?}",
                        before.rss_mb,
                        after.rss_mb,
                        tick.action
                    );
                    after
                }
                None if growth_trim_is_due(
                    reading.rss_mb,
                    trim_baseline_mb,
                    trim_growth_mb,
                    since_trim,
                ) =>
                {
                    // Same pool and the same wait as the pressure trim above.
                    let (after, took) = tokio::task::spawn_blocking(move || {
                        let started = Instant::now();
                        release_free_memory();
                        (MemoryReading::now().unwrap_or(reading), started.elapsed())
                    })
                    .await
                    .expect("an allocator trim cannot panic");
                    last_trim = Some(SystemTime::now());
                    tracing::info!(
                        "trimmed the allocator on growth: RSS {} -> {} MB in {:.1}s ({} MB above the last trim's {} MB)",
                        reading.rss_mb,
                        after.rss_mb,
                        took.as_secs_f64(),
                        reading.rss_mb - trim_baseline_mb,
                        trim_baseline_mb,
                    );
                    trim_baseline_mb = after.rss_mb;
                    after
                }
                None => reading,
            };
            // Memory returned any other way — an unload, `clear_runtime` — moves
            // the baseline down with it, or growth would be measured from a level
            // the daemon has left.
            trim_baseline_mb = trim_baseline_mb.min(rss_mb);
            if rss_moved_enough(last_logged_rss_mb, rss_mb) {
                tracing::info!(
                    "mem: RSS moved {} -> {} MB ({} live client(s), {}retired; machine has {:?} MB available)",
                    last_logged_rss_mb.map_or_else(|| "start".to_string(), |mb| mb.to_string()),
                    rss_mb,
                    live.load(Ordering::SeqCst),
                    if retiring { "" } else { "not " },
                    available_mb,
                );
                last_logged_rss_mb = Some(rss_mb);
            }
            match tick.action {
                WatchdogAction::None => {}
                WatchdogAction::Unload => {
                    // Which of the two reasons fired is worth naming: "RSS 3 GB,
                    // unloading" would read as a bug in the threshold rather
                    // than as the machine-wide floor doing its job.
                    // `retiring` is named too: a retired daemon unloading at 21 GB
                    // reads as a broken soft limit unless the line says which
                    // daemon it came from, and both daemons write to one log file.
                    unload_analysis_contexts(runtime, retiring, rss_mb, available_mb, limits).await;
                    last_unload = Some(SystemTime::now());
                }
                WatchdogAction::RetireAndUnload => {
                    tracing::warn!(
                        "RSS {} MB is over the {} MB hard limit; retiring and unloading analysis contexts: new clients will start a fresh daemon, current ones finish here",
                        rss_mb,
                        limits.hard_mb
                    );
                    // First, before the unload below: a retired daemon must not
                    // load an analysis again. The draining sessions otherwise
                    // reload `Full` on their next question, and the unload buys
                    // five minutes instead of the memory (2026-09-22: five
                    // reloads to 15–19 GB over one grace period, next to a
                    // successor growing its own copy).
                    runtime.state().refuse_new_analyses();
                    // And no more background sync. The successor tracks the
                    // same directories and keeps their index current; a retired
                    // daemon syncing alongside it re-embeds every changed file a
                    // second time and maintains the same vector table in
                    // parallel. Measured 2026-10-19: two interleaved sync series
                    // in one log, the retired daemon's running until its
                    // deadline, while it sat at 25 GB with no analysis loaded.
                    // The background sync is the only task on the runtime's
                    // shutdown channel; a sync already in flight finishes first.
                    runtime.request_shutdown();
                    // Unlinking is what makes this graceful: the address stops
                    // resolving to us, so the next client spawns a successor,
                    // while the connections already open keep working.
                    let _ = fs::remove_file(socket);
                    retiring = true;
                    retiring_since = Some(SystemTime::now());
                    // Retirement changes the economics of unloading: the
                    // daemon accepts no new work and a successor is about to
                    // load its own context. Do not carry an old five-minute
                    // cooldown into that state; release the retired daemon's
                    // contexts on this very tick.
                    unload_analysis_contexts(runtime, true, rss_mb, available_mb, limits).await;
                    last_unload = Some(SystemTime::now());
                }
            }
        }

        // Shared GPU sessions nobody has run for `gpu_idle` go back to the card.
        // A retired daemon whose sync has stopped lets go of every idle one at
        // once: its successor is building its own on the same card, and the
        // two together are what the budget's default half-card split is about.
        let gpu_max_idle = gpu_session_max_idle(
            retiring,
            runtime.state().background_sync_running(),
            gpu_idle,
        );
        if let Some(max_idle) = gpu_max_idle {
            // Blocking pool: dropping a session frees device memory and reads
            // the VRAM meters.
            tokio::task::spawn_blocking(move || release_idle_gpu_sessions(max_idle))
                .await
                .expect("releasing idle GPU sessions does not panic");
        }

        // A rebuilt binary at the launch path: every client of this daemon is
        // the same build and moves to the daemon of the new one between
        // requests (`relay`), so all this daemon has to do is stop being found
        // and stop working in the background. Not refusing analyses and not
        // unloading, unlike the hard-limit retirement: a session's request can
        // still land here in the tick before its client moves, and it should be
        // answered rather than refused.
        if !retiring {
            if let Some(now) = launch.replacement() {
                tracing::warn!(
                    "{} was replaced ({:?} -> {:?}); retiring: clients move to the new binary's daemon between requests, background sync stops",
                    launch.exe.display(),
                    launch.running,
                    now,
                );
                runtime.request_shutdown();
                let _ = fs::remove_file(socket);
                retiring = true;
                retiring_since = Some(SystemTime::now());
            }
        }

        // Garbage collection, on its own timer rather than on the watchdog's
        // thresholds: its job is to keep the working set from growing in the
        // first place, so waiting for the soft limit would be waiting for the
        // problem it prevents. Kept running while retiring too — a retired
        // daemon can sit here for the whole grace period holding what pushed it
        // over the hard limit.
        //
        // A sweep needs every analysis out of rust-analyzer, and it does not
        // wait for that (`collect_garbage`): while one runs, the sweep is
        // skipped and tried again on the next tick. Waiting here also stopped
        // this loop from accepting connections — for the rest of a load, minutes.
        if should_collect_garbage(last_gc.elapsed().unwrap_or_default(), gc_interval) {
            match runtime.state().collect_garbage().await {
                Some(collected) => {
                    last_gc = SystemTime::now();
                    if collected > 0 {
                        tracing::info!("collected garbage in {} loaded project(s)", collected);
                    }
                }
                None => tracing::debug!(
                    "garbage collection deferred: an analysis is in flight; retrying in {:?}",
                    IDLE_TICK
                ),
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

/// Pid of the last SIGTERM's sender; 0 — none seen, or not knowable here.
static SIGTERM_SENDER: AtomicI32 = AtomicI32::new(0);

/// Remember who sends SIGTERM, beside tokio's own handler.
///
/// 2026-10-21 23:54 a daemon got SIGTERM from outside and nobody could say
/// from whom: no journal entry, no audit. The sender's pid is in the
/// `siginfo_t` the kernel hands a `SA_SIGINFO` handler, and signal-hook's
/// registry (which tokio's own handler goes through) runs every action
/// registered for a signal, so this one only adds a recorder.
fn record_sigterm_sender() {
    #[cfg(target_os = "linux")]
    {
        // SAFETY: the action stores into an atomic and nothing else, which is
        // async-signal-safe; `si_pid` reads a field the kernel filled in.
        let registered = unsafe {
            signal_hook_registry::register_sigaction(libc::SIGTERM, |info: &libc::siginfo_t| {
                SIGTERM_SENDER.store(info.si_pid(), Ordering::Relaxed);
            })
        };
        if let Err(e) = registered {
            tracing::warn!("cannot record SIGTERM senders: {e}");
        }
    }
}

/// `pid 123 (comm, parent 45): cmdline`, read from /proc while the sender may
/// still be alive; whatever is gone by then is left out.
fn describe_sigterm_sender() -> String {
    let pid = SIGTERM_SENDER.load(Ordering::Relaxed);
    if pid <= 0 {
        return "an unknown sender".to_string();
    }
    let proc_dir = PathBuf::from(format!("/proc/{pid}"));
    let comm = fs::read_to_string(proc_dir.join("comm")).unwrap_or_default();
    let parent = fs::read_to_string(proc_dir.join("status"))
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("PPid:").map(|p| p.trim().to_string()))
        })
        .unwrap_or_else(|| "?".to_string());
    let cmdline: String = fs::read(proc_dir.join("cmdline"))
        .map(|raw| String::from_utf8_lossy(&raw).replace('\0', " "))
        .unwrap_or_default()
        .chars()
        .take(300)
        .collect();
    format!(
        "pid {pid} ({}, parent {parent}): {}",
        comm.trim(),
        cmdline.trim()
    )
}

/// How long a daemon that has left its accept loop gives the runtime to wind
/// down before it exits anyway. Twice the task wait inside, so the clear after
/// it gets the same again.
pub const EXIT_BUDGET: Duration = Duration::from_secs(20);

/// Wind the runtime down after [`run_daemon`] returned, for at most `budget`.
/// `None` — the budget ran out and the caller must exit regardless.
///
/// The bound is the point. By the time this runs the socket file is gone, so
/// the next client already starts a successor; a daemon that lingers here
/// holds its whole analysis next to that successor's fresh copy. Measured
/// 2026-10-21: a daemon got SIGTERM with a large workspace loaded and an
/// incremental sync re-embedding hundreds of files; the successor bound the
/// address seven minutes later, the old process was still indexing at 14 GB
/// RSS beside it, and the successor aborted inside MIGraphX while both
/// embedded on one GPU. `shutdown_gracefully` aborts its task after 10 s, but its
/// `clear(All)` takes `lock_all`, which waits for every workspace lock — and
/// an in-flight sync (`SyncManager`) or a client's tool call holds one for as
/// long as its work takes.
pub async fn shutdown_within(
    runtime: &ServerRuntime,
    budget: Duration,
) -> Option<RuntimeTaskReport> {
    tokio::time::timeout(budget, runtime.shutdown_gracefully(budget / 2))
        .await
        .ok()
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

    fn reading(rss_mb: u64) -> MemoryReading {
        MemoryReading {
            rss_mb,
            available_mb: ROOMY,
        }
    }

    fn no_trim_expected() -> MemoryReading {
        panic!("a tick that has nothing to settle must not pay for a trim")
    }

    /// A healthy tick costs two `/proc` reads, never an allocator walk.
    #[test]
    fn a_healthy_tick_does_not_trim() {
        let tick = watchdog_tick(reading(3000), LIMITS, None, None, false, no_trim_expected);

        assert_eq!(
            tick,
            WatchdogTick {
                action: WatchdogAction::None,
                trimmed: None
            }
        );
    }

    /// The incident: RSS over the hard limit that was mostly freed heap. The
    /// raw reading retires the daemon; after the trim there is nothing to do.
    #[test]
    fn freed_heap_alone_does_not_retire_the_daemon() {
        assert_eq!(
            decide_watchdog_action(12000, ROOMY, LIMITS, None, false),
            WatchdogAction::RetireAndUnload,
            "precondition: the raw reading is past the hard limit"
        );

        let tick = watchdog_tick(reading(12000), LIMITS, None, None, false, || reading(3000));

        assert_eq!(tick.action, WatchdogAction::None);
        assert_eq!(tick.trimmed, Some((reading(12000), reading(3000))));
    }

    /// Freed heap alone must not unload a live context either — that is the
    /// load/unload loop, one layer down from retirement.
    #[test]
    fn freed_heap_alone_does_not_unload() {
        let tick = watchdog_tick(reading(5000), LIMITS, None, None, false, || reading(3000));

        assert_eq!(tick.action, WatchdogAction::None);
    }

    /// Trimmed below hard but still above soft: unload, the cheaper remedy,
    /// instead of retiring.
    #[test]
    fn a_trim_that_lands_between_the_limits_unloads_instead_of_retiring() {
        let tick = watchdog_tick(reading(12000), LIMITS, None, None, false, || reading(5000));

        assert_eq!(tick.action, WatchdogAction::Unload);
    }

    /// Positive control: memory a trim cannot return still retires the daemon.
    #[test]
    fn memory_that_a_trim_cannot_return_still_retires() {
        let tick = watchdog_tick(reading(12000), LIMITS, None, None, false, || reading(11900));

        assert_eq!(tick.action, WatchdogAction::RetireAndUnload);
    }

    /// Within the trim cooldown the raw reading decides, so RSS hovering over
    /// a limit cannot buy a multi-second arena walk every 15 s.
    #[test]
    fn trims_are_rate_limited() {
        let tick = watchdog_tick(
            reading(12000),
            LIMITS,
            None,
            Some(Duration::from_secs(10)),
            false,
            no_trim_expected,
        );

        assert_eq!(
            tick,
            WatchdogTick {
                action: WatchdogAction::RetireAndUnload,
                trimmed: None
            }
        );

        let tick = watchdog_tick(
            reading(12000),
            LIMITS,
            None,
            Some(TRIM_COOLDOWN),
            false,
            || reading(3000),
        );
        assert_eq!(tick.action, WatchdogAction::None);
    }

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
            WatchdogAction::RetireAndUnload
        );
    }

    /// Retiring twice would unlink a socket that by then belongs to the
    /// successor daemon.
    #[test]
    fn a_retiring_daemon_never_retires_again() {
        assert_ne!(
            decide_watchdog_action(11000, ROOMY, LIMITS, None, true),
            WatchdogAction::RetireAndUnload
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

    /// A retired daemon finishing its last sync pass keeps the GPU session
    /// between files; once the sync stops, every idle session goes at once.
    #[test]
    fn a_retired_daemon_keeps_its_gpu_session_until_the_sync_stops() {
        let idle = Duration::from_secs(900);
        assert_eq!(gpu_session_max_idle(true, true, idle), Some(idle));
        assert_eq!(gpu_session_max_idle(true, true, Duration::ZERO), None);
        assert_eq!(
            gpu_session_max_idle(true, false, idle),
            Some(Duration::ZERO)
        );
        assert_eq!(
            gpu_session_max_idle(true, false, Duration::ZERO),
            Some(Duration::ZERO)
        );
        assert_eq!(gpu_session_max_idle(false, true, idle), Some(idle));
        assert_eq!(gpu_session_max_idle(false, false, Duration::ZERO), None);
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
            decide_watchdog_action(
                3000,
                Some(2000),
                LIMITS,
                Some(Duration::from_secs(60)),
                false
            ),
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

    /// A healthy daemon serving one large workspace, measured live 2026-09-22:
    /// the typical RSS of a `Full` context of that workspace (range 18–26 GB
    /// over twenty load/unload cycles), fixed startup cost included. `Full` is
    /// the only kind anything loads now.
    ///
    /// It was 5300 — a `Fast` load of the same workspace — and the soft default
    /// was derived from that, so this test passed for months while the live
    /// daemon unloaded its working point on every cooldown. A working point in a
    /// test is only as good as the load kind it was measured on.
    const MEASURED_HEALTHY_MB: u64 = 23 * 1024;

    /// The defect this closes, twice: the first `DEFAULT_RSS_SOFT_MB` was 4096,
    /// chosen from the workspace cost alone and forgetting the fixed startup
    /// floor; the second was 12288, priced at `Fast` loads when everything
    /// loads `Full`. Both sat below the normal working point, so a healthy
    /// daemon unloaded rust-analyzer on every cooldown and reloaded it on the
    /// next question. A default that fires during ordinary work is worse than
    /// no watchdog at all.
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

    /// A complete cross-crate/reference query requires the measured `Full`
    /// context. The old 20 GiB default sat inside this normal range and retired
    /// a healthy daemon repeatedly.
    #[test]
    fn defaults_do_not_retire_at_the_measured_full_context_peak() {
        const MEASURED_FULL_PEAK_MB: u64 = 22 * 1024;
        let defaults = WatchdogLimits {
            soft_mb: DEFAULT_RSS_SOFT_MB,
            hard_mb: DEFAULT_RSS_HARD_MB,
            min_available_mb: DEFAULT_MIN_AVAILABLE_MB,
            cooldown: Duration::from_secs(DEFAULT_RSS_COOLDOWN_SECS),
        };

        assert_ne!(
            decide_watchdog_action(MEASURED_FULL_PEAK_MB, ROOMY, defaults, None, false),
            WatchdogAction::RetireAndUnload
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
            WatchdogAction::RetireAndUnload
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

    /// The 2026-10-21 shape: trimmed to 7.7 GB, back at 14.6 GB nine minutes
    /// later with every limit far away. That must trim; growth under the step
    /// must not, and neither may anything inside the cooldown of a trim of
    /// either kind.
    #[test]
    fn growth_above_the_last_trim_trims_without_any_limit() {
        let growth = DEFAULT_TRIM_GROWTH_MB;
        let base = 7_677;
        let later = Some(Duration::from_secs(540));
        let cooldown = GROWTH_TRIM_COOLDOWN;

        assert!(growth_trim_is_due(14_610, base, growth, later));
        assert!(
            growth_trim_is_due(2_100, 30, growth, None),
            "before any trim the baseline is startup's RSS, and no cooldown applies"
        );
        assert!(growth_trim_is_due(base + growth, base, growth, later));
        assert!(!growth_trim_is_due(base + growth - 1, base, growth, later));
        let inside = Some(cooldown - Duration::from_secs(1));
        assert!(!growth_trim_is_due(14_610, base, growth, inside));
        assert!(growth_trim_is_due(14_610, base, growth, Some(cooldown)));
        assert!(
            !growth_trim_is_due(u64::MAX - 1, u64::MAX - 1, growth, None),
            "a baseline near the top must not wrap into a trim on every tick"
        );
    }

    #[test]
    fn zero_growth_disables_the_growth_trim() {
        assert!(!growth_trim_is_due(60_000, 0, 0, None));
    }

    /// The growth trim is paced no faster than the pressure trim it sits
    /// beside: otherwise it would be the one stalling the arenas every minute.
    #[test]
    fn the_growth_trim_is_paced_no_faster_than_the_pressure_trim() {
        assert!(GROWTH_TRIM_COOLDOWN >= TRIM_COOLDOWN);
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

    /// The memory timeline: a baseline first, then only real moves, in
    /// either direction — a drop is as telling as a rise.
    #[test]
    fn rss_is_logged_on_baseline_and_on_gigabyte_moves_only() {
        assert!(
            rss_moved_enough(None, 3000),
            "first reading is the baseline"
        );
        assert!(!rss_moved_enough(Some(3000), 3000 + RSS_LOG_STEP_MB - 1));
        assert!(rss_moved_enough(Some(3000), 3000 + RSS_LOG_STEP_MB));
        assert!(rss_moved_enough(Some(25000), 25000 - RSS_LOG_STEP_MB));
        assert!(!rss_moved_enough(Some(25000), 24500));
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

    /// A workspace lock held past the daemon's exit — an in-flight sync, a
    /// client's tool call — must not hold the process. Without the bound,
    /// `shutdown_gracefully`'s `clear(All)` waits in `lock_all` for as long as
    /// the lock is held: here, forever, which the outer timeout turns into a
    /// failure instead of a hung test.
    #[tokio::test]
    async fn a_held_workspace_lock_cannot_hold_the_exit() {
        let runtime = ServerRuntime::new(3600);
        let held = runtime
            .state()
            .workspace_locks()
            .lock_index_writer(Path::new("/nonexistent/held-by-a-sync"))
            .await;

        let started = Instant::now();
        let report = tokio::time::timeout(
            Duration::from_secs(30),
            shutdown_within(&runtime, Duration::from_millis(200)),
        )
        .await
        .expect("the exit waited for the held workspace lock");
        assert!(
            report.is_none(),
            "a held lock cannot let the shutdown finish"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
        drop(held);
    }

    /// Control for the test above: with nothing held, the runtime winds down
    /// inside the budget and the report comes back — so `None` there is the
    /// lock's doing, not a budget too small for any shutdown.
    #[tokio::test]
    async fn an_idle_runtime_winds_down_inside_the_budget() {
        let runtime = ServerRuntime::new(3600);
        assert!(
            shutdown_within(&runtime, Duration::from_millis(200))
                .await
                .is_some()
        );
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

    /// A client must CARRY `--idle-secs`, because it is the side that starts the
    /// daemon. The flag used to be parsed and then dropped on this path: only
    /// the environment variable reached the daemon, by inheritance, so
    /// `--idle-secs 5` on an ordinary invocation did nothing.
    #[test]
    fn a_client_carries_the_idle_flag_it_was_given() {
        assert_eq!(
            mode_of(&["--client", "--socket", "/tmp/x.sock", "--idle-secs", "7"]),
            Mode::Client {
                socket: PathBuf::from("/tmp/x.sock"),
                idle: Some(Duration::from_secs(7)),
                follow_key: false,
            }
        );
    }

    /// Only a derived socket follows the key: one named by `--socket` stays
    /// where it was put whatever the binary does.
    #[test]
    fn only_a_derived_socket_follows_the_binary() {
        assert!(matches!(
            mode_of(&["--client"]),
            Mode::Client {
                follow_key: true,
                ..
            }
        ));
        assert!(matches!(
            mode_of(&["--client", "--socket", "/tmp/x.sock"]),
            Mode::Client {
                follow_key: false,
                ..
            }
        ));
    }

    /// The daemon's half of a rebuild: a different file at the launch path is a
    /// replacement, the same one is not, and a missing one is not either —
    /// `mv old .prev; mv new old` passes through "missing".
    ///
    /// Mutation that must fail it: compare the path with itself instead of
    /// with the running image, and the rewritten file goes unnoticed.
    #[test]
    fn a_rewritten_launch_path_is_a_replacement() {
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("rust-code-mcp");
        std::fs::write(&exe, b"old build").expect("old binary");
        let launch = Launch {
            exe: exe.clone(),
            running: BinaryIdentity::at(&exe),
        };
        assert_eq!(launch.replacement(), None, "the running binary itself");

        std::fs::remove_file(&exe).expect("mid-swap");
        assert_eq!(
            launch.replacement(),
            None,
            "a missing binary is not a new one"
        );

        std::fs::write(&exe, b"the new build").expect("new binary");
        assert_eq!(launch.replacement(), BinaryIdentity::at(&exe));
    }

    /// And must NOT invent one when the command line was silent: the spawned
    /// daemon inherits `RMC_DAEMON_IDLE_SECS` itself, so passing a resolved
    /// default here would turn "unset" into an explicit argument and hide the
    /// environment from the process that reads it.
    #[test]
    fn a_client_without_the_flag_passes_nothing_on() {
        assert_eq!(
            mode_of(&["--client", "--socket", "/tmp/x.sock"]),
            Mode::Client {
                socket: PathBuf::from("/tmp/x.sock"),
                idle: None,
                follow_key: false,
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

    fn pairs(env: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        env.iter()
            .map(|(name, value)| (OsString::from(name), OsString::from(value)))
            .collect()
    }

    fn key(exe: &str, len: u64, mtime: u128, sync: &str) -> String {
        key_from_parts(
            Path::new("/index-root"),
            Path::new(exe),
            len,
            mtime,
            &pairs(&[(BACKGROUND_SYNC_ENV, sync)]),
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

    /// A directory the daemon did not create keeps the mode its owner gave it.
    ///
    /// The socket directory can be named by the caller (`--socket <dir>/x.sock`,
    /// `RMC_DAEMON_DIR`), and tightening someone's own directory to `0o700` as a
    /// side effect of starting a server is a change nobody asked for.
    #[test]
    fn an_existing_directory_keeps_its_mode() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let dir = temp.path().join("sockets");
        fs::create_dir(&dir).expect("create");
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).expect("chmod");

        ensure_dir(&dir).expect("an owned directory is accepted");

        let mode = fs::metadata(&dir).expect("stat").permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o755,
            "the daemon rewrote a directory it did not create"
        );
    }

    /// A new directory is still created owner-only: the socket is an entry point
    /// into analysing someone's code. This is the positive control for the test
    /// above — without it, "never chmod anything" would pass that one.
    #[test]
    fn a_created_directory_is_owner_only() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let dir = temp.path().join("fresh");

        ensure_dir(&dir).expect("a fresh directory is created");

        let mode = fs::metadata(&dir).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }

    /// A symlink decides where the socket really lands, so it must be seen
    /// rather than followed — `create_dir_all` on it succeeds silently.
    #[test]
    fn a_symlinked_directory_is_refused() {
        let temp = tempfile::TempDir::new().expect("temp dir");
        let real = temp.path().join("real");
        let link = temp.path().join("link");
        fs::create_dir(&real).expect("create");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");

        let refusal = ensure_dir(&link).expect_err("a symlinked socket directory must be refused");
        assert!(
            refusal.to_string().contains("is a symlink"),
            "the refusal must say what is wrong: {refusal}"
        );
    }

    /// The index root must split daemons too. It is not an env var here on
    /// purpose: `data_dir()` is resolved through `XDG_DATA_HOME` and then `HOME`,
    /// neither of which carries a keyed prefix, so keying the variables would
    /// have left two clients with different index roots sharing one server —
    /// and that server reads and writes the root of whoever spawned it.
    #[test]
    fn key_depends_on_the_index_root() {
        let one = key_from_parts(Path::new("/roots/a"), Path::new("/bin/mcp"), 10, 20, &[]);
        let other = key_from_parts(Path::new("/roots/b"), Path::new("/bin/mcp"), 10, 20, &[]);
        assert_ne!(one, other, "two index roots must not meet in one daemon");
    }

    /// The policy, stated as three separate claims about which names are keyed.
    ///
    /// Mutation that must fail it: go back to a fixed list of names, and the
    /// openrouter variable stops being seen — which is the state this replaced.
    #[test]
    fn keyed_env_pairs_follows_the_prefix_policy() {
        let collected = keyed_env_pairs(
            pairs(&[
                ("RUST_CODE_MCP_OPENROUTER_CONCURRENCY", "16"),
                ("RMC_EMBEDDING_PROFILE", "local-cpu-small"),
                ("OPENROUTER_API_KEY", "secret"),
                ("LD_LIBRARY_PATH", "/opt/onnx"),
                // Transport knobs: keyed prefix, deliberately excluded.
                ("RMC_DAEMON", "0"),
                ("RMC_DAEMON_DIR", "/run/user/1000"),
                ("RMC_DAEMON_IDLE_SECS", "5"),
                // A startup diagnostic, excluded for the same reason: it changes
                // what the daemon PRINTS, never what it answers.
                ("RMC_EP_CENSUS", "1"),
                // Nothing to do with this server.
                ("PATH", "/usr/bin"),
                ("HOME", "/home/nobody"),
            ])
            .into_iter(),
        );
        let names: Vec<String> = collected
            .iter()
            .map(|(name, _)| name.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            names,
            vec![
                "LD_LIBRARY_PATH",
                "OPENROUTER_API_KEY",
                "RMC_EMBEDDING_PROFILE",
                "RUST_CODE_MCP_OPENROUTER_CONCURRENCY",
            ],
            "keyed set, sorted by name"
        );
    }

    /// A transport knob must NOT split the fleet: two clients that differ only
    /// in how long a daemon idles are asking the same questions of the same
    /// code, and keying it would double the memory this module exists to save.
    #[test]
    fn transport_knobs_do_not_change_the_key() {
        let with_idle = keyed_env_pairs(pairs(&[("RMC_DAEMON_IDLE_SECS", "5")]).into_iter());
        assert!(
            with_idle.is_empty(),
            "a transport knob leaked into the key: {with_idle:?}"
        );
    }

    /// A startup diagnostic must NOT split the fleet either, and this one is the
    /// case that proved the rule: `RMC_EP_CENSUS` only decides whether the daemon
    /// probes and prints which execution provider runs the graph, yet keying it
    /// gave a client that expanded a different default a whole second daemon —
    /// 1.6 GB of fixed startup for a line of diagnostics.
    ///
    /// Judged through the pair a real fleet splits on, not through one variable
    /// alone: two clients that agree on the profile and differ only in the census
    /// must produce the same keyed set, which is the same thing as one daemon.
    #[test]
    fn a_startup_diagnostic_does_not_change_the_key() {
        let asking = keyed_env_pairs(
            pairs(&[
                ("RMC_EMBEDDING_PROFILE", "local-gpu-bge"),
                ("RMC_EP_CENSUS", "1"),
            ])
            .into_iter(),
        );
        let not_asking = keyed_env_pairs(
            pairs(&[
                ("RMC_EMBEDDING_PROFILE", "local-gpu-bge"),
                ("RMC_EP_CENSUS", "0"),
            ])
            .into_iter(),
        );

        assert_eq!(
            asking, not_asking,
            "the census leaked into the key, so two clients that ask the same questions of the \
             same code land on two daemons"
        );
        assert_eq!(
            asking.len(),
            1,
            "the profile must still be keyed — a test where NOTHING is keyed would pass the \
             equality above while saying nothing: {asking:?}"
        );
    }

    /// The environment arrives in whatever order the OS chose, and two shells
    /// with the same settings must still meet in one daemon.
    #[test]
    fn key_ignores_the_order_the_environment_arrives_in() {
        let forward = keyed_env_pairs(pairs(&[("RMC_A", "1"), ("RMC_B", "2")]).into_iter());
        let backward = keyed_env_pairs(pairs(&[("RMC_B", "2"), ("RMC_A", "1")]).into_iter());
        assert_eq!(forward, backward);
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

    /// The default socket is on tmpfs; its log must not be, or a reboot erases
    /// what the daemon did before it.
    #[test]
    fn a_default_socket_logs_into_the_state_directory() {
        let socket = Path::new("/run/user/1000/rust-code-mcp/c27a480be53c5b37.sock");
        assert_eq!(
            resolve_log_path(
                socket,
                true,
                Some(Path::new("/home/u/.local/state/rust-code-mcp"))
            ),
            PathBuf::from("/home/u/.local/state/rust-code-mcp/c27a480be53c5b37.log"),
        );
        assert_eq!(
            resolve_log_path(
                socket,
                false,
                Some(Path::new("/home/u/.local/state/rust-code-mcp"))
            ),
            PathBuf::from("/run/user/1000/rust-code-mcp/c27a480be53c5b37.log"),
            "a socket the user placed keeps its log beside it"
        );
        assert_eq!(
            resolve_log_path(socket, true, None),
            PathBuf::from("/run/user/1000/rust-code-mcp/c27a480be53c5b37.log"),
            "without a home there is nowhere else to put it"
        );
    }

    #[test]
    fn the_state_directory_follows_xdg_then_home() {
        assert_eq!(
            resolve_state_dir(Some("/s"), Some("/home/u")),
            Some(PathBuf::from("/s/rust-code-mcp"))
        );
        assert_eq!(
            resolve_state_dir(Some(""), Some("/home/u")),
            Some(PathBuf::from("/home/u/.local/state/rust-code-mcp"))
        );
        assert_eq!(resolve_state_dir(None, None), None);
    }

    /// On disk every rebuild leaves a log under a new key; old ones go, fresh
    /// ones and other files stay.
    #[test]
    fn stale_logs_are_pruned_and_fresh_ones_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let old = dir.path().join("old.log");
        let old_rolled = dir.path().join("old.log.1");
        let fresh = dir.path().join("fresh.log");
        let other = dir.path().join("notes.txt");
        for path in [&old, &old_rolled, &fresh, &other] {
            std::fs::write(path, "x").expect("seed");
        }
        let month_ago = std::time::SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
        for path in [&old, &old_rolled, &other] {
            File::options()
                .write(true)
                .open(path)
                .and_then(|f| f.set_modified(month_ago))
                .expect("age the file");
        }

        prune_old_logs(dir.path(), LOG_KEEP);

        assert!(
            !old.exists() && !old_rolled.exists(),
            "stale logs must be removed"
        );
        assert!(fresh.exists(), "a fresh log must stay");
        assert!(other.exists(), "only logs are pruned");
    }
}

// lancedb 0.29's async stack (lance_io::uring + moka::future) pushes the
// auto-trait Send check past the default 128-level recursion limit when
// the sync-manager future is spawned in main. Bump it locally; this is a
// compile-time inference budget, not a runtime cost.
#![recursion_limit = "512"]

// One server per project instead of one per session; see `daemon`. Unix only —
// the transport is a unix socket, other platforms keep the stdio server.
#[cfg(unix)]
mod daemon;
// `--tool-latency`: the no-blocking oracle over daemon logs.
#[cfg(unix)]
mod tool_latency;

// Every Rust allocation is counted by the heap census, which is how the memory
// portrait (`analysis_memory`) tells the allocator's own overhead and C/C++
// allocations apart from the Rust heap; see `rmc_server::mcp::heap_census`.
//
// Under the `mimalloc` feature every allocation in the process — including
// rust-analyzer's salsa database, which is what actually fills the heap — goes
// through mimalloc instead of the system allocator. The reason is in the
// feature's comment in Cargo.toml; the short version is that glibc keeps its
// fragmented arenas and mimalloc gives them back.
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL_ALLOCATOR: rmc_server::mcp::heap_census::CensusAllocator<mimalloc::MiMalloc> =
    rmc_server::mcp::heap_census::CensusAllocator {
        inner: mimalloc::MiMalloc,
        census: &rmc_server::mcp::heap_census::GLOBAL_CENSUS,
    };
#[cfg(not(feature = "mimalloc"))]
#[global_allocator]
static GLOBAL_ALLOCATOR: rmc_server::mcp::heap_census::CensusAllocator<std::alloc::System> =
    rmc_server::mcp::heap_census::CensusAllocator {
        inner: std::alloc::System,
        census: &rmc_server::mcp::heap_census::GLOBAL_CENSUS,
    };

use rmc_server::mcp::{
    BACKGROUND_SYNC_ENABLED_VALUES, BACKGROUND_SYNC_ENV, EMBEDDING_PROFILE_ENV, ServerRuntime,
    automatic_embedding_profile_name, cpu_profile_on_gpu_build_warning, gpu_backends_compiled,
    parse_background_sync_env, resolve_automatic_profile_name, spawn_ep_census_on_startup,
    validate_automatic_profile,
};
use rmc_server::tools::SearchTool;
use rmcp::{ServiceExt, transport::stdio};
use std::time::Duration;
use tracing_subscriber::{self, EnvFilter, fmt::time::FormatTime};

/// Log filter when `RUST_LOG` is unset.
///
/// `rmc_mem` is `rmc_engine::mem_trace::LOG_TARGET`, spelled out because the
/// binary does not depend on `rmc-engine` at runtime (see Cargo.toml); the
/// test `default_filter_enables_memory_lines` keeps the two in step.
///
/// `embeddings::vram` is `rmc_engine::embeddings::VRAM_LOG_TARGET` (GPU
/// session created / first run / granted after a wait / released), kept in
/// step by `default_filter_enables_vram_lines`.
const DEFAULT_LOG_FILTER: &str =
    "warn,rust_code_mcp=info,rmc_server=info,rmc_indexing=info,rmc_mem=info,embeddings::vram=info";

/// The default timestamp followed by `[pid]`.
///
/// Successive and overlapping daemons append to ONE log file per socket key,
/// and a retired daemon keeps writing next to its successor for up to half
/// an hour. Without the pid on every line their sync cycles, watchdog
/// decisions and memory lines interleave indistinguishably — on 2026-10-19 the
/// two daemons could be told apart only by the phase of their 5-minute sync
/// timers.
struct TimeWithPid(u32);

impl FormatTime for TimeWithPid {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        tracing_subscriber::fmt::time::SystemTime.format_time(w)?;
        write!(w, " [{}]", self.0)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Default to WARN for everything, INFO for our own crate. Users who want
    // RA's internal debug logs can set `RUST_LOG=ra_ap_hir=debug,...`.
    //
    // Why this matters: RA emits millions of `tracing::debug!` events during
    // name resolution. With Level::DEBUG enabled globally, the formatter +
    // socket-stderr write pipeline becomes the bottleneck — `build_hypergraph`
    // on a multi-crate workspace went from ~7s to 7+ minutes purely from log
    // formatting overhead. Keep this at WARN unless explicitly overridden.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(DEFAULT_LOG_FILTER)
    });
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_timer(TimeWithPid(std::process::id()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    // Every panic is caught somewhere in a daemon (`rmc_server::panic_boundary`)
    // and the client is told only its text. Where it came from lives here: the
    // default hook writes no backtrace without `RUST_BACKTRACE` — which nobody
    // sets for a spawned daemon, and which would make `anyhow` capture one on
    // every error — and no pid, in a log two daemons share. salsa's
    // cancellations travel by `resume_unwind`, which skips the hook, so this
    // fires only for real panics and the capture costs nothing in between.
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!(target: "panic", "{info}\n{backtrace}");
    }));

    // Resolve the mode before the expensive startup: a client of the shared
    // daemon needs neither a `ServerRuntime`, nor the EP census probe, nor a
    // background sync task — it is a pipe between stdio and the socket.
    #[cfg(unix)]
    let mode = {
        // Before anything can rename the binary under us: the launch path and
        // the running image are what a daemon compares to notice a rebuild.
        if let Err(e) = daemon::launch() {
            tracing::warn!("cannot resolve this binary's path: {e}");
        }
        let args: Vec<String> = std::env::args().skip(1).collect();
        match daemon::resolve_mode(&args) {
            Ok(mode) => mode,
            Err(e) => {
                eprintln!("{e}\n\n{}", daemon::USAGE);
                // Explicit coercion site: `main` returns `Box<dyn Error>` without
                // `Send + Sync`, and without the typed let inference takes the
                // whole body with it.
                let boxed: Box<dyn std::error::Error> = e;
                return Err(boxed);
            }
        }
    };

    // The two modes that serve nothing come first: they answer from the
    // arguments alone, and they are how a host reads back a configuration it
    // got wrong. A rejected profile must not silence them.
    #[cfg(unix)]
    match &mode {
        daemon::Mode::Help => {
            print!("{}", daemon::USAGE);
            return Ok(());
        }
        daemon::Mode::PrintSocket { socket } => {
            println!("{}", socket.display());
            return Ok(());
        }
        daemon::Mode::Bm25Stats { index_dir } => {
            let stats = rmc_indexing::indexing::index_stats::probe_bm25_index(index_dir)
                .map_err(|e| format!("{e:#}"))?;
            println!("{}", serde_json::to_string_pretty(&stats)?);
            return Ok(());
        }
        daemon::Mode::ToolLatency { logs, since, until } => {
            let blocked = tool_latency::run(logs, *since, *until)?;
            if blocked > 0 {
                std::process::exit(1);
            }
            return Ok(());
        }
        daemon::Mode::Client { .. } | daemon::Mode::Daemon { .. } | daemon::Mode::InProcess => {}
    }

    // A statement, not an argument to a log line. `tracing` evaluates arguments
    // only when the callsite level is enabled, so the check that used to live
    // inside `tracing::info!` was skipped under `RUST_LOG=error` — a quiet log
    // level started the server with an unusable profile.
    //
    // Here rather than further down: every mode below this line serves a
    // session, including the client of the shared daemon, which returns from
    // `main` before any of the startup code runs. That is how a typo used to
    // exit 0 through the daemon and exit non-zero with `RMC_DAEMON=0` — one
    // input, two outcomes, chosen by a transport nobody asked about.
    let requested_profile =
        resolve_automatic_profile_name(std::env::var(EMBEDDING_PROFILE_ENV).ok().as_deref());
    if let Err(err) = validate_automatic_profile(&requested_profile) {
        eprintln!("{EMBEDDING_PROFILE_ENV}='{requested_profile}' is not usable: {err}");
        std::process::exit(2);
    }

    #[cfg(unix)]
    match &mode {
        daemon::Mode::Client {
            socket,
            idle,
            follow_key,
        } => {
            // An unreachable daemon never leaves the session without a server:
            // fall through to the previous in-process behaviour.
            match daemon::run_client(socket, *idle, *follow_key).await {
                Ok(true) => return Ok(()),
                Ok(false) => {
                    tracing::info!("shared daemon unavailable; serving this session in-process")
                }
                Err(e) => {
                    // NOT in-process: bytes were already exchanged, so this
                    // session's stdin is partly consumed and a local server
                    // would answer a truncated stream.
                    //
                    // Exit rather than return: `tokio::io::stdin` reads on the
                    // blocking pool, and that read ends only when the host
                    // closes stdin. Returning hands the error to the runtime
                    // drop, which waits for that read — so the failure would
                    // show up as a hang instead of a non-zero exit.
                    eprintln!("shared daemon session failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        daemon::Mode::Daemon { .. }
        | daemon::Mode::InProcess
        | daemon::Mode::Help
        | daemon::Mode::PrintSocket { .. }
        | daemon::Mode::Bm25Stats { .. }
        | daemon::Mode::ToolLatency { .. } => {}
    }

    tracing::info!("Starting MCP Server...");

    // Syncs every 5 minutes (300 seconds).
    let runtime = ServerRuntime::new(300);
    tracing::info!("Created MCP server runtime (5-minute sync interval)");

    let background_sync_env = std::env::var(BACKGROUND_SYNC_ENV).ok();
    let background_sync_enabled = parse_background_sync_env(background_sync_env.as_deref());
    tracing::info!(
        "MCP startup defaults: background sync {} ({}='{}'; enabled only for {}, case-insensitive); automatic/background embedding profile default {}; GPU backends compiled: {}",
        if background_sync_enabled {
            "enabled"
        } else {
            "disabled"
        },
        BACKGROUND_SYNC_ENV,
        background_sync_env.as_deref().unwrap_or("<unset>"),
        BACKGROUND_SYNC_ENABLED_VALUES,
        automatic_embedding_profile_name(),
        gpu_backends_compiled(),
    );

    // Built for GPU but going to compute on CPU: say so out loud. This cannot be
    // a refusal (the build feature does not promise an ORT with the needed EP at runtime), but
    // staying silent is not acceptable either: the difference is ~80x, and it just looks 'slow'.
    if let Some(warning) = cpu_profile_on_gpu_build_warning() {
        tracing::warn!("{warning}");
    }

    // Which execution provider actually runs the graph — behind RMC_EP_CENSUS.
    //
    // Started in the background and NOT awaited. It used to sit right here and
    // block, spending the client's handshake budget: a cold MIGraphX kernel
    // cache costs more (38.7s measured) than a client's connect timeout (30s),
    // so the session that happened to start a cold daemon was left without any
    // of this server's tools. The reasoning, and what this decision costs, are
    // at spawn_ep_census_on_startup.
    let _ep_census = spawn_ep_census_on_startup();

    if background_sync_enabled {
        runtime.start_background_sync();
        tracing::info!("Started background sync task");
    } else {
        tracing::info!(
            "Background sync task disabled; set {}=1 to enable",
            BACKGROUND_SYNC_ENV
        );
    }

    // Daemon: the same runtime, but many connections instead of one stdio pipe.
    #[cfg(unix)]
    if let daemon::Mode::Daemon { socket, idle } = &mode {
        let result = daemon::run_daemon(socket, *idle, &runtime).await;
        match daemon::shutdown_within(&runtime, daemon::EXIT_BUDGET).await {
            Some(shutdown) => {
                tracing::info!("Runtime shutdown after daemon exit: {:?}", shutdown)
            }
            None => tracing::warn!(
                "runtime still winding down after {:?} (an in-flight sync or tool call holds a workspace lock); exiting without it",
                daemon::EXIT_BUDGET
            ),
        }
        // Exit, not return: returning drops the tokio runtime, and that drop
        // waits for every blocking task — an in-flight sync's embedder runs on
        // that pool for minutes. The socket is already gone, so every minute
        // spent here is this process's memory held next to its successor's.
        // An interrupted sync is what a SIGKILL leaves too; the next sync
        // redoes the files whose snapshot was not written.
        if let Err(e) = &result {
            tracing::error!("daemon failed: {e}");
        }
        std::process::exit(if result.is_ok() { 0 } else { 1 });
    }

    let service = match SearchTool::with_server_runtime(&runtime)
        .serve(stdio())
        .await
    {
        Ok(service) => service,
        Err(e) => {
            tracing::error!("serving error: {:?}", e);
            let shutdown = runtime.shutdown_gracefully(Duration::from_secs(10)).await;
            tracing::info!("Runtime shutdown after serve error: {:?}", shutdown);
            return Err(e.into());
        }
    };

    let service_result = service.waiting().await;
    if let Err(e) = &service_result {
        tracing::error!("service wait error: {:?}", e);
    }

    let shutdown = runtime.shutdown_gracefully(Duration::from_secs(10)).await;
    tracing::info!("Runtime shutdown complete: {:?}", shutdown);

    service_result?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::DEFAULT_LOG_FILTER;

    /// Without this the `mem:` lines are filtered out silently — exactly what
    /// the first live probe of `mem_trace` showed.
    #[test]
    fn default_filter_enables_memory_lines() {
        let directive = format!("{}=info", rmc_engine::mem_trace::LOG_TARGET);
        assert!(
            DEFAULT_LOG_FILTER.split(',').any(|d| d == directive),
            "{DEFAULT_LOG_FILTER:?} must contain {directive:?}"
        );
    }

    /// The same for the GPU ledger: its lines are how a session count is read
    /// from the log after a crash, when `runtime_status` is gone.
    #[test]
    fn default_filter_enables_vram_lines() {
        let directive = format!("{}=info", rmc_engine::embeddings::VRAM_LOG_TARGET);
        assert!(
            DEFAULT_LOG_FILTER.split(',').any(|d| d == directive),
            "{DEFAULT_LOG_FILTER:?} must contain {directive:?}"
        );
    }
}

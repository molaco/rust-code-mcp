// lancedb 0.29's async stack (lance_io::uring + moka::future) pushes the
// auto-trait Send check past the default 128-level recursion limit when
// the sync-manager future is spawned in main. Bump it locally; this is a
// compile-time inference budget, not a runtime cost.
#![recursion_limit = "512"]

// One server per project instead of one per session; see `daemon`. Unix only —
// the transport is a unix socket, other platforms keep the stdio server.
#[cfg(unix)]
mod daemon;

use rmc_server::mcp::{
    cuda_capable_features_compiled, install_automatic_backend, parse_background_sync_env,
    resolve_automatic_profile_name, validate_automatic_profile, ServerRuntime,
    BACKGROUND_SYNC_ENABLED_VALUES, BACKGROUND_SYNC_ENV, EMBEDDING_PROFILE_ENV,
};
use rmc_server::tools::SearchTool;
use rmcp::{ServiceExt, transport::stdio};
use std::time::Duration;
use tracing_subscriber::{self, EnvFilter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Resolve the mode before the expensive startup: a client of the shared
    // daemon needs neither a `ServerRuntime` nor a background sync task — it is
    // a pipe between stdio and the socket.
    #[cfg(unix)]
    let mode = {
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
    // arguments alone. They are also how a host reads a configuration it got
    // wrong, so a rejected profile must not silence them.
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
        daemon::Mode::Client { .. } | daemon::Mode::Daemon { .. } | daemon::Mode::InProcess => {}
    }

    // Default to WARN for everything, INFO for our own crate. Users who want
    // RA's internal debug logs can set `RUST_LOG=ra_ap_hir=debug,...`.
    //
    // Why this matters: RA emits millions of `tracing::debug!` events during
    // name resolution. With Level::DEBUG enabled globally, the formatter +
    // socket-stderr write pipeline becomes the bottleneck — `build_hypergraph`
    // on a multi-crate workspace went from ~7s to 7+ minutes purely from log
    // formatting overhead. Keep this at WARN unless explicitly overridden.
    //
    // After the mode, because the daemon writes elsewhere: a bounded file on
    // disk instead of the stderr it inherited. `log_internal_errors(false)` for
    // every mode: a writer that fails must not be reported through `eprintln!`,
    // which panics when stderr is what failed — and stderr is what failed when
    // the daemon's log filled its disk.
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,rust_code_mcp=info,rmc_server=info,rmc_indexing=info"));
    let fmt = tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_ansi(false)
        .log_internal_errors(false);
    #[cfg(unix)]
    if let daemon::Mode::Daemon { socket, .. } = &mode {
        fmt.with_writer(daemon::daemon_log_writer(socket)).init();
    } else {
        fmt.with_writer(std::io::stderr).init();
    }
    #[cfg(not(unix))]
    {
        fmt.with_writer(std::io::stderr).init();
    }
    #[cfg(unix)]
    tracing::debug!("mode {mode:?}");

    // A statement, not a log argument: `tracing` evaluates arguments only when
    // the callsite level is enabled, so `RUST_LOG=error` would skip the check.
    // Every mode that reaches this line serves a session, and a non-unix build
    // has no mode at all, so both paths validate here exactly once, before the
    // client arm below and before any server starts.
    let requested_profile = std::env::var(EMBEDDING_PROFILE_ENV).ok();
    let automatic_backend =
        validate_automatic_profile(resolve_automatic_profile_name(requested_profile.as_deref()))
            .unwrap_or_else(|err| {
                eprintln!("{EMBEDDING_PROFILE_ENV}: {err}");
                std::process::exit(2);
            });
    let automatic_profile = automatic_backend.profile.name().to_string();
    if install_automatic_backend(automatic_backend).is_err() {
        eprintln!("{EMBEDDING_PROFILE_ENV}: the default backend was already resolved before startup");
        std::process::exit(2);
    }

    #[cfg(unix)]
    if let daemon::Mode::Client { socket, idle } = &mode {
        // An unreachable daemon never leaves the session without a server:
        // fall through to the previous in-process behaviour.
        match daemon::run_client(socket, *idle).await {
            Ok(true) => return Ok(()),
            Ok(false) => {
                tracing::info!("shared daemon unavailable; serving this session in-process")
            }
            Err(e) => {
                // Bytes were already exchanged, so this session's stdin is
                // partly consumed: an in-process server would answer a
                // truncated stream.
                //
                // Exit rather than return: `tokio::io::stdin` reads on the
                // blocking pool, and that read only ends when the host
                // closes stdin. Returning would hand the error to the
                // runtime drop, which waits for that read, so the failure
                // would show up as a hang instead of a non-zero exit.
                eprintln!("shared daemon session failed: {e}");
                std::process::exit(1);
            }
        }
    }

    tracing::info!("Starting MCP Server...");

    // Syncs every 5 minutes (300 seconds).
    let runtime = ServerRuntime::new(300);
    tracing::info!("Created MCP server runtime (5-minute sync interval)");

    let background_sync_env = std::env::var(BACKGROUND_SYNC_ENV).ok();
    let background_sync_enabled = parse_background_sync_env(background_sync_env.as_deref());

    tracing::info!(
        "MCP startup defaults: background sync {} ({}='{}'; enabled only for {}, case-insensitive); automatic/background embedding profile default {} ({}='{}'); CUDA-capable features compiled: {}",
        if background_sync_enabled { "enabled" } else { "disabled" },
        BACKGROUND_SYNC_ENV,
        background_sync_env.as_deref().unwrap_or("<unset>"),
        BACKGROUND_SYNC_ENABLED_VALUES,
        automatic_profile,
        EMBEDDING_PROFILE_ENV,
        requested_profile.as_deref().unwrap_or("<unset>"),
        cuda_capable_features_compiled(),
    );

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
        let shutdown = runtime.shutdown_gracefully(Duration::from_secs(10)).await;
        tracing::info!("Runtime shutdown after daemon exit: {:?}", shutdown);
        return result.map_err(|e| -> Box<dyn std::error::Error> { e });
    }

    let service = match SearchTool::with_server_runtime(&runtime).serve(stdio()).await {
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

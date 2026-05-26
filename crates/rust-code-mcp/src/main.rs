// lancedb 0.29's async stack (lance_io::uring + moka::future) pushes the
// auto-trait Send check past the default 128-level recursion limit when
// the sync-manager future is spawned in main. Bump it locally; this is a
// compile-time inference budget, not a runtime cost.
#![recursion_limit = "512"]

use rmc_server::mcp::{
    automatic_embedding_profile_name, cuda_capable_features_compiled,
    parse_background_sync_env, ServerRuntime, BACKGROUND_SYNC_ENABLED_VALUES, BACKGROUND_SYNC_ENV,
};
use rmc_server::tools::SearchTool;
use rmcp::{ServiceExt, transport::stdio};
use std::time::Duration;
use tracing_subscriber::{self, EnvFilter};

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
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,rust_code_mcp=info,rmc_server=info,rmc_indexing=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting MCP Server...");

    // Syncs every 5 minutes (300 seconds).
    let runtime = ServerRuntime::new(300);
    tracing::info!("Created MCP server runtime (5-minute sync interval)");

    let background_sync_env = std::env::var(BACKGROUND_SYNC_ENV).ok();
    let background_sync_enabled = parse_background_sync_env(background_sync_env.as_deref());
    tracing::info!(
        "MCP startup defaults: background sync {} ({}='{}'; enabled only for {}, case-insensitive); automatic/background embedding profile default {}; CUDA-capable features compiled: {}",
        if background_sync_enabled { "enabled" } else { "disabled" },
        BACKGROUND_SYNC_ENV,
        background_sync_env.as_deref().unwrap_or("<unset>"),
        BACKGROUND_SYNC_ENABLED_VALUES,
        automatic_embedding_profile_name(),
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

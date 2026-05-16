// lancedb 0.29's async stack (lance_io::uring + moka::future) pushes the
// auto-trait Send check past the default 128-level recursion limit when
// the sync-manager future is spawned in main. Bump it locally; this is a
// compile-time inference budget, not a runtime cost.
#![recursion_limit = "512"]

use file_search_mcp::mcp::SyncManager;
use file_search_mcp::tools::search_tool::SearchTool;
use rmcp::{ServiceExt, transport::stdio};
use std::sync::Arc;
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
        .unwrap_or_else(|_| EnvFilter::new("warn,file_search_mcp=info"));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("Starting MCP Server...");

    // Create background sync manager
    // Syncs every 5 minutes (300 seconds)
    let sync_manager = Arc::new(SyncManager::with_defaults(300));
    tracing::info!("Created background sync manager (5-minute interval)");

    // Start background sync task
    let sync_manager_clone = Arc::clone(&sync_manager);
    tokio::spawn(async move {
        sync_manager_clone.run().await;
    });
    tracing::info!("Started background sync task");

    // Start MCP server with sync manager integration
    let service = SearchTool::with_sync_manager(Arc::clone(&sync_manager))
        .serve(stdio())
        .await
        .inspect_err(|e| {
            tracing::error!("serving error: {:?}", e);
        })?;

    service.waiting().await?;
    Ok(())
}

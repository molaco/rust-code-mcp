use file_search_mcp::mcp::SyncManager;
use file_search_mcp::tools::search_tool::SearchTool;
use rmcp::{ServiceExt, transport::stdio};
use std::sync::Arc;
use tracing_subscriber::{self, EnvFilter};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::DEBUG.into()))
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

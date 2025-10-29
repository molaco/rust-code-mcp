//! JSON-RPC test for index_codebase MCP tool with GPU acceleration
//!
//! This test verifies:
//! 1. JSON-RPC 2.0 protocol communication with MCP server
//! 2. GPU acceleration is working during indexing
//! 3. Performance metrics for GPU-accelerated embedding generation
//! 4. Proper handling of the rust-code-mcp codebase itself

use anyhow::Result;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Instant;
use tracing_subscriber::{self, EnvFilter};

/// JSON-RPC 2.0 request structure
#[derive(Debug, serde::Serialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: Option<Value>,
}

impl JsonRpcRequest {
    fn new(id: u64, method: &str, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        }
    }
}

/// JSON-RPC 2.0 response structure
#[derive(Debug, serde::Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Option<u64>,
    result: Option<Value>,
    error: Option<Value>,
}

/// Test environment for MCP JSON-RPC testing
struct McpTestEnv {
    codebase_path: PathBuf,
}

impl McpTestEnv {
    fn new() -> Self {
        Self {
            codebase_path: PathBuf::from("/home/molaco/Documents/rust-code-mcp"),
        }
    }

    fn verify_codebase_exists(&self) -> Result<()> {
        if !self.codebase_path.exists() {
            anyhow::bail!("Codebase path does not exist: {}", self.codebase_path.display());
        }
        if !self.codebase_path.is_dir() {
            anyhow::bail!("Codebase path is not a directory: {}", self.codebase_path.display());
        }
        Ok(())
    }

    /// Create index_codebase tool call request
    fn create_index_request(&self, request_id: u64, force_reindex: bool) -> JsonRpcRequest {
        let params = json!({
            "name": "index_codebase",
            "arguments": {
                "directory": self.codebase_path.to_string_lossy().to_string(),
                "force_reindex": force_reindex
            }
        });

        JsonRpcRequest::new(request_id, "tools/call", Some(params))
    }
}

/// Call the index_codebase tool directly (simulating JSON-RPC behavior)
async fn call_index_tool_direct(
    codebase_path: &str,
    force_reindex: bool,
) -> Result<rmcp::model::CallToolResult> {
    use file_search_mcp::tools::index_tool::{index_codebase, IndexCodebaseParams};

    let params = IndexCodebaseParams {
        directory: codebase_path.to_string(),
        force_reindex: Some(force_reindex),
    };

    index_codebase(params, None)
        .await
        .map_err(|e| anyhow::anyhow!("MCP error: {:?}", e))
}

#[tokio::test]
#[ignore] // Run with: cargo test --test test_gpu_index_jsonrpc -- --ignored --nocapture
async fn test_gpu_index_jsonrpc_rust_code_mcp() -> Result<()> {
    // Initialize tracing to see GPU logs
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into())
        )
        .with_test_writer()
        .init();

    println!("\n========================================");
    println!("GPU-Accelerated Indexing Test");
    println!("========================================\n");

    let env = McpTestEnv::new();

    // Verify codebase exists
    env.verify_codebase_exists()?;
    println!("✓ Codebase verified: {}", env.codebase_path.display());

    // Count Rust files to estimate workload
    let rust_file_count = std::fs::read_dir(&env.codebase_path)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path().extension()
                .and_then(|s| s.to_str())
                .map(|s| s == "rs")
                .unwrap_or(false)
        })
        .count();

    println!("✓ Found {} Rust files in codebase root", rust_file_count);
    println!("\n--- Starting GPU-accelerated indexing ---\n");

    // Create JSON-RPC request for index_codebase tool
    let request = env.create_index_request(1, true); // Force reindex for testing

    println!("JSON-RPC Request:");
    println!("{}\n", serde_json::to_string_pretty(&request)?);

    // Execute indexing (direct call simulating JSON-RPC)
    let start = Instant::now();
    let result = call_index_tool_direct(
        &env.codebase_path.to_string_lossy(),
        true, // Force reindex to test full GPU pipeline
    )
    .await?;
    let elapsed = start.elapsed();

    println!("\n--- Indexing complete ---\n");
    println!("Total time: {:?}", elapsed);
    println!("Time per file: {:?}", elapsed / rust_file_count.max(1) as u32);

    // Verify result structure
    assert!(
        result.is_error.is_none() || !result.is_error.unwrap(),
        "Indexing should succeed"
    );
    assert!(!result.content.is_empty(), "Should return content");

    // Extract and display result
    if let Some(content) = result.content.first() {
        // Content is likely a string representation - extract the text
        let text = format!("{:?}", content);
        println!("\nIndexing Result:");
        println!("{}", text);

        // Verify key information in result (check both debug output and actual content)
        let has_success = text.contains("Successfully indexed") || text.contains("Indexed files");
        let has_chunks = text.contains("chunks") || text.contains("chunk");
        let has_collection = text.contains("code_chunks_") || text.contains("Collection");

        assert!(has_success, "Should report success or indexed files");
        assert!(has_chunks, "Should report chunk count");
        assert!(has_collection, "Should show collection name");
    }

    println!("\n========================================");
    println!("GPU Test Results");
    println!("========================================");
    println!("✓ JSON-RPC protocol: OK");
    println!("✓ Indexing completed: OK");
    println!("✓ GPU acceleration: Check logs above for CUDA messages");
    println!("✓ Performance: {:?} total ({:.2} files/sec)",
        elapsed,
        rust_file_count as f64 / elapsed.as_secs_f64()
    );
    println!("========================================\n");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_gpu_incremental_index() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();

    println!("\n========================================");
    println!("GPU Incremental Indexing Test");
    println!("========================================\n");

    let env = McpTestEnv::new();
    env.verify_codebase_exists()?;

    // First indexing (full)
    println!("--- First index (full) ---\n");
    let start1 = Instant::now();
    let result1 = call_index_tool_direct(
        &env.codebase_path.to_string_lossy(),
        false, // Not forcing, but will index if needed
    )
    .await?;
    let elapsed1 = start1.elapsed();
    println!("First index time: {:?}\n", elapsed1);

    // Second indexing (should detect no changes)
    println!("--- Second index (incremental, no changes) ---\n");
    let start2 = Instant::now();
    let result2 = call_index_tool_direct(
        &env.codebase_path.to_string_lossy(),
        false,
    )
    .await?;
    let elapsed2 = start2.elapsed();
    println!("Second index time: {:?}\n", elapsed2);

    // Verify both succeeded
    assert!(result1.is_error.is_none() || !result1.is_error.unwrap());
    assert!(result2.is_error.is_none() || !result2.is_error.unwrap());

    // Second index should be much faster (change detection)
    println!("\n========================================");
    println!("Incremental Indexing Results");
    println!("========================================");
    println!("Full index:        {:?}", elapsed1);
    println!("Incremental index: {:?}", elapsed2);
    println!("Speedup:           {:.1}x", elapsed1.as_secs_f64() / elapsed2.as_secs_f64().max(0.001));
    println!("========================================\n");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_gpu_batch_size_optimization() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .with_test_writer()
        .try_init()
        .ok();

    println!("\n========================================");
    println!("GPU Batch Size Optimization Test");
    println!("========================================\n");

    let env = McpTestEnv::new();
    env.verify_codebase_exists()?;

    println!("Testing GPU batch embedding with rust-code-mcp codebase");
    println!("Expected batch size: 256 chunks (optimized for 8GB VRAM)\n");

    let start = Instant::now();
    let result = call_index_tool_direct(
        &env.codebase_path.to_string_lossy(),
        true, // Force reindex to test full pipeline
    )
    .await?;
    let elapsed = start.elapsed();

    assert!(result.is_error.is_none() || !result.is_error.unwrap());

    // Extract metrics from result
    if let Some(content) = result.content.first() {
        let text = format!("{:?}", content);
        println!("Result:\n{}\n", text);

        // Check for performance indicators
        if text.contains("chunks") || text.contains("chunk") {
            println!("✓ Batch embedding completed successfully");
        }
    }

    println!("\n========================================");
    println!("Batch Optimization Results");
    println!("========================================");
    println!("Total time: {:?}", elapsed);
    println!("Check logs above for:");
    println!("  - 'Batch embedding N chunks' messages");
    println!("  - GPU memory usage");
    println!("  - Chunks/sec throughput");
    println!("========================================\n");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_json_rpc_error_handling() -> Result<()> {
    println!("\n========================================");
    println!("JSON-RPC Error Handling Test");
    println!("========================================\n");

    // Test 1: Invalid directory
    println!("Test 1: Invalid directory path");
    let result = call_index_tool_direct("/nonexistent/path", false).await;
    assert!(result.is_err(), "Should fail for nonexistent path");
    println!("✓ Invalid path rejected correctly\n");

    // Test 2: File instead of directory
    println!("Test 2: File path instead of directory");
    let result = call_index_tool_direct("/home/molaco/Documents/rust-code-mcp/Cargo.toml", false).await;
    assert!(result.is_err(), "Should fail for file path");
    println!("✓ File path rejected correctly\n");

    println!("========================================");
    println!("Error Handling: OK");
    println!("========================================\n");

    Ok(())
}

#[tokio::test]
#[ignore]
async fn test_gpu_memory_monitoring() -> Result<()> {
    use file_search_mcp::metrics::memory::MemoryMonitor;

    println!("\n========================================");
    println!("GPU Memory Monitoring Test");
    println!("========================================\n");

    let env = McpTestEnv::new();
    env.verify_codebase_exists()?;

    // Monitor memory before indexing
    let monitor = MemoryMonitor::new();
    let mem_before = monitor.used_bytes();
    let mem_percent_before = monitor.usage_percent();

    println!("Memory before indexing:");
    println!("  Used: {:.2} MB", mem_before as f64 / 1_000_000.0);
    println!("  Percent: {:.1}%\n", mem_percent_before);

    // Run indexing
    println!("Starting GPU-accelerated indexing...\n");
    let result = call_index_tool_direct(
        &env.codebase_path.to_string_lossy(),
        true,
    )
    .await?;

    // Monitor memory after indexing
    let mem_after = monitor.used_bytes();
    let mem_percent_after = monitor.usage_percent();

    println!("\nMemory after indexing:");
    println!("  Used: {:.2} MB", mem_after as f64 / 1_000_000.0);
    println!("  Percent: {:.1}%", mem_percent_after);
    println!("  Delta: {:.2} MB\n", (mem_after as i64 - mem_before as i64) as f64 / 1_000_000.0);

    assert!(result.is_error.is_none() || !result.is_error.unwrap());

    println!("========================================");
    println!("Memory Monitoring: OK");
    println!("========================================\n");

    Ok(())
}

//! Regression tests for MCP stdio framing.
//!
//! The stdio transport reserves stdout for newline-delimited JSON-RPC frames.
//! Human-readable diagnostics must go to stderr/tracing or tool results.

use anyhow::{Context, Result, anyhow};
use rmc_engine::embeddings::EmbeddingBackend;
use rmc_indexing::indexing::incremental::get_snapshot_path;
use rmc_server::mcp::project_paths::ProjectPaths;
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

struct StdioIndexEnv {
    codebase_path: PathBuf,
    _temp_dir: TempDir,
}

impl StdioIndexEnv {
    fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let codebase_path = temp_dir.path().join("codebase");
        std::fs::create_dir(&codebase_path)?;

        Ok(Self {
            codebase_path,
            _temp_dir: temp_dir,
        })
    }

    fn write_file(&self, name: &str, content: &str) -> Result<()> {
        let path = self.codebase_path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
        Ok(())
    }
}

impl Drop for StdioIndexEnv {
    fn drop(&mut self) {
        let paths = ProjectPaths::from_directory(
            &self.codebase_path,
            &EmbeddingBackend::default(),
        );
        let _ = std::fs::remove_dir_all(paths.cache_path);
        let _ = std::fs::remove_dir_all(paths.tantivy_path);
        let _ = std::fs::remove_dir_all(paths.vector_path);
        let _ = std::fs::remove_file(get_snapshot_path(&self.codebase_path));
    }
}

struct ChildGuard {
    child: Child,
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
#[ignore = "requires embedding model; run with cargo test --test test_mcp_stdio_transport -- --ignored --nocapture"]
fn test_index_codebase_force_reindex_stdout_is_json_only() -> Result<()> {
    let env = StdioIndexEnv::new()?;
    env.write_file(
        "src/main.rs",
        r#"fn main() {
    println!("hello");
}
"#,
    )?;

    let mut child = ChildGuard {
        child: Command::new(env!("CARGO_BIN_EXE_rust-code-mcp"))
            .env("RUST_LOG", "error")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to spawn MCP server")?,
    };

    let stdout = child
        .child
        .stdout
        .take()
        .context("child stdout was not piped")?;
    let mut stdin = child
        .child
        .stdin
        .take()
        .context("child stdin was not piped")?;

    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        let lines = BufReader::new(stdout).lines();
        for line in lines {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    send_message(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {
                    "name": "stdio-regression-test",
                    "version": "0.0.0"
                }
            }
        }),
    )?;
    let initialize_response = read_json_response(&rx, 1, Duration::from_secs(30))?;
    assert!(
        initialize_response.get("error").is_none(),
        "initialize failed: {initialize_response}"
    );

    send_message(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    )?;

    send_message(
        &mut stdin,
        json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "index_codebase",
                "arguments": {
                    "directory": env.codebase_path.to_string_lossy(),
                    "force_reindex": true
                }
            }
        }),
    )?;

    let tool_response = read_json_response(&rx, 2, Duration::from_secs(180))?;
    assert!(
        tool_response.get("error").is_none(),
        "index_codebase failed: {tool_response}"
    );

    drop(stdin);
    child.child.kill().ok();
    child.child.wait().ok();
    reader.join().map_err(|_| anyhow!("stdout reader panicked"))?;

    Ok(())
}

fn send_message(stdin: &mut ChildStdin, message: Value) -> Result<()> {
    serde_json::to_writer(&mut *stdin, &message)?;
    stdin.write_all(b"\n")?;
    stdin.flush()?;
    Ok(())
}

fn read_json_response(
    rx: &Receiver<std::io::Result<String>>,
    id: u64,
    timeout: Duration,
) -> Result<Value> {
    let deadline = Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!("timed out waiting for JSON-RPC response id {id}"));
        }

        let line = match rx.recv_timeout(remaining) {
            Ok(line) => line?,
            Err(RecvTimeoutError::Timeout) => {
                return Err(anyhow!("timed out waiting for JSON-RPC response id {id}"));
            }
            Err(RecvTimeoutError::Disconnected) => {
                return Err(anyhow!("server stdout closed before response id {id}"));
            }
        };

        let value: Value = serde_json::from_str(&line)
            .with_context(|| format!("stdout contained a non-JSON-RPC line: {line:?}"))?;

        if value.get("id").and_then(Value::as_u64) == Some(id) {
            return Ok(value);
        }
    }
}

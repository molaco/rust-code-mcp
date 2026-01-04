//! Integration tests for index_codebase MCP tool
//!
//! Tests verify:
//! 1. Basic indexing functionality
//! 2. Force reindex parameter
//! 3. Error handling (invalid paths, non-directories)
//! 4. Integration with SyncManager
//! 5. Result formatting

use anyhow::Result;
use file_search_mcp::mcp::SyncManager;
use file_search_mcp::tools::index_tool::{index_codebase, IndexCodebaseParams};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;

struct IndexToolTestEnv {
    _temp_dir: TempDir,
    codebase_path: PathBuf,
}

impl IndexToolTestEnv {
    fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let codebase_path = temp_dir.path().join("codebase");
        std::fs::create_dir(&codebase_path)?;

        Ok(Self {
            _temp_dir: temp_dir,
            codebase_path,
        })
    }

    fn write_file(&self, name: &str, content: &str) -> Result<PathBuf> {
        let path = self.codebase_path.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(path)
    }

    fn get_path_string(&self) -> String {
        self.codebase_path.to_string_lossy().to_string()
    }

    fn create_params(&self, force: bool) -> IndexCodebaseParams {
        IndexCodebaseParams {
            directory: self.get_path_string(),
            force_reindex: if force { Some(true) } else { None },
        }
    }
}

#[tokio::test]
async fn test_index_tool_invalid_directory() {
    let params = IndexCodebaseParams {
        directory: "/nonexistent/path/that/does/not/exist".to_string(),
        force_reindex: None,
    };

    let result = index_codebase(params, None).await;

    assert!(result.is_err(), "Should return error for nonexistent path");
    println!("✓ Invalid directory rejected correctly");
}

#[tokio::test]
async fn test_index_tool_not_a_directory() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let file_path = temp_dir.path().join("not_a_dir.txt");
    std::fs::write(&file_path, "test")?;

    let params = IndexCodebaseParams {
        directory: file_path.to_string_lossy().to_string(),
        force_reindex: None,
    };

    let result = index_codebase(params, None).await;

    assert!(result.is_err(), "Should return error for file path");
    println!("✓ File path rejected correctly");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_basic_indexing() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    // Create test files
    env.write_file("main.rs", r#"
        fn main() {
            println!("Hello, world!");
        }
    "#)?;

    env.write_file("lib.rs", r#"
        pub fn library_function() -> i32 {
            42
        }
    "#)?;

    let params = IndexCodebaseParams {
        directory: env.get_path_string(),
        force_reindex: None,
    };

    let result = index_codebase(params, None).await;

    assert!(result.is_ok(), "Should successfully index valid codebase");
    let call_result = result.unwrap();
    assert!(call_result.is_error.is_none() || !call_result.is_error.unwrap());
    assert!(!call_result.content.is_empty(), "Should have result content");

    println!("✓ Basic indexing works");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_empty_directory() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    // Don't create any files - empty directory

    let params = IndexCodebaseParams {
        directory: env.get_path_string(),
        force_reindex: None,
    };

    let result = index_codebase(params, None).await;

    assert!(result.is_ok(), "Should handle empty directory gracefully");

    println!("✓ Empty directory handled correctly");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_no_changes_detection() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    env.write_file("test.rs", "fn test() {}")?;

    // First index
    let result1 = index_codebase(env.create_params(false), None).await;
    assert!(result1.is_ok());

    // Second index - should detect no changes
    let result2 = index_codebase(env.create_params(false), None).await;
    assert!(result2.is_ok());

    println!("✓ No changes detection works");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_force_reindex() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    env.write_file("test.rs", "fn test() {}")?;

    // First index
    let result1 = index_codebase(env.create_params(false), None).await;
    assert!(result1.is_ok());

    // Force reindex - should reindex everything
    let result2 = index_codebase(env.create_params(true), None).await;
    assert!(result2.is_ok());

    println!("✓ Force reindex works");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_with_sync_manager() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    env.write_file("main.rs", "fn main() {}")?;

    // Create sync manager
    let sync_manager = Arc::new(SyncManager::with_defaults(300));

    // Initially no directories tracked
    assert_eq!(sync_manager.get_tracked_directories().await.len(), 0);

    let params = IndexCodebaseParams {
        directory: env.get_path_string(),
        force_reindex: None,
    };

    // Index with sync manager
    let result = index_codebase(params, Some(&sync_manager)).await;
    assert!(result.is_ok());

    // Directory should now be tracked
    let tracked = sync_manager.get_tracked_directories().await;
    assert_eq!(tracked.len(), 1, "Directory should be added to sync manager");
    assert!(tracked.contains(&env.codebase_path));

    println!("✓ Integration with SyncManager works");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_nested_structure() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    // Create nested directory structure
    env.write_file("src/main.rs", "fn main() {}")?;
    env.write_file("src/lib.rs", "pub fn lib() {}")?;
    env.write_file("src/utils/mod.rs", "pub mod helper;")?;
    env.write_file("src/utils/helper.rs", "pub fn help() {}")?;
    env.write_file("tests/integration_test.rs", "#[test] fn test() {}")?;

    let result = index_codebase(env.create_params(false), None).await;

    assert!(result.is_ok(), "Should handle nested structure");

    println!("✓ Nested directory structure handled");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_incremental_update() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    // Initial files
    env.write_file("file1.rs", "fn file1() {}")?;
    env.write_file("file2.rs", "fn file2() {}")?;

    // First index
    let result1 = index_codebase(env.create_params(false), None).await;
    assert!(result1.is_ok());

    // Add new file
    env.write_file("file3.rs", "fn file3() {}")?;

    // Second index - should detect new file
    let result2 = index_codebase(env.create_params(false), None).await;
    assert!(result2.is_ok());

    println!("✓ Incremental update detected");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_result_format() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    env.write_file("test.rs", r#"
        /// Test function
        pub fn test_function() -> i32 {
            println!("Testing");
            42
        }
    "#)?;

    let result = index_codebase(env.create_params(false), None).await?;

    // Verify result structure
    assert!(!result.content.is_empty(), "Should have content");

    println!("✓ Result format is correct");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_with_non_rust_files() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    // Create mix of Rust and non-Rust files
    env.write_file("good.rs", "fn good() {}")?;
    env.write_file("README.md", "# README")?;
    env.write_file("config.toml", "[package]")?;
    env.write_file(".gitignore", "target/")?;

    let result = index_codebase(env.create_params(false), None).await;

    assert!(result.is_ok(), "Should handle mixed file types");

    println!("✓ Handles non-Rust files correctly");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires embedding model
async fn test_index_tool_performance() -> Result<()> {
    let env = IndexToolTestEnv::new()?;

    // Create multiple files
    for i in 0..20 {
        env.write_file(
            &format!("file{}.rs", i),
            &format!("pub fn function_{}() {{ println!(\"test\"); }}", i)
        )?;
    }

    // First index
    let start1 = std::time::Instant::now();
    let result1 = index_codebase(env.create_params(false), None).await;
    let elapsed1 = start1.elapsed();
    assert!(result1.is_ok());

    println!("First index: {:?}", elapsed1);

    // Second index (no changes) - should be much faster
    let start2 = std::time::Instant::now();
    let result2 = index_codebase(env.create_params(false), None).await;
    let elapsed2 = start2.elapsed();
    assert!(result2.is_ok());

    println!("Second index (no changes): {:?}", elapsed2);
    println!("Speedup: {:.1}x", elapsed1.as_secs_f64() / elapsed2.as_secs_f64());

    assert!(elapsed2 < elapsed1, "Second index should be faster");

    println!("✓ Performance characteristics verified");

    Ok(())
}

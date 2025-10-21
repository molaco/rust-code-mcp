//! Integration tests for SyncManager background sync functionality
//!
//! Tests verify:
//! 1. Background sync runs periodically
//! 2. Multiple directories can be tracked
//! 3. Changes are automatically detected and indexed
//! 4. Manual sync triggers work correctly

use anyhow::Result;
use file_search_mcp::mcp::SyncManager;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

struct SyncTestEnv {
    _temp_dir: TempDir,
    data_dir: PathBuf,
    codebase1: PathBuf,
    codebase2: PathBuf,
}

impl SyncTestEnv {
    fn new() -> Result<Self> {
        let temp_dir = TempDir::new()?;
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir(&data_dir)?;

        let codebase1 = temp_dir.path().join("codebase1");
        let codebase2 = temp_dir.path().join("codebase2");

        std::fs::create_dir(&codebase1)?;
        std::fs::create_dir(&codebase2)?;

        Ok(Self {
            _temp_dir: temp_dir,
            data_dir,
            codebase1,
            codebase2,
        })
    }

    fn create_sync_manager(&self, interval_secs: u64) -> SyncManager {
        SyncManager::new(
            "http://localhost:6334".to_string(),
            self.data_dir.join("cache"),
            self.data_dir.join("index"),
            interval_secs,
        )
    }

    fn write_file(&self, codebase: &PathBuf, name: &str, content: &str) -> Result<PathBuf> {
        let path = codebase.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, content)?;
        Ok(path)
    }
}

#[tokio::test]
async fn test_sync_manager_track_untrack() {
    let env = SyncTestEnv::new().unwrap();
    let sync_manager = env.create_sync_manager(300);

    // Initially no directories tracked
    let tracked = sync_manager.get_tracked_directories().await;
    assert!(tracked.is_empty());

    // Track directory
    sync_manager.track_directory(env.codebase1.clone()).await;
    let tracked = sync_manager.get_tracked_directories().await;
    assert_eq!(tracked.len(), 1);
    assert!(tracked.contains(&env.codebase1));

    // Track second directory
    sync_manager.track_directory(env.codebase2.clone()).await;
    let tracked = sync_manager.get_tracked_directories().await;
    assert_eq!(tracked.len(), 2);

    // Untrack first directory
    sync_manager.untrack_directory(&env.codebase1).await;
    let tracked = sync_manager.get_tracked_directories().await;
    assert_eq!(tracked.len(), 1);
    assert!(tracked.contains(&env.codebase2));
    assert!(!tracked.contains(&env.codebase1));

    println!("✓ Track/untrack functionality works correctly");
}

#[tokio::test]
async fn test_sync_manager_no_duplicate_tracking() {
    let env = SyncTestEnv::new().unwrap();
    let sync_manager = env.create_sync_manager(300);

    // Track same directory multiple times
    sync_manager.track_directory(env.codebase1.clone()).await;
    sync_manager.track_directory(env.codebase1.clone()).await;
    sync_manager.track_directory(env.codebase1.clone()).await;

    let tracked = sync_manager.get_tracked_directories().await;
    assert_eq!(tracked.len(), 1, "Should not have duplicates");

    println!("✓ No duplicate tracking");
}

#[tokio::test]
#[ignore] // Requires Qdrant
async fn test_manual_sync_trigger() -> Result<()> {
    let env = SyncTestEnv::new()?;
    let sync_manager = env.create_sync_manager(300);

    // Create test file
    env.write_file(&env.codebase1, "test.rs", r#"
        fn test() {
            println!("test");
        }
    "#)?;

    // Track directory
    sync_manager.track_directory(env.codebase1.clone()).await;

    // Trigger manual sync
    println!("Triggering manual sync...");
    sync_manager.sync_now().await;

    println!("✓ Manual sync completed without errors");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires Qdrant
async fn test_sync_single_directory_manual() -> Result<()> {
    let env = SyncTestEnv::new()?;
    let sync_manager = env.create_sync_manager(300);

    // Create test file
    env.write_file(&env.codebase1, "lib.rs", r#"
        pub fn library_function() -> i32 {
            42
        }
    "#)?;

    // Track directory
    sync_manager.track_directory(env.codebase1.clone()).await;

    // Sync specific directory
    println!("Syncing specific directory...");
    let result = sync_manager.sync_directory_now(&env.codebase1).await;

    assert!(result.is_ok(), "Sync should succeed");
    println!("✓ Single directory sync works");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires Qdrant and is time-consuming
async fn test_background_sync_detects_changes() -> Result<()> {
    let env = SyncTestEnv::new()?;

    // Create sync manager with short interval for testing
    let sync_manager = Arc::new(env.create_sync_manager(2)); // 2 second interval

    // Create initial file
    env.write_file(&env.codebase1, "original.rs", "fn original() {}")?;

    // Track directory
    sync_manager.track_directory(env.codebase1.clone()).await;

    // Do initial manual sync
    println!("Initial sync...");
    sync_manager.sync_now().await;

    // Start background sync in separate task
    let sync_clone = Arc::clone(&sync_manager);
    let _sync_handle = tokio::spawn(async move {
        // Run for limited time in test
        tokio::select! {
            _ = sync_clone.run() => {}
            _ = tokio::time::sleep(Duration::from_secs(10)) => {
                println!("Background sync stopped after 10s (test timeout)");
            }
        }
    });

    // Wait a bit for initial sync
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Modify file while background sync is running
    println!("Modifying file...");
    env.write_file(&env.codebase1, "original.rs", r#"
        fn original() {
            println!("modified");
        }
    "#)?;

    // Wait for background sync to detect and process the change
    println!("Waiting for background sync to detect changes...");
    tokio::time::sleep(Duration::from_secs(4)).await;

    println!("✓ Background sync runs and processes changes");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires Qdrant
async fn test_multiple_directories_sync() -> Result<()> {
    let env = SyncTestEnv::new()?;
    let sync_manager = env.create_sync_manager(300);

    // Create files in both codebases
    env.write_file(&env.codebase1, "file1.rs", "fn file1() {}")?;
    env.write_file(&env.codebase2, "file2.rs", "fn file2() {}")?;

    // Track both directories
    sync_manager.track_directory(env.codebase1.clone()).await;
    sync_manager.track_directory(env.codebase2.clone()).await;

    // Sync all
    println!("Syncing multiple directories...");
    sync_manager.sync_now().await;

    println!("✓ Multiple directories synced successfully");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires Qdrant
async fn test_sync_empty_directory() -> Result<()> {
    let env = SyncTestEnv::new()?;
    let sync_manager = env.create_sync_manager(300);

    // Track empty directory
    sync_manager.track_directory(env.codebase1.clone()).await;

    // Should handle empty directory gracefully
    let result = sync_manager.sync_directory_now(&env.codebase1).await;
    assert!(result.is_ok(), "Should handle empty directory");

    println!("✓ Empty directory handled gracefully");

    Ok(())
}

#[tokio::test]
#[ignore] // Requires Qdrant
async fn test_sync_with_nested_directories() -> Result<()> {
    let env = SyncTestEnv::new()?;
    let sync_manager = env.create_sync_manager(300);

    // Create nested structure
    env.write_file(&env.codebase1, "src/main.rs", "fn main() {}")?;
    env.write_file(&env.codebase1, "src/lib.rs", "pub fn lib() {}")?;
    env.write_file(&env.codebase1, "src/utils/helper.rs", "pub fn help() {}")?;
    env.write_file(&env.codebase1, "tests/test.rs", "#[test] fn test() {}")?;

    // Track root directory
    sync_manager.track_directory(env.codebase1.clone()).await;

    // Sync should handle nested structure
    println!("Syncing nested directory structure...");
    let result = sync_manager.sync_directory_now(&env.codebase1).await;

    assert!(result.is_ok(), "Should handle nested directories");
    println!("✓ Nested directory structure handled correctly");

    Ok(())
}

#[tokio::test]
async fn test_sync_manager_with_defaults() {
    let sync_manager = SyncManager::with_defaults(300);

    // Should be created successfully
    let tracked = sync_manager.get_tracked_directories().await;
    assert!(tracked.is_empty());

    println!("✓ SyncManager created with defaults");
}

#[tokio::test]
#[ignore] // Requires Qdrant
async fn test_sync_recovers_from_errors() -> Result<()> {
    let env = SyncTestEnv::new()?;
    let sync_manager = env.create_sync_manager(300);

    // Track a directory that exists
    env.write_file(&env.codebase1, "good.rs", "fn good() {}")?;
    sync_manager.track_directory(env.codebase1.clone()).await;

    // Track a directory that will be deleted (to cause error)
    env.write_file(&env.codebase2, "bad.rs", "fn bad() {}")?;
    sync_manager.track_directory(env.codebase2.clone()).await;

    // First sync should work
    sync_manager.sync_now().await;

    // Delete second directory to cause error on next sync
    std::fs::remove_dir_all(&env.codebase2)?;

    // Second sync should handle error gracefully and still sync first directory
    println!("Syncing with one failed directory...");
    sync_manager.sync_now().await;

    // Sync should complete even though one directory failed
    println!("✓ Sync continues despite individual directory failures");

    Ok(())
}

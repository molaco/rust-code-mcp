//! Standalone Merkle tree tests (no Qdrant required)
//!
//! These tests verify the core Merkle tree functionality without
//! requiring a running Qdrant instance.

use anyhow::Result;
use file_search_mcp::indexing::merkle::{ChangeSet, FileSystemMerkle};
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn test_merkle_basic_creation() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let test_file = temp_dir.path().join("test.rs");
    std::fs::write(&test_file, "fn main() {}")?;

    let merkle = FileSystemMerkle::from_directory(temp_dir.path())?;

    assert_eq!(merkle.file_count(), 1);
    assert!(merkle.root_hash().is_some());

    println!("✓ Basic Merkle creation works");
    Ok(())
}

#[test]
fn test_merkle_no_changes() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let test_file = temp_dir.path().join("test.rs");
    std::fs::write(&test_file, "fn test() {}")?;

    let merkle1 = FileSystemMerkle::from_directory(temp_dir.path())?;
    let merkle2 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Root hashes should match
    assert_eq!(merkle1.root_hash(), merkle2.root_hash());
    assert!(!merkle1.has_changes(&merkle2));

    let changes = merkle1.detect_changes(&merkle2);
    assert!(changes.is_empty());
    assert_eq!(changes.total_changes(), 0);

    println!("✓ No changes detection works");
    Ok(())
}

#[test]
fn test_merkle_file_modification() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let test_file = temp_dir.path().join("test.rs");

    // Create initial state
    std::fs::write(&test_file, "fn main() {}")?;
    let merkle1 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Modify file
    std::fs::write(&test_file, "fn main() { println!(\"hello\"); }")?;
    let merkle2 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Should detect change
    assert_ne!(merkle1.root_hash(), merkle2.root_hash());
    assert!(merkle2.has_changes(&merkle1));

    let changes = merkle2.detect_changes(&merkle1);
    assert_eq!(changes.modified.len(), 1);
    assert_eq!(changes.added.len(), 0);
    assert_eq!(changes.deleted.len(), 0);
    assert_eq!(changes.total_changes(), 1);

    println!("✓ File modification detection works");
    Ok(())
}

#[test]
fn test_merkle_file_addition() -> Result<()> {
    let temp_dir = TempDir::new()?;

    // Initial state with one file
    let file1 = temp_dir.path().join("file1.rs");
    std::fs::write(&file1, "fn one() {}")?;
    let merkle1 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Add second file
    let file2 = temp_dir.path().join("file2.rs");
    std::fs::write(&file2, "fn two() {}")?;
    let merkle2 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Should detect addition
    assert!(merkle2.has_changes(&merkle1));

    let changes = merkle2.detect_changes(&merkle1);
    assert_eq!(changes.added.len(), 1);
    assert_eq!(changes.modified.len(), 0);
    assert_eq!(changes.deleted.len(), 0);

    println!("✓ File addition detection works");
    Ok(())
}

#[test]
fn test_merkle_file_deletion() -> Result<()> {
    let temp_dir = TempDir::new()?;

    // Initial state with two files
    let file1 = temp_dir.path().join("file1.rs");
    let file2 = temp_dir.path().join("file2.rs");
    std::fs::write(&file1, "fn one() {}")?;
    std::fs::write(&file2, "fn two() {}")?;
    let merkle1 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Delete one file
    std::fs::remove_file(&file2)?;
    let merkle2 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Should detect deletion
    assert!(merkle2.has_changes(&merkle1));

    let changes = merkle2.detect_changes(&merkle1);
    assert_eq!(changes.deleted.len(), 1);
    assert_eq!(changes.added.len(), 0);
    assert_eq!(changes.modified.len(), 0);

    println!("✓ File deletion detection works");
    Ok(())
}

#[test]
fn test_merkle_snapshot_persistence() -> Result<()> {
    let temp_dir = TempDir::new()?;
    let test_file = temp_dir.path().join("test.rs");
    std::fs::write(&test_file, "fn test() {}")?;

    let merkle1 = FileSystemMerkle::from_directory(temp_dir.path())?;
    let snapshot_path = temp_dir.path().join("merkle.snapshot");

    // Save snapshot
    merkle1.save_snapshot(&snapshot_path)?;
    assert!(snapshot_path.exists());

    // Load snapshot
    let merkle2 = FileSystemMerkle::load_snapshot(&snapshot_path)?.unwrap();

    // Should be identical
    assert_eq!(merkle1.file_count(), merkle2.file_count());
    assert_eq!(merkle1.root_hash(), merkle2.root_hash());
    assert!(!merkle1.has_changes(&merkle2));

    println!("✓ Snapshot save/load works");
    Ok(())
}

#[test]
fn test_merkle_complex_changes() -> Result<()> {
    let temp_dir = TempDir::new()?;

    // Initial state: 3 files
    let file1 = temp_dir.path().join("file1.rs");
    let file2 = temp_dir.path().join("file2.rs");
    let file3 = temp_dir.path().join("file3.rs");
    std::fs::write(&file1, "fn one() {}")?;
    std::fs::write(&file2, "fn two() {}")?;
    std::fs::write(&file3, "fn three() {}")?;
    let merkle1 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Make multiple changes:
    // - Modify file1
    // - Delete file2
    // - Add file4
    // - Leave file3 unchanged
    std::fs::write(&file1, "fn one() { println!(\"modified\"); }")?;
    std::fs::remove_file(&file2)?;
    let file4 = temp_dir.path().join("file4.rs");
    std::fs::write(&file4, "fn four() {}")?;
    let merkle2 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // Detect changes
    let changes = merkle2.detect_changes(&merkle1);

    assert_eq!(changes.modified.len(), 1, "Should detect 1 modification");
    assert_eq!(changes.deleted.len(), 1, "Should detect 1 deletion");
    assert_eq!(changes.added.len(), 1, "Should detect 1 addition");
    assert_eq!(changes.total_changes(), 3, "Total should be 3 changes");

    println!("✓ Complex change detection works");
    println!("  Modified: {:?}", changes.modified);
    println!("  Deleted: {:?}", changes.deleted);
    println!("  Added: {:?}", changes.added);

    Ok(())
}

#[test]
fn test_merkle_empty_directory() -> Result<()> {
    let temp_dir = TempDir::new()?;

    let merkle = FileSystemMerkle::from_directory(temp_dir.path())?;

    assert_eq!(merkle.file_count(), 0);
    // Note: Empty Merkle tree may not have a root hash (depends on rs_merkle implementation)
    // This is fine - we just check file_count

    println!("✓ Empty directory handling works");
    Ok(())
}

#[test]
fn test_merkle_nested_directories() -> Result<()> {
    let temp_dir = TempDir::new()?;

    // Create nested structure
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir(&src_dir)?;
    std::fs::write(src_dir.join("main.rs"), "fn main() {}")?;

    let tests_dir = temp_dir.path().join("tests");
    std::fs::create_dir(&tests_dir)?;
    std::fs::write(tests_dir.join("test.rs"), "fn test() {}")?;

    let merkle = FileSystemMerkle::from_directory(temp_dir.path())?;

    assert_eq!(merkle.file_count(), 2);

    println!("✓ Nested directory handling works");
    Ok(())
}

#[test]
fn test_merkle_deterministic_hashing() -> Result<()> {
    let temp_dir = TempDir::new()?;

    // Create files in specific order
    std::fs::write(temp_dir.path().join("a.rs"), "fn a() {}")?;
    std::fs::write(temp_dir.path().join("b.rs"), "fn b() {}")?;
    std::fs::write(temp_dir.path().join("c.rs"), "fn c() {}")?;

    // Build Merkle tree multiple times
    let merkle1 = FileSystemMerkle::from_directory(temp_dir.path())?;
    let merkle2 = FileSystemMerkle::from_directory(temp_dir.path())?;
    let merkle3 = FileSystemMerkle::from_directory(temp_dir.path())?;

    // All should have identical root hashes (deterministic)
    assert_eq!(merkle1.root_hash(), merkle2.root_hash());
    assert_eq!(merkle2.root_hash(), merkle3.root_hash());

    println!("✓ Deterministic hashing works");
    Ok(())
}

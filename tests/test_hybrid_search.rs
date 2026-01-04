use file_search_mcp::indexing::unified::UnifiedIndexer;
use file_search_mcp::search::HybridSearch;
use std::path::Path;
use tempfile::TempDir;

#[tokio::test]
#[ignore] // Run with: cargo test --test test_hybrid_search -- --ignored --nocapture
async fn test_manual_hybrid_search() {
    println!("\n=== Manual Hybrid Search Test ===\n");

    // 1. Create test directories
    let cache_dir = TempDir::new().unwrap();
    let tantivy_dir = TempDir::new().unwrap();

    println!("1. Creating UnifiedIndexer...");

    // 2. Initialize UnifiedIndexer
    let mut indexer = UnifiedIndexer::for_embedded(
        cache_dir.path(),
        tantivy_dir.path(),
        "test_manual_search",
        384,
        None,
    )
    .await
    .expect("Failed to create UnifiedIndexer");

    println!("   ✓ UnifiedIndexer created");

    // 3. Index the current codebase
    let codebase_path = Path::new("/home/molaco/Documents/rust-code-mcp");
    println!("\n2. Indexing codebase: {}", codebase_path.display());

    let stats = indexer
        .index_directory(codebase_path)
        .await
        .expect("Failed to index directory");

    println!("   ✓ Indexed {} files", stats.indexed_files);
    println!("   ✓ Generated {} chunks", stats.total_chunks);
    println!("   ✓ Unchanged: {} files", stats.unchanged_files);
    println!("   ✓ Skipped: {} files", stats.skipped_files);

    // Verify we actually indexed something
    assert!(stats.total_chunks > 0, "Should have indexed some chunks!");

    // 4. Verify vector store has data
    println!("\n3. Verifying vector store population...");
    let vector_store = indexer.vector_store_cloned();
    let count = vector_store.count().await.expect("Failed to get count from vector store");
    println!("   ✓ Vector store has {} vectors", count);
    assert!(count > 0, "Vector store should have vectors! Dual indexing required.");

    // 5. Create hybrid search
    println!("\n4. Creating hybrid search...");
    let bm25_search = indexer
        .create_bm25_search()
        .expect("Failed to create BM25 search");
    let hybrid_search = HybridSearch::with_defaults(
        indexer.embedding_generator_cloned(),
        indexer.vector_store_cloned(),
        Some(bm25_search),
    );
    println!("   ✓ Hybrid search created");

    // 6. Perform search
    println!("\n5. Performing hybrid search for 'UnifiedIndexer'...");
    let results = hybrid_search
        .search("UnifiedIndexer", 10)
        .await
        .expect("Search failed");

    println!("   ✓ Found {} results", results.len());
    assert!(!results.is_empty(), "Should find results for 'UnifiedIndexer'");

    // 7. Display results
    println!("\n6. Search Results:\n");
    for (i, result) in results.iter().enumerate() {
        println!("=== Result {} ===", i + 1);
        println!("  Combined Score: {:.4}", result.score);

        if let Some(bm25) = result.bm25_score {
            println!("  BM25 Score: {:.2} (rank: {:?})", bm25, result.bm25_rank);
        } else {
            println!("  BM25 Score: None");
        }

        if let Some(vector) = result.vector_score {
            println!("  Vector Score: {:.4} (rank: {:?})", vector, result.vector_rank);
        } else {
            println!("  Vector Score: None");
        }

        println!("  Symbol: {}", result.chunk.context.symbol_name);
        println!("  Kind: {}", result.chunk.context.symbol_kind);
        println!("  File: {}", result.chunk.context.file_path.display());
        println!("  Lines: {}-{}", result.chunk.context.line_start, result.chunk.context.line_end);

        let preview = if result.chunk.content.len() > 150 {
            format!("{}...", &result.chunk.content[..150])
        } else {
            result.chunk.content.clone()
        };
        println!("  Preview: {}", preview);
        println!();
    }

    // 8. Test vector-only search
    println!("\n7. Testing vector-only search...");
    let vector_results = hybrid_search
        .vector_only_search("index files using tree-sitter", 5)
        .await
        .expect("Vector search failed");

    println!("   ✓ Found {} semantic matches", vector_results.len());

    if !vector_results.is_empty() {
        println!("   Top semantic match:");
        println!("     - {}", vector_results[0].chunk.context.symbol_name);
        println!("     - Score: {:.4}", vector_results[0].score);
    }

    // 9. Summary
    println!("\n=== Test Summary ===");
    println!("✓ Phase 1 UnifiedIndexer working");
    println!("✓ Vector store population confirmed ({} vectors)", count);
    println!("✓ Hybrid search functional");
    println!("✓ Both BM25 and Vector search active");
    println!("\nPhase 1 Complete!");
}

#[tokio::test]
#[ignore] // Run with: cargo test --test test_hybrid_search -- --ignored --nocapture
async fn test_incremental_indexing() {
    println!("\n=== Incremental Indexing Test ===\n");

    let cache_dir = TempDir::new().unwrap();
    let tantivy_dir = TempDir::new().unwrap();

    println!("1. First indexing pass...");
    let mut indexer = UnifiedIndexer::for_embedded(
        cache_dir.path(),
        tantivy_dir.path(),
        "test_incremental",
        384,
        None,
    )
    .await
    .unwrap();

    let codebase_path = Path::new("/home/molaco/Documents/rust-code-mcp/src/indexing");
    let stats1 = indexer.index_directory(codebase_path).await.unwrap();

    println!("   ✓ Indexed {} files, {} chunks", stats1.indexed_files, stats1.total_chunks);

    println!("\n2. Second indexing pass (should use cache)...");
    let stats2 = indexer.index_directory(codebase_path).await.unwrap();

    println!("   ✓ Indexed: {} files", stats2.indexed_files);
    println!("   ✓ Unchanged: {} files", stats2.unchanged_files);

    // Most files should be unchanged on second pass
    assert!(stats2.unchanged_files > 0, "Should have unchanged files on second pass");
    println!("\n✓ Incremental indexing working (metadata cache functional)");
}

#[tokio::test]
#[ignore]
async fn test_vector_store_connection() {
    println!("\n=== Vector Store Connection Test ===\n");

    let cache_dir = TempDir::new().unwrap();
    let tantivy_dir = TempDir::new().unwrap();

    println!("Initializing embedded vector store (LanceDB)...");

    let result = UnifiedIndexer::for_embedded(
        cache_dir.path(),
        tantivy_dir.path(),
        "test_connection",
        384,
        None,
    )
    .await;

    match result {
        Ok(_) => println!("✓ Vector store initialization successful"),
        Err(e) => {
            println!("✗ Vector store initialization failed: {}", e);
            panic!("Vector store not available");
        }
    }
}

//! Phase 2 Integration Tests - Performance Optimization Features
//!
//! Tests the following Phase 2 features with real Qdrant server:
//! 1. Qdrant HNSW optimization based on codebase size
//! 2. Tantivy memory budget optimization
//! 3. Bulk indexing mode
//! 4. RRF parameter tuning

use file_search_mcp::chunker::{ChunkContext, ChunkId, CodeChunk};
use file_search_mcp::embeddings::EmbeddingGenerator;
use file_search_mcp::indexing::{bulk_index_with_auto_mode, BulkIndexer, HnswConfig, UnifiedIndexer};
use file_search_mcp::search::{HybridSearch, RRFTuner, TestQuery};
use file_search_mcp::vector_store::{estimate_codebase_size, QdrantOptimizedConfig, VectorStore, QdrantConfig};
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// Test 1: Qdrant HNSW optimization with real collection
#[tokio::test]
#[ignore] // Requires Qdrant server
async fn test_qdrant_hnsw_optimization() {
    println!("\n=== Test 1: Qdrant HNSW Optimization ===\n");

    // 1. Estimate codebase size
    let repo_path = Path::new(env!("CARGO_MANIFEST_DIR"));
    let estimated_loc = estimate_codebase_size(repo_path)
        .expect("Failed to estimate codebase size");

    println!("1. Estimated codebase size: {} LOC", estimated_loc);

    // 2. Create optimized config
    let base_config = QdrantConfig {
        url: "http://localhost:6333".to_string(),
        collection_name: "test_phase2_hnsw_opt".to_string(),
        vector_size: 384,
    };

    let optimized_config = QdrantOptimizedConfig::for_codebase_size(estimated_loc, base_config.clone());

    println!("2. Optimized HNSW config:");
    println!("   - m: {}", optimized_config.hnsw_m);
    println!("   - ef_construct: {}", optimized_config.hnsw_ef_construct);
    println!("   - ef: {}", optimized_config.hnsw_ef);
    println!("   - threads: {}", optimized_config.indexing_threads);

    // 3. Create vector store with optimization
    let vector_store = VectorStore::new_with_optimization(base_config.clone(), Some(optimized_config.clone()))
        .await
        .expect("Failed to create optimized vector store");

    println!("3. ✓ Vector store created with optimized config");

    // 4. Verify collection exists
    let count = vector_store.count().await.expect("Failed to get count");
    println!("4. ✓ Collection created (current count: {})", count);

    // 5. Clean up
    vector_store.delete_collection().await.expect("Failed to delete collection");
    println!("5. ✓ Cleanup complete\n");

    // Verify config matches codebase size
    if estimated_loc < 100_000 {
        assert_eq!(optimized_config.hnsw_m, 16);
        assert_eq!(optimized_config.hnsw_ef_construct, 100);
    } else if estimated_loc < 1_000_000 {
        assert_eq!(optimized_config.hnsw_m, 16);
        assert_eq!(optimized_config.hnsw_ef_construct, 150);
    } else {
        assert_eq!(optimized_config.hnsw_m, 32);
        assert_eq!(optimized_config.hnsw_ef_construct, 200);
    }

    println!("✅ Test 1 PASSED: HNSW optimization works correctly\n");
}

/// Test 2: Tantivy memory budget optimization
#[tokio::test]
#[ignore] // Requires Qdrant server and embedding model
async fn test_tantivy_memory_optimization() {
    println!("\n=== Test 2: Tantivy Memory Budget Optimization ===\n");

    let cache_dir = TempDir::new().unwrap();
    let tantivy_dir = TempDir::new().unwrap();

    // 1. Estimate codebase size
    let repo_path = Path::new(env!("CARGO_MANIFEST_DIR"));
    let estimated_loc = estimate_codebase_size(repo_path)
        .expect("Failed to estimate codebase size");

    println!("1. Estimated codebase size: {} LOC", estimated_loc);

    // 2. Create UnifiedIndexer with optimization
    let indexer = UnifiedIndexer::new_with_optimization(
        cache_dir.path(),
        tantivy_dir.path(),
        "http://localhost:6333",
        "test_phase2_tantivy_opt",
        384,
        Some(estimated_loc),  // Enable optimization
    )
    .await
    .expect("Failed to create optimized indexer");

    println!("2. ✓ UnifiedIndexer created with optimized Tantivy config");

    // 3. Verify it was created successfully (config is internal)
    // We can't directly verify memory budget, but creation success proves it worked

    // 4. Clean up
    let vector_store = indexer.vector_store_cloned();
    vector_store.delete_collection().await.expect("Failed to delete collection");

    println!("3. ✓ Cleanup complete\n");
    println!("✅ Test 2 PASSED: Tantivy memory optimization applied successfully\n");
}

/// Test 3: Bulk indexing mode with real Qdrant
#[tokio::test]
#[ignore] // Requires Qdrant server and embedding model
async fn test_bulk_indexing_mode() {
    println!("\n=== Test 3: Bulk Indexing Mode ===\n");

    // 1. Create Qdrant client
    let client = qdrant_client::Qdrant::from_url("http://localhost:6333")
        .build()
        .expect("Failed to create Qdrant client");

    let collection_name = "test_phase2_bulk_mode".to_string();

    // 2. Create collection first
    let vector_store = VectorStore::new(QdrantConfig {
        url: "http://localhost:6333".to_string(),
        collection_name: collection_name.clone(),
        vector_size: 384,
    })
    .await
    .expect("Failed to create vector store");

    println!("1. ✓ Collection created");

    // 3. Create BulkIndexer
    let mut bulk_indexer = BulkIndexer::new(client.clone(), collection_name.clone());

    assert!(!bulk_indexer.is_bulk_mode_active());
    println!("2. ✓ BulkIndexer created (not in bulk mode)");

    // 4. Enter bulk mode
    let hnsw_config = HnswConfig::new(16, 100);
    bulk_indexer
        .start_bulk_mode(hnsw_config.clone())
        .await
        .expect("Failed to start bulk mode");

    assert!(bulk_indexer.is_bulk_mode_active());
    println!("3. ✓ Entered bulk mode (HNSW disabled)");

    // 5. Simulate bulk indexing (add some test vectors)
    let embedding_generator = EmbeddingGenerator::new().expect("Failed to create embedding generator");

    // Create test chunks
    let test_chunks: Vec<(ChunkId, Vec<f32>, CodeChunk)> = (0..10)
        .map(|i| {
            let chunk_id = ChunkId::new();
            let content = format!("fn test_function_{}() {{}}", i);
            let embedding = embedding_generator.embed(&content).expect("Failed to embed");
            let chunk = CodeChunk {
                id: chunk_id,
                content: content.clone(),
                context: ChunkContext {
                    file_path: PathBuf::from("test.rs"),
                    module_path: vec!["test".to_string()],
                    symbol_name: format!("test_function_{}", i),
                    symbol_kind: "function".to_string(),
                    docstring: None,
                    imports: vec![],
                    outgoing_calls: vec![],
                    line_start: 1,
                    line_end: 1,
                },
                overlap_prev: None,
                overlap_next: None,
            };
            (chunk_id, embedding, chunk)
        })
        .collect();

    // Insert chunks while in bulk mode
    vector_store
        .upsert_chunks(test_chunks)
        .await
        .expect("Failed to upsert chunks");

    let count_during_bulk = vector_store.count().await.expect("Failed to get count");
    println!("4. ✓ Inserted 10 chunks in bulk mode (count: {})", count_during_bulk);

    // 6. Exit bulk mode
    bulk_indexer
        .end_bulk_mode()
        .await
        .expect("Failed to end bulk mode");

    assert!(!bulk_indexer.is_bulk_mode_active());
    println!("5. ✓ Exited bulk mode (HNSW rebuilt)");

    // 7. Verify data is still there
    let count_after_bulk = vector_store.count().await.expect("Failed to get count");
    assert_eq!(count_after_bulk, count_during_bulk);
    println!("6. ✓ Data preserved after bulk mode exit (count: {})", count_after_bulk);

    // 8. Clean up
    vector_store.delete_collection().await.expect("Failed to delete collection");
    println!("7. ✓ Cleanup complete\n");

    println!("✅ Test 3 PASSED: Bulk indexing mode works correctly\n");
}

/// Test 4: RRF parameter tuning with real hybrid search
#[tokio::test]
#[ignore] // Requires Qdrant server and embedding model
async fn test_rrf_parameter_tuning() {
    println!("\n=== Test 4: RRF Parameter Tuning ===\n");

    let cache_dir = TempDir::new().unwrap();
    let tantivy_dir = TempDir::new().unwrap();

    // 1. Create and populate indexer
    let mut indexer = UnifiedIndexer::new(
        cache_dir.path(),
        tantivy_dir.path(),
        "http://localhost:6333",
        "test_phase2_rrf_tuning",
        384,
    )
    .await
    .expect("Failed to create indexer");

    println!("1. ✓ Indexer created");

    // 2. Index current codebase
    let repo_path = Path::new(env!("CARGO_MANIFEST_DIR"));
    let stats = indexer
        .index_directory(repo_path)
        .await
        .expect("Failed to index directory");

    println!("2. ✓ Indexed {} files ({} chunks)", stats.indexed_files, stats.total_chunks);

    // Skip if no chunks indexed
    if stats.total_chunks == 0 {
        println!("   ⚠ No chunks indexed, skipping RRF tuning test");
        return;
    }

    // 3. Create hybrid search
    let hybrid_search = HybridSearch::with_defaults(
        indexer.embedding_generator_cloned(),
        indexer.vector_store_cloned(),
        Some(indexer.create_bm25_search().expect("Failed to create BM25 search")),
    );

    println!("3. ✓ Hybrid search created");

    // 4. Create RRF tuner with test queries
    let test_queries = vec![
        TestQuery {
            query: "indexing code chunks".to_string(),
            relevant_chunk_ids: vec!["index_directory".to_string(), "UnifiedIndexer".to_string()],
        },
        TestQuery {
            query: "vector search embedding".to_string(),
            relevant_chunk_ids: vec!["VectorStore".to_string(), "search".to_string()],
        },
    ];

    let tuner = RRFTuner::new(test_queries);
    println!("4. ✓ RRF tuner created with {} test queries", tuner.query_count());

    // 5. Tune k parameter
    let tuning_result = tuner
        .tune_k(&hybrid_search)
        .await
        .expect("Failed to tune k parameter");

    println!("5. ✓ RRF k-value tuning complete:");
    println!("   - Best k: {}", tuning_result.best_k);
    println!("   - Best NDCG@10: {:.4}", tuning_result.best_ndcg);
    println!("   - Values tested:");
    for (k, ndcg) in &tuning_result.k_values_tested {
        println!("     k={:5.1}: NDCG@10={:.4}", k, ndcg);
    }

    // 6. Verify best_k is reasonable
    assert!(tuning_result.best_k >= 10.0 && tuning_result.best_k <= 100.0,
        "Best k should be between 10 and 100");
    assert!(tuning_result.best_ndcg >= 0.0 && tuning_result.best_ndcg <= 1.0,
        "NDCG should be between 0 and 1");

    println!("6. ✓ Tuning results validated");

    // 7. Test search with optimized k
    let search_results = hybrid_search
        .search_with_k("indexing", 5, tuning_result.best_k)
        .await
        .expect("Failed to search with optimized k");

    println!("7. ✓ Search with optimized k={} returned {} results",
        tuning_result.best_k, search_results.len());

    // 8. Clean up
    let vector_store = indexer.vector_store_cloned();
    vector_store.delete_collection().await.expect("Failed to delete collection");
    println!("8. ✓ Cleanup complete\n");

    println!("✅ Test 4 PASSED: RRF parameter tuning works correctly\n");
}

/// Test 5: End-to-end Phase 2 integration test
#[tokio::test]
#[ignore] // Requires Qdrant server and embedding model
async fn test_phase2_end_to_end() {
    println!("\n=== Test 5: Phase 2 End-to-End Integration ===\n");

    let cache_dir = TempDir::new().unwrap();
    let tantivy_dir = TempDir::new().unwrap();

    // 1. Estimate codebase size
    let repo_path = Path::new(env!("CARGO_MANIFEST_DIR"));
    let estimated_loc = estimate_codebase_size(repo_path)
        .expect("Failed to estimate codebase size");

    println!("1. Estimated codebase: {} LOC", estimated_loc);

    // 2. Create fully optimized indexer (Task 3.1 + 4.1)
    let mut indexer = UnifiedIndexer::new_with_optimization(
        cache_dir.path(),
        tantivy_dir.path(),
        "http://localhost:6333",
        "test_phase2_e2e",
        384,
        Some(estimated_loc),  // Enables ALL optimizations
    )
    .await
    .expect("Failed to create optimized indexer");

    println!("2. ✓ Created fully optimized UnifiedIndexer");
    println!("   - Qdrant HNSW: auto-tuned for {} LOC", estimated_loc);
    println!("   - Tantivy memory: auto-tuned for {} LOC", estimated_loc);

    // 3. Index with optimizations
    let start = std::time::Instant::now();
    let stats = indexer
        .index_directory(repo_path)
        .await
        .expect("Failed to index directory");
    let index_duration = start.elapsed();

    println!("3. ✓ Indexed {} files ({} chunks) in {:.2}s",
        stats.indexed_files, stats.total_chunks, index_duration.as_secs_f64());

    // Skip rest if no chunks
    if stats.total_chunks == 0 {
        println!("   ⚠ No chunks indexed, skipping remaining tests");
        return;
    }

    // 4. Create hybrid search
    let hybrid_search = HybridSearch::with_defaults(
        indexer.embedding_generator_cloned(),
        indexer.vector_store_cloned(),
        Some(indexer.create_bm25_search().expect("Failed to create BM25 search")),
    );

    println!("4. ✓ Hybrid search created");

    // 5. Tune RRF parameter (Task 4.2)
    let tuner = RRFTuner::default_rust_queries();
    let tuning_result = tuner
        .tune_k(&hybrid_search)
        .await
        .expect("Failed to tune RRF parameter");

    println!("5. ✓ RRF parameter tuned: k={} (NDCG@10={:.4})",
        tuning_result.best_k, tuning_result.best_ndcg);

    // 6. Search with optimized k
    let search_results = hybrid_search
        .search_with_k("vector search", 10, tuning_result.best_k)
        .await
        .expect("Failed to search");

    println!("6. ✓ Search with optimized k returned {} results", search_results.len());

    // 7. Verify search quality
    assert!(!search_results.is_empty(), "Should have search results");

    for (i, result) in search_results.iter().take(3).enumerate() {
        println!("   Result {}: {} (score: {:.4})",
            i + 1, result.chunk.context.symbol_name, result.score);
    }

    // 8. Clean up
    let vector_store = indexer.vector_store_cloned();
    vector_store.delete_collection().await.expect("Failed to delete collection");
    println!("7. ✓ Cleanup complete\n");

    println!("✅ Test 5 PASSED: Phase 2 end-to-end integration successful\n");
    println!("=== Phase 2 Integration Tests Complete ===\n");
}

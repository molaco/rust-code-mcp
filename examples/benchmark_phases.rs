//! Benchmark each indexing phase separately

use file_search_mcp::indexing::incremental::IncrementalIndexer;
use file_search_mcp::indexing::merkle::FileSystemMerkle;
use file_search_mcp::parser::RustParser;
use file_search_mcp::chunker::Chunker;
use file_search_mcp::embeddings::EmbeddingGenerator;
use file_search_mcp::security::SensitiveFileFilter;
use file_search_mcp::security::secrets::SecretsScanner;
use std::path::PathBuf;
use std::time::Instant;
use walkdir::WalkDir;
use rayon::prelude::*;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let dir = PathBuf::from("/home/molaco/Documents/rust-code-mcp-final");

    println!("\n{}", "=".repeat(60));
    println!("PHASE BENCHMARK");
    println!("{}\n", "=".repeat(60));

    // PHASE 0: Merkle tree building
    println!("PHASE 0: MERKLE TREE");
    let merkle_start = Instant::now();
    let merkle = FileSystemMerkle::from_directory(&dir)?;
    let merkle_time = merkle_start.elapsed();
    println!("  Built Merkle tree ({} files) in {:.2}s\n",
             merkle.file_count(), merkle_time.as_secs_f64());

    // Collect Rust files
    let rust_files: Vec<PathBuf> = WalkDir::new(&dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "rs").unwrap_or(false))
        .filter(|e| !e.path().to_string_lossy().contains("target"))
        .map(|e| e.path().to_path_buf())
        .collect();

    println!("Found {} Rust files\n", rust_files.len());

    // PHASE 1: Parse all files (sequential to isolate timing)
    println!("PHASE 1: PARSING (sequential)");
    let parse_start = Instant::now();
    let mut all_chunks = Vec::new();
    let mut parse_errors = 0;
    let chunker = Chunker::new();

    for file in &rust_files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => { parse_errors += 1; continue; }
        };

        let mut parser = match RustParser::new() {
            Ok(p) => p,
            Err(_) => { parse_errors += 1; continue; }
        };

        let parse_result = match parser.parse_source_complete(&content) {
            Ok(r) => r,
            Err(_) => { parse_errors += 1; continue; }
        };

        let chunks = match chunker.chunk_file(file, &content, &parse_result) {
            Ok(c) => c,
            Err(_) => { parse_errors += 1; continue; }
        };

        all_chunks.extend(chunks);
    }
    let parse_time = parse_start.elapsed();
    println!("  Parsed {} files -> {} chunks in {:.2}s ({:.1} files/sec)",
             rust_files.len() - parse_errors, all_chunks.len(),
             parse_time.as_secs_f64(),
             (rust_files.len() - parse_errors) as f64 / parse_time.as_secs_f64());
    println!("  Errors: {}\n", parse_errors);

    // PHASE 1b: Parse with Rayon (parallel)
    println!("PHASE 1b: PARSING (parallel with Rayon)");
    let parse_par_start = Instant::now();
    let chunks_par: Vec<_> = rust_files.par_iter()
        .filter_map(|file| {
            let content = std::fs::read_to_string(file).ok()?;
            let mut parser = RustParser::new().ok()?;
            let parse_result = parser.parse_source_complete(&content).ok()?;
            chunker.chunk_file(file, &content, &parse_result).ok()
        })
        .flatten()
        .collect();
    let parse_par_time = parse_par_start.elapsed();
    println!("  Parsed -> {} chunks in {:.2}s ({:.1} files/sec)\n",
             chunks_par.len(),
             parse_par_time.as_secs_f64(),
             rust_files.len() as f64 / parse_par_time.as_secs_f64());

    // PHASE 2: Embedding (batched like actual indexer)
    println!("PHASE 2: EMBEDDING (batch size 128)");

    // Measure generator creation separately
    let gen_start = Instant::now();
    let generator = EmbeddingGenerator::new()
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?;
    let gen_time = gen_start.elapsed();
    println!("  Generator init: {:.2}s", gen_time.as_secs_f64());

    // Warmup like test_gpu_speed does
    println!("  Warming up GPU...");
    let warmup_texts: Vec<String> = (0..16).map(|i| format!("fn warmup_{}() {{}}", i)).collect();
    let _ = generator.embed_batch(warmup_texts)
        .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
            Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
        })?;
    println!("  Warmup complete");

    let embed_start = Instant::now();

    let mut embeddings = Vec::new();
    let batch_size = 128; // Real chunks are bigger, need smaller batch
    for (i, chunk_batch) in all_chunks.chunks(batch_size).enumerate() {
        let chunk_texts: Vec<String> = chunk_batch.iter()
            .map(|c| c.format_for_embedding())
            .collect();
        let batch_embeddings = generator.embed_batch(chunk_texts)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                Box::new(std::io::Error::new(std::io::ErrorKind::Other, e.to_string()))
            })?;
        embeddings.extend(batch_embeddings);
        if i == 0 {
            println!("  First batch: {} chunks", chunk_batch.len());
        }
    }
    let embed_time = embed_start.elapsed();
    println!("  Generated {} embeddings in {:.2}s ({:.0} chunks/sec)\n",
             embeddings.len(),
             embed_time.as_secs_f64(),
             embeddings.len() as f64 / embed_time.as_secs_f64());

    // PHASE 3: LanceDB writes (simulated with actual backend)
    println!("PHASE 3: LANCEDB WRITES");
    let index_start = Instant::now();

    // We'll use the actual incremental indexer for accurate timing
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        use file_search_mcp::vector_store::VectorStore;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let vector_store = VectorStore::new_embedded(
            temp_dir.path().join("vectors"),
            384
        ).await.unwrap();

        // Simulate per-file writes (current behavior)
        let per_file_start = Instant::now();
        let chunks_per_file = all_chunks.len() / rust_files.len().max(1);
        let mut written = 0;

        for (i, chunk_batch) in all_chunks.chunks(chunks_per_file.max(1)).enumerate() {
            if i >= 20 { break; } // Sample 20 files
            let chunk_data: Vec<_> = chunk_batch.iter()
                .zip(embeddings[written..written+chunk_batch.len()].iter())
                .map(|(chunk, emb)| (chunk.id, emb.clone(), chunk.clone()))
                .collect();
            written += chunk_batch.len();
            vector_store.upsert_chunks(chunk_data).await.unwrap();
        }
        let per_file_time = per_file_start.elapsed();
        let estimated_total = per_file_time.as_secs_f64() * (rust_files.len() as f64 / 20.0);
        println!("  Per-file writes (20 samples): {:.2}s", per_file_time.as_secs_f64());
        println!("  Estimated total for {} files: {:.2}s\n", rust_files.len(), estimated_total);

        // Simulate batch write (proposed optimization)
        let batch_start = Instant::now();
        let all_chunk_data: Vec<_> = all_chunks.iter()
            .zip(embeddings.iter())
            .map(|(chunk, emb)| (chunk.id, emb.clone(), chunk.clone()))
            .collect();
        vector_store.upsert_chunks(all_chunk_data).await.unwrap();
        let batch_time = batch_start.elapsed();
        println!("  Batch write (all {} chunks): {:.2}s", all_chunks.len(), batch_time.as_secs_f64());
    });

    let index_time = index_start.elapsed();

    // PHASE 4: Tantivy indexing
    println!("\nPHASE 4: TANTIVY INDEXING");
    let tantivy_start = Instant::now();
    {
        use file_search_mcp::indexing::tantivy_adapter::TantivyAdapter;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config = file_search_mcp::config::TantivyConfig::for_codebase_size(
            temp_dir.path(), None
        );
        let mut adapter = TantivyAdapter::new(config).unwrap();

        for chunk in &all_chunks {
            adapter.index_chunk(chunk).unwrap();
        }
        adapter.commit().unwrap();
    }
    let tantivy_time = tantivy_start.elapsed();
    println!("  Indexed {} chunks in {:.2}s ({:.0} chunks/sec)",
             all_chunks.len(), tantivy_time.as_secs_f64(),
             all_chunks.len() as f64 / tantivy_time.as_secs_f64());

    // PHASE 5: Full IncrementalIndexer (the actual flow)
    println!("\nPHASE 5: ACTUAL INCREMENTALINDEXER FLOW");
    let actual_start = Instant::now();

    let rt2 = tokio::runtime::Runtime::new()?;
    let actual_time = rt2.block_on(async {
        use directories::ProjectDirs;

        // Use the SAME paths as MCP tool
        let data_dir = ProjectDirs::from("dev", "rust-code-mcp", "search")
            .map(|dirs| dirs.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".rust-code-mcp"));

        // Hash the directory path like MCP does
        let dir_hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(dir.to_string_lossy().as_bytes());
            format!("{:x}", hasher.finalize())
        };

        let cache_path = data_dir.join("cache").join(&dir_hash);
        let tantivy_path = data_dir.join("index").join(&dir_hash);
        let collection_name = format!("code_chunks_{}", &dir_hash[..8]);

        println!("  Using MCP paths:");
        println!("    cache: {}", cache_path.display());
        println!("    index: {}", tantivy_path.display());
        println!("    collection: {}", collection_name);

        // Delete existing snapshot to force full reindex
        let snapshot_path = file_search_mcp::indexing::incremental::get_snapshot_path(&dir);
        let _ = std::fs::remove_file(&snapshot_path);
        println!("  Deleted snapshot to force full reindex");

        let init_start = Instant::now();
        let mut indexer = IncrementalIndexer::new(
            &cache_path,
            &tantivy_path,
            &collection_name,
            384,
            None
        ).await.expect("Failed to create indexer");
        let init_time = init_start.elapsed();
        println!("  Indexer init: {:.2}s", init_time.as_secs_f64());

        // Also clear the metadata cache to ensure all files are processed
        indexer.indexer_mut().clear_all_data().await.expect("Failed to clear data");

        let index_start = Instant::now();
        let stats = indexer.index_with_change_detection(&dir).await
            .expect("Indexing failed");
        let index_time = index_start.elapsed();

        println!("  Indexed {} files, {} chunks in {:.2}s",
                 stats.indexed_files, stats.total_chunks, index_time.as_secs_f64());

        init_time + index_time
    });

    println!("  Total actual flow: {:.2}s", actual_time.as_secs_f64());

    println!("\n{}", "=".repeat(60));
    println!("SUMMARY");
    println!("{}", "=".repeat(60));
    println!("Merkle tree:         {:.2}s", merkle_time.as_secs_f64());
    println!("Parse (sequential):  {:.2}s", parse_time.as_secs_f64());
    println!("Parse (parallel):    {:.2}s", parse_par_time.as_secs_f64());
    println!("Generator init:      {:.2}s", gen_time.as_secs_f64());
    println!("Embed:               {:.2}s", embed_time.as_secs_f64());
    println!("Tantivy:             {:.2}s", tantivy_time.as_secs_f64());
    let total = merkle_time.as_secs_f64() + parse_par_time.as_secs_f64() +
                gen_time.as_secs_f64() + embed_time.as_secs_f64() + tantivy_time.as_secs_f64();
    println!("─────────────────────────────");
    println!("Phases sum:          {:.2}s", total);
    println!("Actual full flow:    {:.2}s", actual_time.as_secs_f64());
    println!("Difference:          {:.2}s", actual_time.as_secs_f64() - total);
    println!("{}\n", "=".repeat(60));

    Ok(())
}

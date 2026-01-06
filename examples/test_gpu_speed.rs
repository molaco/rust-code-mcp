//! Simple GPU embedding speed test - no Qdrant needed

use file_search_mcp::embeddings::EmbeddingGenerator;
use std::time::Instant;
use tracing_subscriber::EnvFilter;

fn main() {
    // Initialize tracing to see CUDA debug logs
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Err(e) = run() {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error + Send>> {
    println!("\n{}", "=".repeat(60));
    println!("GPU EMBEDDING SPEED TEST");
    println!("{}\n", "=".repeat(60));

    println!("Initializing embedding generator with GPU...");
    let generator = EmbeddingGenerator::new()
        .map_err(|e| format!("Failed to create generator: {}", e))
        .map_err(|e| Box::new(std::io::Error::new(std::io::ErrorKind::Other, e)) as Box<dyn std::error::Error + Send>)?;

    // Test data - simulate 750 code chunks
    let test_texts: Vec<String> = (0..750)
        .map(|i| format!("fn test_function_{}() {{\n    println!(\"Hello from function {}\");\n    let x = {};\n    return x * 2;\n}}", i, i, i))
        .collect();

    println!("Generated {} test chunks\n", test_texts.len());

    // Warmup
    println!("Warming up GPU...");
    let _ = generator.embed_batch(test_texts[..16].to_vec())?;
    println!("Warmup complete\n");

    // Benchmark with batch size 256 (our optimization)
    println!("Testing with batch size 256 (optimized):");
    let start = Instant::now();
    let mut processed = 0;

    for (i, batch) in test_texts.chunks(256).enumerate() {
        let batch_start = Instant::now();
        let _ = generator.embed_batch(batch.to_vec())?;
        let batch_time = batch_start.elapsed();
        processed += batch.len();
        println!("  Batch {}: {} chunks in {:.3}s ({:.0} chunks/sec)",
                 i + 1, batch.len(), batch_time.as_secs_f64(),
                 batch.len() as f64 / batch_time.as_secs_f64());
    }

    let total_time = start.elapsed();
    let throughput = processed as f64 / total_time.as_secs_f64();

    println!("\n{}", "=".repeat(60));
    println!("RESULTS");
    println!("{}", "=".repeat(60));
    println!("Total chunks:        {}", processed);
    println!("Total time:          {:.2}s", total_time.as_secs_f64());
    println!("Throughput:          {:.0} chunks/sec", throughput);

    println!("\n{}", "=".repeat(60));
    if throughput > 150.0 {
        println!("✅ GPU IS ACTIVE!");
        println!("   {:.0} chunks/sec indicates GPU acceleration", throughput);
        println!("   Estimated speedup vs CPU: {:.1}x", throughput / 50.0);
    } else {
        println!("⚠️  GPU may be inactive");
        println!("   {:.0} chunks/sec is close to CPU speeds", throughput);
    }

    if total_time.as_secs_f64() < 5.0 {
        println!("🎉 Excellent performance!");
    } else if total_time.as_secs_f64() < 10.0 {
        println!("✅ Good performance");
    }

    println!("{}\n", "=".repeat(60));

    Ok(())
}

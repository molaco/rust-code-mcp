//! Time each pass of `LanceDbBackend::maintain` separately on a COPY of a
//! vector store: `cargo run --release -p rmc-engine --features vector-store
//! --example maintain_probe -- <collection dir> [merge N]`.
//!
//! The live log only prints the whole pass (136–425 s on a large workspace), which
//! cannot tell compaction, the scalar/ANN index update and prune apart. Never
//! point it at a store a daemon has open: the passes write.

use std::time::Instant;

use lancedb::table::{CompactionOptions, OptimizeAction, OptimizeOptions};

async fn report(table: &lancedb::Table, when: &str) {
    let stats = table.stats().await.unwrap();
    println!(
        "[{when}] version {} rows {} fragments {} small {}",
        table.version().await.unwrap(),
        stats.num_rows,
        stats.fragment_stats.num_fragments,
        stats.fragment_stats.num_small_fragments,
    );
    // `list_indices` lists one entry per DELTA of an index, all under the
    // same name (196 for the ANN index of a large workspace on 2026-10-31),
    // and every `index_stats` costs seconds there: one line per name.
    let mut seen = std::collections::HashSet::new();
    let started = Instant::now();
    let configs = table.list_indices().await.unwrap();
    println!(
        "  list_indices: {} entries in {:.2}s",
        configs.len(),
        started.elapsed().as_secs_f64()
    );
    for config in configs {
        if !seen.insert(config.name.clone()) {
            continue;
        }
        let started = Instant::now();
        let index = table.index_stats(&config.name).await.unwrap().unwrap();
        println!(
            "  index {} {:?} {:?}: parts {:?}, indexed {}, unindexed {} (index_stats {:.2}s)",
            config.name,
            config.columns,
            index.index_type,
            index.num_indices,
            index.num_indexed_rows,
            index.num_unindexed_rows,
            started.elapsed().as_secs_f64(),
        );
    }
}

/// What one changed sync writes: `rows` rows appended as one fragment (here
/// copies of existing rows — the index pass does not care whose vectors).
async fn append_like_a_sync(table: &lancedb::Table, rows: usize) {
    use futures::TryStreamExt;
    use lancedb::query::{ExecutableQuery, QueryBase};
    let batches: Vec<arrow_array::RecordBatch> = table
        .query()
        .limit(rows)
        .execute()
        .await
        .unwrap()
        .try_collect()
        .await
        .unwrap();
    let started = Instant::now();
    table.add(batches).execute().await.unwrap();
    println!(
        "appended {rows} rows in {:.2}s",
        started.elapsed().as_secs_f64()
    );
}

/// One `OptimizeAction::Compact` with the target `maintain` uses, timed.
async fn compact(table: &lancedb::Table, when: &str) {
    let started = Instant::now();
    let stats = table
        .optimize(OptimizeAction::Compact {
            options: CompactionOptions {
                target_rows_per_fragment: 100_000,
                ..CompactionOptions::default()
            },
            remap_options: None,
        })
        .await
        .unwrap()
        .compaction
        .unwrap_or_default();
    println!(
        "Compact ({when}) took {:.2}s: {} fragment(s) removed, {} added",
        started.elapsed().as_secs_f64(),
        stats.fragments_removed,
        stats.fragments_added,
    );
    report(table, &format!("after compact {when}")).await;
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = &args[1];
    let merge = args.get(2).map(|n| n.parse::<usize>().unwrap());

    let db = lancedb::connect(dir).execute().await.unwrap();
    let table = db.open_table("vectors").execute().await.unwrap();
    report(&table, "before").await;

    // The compaction `maintain` runs (same target), timed on its own: lance
    // bins only neighbours covered by the SAME set of index deltas, so with
    // one ANN delta per sync it can compact almost nothing.
    compact(&table, "before merge").await;

    // `RMC_PROBE_APPEND=N`: append N rows first, as a changed sync does, so
    // the index pass below has new rows to index (a delta to write).
    if let Ok(rows) = std::env::var("RMC_PROBE_APPEND") {
        append_like_a_sync(&table, rows.parse().unwrap()).await;
    }

    let started = Instant::now();
    table
        .optimize(OptimizeAction::Index(OptimizeOptions::default()))
        .await
        .unwrap();
    println!("Index(default) took {:.2}s", started.elapsed().as_secs_f64());
    report(&table, "after Index(default)").await;

    let started = Instant::now();
    table
        .optimize(OptimizeAction::Index(OptimizeOptions::default()))
        .await
        .unwrap();
    println!(
        "Index(default) again, nothing new: {:.2}s",
        started.elapsed().as_secs_f64()
    );

    if let Some(n) = merge {
        let started = Instant::now();
        table
            .optimize(OptimizeAction::Index(OptimizeOptions::merge(n)))
            .await
            .unwrap();
        println!("Index(merge {n}) took {:.2}s", started.elapsed().as_secs_f64());
        report(&table, "after merge").await;

        compact(&table, "after merge").await;

        // The next changed sync, now against one delta.
        if let Ok(rows) = std::env::var("RMC_PROBE_APPEND") {
            append_like_a_sync(&table, rows.parse().unwrap()).await;
        }

        let started = Instant::now();
        table
            .optimize(OptimizeAction::Index(OptimizeOptions::default()))
            .await
            .unwrap();
        println!(
            "Index(default) after merge: {:.2}s",
            started.elapsed().as_secs_f64()
        );
    }

    let started = Instant::now();
    table
        .optimize(OptimizeAction::Prune {
            older_than: Some(lancedb::table::optimize::Duration::seconds(600)),
            delete_unverified: None,
            error_if_tagged_old_versions: None,
        })
        .await
        .unwrap();
    println!("Prune took {:.2}s", started.elapsed().as_secs_f64());
}

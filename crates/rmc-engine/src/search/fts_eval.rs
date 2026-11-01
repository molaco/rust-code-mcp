//! Opt-in comparison of the pinned Lance FTS with the production BM25 schema.
//! No live writes: `RMC_FTS_SOURCE` is a copied collection (containing vectors.lance),
//! `RMC_FTS_WORK` is the session scratch directory, and `RMC_LANCE_PROBE_QUERIES` is
//! a project-owned text file, one query per line. Run the ignored test in release.
//! `RMC_FTS_POSITIONS=false` measures a separate candidate without positions;
//! the default is true. Linux only (process CPU is read from procfs).
//! Both Lance tables retain every source column; FTS adds a search text column.
//! they have id/path BTree indices but no ANN. Shared vector maintenance is thus
//! excluded from BOTH sides. FTS uses content + symbol + docstring in one column;
//! ranking equivalence to Tantivy's three fields is measured, not presumed.

use super::Bm25Search;
use crate::chunker::CodeChunk;
use arrow_array::{Array, RecordBatch, RecordBatchIterator, RecordBatchReader, StringArray};
use arrow_schema::{DataType, Field, Schema};
use futures::TryStreamExt;
use lancedb::index::scalar::{BTreeIndexBuilder, FtsIndexBuilder, FullTextSearchQuery};
use lancedb::index::Index;
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use lancedb::table::{CompactionOptions, OptimizeAction, OptimizeOptions};
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

fn cpu() -> f64 {
    let stat = std::fs::read_to_string("/proc/self/stat").unwrap();
    let fields: Vec<_> = stat[stat.rfind(')').unwrap() + 2..].split_whitespace().collect();
    // Linux USER_HZ is 100 on the benchmark host.
    (fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()) as f64 / 100.0
}

fn report(label: &str, start: Instant, before: f64) {
    eprintln!("MEASURE {label}: wall_s={:.4} cpu_s={:.4}", start.elapsed().as_secs_f64(), cpu() - before);
}

fn bytes(path: &Path) -> u64 {
    std::fs::read_dir(path).unwrap().map(|entry| {
        let path = entry.unwrap().path();
        if path.is_dir() { bytes(&path) } else { path.metadata().unwrap().len() }
    }).sum()
}

fn strings(batch: &RecordBatch, column: &str) -> Vec<String> {
    batch.column_by_name(column).unwrap().as_any().downcast_ref::<StringArray>()
        .unwrap().iter().map(|v| v.unwrap().to_string()).collect()
}

fn chunks(batches: &[RecordBatch]) -> Vec<CodeChunk> {
    batches.iter().flat_map(|b| strings(b, "chunk_json")).map(|s| serde_json::from_str(&s).unwrap()).collect()
}

fn with_text(batch: &RecordBatch) -> RecordBatch {
    let text: Vec<String> = chunks(std::slice::from_ref(batch)).iter().map(|c| {
        format!("{}\n{}\n{}", c.content, c.context.symbol_name, c.context.docstring.as_deref().unwrap_or(""))
    }).collect();
    let mut fields: Vec<_> = batch.schema().fields().iter().cloned().collect();
    fields.push(Arc::new(Field::new("search_text", DataType::Utf8, false)));
    let mut columns = batch.columns().to_vec();
    columns.push(Arc::new(StringArray::from(text)));
    RecordBatch::try_new(Arc::new(Schema::new(fields)), columns).unwrap()
}

fn reader(batches: Vec<RecordBatch>) -> Box<dyn RecordBatchReader + Send> {
    let schema = batches[0].schema();
    Box::new(RecordBatchIterator::new(batches.into_iter().map(Ok), schema))
}

// Match Tantivy's default simple/lowercase tokenizer: no English stemming or
// stop words, tokens <= 40 bytes. Keep positions so phrase support is available.
fn code_fts() -> FtsIndexBuilder {
    FtsIndexBuilder::default().stem(false).remove_stop_words(false)
        .ascii_folding(false).with_position(true)
}

async fn fts(table: &lancedb::Table, query: &str, k: usize) -> Vec<String> {
    let batches: Vec<RecordBatch> = table.query()
        .full_text_search(FullTextSearchQuery::new(query.to_string()).with_column("search_text".to_string()).unwrap())
        .select(Select::columns(&["id"]))
        .limit(k).execute().await.unwrap().try_collect().await.unwrap();
    batches.iter().flat_map(|b| strings(b, "id")).collect()
}

fn bm25(index: &Bm25Search, query: &str, k: usize) -> Vec<String> {
    index.search(query, k).unwrap().into_iter().map(|(id, _)| id.to_string()).collect()
}

async fn indexed_table(db: &lancedb::Connection, name: &str, batches: Vec<RecordBatch>) -> lancedb::Table {
    let table = db.create_table(name, reader(batches)).execute().await.unwrap();
    for column in ["id", "file_path"] {
        table.create_index(&[column], Index::BTree(BTreeIndexBuilder::default())).execute().await.unwrap();
    }
    table
}

fn writer(index: &Bm25Search) -> tantivy::IndexWriter {
    let writer = index.index().writer_with_num_threads(2, 100_000_000).unwrap();
    let mut policy = tantivy::indexer::LogMergePolicy::default();
    policy.set_del_docs_ratio_before_merge(0.3);
    policy.set_min_num_segments(8);
    policy.set_min_layer_size(10_000);
    writer.set_merge_policy(Box::new(policy));
    writer
}

async fn queries(table: &lancedb::Table, index: &Bm25Search, queries: &[String], label: &str) {
    for k in [10, 100] {
        for q in queries.iter().take(3) { fts(table, q, k).await; bm25(index, q, k); }
        let mut overlaps = 0;
        let mut denominator = 0;
        for engine in ["tantivy", "lance"] {
            let mut times = vec![];
            let before = cpu();
            // Amortize the 10 ms CPU counter resolution over 240 queries for
            // a 48-query input, rather than reporting just a few clock ticks.
            let measured_queries = queries.len() * 5;
            for q in queries.iter().cycle().take(measured_queries) {
                let start = Instant::now();
                if engine == "tantivy" { bm25(index, q, k); } else { fts(table, q, k).await; }
                times.push(start.elapsed().as_secs_f64() * 1000.0);
            }
            let cpu_per_query = (cpu() - before) / measured_queries as f64;
            times.sort_by(f64::total_cmp);
            eprintln!("QUERY {label} {engine} k={k} p50_ms={:.3} p95_ms={:.3} cpu_s={cpu_per_query:.5}", times[times.len()/2], times[times.len()*95/100]);
        }
        for q in queries {
            let expected = bm25(index, q, k);
            let actual = fts(table, q, k).await;
            let expected_set: HashSet<_> = expected.iter().collect();
            let common = actual.iter().filter(|id| expected_set.contains(id)).count();
            overlaps += common;
            denominator += expected.len();
            eprintln!("OVERLAP {label} k={k} common={common} tantivy={} lance={} q={q:?}", expected.len(), actual.len());
        }
        eprintln!("OVERLAP_TOTAL {label} k={k} common={overlaps} denominator={denominator} ratio={:.5}", overlaps as f64 / denominator.max(1) as f64);
    }
}

async fn cycle(table: &lancedb::Table, batch: Vec<RecordBatch>, filter: &str, label: &str) {
    let start = Instant::now(); let before = cpu();
    table.delete(filter).await.unwrap();
    table.add(reader(batch)).execute().await.unwrap();
    report(&format!("{label} delete_add"), start, before);
    let start = Instant::now(); let before = cpu();
    table.optimize(OptimizeAction::Index(OptimizeOptions::default())).await.unwrap();
    report(&format!("{label} optimize_index"), start, before);
}

async fn compact(table: &lancedb::Table, label: &str) {
    let start = Instant::now(); let before = cpu();
    let result = table.optimize(OptimizeAction::Compact {
        options: CompactionOptions { target_rows_per_fragment: 100_000, ..Default::default() },
        remap_options: None,
    }).await;
    report(label, start, before);
    let result = result.unwrap();
    eprintln!("COMPACT {label}: {result:?}");
}

#[tokio::test]
async fn fts_probe_checks_delete_append_and_tokenization() {
    let root = std::env::var("RMC_FTS_WORK").unwrap_or_else(|_| format!("{}/t/scratch", std::env::var("HOME").unwrap()));
    std::fs::create_dir_all(&root).unwrap();
    let dir = tempfile::tempdir_in(root).unwrap();
    let db = lancedb::connect(dir.path().to_str().unwrap()).execute().await.unwrap();
    let schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false), Field::new("search_text", DataType::Utf8, false)]));
    let batch = RecordBatch::try_new(schema.clone(), vec![Arc::new(StringArray::from(vec!["old", "other"])), Arc::new(StringArray::from(vec!["fn parse_file src/search/bm25.rs", "fn unrelated() {}"]))]).unwrap();
    let table = db.create_table("oracle", reader(vec![batch])).execute().await.unwrap();
    table.create_index(&["search_text"], Index::FTS(code_fts())).execute().await.unwrap();
    for q in ["parse_file", "src/search/bm25.rs"] { assert_eq!(fts(&table, q, 10).await, ["old"]); }
    table.delete("id = 'old'").await.unwrap();
    let new = RecordBatch::try_new(schema, vec![Arc::new(StringArray::from(vec!["fresh"])), Arc::new(StringArray::from(vec!["fn parse_file src/search/bm25.rs"]))]).unwrap();
    table.add(reader(vec![new])).execute().await.unwrap();
    assert_eq!(fts(&table, "parse_file", 10).await, ["fresh"], "unindexed append and deletes must be visible");
    table.optimize(OptimizeAction::Index(OptimizeOptions::default())).await.unwrap();
    assert_eq!(fts(&table, "parse_file", 10).await, ["fresh"]);
    compact(&table, "oracle").await;
    assert_eq!(fts(&table, "parse_file", 10).await, ["fresh"]);
    // A MatchQuery tokenizes snake_case but ORs its tokens. Tantivy's query
    // parser makes that identifier a phrase instead: this is an observable
    // semantic difference, even with identical tokenizer options.
    let partial = RecordBatch::try_new(table.schema().await.unwrap(), vec![
        Arc::new(StringArray::from(vec!["partial"])),
        Arc::new(StringArray::from(vec!["fn parse_config() {}"])),
    ]).unwrap();
    table.add(reader(vec![partial])).execute().await.unwrap();
    assert_eq!(fts(&table, "parse_file", 10).await.into_iter().collect::<BTreeSet<_>>(), BTreeSet::from(["fresh".to_string(), "partial".to_string()]));
    let mut lexical = Bm25Search::new(&dir.path().join("tantivy")).unwrap();
    let schema = lexical.schema();
    let exact = crate::chunker::ChunkId::new().to_string();
    let other = crate::chunker::ChunkId::new().to_string();
    let mut w = writer(&lexical);
    use tantivy::doc;
    for (id, content) in [(&exact, "fn parse_file() {}"), (&other, "fn parse_config() {}")] {
        w.add_document(doc!(schema.chunk_id => id.clone(), schema.content => content)).unwrap();
    }
    w.commit().unwrap(); w.wait_merging_threads().unwrap(); lexical.reload().unwrap();
    assert_eq!(bm25(&lexical, "parse_file", 10), [exact]);
}

#[tokio::test]
#[ignore]
async fn probe_lance_fts_on_copied_collection() {
    let root = std::path::PathBuf::from(std::env::var("RMC_FTS_WORK").unwrap());
    let source = std::path::PathBuf::from(std::env::var("RMC_FTS_SOURCE").unwrap());
    assert!(source.canonicalize().unwrap().starts_with(root.canonicalize().unwrap()), "source must be a snapshot inside session scratch");
    let run = tempfile::tempdir_in(&root).unwrap();
    let positions = std::env::var("RMC_FTS_POSITIONS").map(|v| v.parse::<bool>().unwrap()).unwrap_or(true);
    eprintln!("CONFIG positions={positions}");
    let db = lancedb::connect(run.path().to_str().unwrap()).execute().await.unwrap();
    let source_db = lancedb::connect(source.to_str().unwrap()).execute().await.unwrap();
    let source_table = source_db.open_table("vectors").execute().await.unwrap();
    let raw: Vec<RecordBatch> = source_table.query().execute().await.unwrap().try_collect().await.unwrap();
    let all_chunks = chunks(&raw);
    for (id, chunk) in raw.iter().flat_map(|b| strings(b, "id")).zip(&all_chunks) { assert_eq!(id, chunk.id.to_string()); }
    assert_eq!(all_chunks.len(), source_table.count_rows(None).await.unwrap());
    assert_eq!(all_chunks.iter().map(|c| c.id).collect::<HashSet<_>>().len(), all_chunks.len(), "the snapshot must have unique chunk ids");
    let start = Instant::now(); let before = cpu();
    let batches: Vec<_> = raw.iter().map(with_text).collect();
    report("materialize_text", start, before);
    let plain = indexed_table(&db, "plain", raw.clone()).await;
    let candidate = indexed_table(&db, "fts", batches.clone()).await;
    let baseline_bytes = bytes(&run.path().join("fts.lance"));
    let index = Bm25Search::new(&run.path().join("tantivy")).unwrap();
    let start = Instant::now(); let before = cpu();
    let mut w = writer(&index);
    for c in &all_chunks { w.add_document(index.schema().document(c)).unwrap(); }
    w.commit().unwrap(); w.wait_merging_threads().unwrap();
    report("tantivy_build", start, before);
    let mut index = index; index.reload().unwrap();
    let start = Instant::now(); let before = cpu();
    candidate.create_index(&["search_text"], Index::FTS(code_fts().with_position(positions))).execute().await.unwrap();
    report("fts_build", start, before);
    eprintln!("CORPUS rows={} source_version={} source_stats={:?}", all_chunks.len(), source_table.version().await.unwrap(), source_table.stats().await.unwrap());
    eprintln!("SIZE source={} plain={} fts={} fts_index_delta={} tantivy={}", bytes(&source), bytes(&run.path().join("plain.lance")), bytes(&run.path().join("fts.lance")), bytes(&run.path().join("fts.lance")) - baseline_bytes, bytes(&run.path().join("tantivy")));
    let query_text = std::fs::read_to_string(std::env::var("RMC_LANCE_PROBE_QUERIES").unwrap()).unwrap();
    let qs: Vec<String> = query_text.lines().map(str::trim).filter(|s| !s.is_empty() && !s.starts_with('#')).map(str::to_owned).collect();
    assert!(!qs.is_empty());
    queries(&candidate, &index, &qs, "initial").await;
    let paths: Vec<String> = all_chunks.iter().map(|c| c.context.file_path.display().to_string()).collect::<BTreeSet<_>>().into_iter().collect();
    assert!(paths.len() >= 160);
    for round in 0..8 {
        let offset = round * (paths.len() - 20) / 8;
        let selected: HashSet<_> = paths[offset..offset+20].iter().cloned().collect();
        let filter = format!("file_path IN ({})", selected.iter().map(|s| format!("'{}'", s.replace('\'', "''"))).collect::<Vec<_>>().join(","));
        let replacement: Vec<RecordBatch> = candidate.query().only_if(&filter).execute().await.unwrap().try_collect().await.unwrap();
        let replaced_chunks = chunks(&replacement);
        eprintln!("SYNC round={round} files=20 rows={}", replaced_chunks.len());
        let without_text = replacement.iter().map(|b| b.project(&(0..b.num_columns()-1).collect::<Vec<_>>()).unwrap()).collect();
        cycle(&plain, without_text, &filter, &format!("plain_{round}")).await;
        let start = Instant::now(); let before = cpu();
        let mut w = writer(&index);
        for path in &selected { w.delete_term(tantivy::Term::from_field_text(index.schema().file_path, path)); }
        for c in &replaced_chunks { w.add_document(index.schema().document(c)).unwrap(); }
        w.commit().unwrap(); w.wait_merging_threads().unwrap(); index.reload().unwrap();
        report(&format!("tantivy_{round}"), start, before);
        cycle(&candidate, replacement, &filter, &format!("fts_{round}")).await;
        assert_eq!(candidate.count_rows(None).await.unwrap(), all_chunks.len());
        assert_eq!(plain.count_rows(None).await.unwrap(), all_chunks.len());
        assert_eq!(index.index().reader().unwrap().searcher().num_docs() as usize, all_chunks.len());
    }
    queries(&candidate, &index, &qs, "after_eight_syncs").await;
    compact(&plain, "plain_compact").await;
    compact(&candidate, "fts_compact").await;
    queries(&candidate, &index, &qs, "after_compact").await;
    let before_prune = fts(&candidate, &qs[0], 10).await;
    for (table, label) in [(&plain, "plain_prune"), (&candidate, "fts_prune")] {
        let start = Instant::now(); let before = cpu();
        table.optimize(OptimizeAction::Prune {
            older_than: Some(lancedb::table::optimize::Duration::zero()),
            delete_unverified: Some(false),
            error_if_tagged_old_versions: Some(false),
        }).await.unwrap();
        report(label, start, before);
        assert_eq!(table.count_rows(None).await.unwrap(), all_chunks.len());
    }
    assert_eq!(fts(&candidate, &qs[0], 10).await, before_prune);
    eprintln!("PRUNED_SIZE plain={} fts={} tantivy={}", bytes(&run.path().join("plain.lance")), bytes(&run.path().join("fts.lance")), bytes(&run.path().join("tantivy")));
}

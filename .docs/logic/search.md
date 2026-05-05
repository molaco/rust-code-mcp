# search — Detailed Logic

## Module: mod

### `HybridSearchConfig::default() -> Self`
**Call graph:** (none)
**Steps:**
1. Returns a `HybridSearchConfig` with `bm25_weight=0.5`, `vector_weight=0.5`, `rrf_k=60.0`, and `candidate_count=100`.

### `VectorSearch::new(embedding_generator: EmbeddingGenerator, vector_store: VectorStore) -> Self`
**Call graph:** (none)
**Steps:**
1. Constructs a `VectorSearch` struct holding the supplied `embedding_generator` and `vector_store`.

### `VectorSearch::search(&self, query: &str, limit: usize) -> Result<Vec<VectorSearchResult>, SearchError>`
**Call graph:** EmbeddingGenerator::embed_async -> VectorStore::search -> SearchError::VectorStore
**Steps:**
1. Generates an embedding for `query` by calling `embedding_generator.embed_async(query.to_string()).await`, propagating any `EmbeddingError` via `?`.
2. Calls `vector_store.search(query_embedding, limit).await`, mapping any error to `SearchError::VectorStore` and returning the resulting `Vec<VectorSearchResult>`.

### `HybridSearch::new(embedding_generator, vector_store, bm25_search, config) -> Self`
**Call graph:** VectorSearch::new
**Steps:**
1. Creates an inner `VectorSearch` from the embedding generator and vector store via `VectorSearch::new`.
2. Stores the optional `bm25_search` and the supplied `config` on the new `HybridSearch`.

### `HybridSearch::with_defaults(embedding_generator, vector_store, bm25_search) -> Self`
**Call graph:** HybridSearch::new -> HybridSearchConfig::default
**Steps:**
1. Builds a default `HybridSearchConfig` and forwards everything to `HybridSearch::new` to obtain a fully configured instance.

### `HybridSearch::search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError>`
**Call graph:** HybridSearch::search_with_k
**Steps:**
1. Delegates to `self.search_with_k(query, limit, self.config.rrf_k)` so the configured RRF `k` is used.

### `HybridSearch::search_with_k(&self, query: &str, limit: usize, rrf_k: f32) -> Result<Vec<SearchResult>, SearchError>`
**Call graph:** Bm25Search::clone -> tokio::join! -> VectorSearch::search -> tokio::task::spawn_blocking -> Bm25Search::search -> SearchError::Bm25 -> HybridSearch::reciprocal_rank_fusion_with_k
**Steps:**
1. If a `Bm25Search` is configured, clones it and the query string and reads `candidate_count` from the config.
2. Uses `tokio::join!` to await `vector_search.search(query, candidate_count)` concurrently with a `spawn_blocking` task that invokes `bm25_clone.search(&query_clone, candidate_count)`.
3. Unwraps the vector future via `?`, then unwraps the join handle (mapping `JoinError` into `SearchError::Bm25`) and the inner `Result` (mapping the boxed error into `SearchError::Bm25`).
4. If no BM25 backend is configured, falls back to `vector_search.search(query, candidate_count).await` and uses an empty BM25 result vector.
5. Calls `self.reciprocal_rank_fusion_with_k` to merge the two ranked lists with the supplied `rrf_k`.
6. Truncates the merged list to `limit` items via `into_iter().take(limit).collect()` and returns it.

### `HybridSearch::reciprocal_rank_fusion_with_k(&self, vector_results, bm25_results, k) -> Vec<SearchResult>` (private)
**Call graph:** reciprocal_rank_fusion_core
**Steps:**
1. Maps the borrowed `VectorSearchResult` slice into a `Vec<(ChunkId, f32, CodeChunk)>` by cloning each chunk.
2. Forwards the converted vector list, the BM25 list, `k`, and the configured `vector_weight`/`bm25_weight` to `reciprocal_rank_fusion_core`.

### `HybridSearch::vector_only_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>, SearchError>`
**Call graph:** VectorSearch::search
**Steps:**
1. Calls `vector_search.search(query, limit).await` and propagates errors with `?`.
2. Enumerates the returned vector results to assign 1-based ranks.
3. Maps each `(rank, VectorSearchResult)` into a `SearchResult` whose `score`/`vector_score` come from the result, with BM25 fields set to `None` and `vector_rank = Some(rank + 1)`.
4. Collects the iterator into a `Vec<SearchResult>` and returns it.

### `HybridSearch::reciprocal_rank_fusion_static(bm25_results, vector_results, k) -> Vec<SearchResult>`
**Call graph:** reciprocal_rank_fusion_core
**Steps:**
1. Converts each `bm25_results` `SearchResult` into `(chunk_id, bm25_score.unwrap_or(score), chunk)` triples.
2. Converts each `vector_results` `SearchResult` into `(chunk_id, vector_score.unwrap_or(score), chunk)` triples.
3. Calls `reciprocal_rank_fusion_core` with both lists, `k`, and equal weights of `0.5`.

### `reciprocal_rank_fusion_core(vector_results, bm25_results, k, vector_weight, bm25_weight) -> Vec<SearchResult>` (private)
**Call graph:** HashMap::entry -> partial_cmp
**Steps:**
1. Initializes an empty `HashMap<ChunkId, RrfScore>` to accumulate fused scores.
2. Iterates `vector_results` with `enumerate`, computing `rrf_score = 1.0 / (k + (rank + 1))` and inserting/updating an `RrfScore` whose `rrf_score` is incremented by `rrf_score * vector_weight` and stamped with the vector raw score and 1-based rank.
3. Iterates `bm25_results` with `enumerate`, computing the same RRF formula and updating the same map by adding `rrf_score * bm25_weight` plus stamping the BM25 raw score and rank.
4. Drains the map's values into a `Vec<SearchResult>`, copying RRF totals, both raw scores/ranks, the chunk id, and the chunk.
5. Sorts the vector descending by `score` using `b.score.partial_cmp(&a.score).unwrap_or(Ordering::Equal)` and returns it.

## Module: bm25

### `Bm25Search::new(index_path: &Path) -> Result<Self, Box<dyn std::error::Error + Send>>`
**Call graph:** ChunkSchema::new -> Path::join -> Path::exists -> Index::open_in_dir -> std::fs::create_dir_all -> Index::create_in_dir -> Index::reader
**Steps:**
1. Builds a fresh `ChunkSchema` via `ChunkSchema::new()`.
2. Checks whether `index_path/meta.json` exists; if so opens the existing Tantivy index with `Index::open_in_dir`, otherwise creates the directory tree with `std::fs::create_dir_all` and a new index via `Index::create_in_dir(index_path, schema.schema())`.
3. Each Tantivy/IO error is boxed into `Box<dyn std::error::Error + Send>` via `map_err`.
4. Acquires an `IndexReader` from the index using `index.reader()`.
5. Returns the `Bm25Search { index, schema, reader }`.

### `Bm25Search::from_index(index: Index) -> Result<Self, Box<dyn std::error::Error + Send>>`
**Call graph:** ChunkSchema::new -> Index::reader
**Steps:**
1. Creates a fresh `ChunkSchema` instance.
2. Calls `index.reader()` on the supplied Tantivy `Index`, boxing any error.
3. Wraps the index, schema, and reader in a `Bm25Search` and returns it.

### `Bm25Search::search(&self, query: &str, limit: usize) -> Result<Vec<(ChunkId, f32, CodeChunk)>, Box<dyn std::error::Error + Send>>`
**Call graph:** IndexReader::searcher -> QueryParser::for_index -> QueryParser::parse_query -> Searcher::search -> TopDocs::with_limit -> Searcher::doc -> TantivyDocument::get_first -> Value::as_str -> ChunkId::from_string -> serde_json::from_str
**Steps:**
1. Obtains a `Searcher` snapshot via `self.reader.searcher()`.
2. Builds a `QueryParser` over the `content`, `symbol_name`, and `docstring` fields with `QueryParser::for_index`.
3. Parses `query` using `query_parser.parse_query(query)`, boxing parse errors.
4. Executes the query with `searcher.search(&query, &TopDocs::with_limit(limit))`, again boxing errors.
5. Iterates each `(score, doc_address)` pair: fetches the document via `searcher.doc(doc_address)`, extracts the `chunk_id` text field, and converts it into a `ChunkId` via `ChunkId::from_string` (mapping failure to `io::Error::InvalidData`).
6. Extracts the `chunk_json` text field and deserializes it back to a `CodeChunk` with `serde_json::from_str`.
7. Pushes `(chunk_id, score, chunk)` into `results` and returns the collected vector after the loop.

### `Bm25Search::index(&self) -> &Index`
**Call graph:** (none)
**Steps:**
1. Returns a borrowed reference to the underlying Tantivy `Index`.

### `Bm25Search::schema(&self) -> &ChunkSchema`
**Call graph:** (none)
**Steps:**
1. Returns a borrowed reference to the cached `ChunkSchema`.

### `Bm25Search::reload(&mut self) -> Result<(), Box<dyn std::error::Error + Send>>`
**Call graph:** IndexReader::reload
**Steps:**
1. Calls `self.reader.reload()` to pick up freshly committed segments.
2. Boxes any Tantivy error and returns `Ok(())` on success.

### `impl Clone for Bm25Search :: clone(&self) -> Self`
**Call graph:** Index::clone -> ChunkSchema::clone -> IndexReader::clone
**Steps:**
1. Clones the inner `Index` (cheap, `Arc`-backed), the `ChunkSchema`, and the `IndexReader`.
2. Returns a new `Bm25Search` that shares the underlying index and reader so it can be used from parallel tasks.

## Module: error

### `enum SearchError`
**Call graph:** (derived) thiserror::Error::source / Display, From<EmbeddingError>, From<VectorStoreError>
**Steps:**
1. Defines four variants: `Embedding(EmbeddingError)` with auto `From`, `VectorStore(VectorStoreError)` with auto `From`, `Bm25(Box<dyn std::error::Error + Send>)` for boxed BM25 failures, and `NoResults` for empty searches.
2. The `#[error("...")]` attributes generate `Display` impls; `#[from]` attributes generate `From` conversions used elsewhere in the crate.

## Module: resilient

### `ResilientHybridSearch::new(bm25, vector_store, embedding_generator, rrf_k) -> Self`
**Call graph:** Option::map -> Arc::new -> AtomicBool::new
**Steps:**
1. Wraps each provided component in `Arc` via `Option::map(Arc::new)` so it can be shared across async tasks.
2. Stores the `rrf_k` parameter and creates an `Arc<AtomicBool>` initialized to `false` for `fallback_mode` tracking.
3. Returns the constructed `ResilientHybridSearch`.

### `ResilientHybridSearch::with_defaults(bm25, vector_store, embedding_generator) -> Self`
**Call graph:** ResilientHybridSearch::new
**Steps:**
1. Calls `ResilientHybridSearch::new` with `rrf_k = 60.0` to obtain an instance preconfigured with the standard RRF k.

### `ResilientHybridSearch::search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>>`
**Call graph:** ResilientHybridSearch::try_hybrid_search -> AtomicBool::store -> tracing::warn -> ResilientHybridSearch::fallback_search
**Steps:**
1. Awaits `self.try_hybrid_search(query, limit)`.
2. On `Ok`, clears `fallback_mode` to `false` (Relaxed) and returns the results.
3. On `Err`, logs a warning via `tracing::warn!`, sets `fallback_mode` to `true`, and awaits `self.fallback_search(query, limit)` to attempt degraded modes.

### `ResilientHybridSearch::is_fallback_mode(&self) -> bool`
**Call graph:** AtomicBool::load
**Steps:**
1. Returns `self.fallback_mode.load(Ordering::Relaxed)`.

### `ResilientHybridSearch::try_hybrid_search(&self, query, limit) -> Result<Vec<SearchResult>>` (private)
**Call graph:** tokio::join! -> ResilientHybridSearch::bm25_search -> ResilientHybridSearch::vector_search -> ResilientHybridSearch::merge_results -> tracing::warn -> anyhow!
**Steps:**
1. Uses `tokio::join!` to run `self.bm25_search(query, limit)` and `self.vector_search(query, limit)` concurrently.
2. If both succeed, calls `self.merge_results` to fuse the two lists with RRF.
3. If only BM25 succeeds, logs the vector failure and returns BM25 results unchanged.
4. If only vector succeeds, logs the BM25 failure and returns vector results unchanged.
5. If both fail, returns `anyhow!("Both search engines failed - BM25: {}, Vector: {}", ...)` describing both errors.

### `ResilientHybridSearch::fallback_search(&self, query, limit) -> Result<Vec<SearchResult>>` (private)
**Call graph:** ResilientHybridSearch::bm25_search -> ResilientHybridSearch::vector_search -> tracing::info -> anyhow!
**Steps:**
1. Tries `self.bm25_search(query, limit)` first because it has no external dependencies; on success logs an info message and returns.
2. Falls through to `self.vector_search(query, limit)`; on success logs an info message and returns.
3. If both attempts fail, returns `anyhow!("All search engines unavailable - both BM25 and vector failed")`.

### `ResilientHybridSearch::bm25_search(&self, query, limit) -> Result<Vec<SearchResult>>` (private)
**Call graph:** Option::as_ref -> anyhow! -> Arc::clone -> tokio::task::spawn_blocking -> Bm25Search::search -> Result::context
**Steps:**
1. Resolves `self.bm25.as_ref()`, returning `anyhow!("BM25 search not configured")` if missing.
2. Clones the `Arc<Bm25Search>` and the query string for the blocking task.
3. Calls `tokio::task::spawn_blocking(move || bm25_clone.search(&query_clone, limit))`, awaiting and `.context("BM25 search task failed")`-wrapping any join error.
4. Maps any inner BM25 error into `anyhow!("BM25 search failed: {:?}", e)`.
5. Maps each `(chunk_id, score, chunk)` triple into a `SearchResult` with `score`, `bm25_score=Some(score)`, and the remaining vector/rank fields set to `None`.

### `ResilientHybridSearch::vector_search(&self, query, limit) -> Result<Vec<SearchResult>>` (private)
**Call graph:** Option::as_ref -> anyhow! -> EmbeddingGenerator::embed -> VectorStore::search
**Steps:**
1. Resolves `self.vector_store.as_ref()` and `self.embedding_generator.as_ref()`, erroring with `anyhow!` when either is missing.
2. Calls `embedding_generator.embed(query)` synchronously and maps failure to `anyhow!`.
3. Awaits `vector_store.search(query_embedding, limit)`, mapping failure to `anyhow!`.
4. Enumerates the returned results to compute 1-based ranks and maps each into a `SearchResult` with `vector_score=Some(score)`, `vector_rank=Some(rank+1)` and BM25 fields `None`.

### `ResilientHybridSearch::merge_results(&self, bm25_results, vector_results) -> Vec<SearchResult>` (private)
**Call graph:** HybridSearch::reciprocal_rank_fusion_static
**Steps:**
1. Forwards both lists and `self.rrf_k` to `HybridSearch::reciprocal_rank_fusion_static` for canonical RRF merging.

## Module: rrf_tuner

### `RRFTuner::new(test_queries: Vec<TestQuery>) -> Self`
**Call graph:** (none)
**Steps:**
1. Stores `test_queries` directly on a new `RRFTuner` and returns it.

### `RRFTuner::default_rust_queries() -> Self`
**Call graph:** (none)
**Steps:**
1. Builds a hard-coded list of eight `TestQuery` entries covering common Rust topics (CLI parsing, async HTTP, error handling, JSON serialization, filesystem reads, vector search, rust-analyzer, and search index creation).
2. Each `TestQuery` pairs the query text with a list of three `relevant_chunk_ids` (symbol names) used as ground truth.
3. Returns the populated `RRFTuner`.

### `RRFTuner::tune_k(&self, hybrid_search: &HybridSearch) -> Result<TuningResult, Box<dyn std::error::Error + Send + Sync>>`
**Call graph:** tracing::info -> HybridSearch::search_with_k -> calculate_ndcg
**Steps:**
1. Hardcodes the candidate `k_values` slice `[10.0, 20.0, 40.0, 60.0, 80.0, 100.0]` and initializes `best_k=60.0`, `best_ndcg=0.0`, and an empty `results` vector.
2. Logs a tuning-start info message including the query count.
3. For each candidate `k`, iterates every `test_query`, awaits `hybrid_search.search_with_k(&query, 20, *k)` (mapping errors to `format!("Search failed: {}", e)`), and accumulates the per-query NDCG@10 from `calculate_ndcg(&search_results, &relevant_chunk_ids, 10)`.
4. Computes `avg_ndcg = total_ndcg / test_queries.len()`, pushes `(k, avg_ndcg)` into `results`, and logs the value via `tracing::info!`.
5. Tracks the maximum, updating `best_ndcg`/`best_k` whenever `avg_ndcg > best_ndcg`.
6. Logs the final optimal `(best_k, best_ndcg)` and returns a `TuningResult` containing them along with the list of tested `(k, ndcg)` pairs.

### `RRFTuner::tune_k_verbose(&self, hybrid_search: &HybridSearch) -> Result<TuningResult, Box<dyn std::error::Error + Send + Sync>>`
**Call graph:** tracing::info -> tracing::debug -> HybridSearch::search_with_k -> calculate_ndcg -> calculate_mrr
**Steps:**
1. Mirrors `tune_k` but additionally collects per-query `(query, ndcg, mrr)` tuples by also calling `calculate_mrr` for each result set.
2. After each `k` sweep, emits per-query `tracing::debug!` lines showing NDCG and MRR.
3. Selects the best k by NDCG (same rule as `tune_k`) and returns a `TuningResult` with the same shape.

### `RRFTuner::query_count(&self) -> usize`
**Call graph:** Vec::len
**Steps:**
1. Returns `self.test_queries.len()`.

### `calculate_ndcg(results: &[SearchResult], relevant: &[String], k: usize) -> f64` (private)
**Call graph:** Iterator::take -> Iterator::enumerate -> Iterator::filter -> Vec::contains -> f64::log2 -> Iterator::sum
**Steps:**
1. Computes DCG by taking the top `k` results, filtering those whose `chunk.context.symbol_name` is in `relevant`, and summing `1.0 / ((i + 2) as f64).log2()` over their indices.
2. Computes IDCG over the first `k.min(relevant.len())` ideal positions using the same `1.0 / log2(i + 2)` discount.
3. If `ideal_dcg == 0.0`, returns `0.0`; otherwise returns `dcg / ideal_dcg`.

### `calculate_mrr(results: &[SearchResult], relevant: &[String]) -> f64` (private)
**Call graph:** Iterator::position -> Vec::contains -> Option::map -> Option::unwrap_or
**Steps:**
1. Finds the first index where the result's symbol name is in `relevant` via `Iterator::position`.
2. Maps that position to `1.0 / (pos + 1)`.
3. Returns `0.0` when no relevant result is found via `unwrap_or(0.0)`.

### `calculate_map(results: &[SearchResult], relevant: &[String]) -> f64` (private)
**Call graph:** Iterator::enumerate -> Vec::contains
**Steps:**
1. Tracks `relevant_found` and `sum_precision` counters initialized to zero.
2. Walks each `(i, result)` pair; whenever the symbol name is in `relevant`, increments `relevant_found` and adds `relevant_found / (i + 1)` to `sum_precision`.
3. Returns `0.0` if `relevant.is_empty()`, otherwise `sum_precision / relevant.len()`.

### `calculate_recall_at_k(results: &[SearchResult], relevant: &[String], k: usize) -> f64` (private)
**Call graph:** Iterator::take -> Iterator::filter -> Vec::contains -> Iterator::count
**Steps:**
1. Counts the relevant matches among the first `k` results.
2. Returns `0.0` when `relevant` is empty, else `found / relevant.len()`.

### `calculate_precision_at_k(results: &[SearchResult], relevant: &[String], k: usize) -> f64` (private)
**Call graph:** Iterator::take -> Iterator::filter -> Vec::contains -> Iterator::count -> usize::min
**Steps:**
1. Counts relevant matches among the first `k` results.
2. Returns `found / k.min(results.len())`, guarding against the divisor exceeding the actual result count.

### `evaluate_hybrid_search(hybrid_search: &HybridSearch, test_queries: &[TestQuery]) -> Result<EvaluationMetrics, Box<dyn std::error::Error + Send + Sync>>`
**Call graph:** HybridSearch::search -> calculate_ndcg -> calculate_mrr -> calculate_map -> calculate_recall_at_k -> calculate_precision_at_k
**Steps:**
1. Initializes accumulators `ndcg_sum`, `mrr_sum`, `map_sum`, `recall_sum`, and `precision_sum` to `0.0`.
2. For each `test_query`, awaits `hybrid_search.search(&query, 20)` (mapping errors to `format!("Search failed: {}", e)`).
3. Adds NDCG@10, MRR, MAP, Recall@20, and Precision@10 from the helper functions into the corresponding accumulators.
4. Divides each accumulator by `n = test_queries.len() as f64` to obtain averaged metrics.
5. Returns an `EvaluationMetrics` populated with `ndcg_at_10`, `mrr`, `map`, `recall_at_20`, and `precision_at_10`.

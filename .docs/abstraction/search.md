# search — Abstract Logic

## Module: mod
**Purpose:** Orchestrates vector-only and hybrid (BM25 + vector) search with configurable Reciprocal Rank Fusion.

1. **Configure fusion defaults** -> `HybridSearchConfig::default()`
2. **Construct vector-only searcher** -> `VectorSearch::new()`
3. **Run a vector-only query (embed then search)** -> `VectorSearch::search()`, `HybridSearch::vector_only_search()`
4. **Construct a hybrid searcher with optional BM25 backend** -> `HybridSearch::new()`, `HybridSearch::with_defaults()`
5. **Run hybrid search by fanning out BM25 + vector concurrently** -> `HybridSearch::search()`, `HybridSearch::search_with_k()`
6. **Fuse two ranked lists into one weighted RRF result** -> `HybridSearch::reciprocal_rank_fusion_with_k()`, `HybridSearch::reciprocal_rank_fusion_static()`, `reciprocal_rank_fusion_core()`

## Module: bm25
**Purpose:** Wraps a Tantivy index to provide BM25 keyword search over code chunks.

1. **Open or create a Tantivy index on disk** -> `Bm25Search::new()`
2. **Adopt an existing in-memory index** -> `Bm25Search::from_index()`
3. **Parse and execute a multi-field BM25 query, hydrating chunks** -> `Bm25Search::search()`
4. **Expose underlying index/schema and refresh segments** -> `Bm25Search::index()`, `Bm25Search::schema()`, `Bm25Search::reload()`
5. **Cheaply share the searcher across async tasks** -> `Bm25Search::clone()`

## Module: error
**Purpose:** Defines the unified error type returned by all search operations.

1. **Enumerate embedding, vector store, BM25, and empty-result failures with derived `Display`/`From` impls** -> `SearchError`

## Module: resilient
**Purpose:** Provides a fault-tolerant hybrid search that degrades gracefully when one backend fails.

1. **Construct a resilient searcher with shared `Arc` components and fallback flag** -> `ResilientHybridSearch::new()`, `ResilientHybridSearch::with_defaults()`
2. **Attempt full hybrid search and fall back on failure** -> `ResilientHybridSearch::search()`, `ResilientHybridSearch::try_hybrid_search()`, `ResilientHybridSearch::fallback_search()`
3. **Inspect whether the searcher is currently in degraded mode** -> `ResilientHybridSearch::is_fallback_mode()`
4. **Run each backend independently with error normalization** -> `ResilientHybridSearch::bm25_search()`, `ResilientHybridSearch::vector_search()`
5. **Merge dual-backend results via canonical RRF** -> `ResilientHybridSearch::merge_results()`

## Module: rrf_tuner
**Purpose:** Tunes the RRF `k` parameter and evaluates hybrid search quality against ground-truth queries.

1. **Build a tuner from custom or default Rust test queries** -> `RRFTuner::new()`, `RRFTuner::default_rust_queries()`, `RRFTuner::query_count()`
2. **Sweep candidate `k` values to pick the best by NDCG** -> `RRFTuner::tune_k()`, `RRFTuner::tune_k_verbose()`
3. **Compute ranking-quality metrics for a result set** -> `calculate_ndcg()`, `calculate_mrr()`, `calculate_map()`, `calculate_recall_at_k()`, `calculate_precision_at_k()`
4. **Aggregate end-to-end evaluation metrics across a query set** -> `evaluate_hybrid_search()`

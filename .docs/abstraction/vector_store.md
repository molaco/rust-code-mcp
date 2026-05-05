# vector_store — Abstract Logic

## Module: mod
**Purpose:** Provides the public `VectorStore` facade that wraps a pluggable backend behind a unified async API for embedding-based chunk storage and retrieval.

1. **Resolve a default on-disk cache location and vector size** -> `VectorStoreConfig::default()`
2. **Construct a store backed by the embedded LanceDB backend** -> `VectorStore::new_embedded()`, `VectorStore::from_config()`, `VectorStore::new_default()`
3. **Insert or update embedded code chunks atomically** -> `VectorStore::upsert_chunks()`
4. **Run nearest-neighbor similarity queries against stored vectors** -> `VectorStore::search()`
5. **Remove vectors by chunk id or by source file path** -> `VectorStore::delete_chunks()`, `VectorStore::delete_by_file_path()`
6. **Inspect or reset the collection state** -> `VectorStore::count()`, `VectorStore::clear_collection()`, `VectorStore::delete_collection()`
7. **Verify backend reachability** -> `VectorStore::health_check()`

## Module: error
**Purpose:** Defines the unified `VectorStoreError` enum and ergonomic constructors for backend, query, and serialization failures.

1. **Build typed error variants from string messages** -> `VectorStoreError::connection()`, `VectorStoreError::query()`, `VectorStoreError::serialization()`, `VectorStoreError::not_found()`, `VectorStoreError::backend()`
2. **Bridge the error into trait-object and IO error contexts** -> `impl From<VectorStoreError> for Box<dyn Error + Send>`, `impl From<std::io::Error> for VectorStoreError`

## Module: lancedb
**Purpose:** Implements the `VectorStoreBackend` trait against LanceDB, owning Arrow schema construction, table lifecycle, and SQL-style filtered operations.

1. **Open or create the on-disk LanceDB database and ensure the vectors table exists** -> `LanceDbBackend::new()`, `LanceDbBackend::ensure_table_exists()`, `LanceDbBackend::get_table()`
2. **Define the six-column Arrow schema and reusable empty arrays** -> `LanceDbBackend::create_schema_for_dim()`, `LanceDbBackend::schema()`, `LanceDbBackend::create_empty_vector_array()`
3. **Convert chunk/embedding tuples into a single Arrow `RecordBatch` for ingestion** -> `LanceDbBackend::chunks_to_batch()`
4. **Atomically merge-insert chunks keyed on id** -> `<LanceDbBackend as VectorStoreBackend>::upsert_chunks()`
5. **Run cosine vector search and decode results back into `SearchResult`s** -> `<LanceDbBackend as VectorStoreBackend>::search()`
6. **Delete rows by id list or by file path using SQL filters** -> `<LanceDbBackend as VectorStoreBackend>::delete_chunks()`, `<LanceDbBackend as VectorStoreBackend>::delete_by_file_path()`
7. **Report row counts, clear all rows, and probe connectivity** -> `<LanceDbBackend as VectorStoreBackend>::count()`, `<LanceDbBackend as VectorStoreBackend>::clear()`, `<LanceDbBackend as VectorStoreBackend>::health_check()`

## Module: traits
**Purpose:** Declares the async `VectorStoreBackend` contract that any concrete vector store must satisfy.

1. **Specify the `Send + Sync` async backend interface for upsert, search, deletion, counting, clearing, and health checks** -> `trait VectorStoreBackend`

# vector_store — Detailed Logic

## Module: mod

### `VectorStoreConfig::default() -> Self`
**Call graph:** directories::ProjectDirs::from -> map -> unwrap_or_else -> PathBuf::from -> PathBuf::join
**Steps:**
1. Resolve the platform-specific cache directory via `directories::ProjectDirs::from("", "", "rust-code-mcp")`.
2. Map the resolved `ProjectDirs` to its `cache_dir` as a `PathBuf`.
3. Fall back to `.cache/rust-code-mcp` if `ProjectDirs` returns `None`.
4. Return `VectorStoreConfig::Embedded` with `path = cache_dir.join("vectors")` and `vector_size = 384` (matching `all-MiniLM-L6-v2`).

### `VectorStore::new_embedded(path: PathBuf, vector_size: usize) -> Result<Self, VectorStoreError>`
**Call graph:** LanceDbBackend::new -> Arc::new
**Steps:**
1. Construct a `LanceDbBackend` by awaiting `LanceDbBackend::new(path, vector_size)`.
2. Wrap the returned backend in an `Arc<dyn VectorStoreBackend>`.
3. Return a `VectorStore` containing that arc.

### `VectorStore::from_config(config: VectorStoreConfig) -> Result<Self, VectorStoreError>`
**Call graph:** VectorStore::new_embedded
**Steps:**
1. Match the variant of `VectorStoreConfig`.
2. For `Embedded { path, vector_size }`, delegate to `Self::new_embedded(path, vector_size).await`.

### `VectorStore::new_default() -> Result<Self, VectorStoreError>`
**Call graph:** VectorStoreConfig::default -> VectorStore::from_config
**Steps:**
1. Build a default `VectorStoreConfig` via `VectorStoreConfig::default()`.
2. Forward it to `Self::from_config(...)` and return the result.

### `VectorStore::upsert_chunks(&self, chunks_with_embeddings: Vec<(ChunkId, Embedding, CodeChunk)>) -> Result<(), VectorStoreError>`
**Call graph:** VectorStoreBackend::upsert_chunks
**Steps:**
1. Forward the chunk/embedding tuples directly to `self.backend.upsert_chunks(...)`.
2. Return the backend's result unchanged.

### `VectorStore::search(&self, query_vector: Embedding, limit: usize) -> Result<Vec<SearchResult>, VectorStoreError>`
**Call graph:** VectorStoreBackend::search
**Steps:**
1. Delegate to `self.backend.search(query_vector, limit).await`.
2. Return the produced `Vec<SearchResult>`.

### `VectorStore::delete_chunks(&self, chunk_ids: Vec<ChunkId>) -> Result<(), VectorStoreError>`
**Call graph:** VectorStoreBackend::delete_chunks
**Steps:**
1. Forward `chunk_ids` to `self.backend.delete_chunks(...)`.
2. Return the backend's result.

### `VectorStore::delete_by_file_path(&self, file_path: &str) -> Result<(), VectorStoreError>`
**Call graph:** VectorStoreBackend::delete_by_file_path
**Steps:**
1. Pass the file path through to `self.backend.delete_by_file_path(file_path).await`.
2. Return the propagated result.

### `VectorStore::count(&self) -> Result<usize, VectorStoreError>`
**Call graph:** VectorStoreBackend::count
**Steps:**
1. Await `self.backend.count()` and return the row count.

### `VectorStore::clear_collection(&self) -> Result<(), VectorStoreError>`
**Call graph:** VectorStoreBackend::clear
**Steps:**
1. Delegate to `self.backend.clear().await` to remove all vectors while preserving table structure.

### `VectorStore::delete_collection(&self) -> Result<(), VectorStoreError>`
**Call graph:** VectorStoreBackend::clear
**Steps:**
1. Alias for `clear_collection`; calls `self.backend.clear().await` (LanceDB clears rather than dropping the table).

### `VectorStore::health_check(&self) -> Result<(), VectorStoreError>`
**Call graph:** VectorStoreBackend::health_check
**Steps:**
1. Forward to `self.backend.health_check().await`.
2. Return `Ok(())` on success or propagate the connection error.

## Module: error

### `VectorStoreError::connection(msg: impl Into<String>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Convert the message argument into a `String` via `Into::into`.
2. Wrap it in `VectorStoreError::Connection(...)`.

### `VectorStoreError::query(msg: impl Into<String>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Convert input into a `String`.
2. Return `VectorStoreError::Query(...)`.

### `VectorStoreError::serialization(msg: impl Into<String>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Convert input into a `String`.
2. Return `VectorStoreError::Serialization(...)`.

### `VectorStoreError::not_found(msg: impl Into<String>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Convert input into a `String`.
2. Return `VectorStoreError::NotFound(...)`.

### `VectorStoreError::backend(msg: impl Into<String>) -> Self`
**Call graph:** Into::into
**Steps:**
1. Convert input into a `String`.
2. Return `VectorStoreError::Backend(...)`.

### `impl From<VectorStoreError> for Box<dyn std::error::Error + Send>`
**Call graph:** Box::new
**Steps:**
1. Box the `VectorStoreError` value as `Box<dyn Error + Send>` via `Box::new(err)`.

### `impl From<std::io::Error> for VectorStoreError` (derived via `#[from]`)
**Call graph:** thiserror generated `From::from`
**Steps:**
1. Wrap the underlying `std::io::Error` directly in `VectorStoreError::Io`.

## Module: lancedb

### `LanceDbBackend::new(path: PathBuf, vector_dim: usize) -> Result<Self, VectorStoreError>`
**Call graph:** std::fs::create_dir_all -> lancedb::connect -> ConnectBuilder::execute -> LanceDbBackend::create_schema_for_dim -> Arc::new -> LanceDbBackend::ensure_table_exists -> VectorStoreError::connection
**Steps:**
1. Call `std::fs::create_dir_all(&path)` to ensure the storage directory exists, mapping any I/O error into `VectorStoreError::connection`.
2. Open a LanceDB connection by calling `connect(path).execute().await`, mapping failures into `VectorStoreError::connection`.
3. Build the Arrow schema for `vector_dim` via `Self::create_schema_for_dim` and wrap it in `Arc::new`.
4. Construct the `LanceDbBackend` struct with the connection, table name `"vectors"`, dim, and cached schema.
5. Call `backend.ensure_table_exists().await` to create the table and BTree indices on first use.
6. Return the fully initialized backend.

### `LanceDbBackend::create_schema_for_dim(vector_dim: usize) -> Schema`
**Call graph:** Schema::new -> Field::new (multiple) -> Arc::new -> DataType::FixedSizeList
**Steps:**
1. Build six Arrow `Field`s: `id` (Utf8), `vector` (FixedSizeList<Float32, vector_dim>), `chunk_json` (Utf8), `file_path` (Utf8), `symbol_kind` (Utf8), `module_path` (Utf8).
2. Wrap the inner `item` field of the FixedSizeList in `Arc::new` as required by Arrow.
3. Return `Schema::new(vec![...])` with the six fields.

### `LanceDbBackend::schema(&self) -> Arc<Schema>`
**Call graph:** Arc::clone
**Steps:**
1. Clone the cached `Arc<Schema>` (cheap reference-count bump) and return it.

### `LanceDbBackend::ensure_table_exists(&self) -> Result<(), VectorStoreError>`
**Call graph:** Connection::table_names -> NamesBuilder::execute -> LanceDbBackend::schema -> StringArray::from -> LanceDbBackend::create_empty_vector_array -> RecordBatch::try_new -> Arc::new -> RecordBatchIterator::new -> Connection::create_table -> CreateTableBuilder::execute -> Table::create_index -> CreateIndexBuilder::execute -> tracing::info -> VectorStoreError::backend
**Steps:**
1. List existing table names via `self.db.table_names().execute().await`.
2. If `self.table_name` is already present, return `Ok(())`.
3. Otherwise, fetch the cached schema and construct empty `StringArray`s for the five string columns.
4. Build an empty `FixedSizeListArray` for the `vector` column via `create_empty_vector_array`.
5. Combine the empty arrays into a `RecordBatch` (`RecordBatch::try_new`), mapping errors to `VectorStoreError::backend`.
6. Wrap the batch in a `RecordBatchIterator` and call `self.db.create_table(name, ...).execute().await`.
7. Log table creation, then create three BTree indices on `id`, `file_path`, and `symbol_kind` columns sequentially.
8. Log index creation and return `Ok(())`.

### `LanceDbBackend::create_empty_vector_array(&self) -> FixedSizeListArray`
**Call graph:** Float32Array::from -> Arc::new -> Field::new -> FixedSizeListArray::try_new -> Result::unwrap
**Steps:**
1. Create an empty `Float32Array` from an empty `Vec<f32>`.
2. Build the `item` field for the inner element type (`Float32`, nullable).
3. Construct a `FixedSizeListArray` with width `self.vector_dim` and zero rows; unwrap because the configuration is statically valid.

### `LanceDbBackend::chunks_to_batch(&self, chunks: &[(ChunkId, Embedding, CodeChunk)]) -> Result<RecordBatch, VectorStoreError>`
**Call graph:** LanceDbBackend::schema -> Vec::with_capacity -> ChunkId::to_string -> Vec::extend_from_slice -> serde_json::to_string -> PathBuf::display -> Vec<String>::join -> StringArray::from -> Float32Array::from -> Arc::new -> Field::new -> FixedSizeListArray::try_new -> RecordBatch::try_new -> VectorStoreError::serialization -> VectorStoreError::backend
**Steps:**
1. Compute `n = chunks.len()` and clone the cached schema.
2. Pre-allocate six vectors with capacity `n` (and `n * vector_dim` for the flat embedding buffer).
3. Iterate over each `(id, embedding, chunk)`: push stringified id, extend flat vector buffer with the embedding, JSON-serialize the chunk (mapping serde errors to `serialization`), push file path, symbol kind, and `::`-joined module path.
4. Build a `StringArray` for ids and a `Float32Array` for the flat vectors.
5. Construct the `FixedSizeListArray` for the `vector` column with width `vector_dim`, mapping errors to `serialization`.
6. Build `StringArray`s for `chunk_json`, `file_path`, `symbol_kind`, and `module_path`.
7. Assemble all six arrays into a `RecordBatch::try_new`, mapping creation errors to `VectorStoreError::backend`.

### `LanceDbBackend::get_table(&self) -> Result<lancedb::Table, VectorStoreError>`
**Call graph:** Connection::open_table -> OpenTableBuilder::execute -> VectorStoreError::not_found
**Steps:**
1. Call `self.db.open_table(&self.table_name).execute().await`.
2. Map any error to `VectorStoreError::not_found(...)` and return the resolved `Table`.

### `impl VectorStoreBackend for LanceDbBackend::upsert_chunks(...)`
**Call graph:** Vec::is_empty -> LanceDbBackend::get_table -> LanceDbBackend::chunks_to_batch -> RecordBatch::schema -> RecordBatchIterator::new -> Table::merge_insert -> MergeInsertBuilder::when_matched_update_all -> MergeInsertBuilder::when_not_matched_insert_all -> MergeInsertBuilder::execute -> tracing::debug -> VectorStoreError::backend
**Steps:**
1. Short-circuit with `Ok(())` if the input vector is empty.
2. Capture `num_chunks` for logging and open the table via `get_table`.
3. Convert chunks to a single Arrow `RecordBatch` using `chunks_to_batch`.
4. Wrap the batch in a `RecordBatchIterator` reusing its schema.
5. Configure a merge-insert builder keyed on `id` with `when_matched_update_all(None)` and `when_not_matched_insert_all()`.
6. Execute the merge operation atomically, mapping errors to `VectorStoreError::backend`.
7. Emit a debug log noting the number of upserted chunks and return `Ok(())`.

### `impl VectorStoreBackend for LanceDbBackend::search(...)`
**Call graph:** LanceDbBackend::get_table -> Table::vector_search -> VectorQuery::distance_type -> VectorQuery::limit -> VectorQuery::execute -> futures::TryStreamExt::try_collect -> RecordBatch::column_by_name -> Array::as_any -> downcast_ref::<StringArray> -> downcast_ref::<Float32Array> -> RecordBatch::num_rows -> StringArray::value -> Float32Array::value -> ChunkId::from_string -> serde_json::from_str -> Vec::push -> VectorStoreError::query -> VectorStoreError::serialization
**Steps:**
1. Open the table via `get_table`.
2. Call `table.vector_search(query_vector)`, set distance to `Cosine`, apply `limit`, and execute.
3. Collect the resulting stream of `RecordBatch`es into a `Vec` via `try_collect`, mapping errors to `VectorStoreError::query`.
4. Allocate an empty `Vec<SearchResult>` for results.
5. For each batch, look up the `id`, `chunk_json`, and synthetic `_distance` columns and downcast them to `StringArray`/`Float32Array`, returning `query` errors on missing or wrongly-typed columns.
6. For each row index, read the id string, chunk JSON, and distance value.
7. Parse the id via `ChunkId::from_string` and deserialize the chunk JSON, mapping errors to `serialization`.
8. Convert cosine distance `[0,2]` to similarity `score = 1.0 - distance/2.0`.
9. Push a `SearchResult { chunk_id, score, chunk }` into the accumulator.
10. After processing all batches, return the accumulated results.

### `impl VectorStoreBackend for LanceDbBackend::delete_chunks(...)`
**Call graph:** Vec::is_empty -> LanceDbBackend::get_table -> Iterator::map -> ChunkId::to_string -> Iterator::collect -> Vec::join -> format! -> Table::delete -> tracing::debug -> VectorStoreError::backend
**Steps:**
1. Return `Ok(())` early if `chunk_ids` is empty.
2. Open the table via `get_table`.
3. Map each `ChunkId` to a single-quoted string and collect into a `Vec<String>`.
4. Build the SQL filter `id IN (id1, id2, ...)` using `Vec::join(", ")`.
5. Execute `table.delete(&filter).await`, mapping errors to `VectorStoreError::backend`.
6. Log the deletion count and return `Ok(())`.

### `impl VectorStoreBackend for LanceDbBackend::delete_by_file_path(...)`
**Call graph:** LanceDbBackend::get_table -> str::replace -> format! -> Table::delete -> tracing::debug -> VectorStoreError::backend
**Steps:**
1. Open the table.
2. Escape single quotes in `file_path` by doubling them.
3. Build the filter `file_path = '<escaped_path>'`.
4. Execute `table.delete(&filter).await`, mapping errors to `VectorStoreError::backend`.
5. Log the operation and return `Ok(())`.

### `impl VectorStoreBackend for LanceDbBackend::count(...)`
**Call graph:** LanceDbBackend::get_table -> Table::count_rows -> VectorStoreError::query
**Steps:**
1. Open the table via `get_table`.
2. Call `table.count_rows(None).await`, mapping errors to `VectorStoreError::query`.
3. Return the resulting `usize`.

### `impl VectorStoreBackend for LanceDbBackend::clear(...)`
**Call graph:** LanceDbBackend::get_table -> Table::delete -> tracing::info -> VectorStoreError::backend
**Steps:**
1. Open the table.
2. Run `table.delete("1=1").await` to remove all rows while keeping the table.
3. Log the clear and return `Ok(())`.

### `impl VectorStoreBackend for LanceDbBackend::health_check(...)`
**Call graph:** Connection::table_names -> NamesBuilder::execute -> VectorStoreError::connection
**Steps:**
1. Invoke `self.db.table_names().execute().await` purely to verify connectivity.
2. Map any error to `VectorStoreError::connection`.
3. Discard the table list and return `Ok(())`.

## Module: traits

### `trait VectorStoreBackend: Send + Sync`
**Call graph:** none (trait definition)
**Steps:**
1. Define the contract for any backend behind `VectorStore`, requiring `Send + Sync` to allow shared use across async tasks.
2. Declare async method `upsert_chunks` accepting `Vec<(ChunkId, Embedding, CodeChunk)>` for atomic insert/update.
3. Declare async method `search` accepting a query embedding and a result `limit`, returning `Vec<SearchResult>`.
4. Declare async method `delete_chunks` taking explicit `ChunkId`s.
5. Declare async method `delete_by_file_path` for path-based bulk deletion.
6. Declare async method `count` to return the total stored vector count.
7. Declare async method `clear` to remove all rows while preserving the collection structure.
8. Declare async method `health_check` to verify backend reachability.
9. All methods return `Result<_, VectorStoreError>` for unified error handling across implementations.

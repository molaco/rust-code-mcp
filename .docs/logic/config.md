# config — Detailed Logic

## Module: config (src/config.rs)

### `Config::default() -> Self` (trait impl: Default)
**Call graph:** Config::default -> default_data_dir
**Steps:**
1. Construct a `Config` literal with `server_port` set to `3000`.
2. Populate `data_dir` by calling `default_data_dir()` to resolve the OS-appropriate data directory.
3. Set `max_file_size` to `10_000_000` bytes (10 MB).
4. Set `num_threads` to `0` so callers interpret it as auto-detect.
5. Disable `debug` and seed retry policy with `retry_attempts = 3` and `retry_delay_ms = 100`.

### `Config::from_env() -> Self`
**Call graph:** Config::from_env -> Config::default, std::env::var, str::parse, str::eq_ignore_ascii_case, PathBuf::from
**Steps:**
1. Initialize a mutable `Config` from `Self::default()` to provide baseline values.
2. Attempt to read `SERVER_PORT`; if present and parses as `u16`, overwrite `server_port`.
3. Attempt to read `DATA_DIR`; if present, replace `data_dir` with `PathBuf::from(dir)`.
4. Attempt to read `MAX_FILE_SIZE` as a `u64` count of megabytes and convert to bytes via `* 1_000_000`.
5. Attempt to read `NUM_THREADS` as `usize` and overwrite `num_threads`.
6. Attempt to read `DEBUG`; treat ASCII case-insensitive `"true"` or literal `"1"` as enabled.
7. Attempt to read `RETRY_ATTEMPTS` as `u32` and `RETRY_DELAY_MS` as `u64`, overwriting on success.
8. Return the populated `config`, silently keeping defaults whenever an env var is missing or malformed.

### `Config::tantivy_dir(&self) -> PathBuf`
**Call graph:** Config::tantivy_dir -> PathBuf::join
**Steps:**
1. Take a reference to `self.data_dir` and append the `"tantivy"` segment via `join` to form the index directory.
2. Return the resulting `PathBuf` so callers know where Tantivy data lives.

### `Config::cache_dir(&self) -> PathBuf`
**Call graph:** Config::cache_dir -> PathBuf::join
**Steps:**
1. Append the `"cache"` subdirectory to `self.data_dir` via `join`.
2. Return the resolved cache directory `PathBuf`.

### `Config::print_summary(&self)`
**Call graph:** Config::print_summary -> usize::to_string, PathBuf::display, tracing::info!
**Steps:**
1. Compute a human-readable `threads` string: `"auto"` when `num_threads == 0`, otherwise `num_threads.to_string()`.
2. Emit a structured `tracing::info!` event recording port, data dir display path, max file size in MB, threads, debug flag, and retry policy.
3. Avoid writing to stdout because MCP stdio mode reserves stdout for JSON-RPC frames.

### `default_data_dir() -> PathBuf` (private helper documented for completeness)
**Call graph:** default_data_dir -> directories::ProjectDirs::from, ProjectDirs::data_dir, Path::to_path_buf, PathBuf::from
**Steps:**
1. Call `directories::ProjectDirs::from("com", "rust-code-mcp", "rust-code-mcp")` to locate the platform data directory.
2. If the call returns `Some`, take its `data_dir()` reference and convert it via `to_path_buf`.
3. Otherwise, fall back to the relative `./data` path so the binary still has somewhere to write.

## Module: errors (src/config/errors.rs)

### `<Error as ErrorMessage>::to_user_message(&self) -> String` (trait impl)
**Call graph:** ErrorMessage::to_user_message -> format!, Display::fmt
**Steps:**
1. Format `self` through `Display` with the `"Error: {}"` template.
2. Return the resulting `String` for surface-level user-facing messaging.

### `<Result<T> as ErrorContextExt<T>>::indexing_context(self, operation: &str) -> Result<T>` (trait impl)
**Call graph:** ErrorContextExt::indexing_context -> anyhow::Context::with_context, format!
**Steps:**
1. Forward `self` to `with_context` with a closure that formats `"Indexing operation failed: {}"`.
2. Allow the closure to be evaluated lazily so context strings are only built on the error path.
3. Return the augmented `Result<T>` to the caller.

### `<Result<T> as ErrorContextExt<T>>::search_context(self, query: &str) -> Result<T>` (trait impl)
**Call graph:** ErrorContextExt::search_context -> anyhow::Context::with_context, format!
**Steps:**
1. Chain `with_context` onto `self`, embedding the query string in `"Search operation failed for query: {}"`.
2. Return the wrapped result so callers can surface the offending query.

### `<Result<T> as ErrorContextExt<T>>::file_context(self, path: &Path) -> Result<T>` (trait impl)
**Call graph:** ErrorContextExt::file_context -> anyhow::Context::with_context, format!, Path::display
**Steps:**
1. Use `path.display()` to obtain a printable form of the filesystem path.
2. Pass a closure to `with_context` building `"File operation failed: {path}"` only when an error occurs.
3. Return the contextualized `Result<T>`.

### `<Result<T> as ErrorContextExt<T>>::vector_store_context(self, operation: &str) -> Result<T>` (trait impl)
**Call graph:** ErrorContextExt::vector_store_context -> anyhow::Context::with_context, format!
**Steps:**
1. Attach a `"Vector store operation failed: {operation}"` context lazily via `with_context`.
2. Return the resulting `Result<T>`.

### `box_error_to_anyhow(e: Box<dyn std::error::Error + Send + Sync>) -> Error`
**Call graph:** box_error_to_anyhow -> anyhow!, Display::fmt
**Steps:**
1. Format the boxed error through `Display` so the caller's stringification logic is preserved.
2. Wrap that string in an `anyhow!` macro invocation to obtain an `anyhow::Error`.
3. Return the new error so legacy `Box<dyn Error>` flows can plug into anyhow-based plumbing.

### `is_retryable(error: &Error) -> bool`
**Call graph:** is_retryable -> Error::to_string, str::to_lowercase, str::contains
**Steps:**
1. Render the error to a `String` and lowercase it for case-insensitive substring matching.
2. Test the lowered text for hallmark transient phrases: `"timeout"`, `"connection"`, `"would block"`, `"try again"`, `"unavailable"`.
3. Return `true` if any phrase matches, marking the error as retryable; otherwise `false`.

## Module: indexer (src/config/indexer.rs)

### `IndexerConfig::for_codebase_size(codebase_loc: usize, cache_path: &Path, tantivy_path: &Path) -> Self`
**Call graph:** IndexerConfig::for_codebase_size -> Path::to_path_buf
**Steps:**
1. Branch on `codebase_loc`: `<100_000` selects small profile `(10_000_000, 96, 50, 2)`; `<1_000_000` medium `(10_000_000, 96, 100, 4)`; otherwise large `(15_000_000, 128, 200, 8)`.
2. Bind the resulting tuple to `(max_file_size, gpu_batch_size, tantivy_memory_mb, tantivy_threads)`.
3. Build an `IndexerCoreConfig` cloning the cache path via `to_path_buf`, plus the chosen file-size and batch settings.
4. Build a `TantivyConfig` cloning the tantivy path and applying chosen memory budget and thread count.
5. Return the assembled `IndexerConfig`.

### `IndexerConfig::default(cache_path: &Path, tantivy_path: &Path) -> Self`
**Call graph:** IndexerConfig::default -> Path::to_path_buf
**Steps:**
1. Convert both incoming paths to owned `PathBuf` instances using `to_path_buf`.
2. Construct an `IndexerCoreConfig` with the default `max_file_size = 10_000_000` and `gpu_batch_size = 96`.
3. Construct a `TantivyConfig` with default `memory_budget_mb = 50` and `num_threads = 2`.
4. Return the combined `IndexerConfig` containing both halves.

### `IndexerCoreConfig::default() -> Self` (trait impl: Default)
**Call graph:** IndexerCoreConfig::default -> PathBuf::from
**Steps:**
1. Initialize `cache_path` to a relative `./cache` `PathBuf` for runtime defaults.
2. Set `max_file_size` to `10_000_000` (10 MB) to match the server-level cap.
3. Set `gpu_batch_size` to `96`, tuned for ~8 GB GPU VRAM headroom.

### `TantivyConfig::for_codebase_size(index_path: &Path, codebase_loc: Option<usize>) -> Self`
**Call graph:** TantivyConfig::for_codebase_size -> Path::to_path_buf
**Steps:**
1. Match on `codebase_loc`: when `Some(loc)` apply tiered tuning (`<100k`->`(50,2)`, `<1M`->`(100,4)`, else `(200,8)`).
2. When `None`, fall back to `(50, 2)` because size is unknown.
3. Convert `index_path` to `PathBuf` via `to_path_buf` and assemble the `TantivyConfig` with the selected memory and thread budgets.
4. Return the resulting struct.

### `TantivyConfig::default(index_path: &Path) -> Self`
**Call graph:** TantivyConfig::default -> Path::to_path_buf
**Steps:**
1. Take the borrowed `index_path` and clone it into an owned `PathBuf`.
2. Set `memory_budget_mb` to `50` and `num_threads` to `2` as conservative defaults.
3. Return the populated `TantivyConfig`.

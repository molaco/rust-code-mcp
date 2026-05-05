# config — Abstract Logic

## Module: config
**Purpose:** Defines the top-level server configuration with defaults, environment overrides, and derived paths.

1. **Provide baseline configuration values** -> `Config::default()`, `default_data_dir()`
2. **Override defaults from environment variables** -> `Config::from_env()`
3. **Derive subsystem paths from the data directory** -> `Config::tantivy_dir()`, `Config::cache_dir()`
4. **Emit a structured summary of the active configuration** -> `Config::print_summary()`

## Module: errors
**Purpose:** Provides shared error formatting, contextual wrappers, and retry classification for the server.

1. **Render errors as user-facing messages** -> `<Error as ErrorMessage>::to_user_message()`
2. **Attach domain-specific context to fallible results** -> `ErrorContextExt::indexing_context()`, `ErrorContextExt::search_context()`, `ErrorContextExt::file_context()`, `ErrorContextExt::vector_store_context()`
3. **Bridge boxed legacy errors into anyhow** -> `box_error_to_anyhow()`
4. **Classify transient errors as retryable** -> `is_retryable()`

## Module: indexer
**Purpose:** Builds indexer and Tantivy configuration profiles tuned to codebase size with sensible defaults.

1. **Select tiered indexer settings based on codebase size** -> `IndexerConfig::for_codebase_size()`
2. **Provide default indexer configuration** -> `IndexerConfig::default()`, `IndexerCoreConfig::default()`
3. **Select tiered Tantivy settings based on codebase size** -> `TantivyConfig::for_codebase_size()`
4. **Provide default Tantivy configuration** -> `TantivyConfig::default()`

# rmc-config

Centralized configuration for the `rust-code-mcp` workspace. Owns the server `Config`, the unified `IndexerConfig` (core + Tantivy settings with size-based auto-tuning and embedding-profile-aware chunk defaults), and the shared `anyhow`-based error helpers (`ErrorContextExt`, retry classification). Depends only on `rmc-engine` for `EmbeddingProfile`, keeping the config layer free of indexing or server concerns.

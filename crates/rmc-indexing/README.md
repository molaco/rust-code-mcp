# rmc-indexing

Indexing pipeline for the rust-code-mcp workspace. Bundles the unified
Tantivy + vector indexing core (`indexing`), runtime health and backup
monitoring (`monitoring`), the file metadata cache used by incremental
indexing (`metadata_cache`), the indexing metrics helpers (`metrics`),
and the sensitive-file/secrets filters (`security`). Depends on
`rmc-engine` (chunker, embeddings, parser, schema, search, vector_store)
and `rmc-config` (typed config surface).

# rmc-server

MCP server cluster for `rust-code-mcp`. Contains three modules:

- `tools` — `rmcp`-based endpoint adapters (search, analysis, audits, graph queries, indexing).
- `mcp` — `SyncManager` (background re-index loop) and the `project_paths` resolver shared by every endpoint.
- `semantic` — thin rust-analyzer IDE wrapper used by the analysis endpoints.

Depends on `rmc-engine`, `rmc-graph`, `rmc-config`, and `rmc-indexing`.

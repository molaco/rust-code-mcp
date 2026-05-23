# Architectural Rules

This document codifies the crate-boundary rules for the `rust-code-mcp`
workspace. These rules are currently documentation-only: they are repeatable via
the MCP `forbidden_dependency_check` tool, but they are not yet wired into CI or
a repo-local test harness.

## Current Boundary Rule Set

```json
[
  {
    "consumer": "rmc_engine",
    "producer": "rmc_*",
    "severity": "error",
    "message": "rmc_engine is the foundation crate and must not depend on other rmc_* crates"
  },
  {
    "consumer": "rmc_graph",
    "producer": "rmc_server",
    "severity": "error",
    "message": "rmc_graph must not depend on the MCP server layer"
  },
  {
    "consumer": "rmc_graph",
    "producer": "rmc_indexing",
    "severity": "warn",
    "message": "rmc_graph should remain independent from indexing"
  },
  {
    "consumer": "rmc_indexing",
    "producer": "rmc_server",
    "severity": "error",
    "message": "rmc_indexing must not depend on the MCP server layer"
  },
  {
    "consumer": "rmc_indexing",
    "producer": "rmc_graph",
    "severity": "warn",
    "message": "rmc_indexing should remain independent from graph"
  }
]
```

## How to run the check

From inside the workspace (with `mcp__rust-code-mcp` available, e.g. via Claude Code):

1. Refresh the snapshot: `mcp__rust-code-mcp__build_hypergraph` with `directory = .`.
2. Call `mcp__rust-code-mcp__forbidden_dependency_check` with the rule set above.
3. Expect `"violation_count": 0`.

The check is also available via the project's main binary (`rust-code-mcp` itself ships this tool — it's dogfooding).

## Phase C extension (planned, not yet enforced)

When Phase C lands, append these rules:

```json
[
  {"consumer": "rmc-graph",    "producer": "rust-code-mcp", "severity": "error"},
  {"consumer": "rmc-config",   "producer": "rmc-graph",     "severity": "error"},
  {"consumer": "rmc-config",   "producer": "rmc-indexing",  "severity": "error"},
  {"consumer": "rmc-config",   "producer": "rmc-server",    "severity": "error"},
  {"consumer": "rmc-config",   "producer": "rust-code-mcp", "severity": "error"},
  {"consumer": "rmc-indexing", "producer": "rmc-server",    "severity": "error"},
  {"consumer": "rmc-indexing", "producer": "rust-code-mcp", "severity": "error"},
  {"consumer": "rmc-server",   "producer": "rust-code-mcp", "severity": "error"}
]
```

This encodes the dependency hierarchy:

```text
rmc-engine    ←  rmc-graph
              ←  rmc-config
              ←  rmc-indexing  (also ← rmc-config)
              ←  rmc-server    (← rmc-graph, rmc-config, rmc-indexing)
              ←  rust-code-mcp (main binary; → all above)
```

## §2 rule equivalents (not directly expressible)

The parent plan's §2 listed these forbidden edges at module level:

- `graph → tools`, `graph → mcp` — now enforced by `rmc-graph` not depending on `rust-code-mcp` (which holds tools, mcp).
- `engine → tools`, `engine → mcp` — enforced by `rmc-engine` not depending on `rust-code-mcp`.
- `embeddings → indexing` — still a module-level edge within `rmc-engine` and the (future) `rmc-indexing` crate. After Phase C this becomes a real crate boundary (`rmc-engine` must not depend on `rmc-indexing`), expressible as the rule:
  ```json
  {"consumer": "rmc-engine", "producer": "rmc-indexing", "severity": "error"}
  ```

Until Phase C, that one rule has no crate boundary to anchor on (everything in `rmc-indexing` still lives in the main crate); enforce it by grep inside `crates/rmc-engine/src/`.

## Status

Last verified: 2026-05-21 (Phase B.8). Result: `violation_count = 0` against the Phase B rule set.

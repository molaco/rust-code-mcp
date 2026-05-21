# Architectural Rules (machine-enforceable)

This document codifies the crate-boundary rules for the `rust-code-mcp` workspace. They are checked by `mcp__rust-code-mcp__forbidden_dependency_check`, which runs as a filter over real cross-crate edges from the persisted hypergraph (`build_hypergraph` must be run first).

The rules below reflect the workspace state after Phase 7 B.7 (rmc-engine + rmc-graph lifted). When Phase C lands, the rule set extends to cover `rmc-config`, `rmc-indexing`, `rmc-server`.

## Rule set — Phase B end-state

```json
[
  {
    "consumer": "rmc-engine",
    "producer": "rmc-graph",
    "severity": "error",
    "message": "rmc-engine must not depend on rmc-graph (engine is foundation)"
  },
  {
    "consumer": "rmc-engine",
    "producer": "rust-code-mcp",
    "severity": "error",
    "message": "rmc-engine must not depend on the main crate"
  },
  {
    "consumer": "rmc-graph",
    "producer": "rust-code-mcp",
    "severity": "error",
    "message": "rmc-graph must not depend on the main crate"
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

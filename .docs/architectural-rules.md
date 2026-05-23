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

This is documentation-only enforcement. Run the check manually with the
`rust-code-mcp-refactor` MCP tools:

```text
build_hypergraph(
  directory="/home/molaco/Documents/rust-code-mcp-refactor",
  force_rebuild=false
)

forbidden_dependency_check(
  directory="/home/molaco/Documents/rust-code-mcp-refactor",
  rules=[
    {
      consumer: "rmc_engine",
      producer: "rmc_*",
      severity: "error",
      message: "rmc_engine is the foundation crate and must not depend on other rmc_* crates"
    },
    {
      consumer: "rmc_graph",
      producer: "rmc_server",
      severity: "error",
      message: "rmc_graph must not depend on the MCP server layer"
    },
    {
      consumer: "rmc_graph",
      producer: "rmc_indexing",
      severity: "warn",
      message: "rmc_graph should remain independent from indexing"
    },
    {
      consumer: "rmc_indexing",
      producer: "rmc_server",
      severity: "error",
      message: "rmc_indexing must not depend on the MCP server layer"
    },
    {
      consumer: "rmc_indexing",
      producer: "rmc_graph",
      severity: "warn",
      message: "rmc_indexing should remain independent from graph"
    }
  ],
  summary=false,
  limit=300
)
```

Expected result:

```text
rule_count=5
violation_count=0
total_match_count=0
```

## Current Dependency Direction

The intended crate direction is:

```text
rmc_server   -> rmc_graph, rmc_indexing, rmc_engine, rmc_config
rmc_graph    -> rmc_engine
rmc_indexing -> rmc_engine, rmc_config
rmc_config   -> rmc_engine
rmc_engine   -> no rmc_* dependencies
```

The current rule set enforces the direction that is most relevant to the
boundaries cleanup:

- `rmc_engine` stays foundation-only.
- `rmc_graph` does not depend on server and should not depend on indexing.
- `rmc_indexing` does not depend on server and should not depend on graph.

Edges from top-level server code into lower crates are allowed; this cleanup
narrows those APIs rather than inverting the dependency direction.

## Status

Last verified during Phase 1 Step 6 of `.plans/boundries-plan.md` execution on
2026-05-23. Result against the current five-rule set:

```text
rule_count=5
violation_count=0
total_match_count=0
returned_match_count=0
```

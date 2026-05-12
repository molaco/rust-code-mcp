---
name: rmc-architecture-rules
description: Enforce Rust crate-edge rules.
argument-hint: "[rules-file-or-inline-rules]"
allowed-tools: Read, Bash, mcp__rust-code-mcp__*
---

# Rust architectural rule enforcement

Declarative crate-edge rule check. CI-friendly: rules are passed as a
list, the tool returns concrete violations. Empty `violations` is the
pass signal. Scope: workspace-wide.

For raw cross-crate edge inspection without rules, use
`rmc-imports-exports`. For sortable per-crate metrics (Robert Martin
instability), use `rmc-dependency-metric`.

## Scope — workspace-wide, declarative rule check

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Define rules

Each rule has a glob-style `consumer` pattern and `producer` pattern
(with `*` wildcards), plus optional `except` (consumer-side override),
`severity`, and `message`:

```
rules = [
  { consumer: "domain*", producer: "tokio",      severity: "error", message: "domain crates must be runtime-agnostic" },
  { consumer: "domain*", producer: "serde_json", severity: "warn"  },
  { consumer: "domain*", producer: "reqwest",    severity: "error" },
  { consumer: "domain*", producer: "hyper",      severity: "error" },
  { consumer: "domain*", producer: "bevy*",      severity: "error" },
]
```

Glob semantics: `*` matches zero or more characters in the crate name.
`consumer="domain*"` catches `domain`, `domain_core`, `domain_types`, etc.

### Step 2. Run the check

```
forbidden_dependency_check(directory=..., rules=[...])
```

Returns:

```json
{
  "rule_count": 5,
  "violation_count": 2,
  "violations": [
    { "rule": { "consumer": "domain*", "producer": "tokio" },
      "edge": { "consumer_crate": "domain_x", "producer_crate": "tokio" },
      "sample_symbol": "tokio::spawn",
      "unique_symbols": 5,
      "total_refs": 17 }
  ]
}
```

One violation per (rule × matching edge). The tool is a pure filter over
`crate_edges` — same data, declarative shape, no extra graph cost.

### Step 3. Triage

For each violation:

- `read_file_content(file=<sample_call_site>)` at the span (use
  `who_imports(target=<sample_symbol>)` to surface the actual file:line).
- Confirm whether the import is legitimate (e.g. an integration test in
  the domain crate) or a real layering break.
- For legitimate cases, add an `except` clause to the rule (or narrow
  the `consumer` glob) and re-run.
- For real breaks, fix the import — move the offending code out of the
  domain crate or factor the dependency through an abstraction.

## Recipes

### Recipe — "Layered architecture audit (DAG enforcement)"

For each layer pair where the lower layer must not consume from the
upper:

```
forbidden_dependency_check(rules=[
  { consumer: "domain*", producer: "agent*", severity: "error" },
  { consumer: "domain*", producer: "tui*",   severity: "error" },
  { consumer: "agent*",  producer: "tui*",   severity: "error" },
])
```

Empty `violations` confirms the layer DAG holds. Any non-empty result is
a layering break.

### Recipe — "Async boundary check"

The domain crate must not import async runtimes:

```
forbidden_dependency_check(rules=[
  { consumer: "domain*", producer: "tokio",   severity: "error" },
  { consumer: "domain*", producer: "futures", severity: "error" },
  { consumer: "domain*", producer: "async-*", severity: "error" },
])
```

Real example: `domain` imported `tokio::sync::Mutex` because a refactor
never finished — surfaced as a single violation with `unique_symbols=1`.

### Recipe — "Domain crate framework hygiene"

```
forbidden_dependency_check(rules=[
  { consumer: "domain*", producer: "bevy*",     severity: "error" },
  { consumer: "domain*", producer: "reqwest",   severity: "error" },
  { consumer: "domain*", producer: "hyper",     severity: "error" },
  { consumer: "domain*", producer: "axum",      severity: "error" },
  { consumer: "domain*", producer: "actix-web", severity: "error" },
])
```

## Decision frames

| Question | Answer |
|---|---|
| Should this rule live in CI? | Yes — `violation_count > 0` is a non-zero exit-code candidate. |
| Should it run in-IDE? | Cheap enough (filter over `crate_edges`) to run on every save. |
| How to handle "partial" rules? | Use `except` for consumer-side overrides (e.g. domain may import `serde` but not `serde_json` — express as two rules with the broader rule narrowed). |
| Multiple rules contradicting? | The check evaluates each rule independently — overlapping rules each produce their own violation rows. Keep rules orthogonal. |

## Pattern reference

| Use case | Rule shape |
|---|---|
| Layered DAG | `consumer: "lower*", producer: "upper*"` |
| Async-free domain | `consumer: "domain*", producer: "tokio"` |
| Framework-free domain | `consumer: "domain*", producer: "bevy*"` |
| Forbid binary→library reverse | `consumer: "lib*", producer: "bin*"` |

## Output format

```
Rules: <n>
Violations: <m>
Severity breakdown: error <e>, warn <w>
Top offenders:
  - domain_x → tokio: 5 unique symbols, 17 refs (error)
  - ...
Verdict: <PASS | FAIL (e errors, w warnings)>
```

## Limitations

- Only crate-level edges. Can't enforce "domain modules must not import…"
  within a single crate — for that, drop to `get_imports(module=...)` and
  hand-roll a check.
- No cycle detection. `crate_edges` lists forward edges; the check is
  filter-only. For cycles, walk `crate_edges` manually.
- Cross-crate method calls / trait dispatch are NOT counted in
  `total_refs`. A consumer that imports a trait but only uses it via
  method dispatch may register `total_refs=0` while still violating the
  rule.
- Glob is `*`-only — no `?`, no character classes, no negation in the
  pattern itself (use `except` for that).

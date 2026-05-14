---
name: rmc-test-vs-prod
description: Rust test vs production split.
argument-hint: "<qualified-symbol-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust test vs production analysis

Layer 8 categorizes every reference as `Read` / `Write` / `Test` / `Other`.
The Test split is the load-bearing one for "what's only used by tests?"
Scope: single symbol or symbol family.

For broader refactor decisions, hand off to `rmc-refactor-plan`. For
method-by-method audits on one type, use `rmc-method-api`.

## Scope — single symbol or symbol family

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Pull the breakdown

```
who_uses_summary(directory=..., target=<qualified_name>)
```

Each row has a `category_breakdown` with `Test` and `Other` counts (some
payload shapes also include `Read` and `Write` sub-counts).

### Step 2. Classify

| Pattern | Verdict |
|---|---|
| All rows 100% Test | Test fixture / builder. Demote to `#[cfg(test)]` or move to dev-deps. |
| All rows 100% Other (Test=0) | Production-only. Critical path. High refactor risk. |
| Mixed Test + Other | Legitimate API. Both tested and used. |
| Test >> Other | Either under-used in production or over-tested in isolation. |

### Step 3. Read vs Write encapsulation check

When payloads include `Read` and `Write` sub-counts:

- Many readers + few writers = good encapsulation.
- Many writers = diffuse invariants — the symbol's value flows through
  too many places.

## Targeted recipes

### Recipe — "Test-only constructor audit"

For each `Type::new` (and `with_*` / `from_*` constructors), run
`who_uses_summary`. Rows 100% Test = builder used only by tests. Move to
test fixtures.

### Recipe — "Production-only methods"

Filter `module_tree` to methods. For each, `who_uses_summary`. All-Other
rows = critical path. Annotate as "high touch risk" in PR descriptions.

### Recipe — "Mostly-tested public API"

For pub items, `who_uses_summary`. Test >> Other (e.g. 30 Test, 2 Other)
usually means the symbol is tested in isolation but barely consumed in
production — under-used or over-tested.

## Decision frames

| Finding | Action |
|---|---|
| 100% Test fan-in for a `pub` item | Demote to `pub(crate)` + `#[cfg(test)]` |
| 100% Test fan-in for a `pub` constructor | Move to a test-fixtures crate / `tests/common` |
| 100% Test for an entire trait's methods | Trait is test-only; consider deleting and using concrete types in tests |
| Heavy Write counts on a shared type | Diffuse invariants; consider `&mut self` API audit |
| Mixed but Test >> Other | Over-tested in isolation; verify production callers are real users |

## Pattern reference

| Signal | Means |
|---|---|
| `Test=N, Other=0` for `Type::new` | Constructor used only by tests |
| `Test=0, Other=N` for a trait method | Production-only API; refactor with care |
| `Test=N, Other=M, Read=X, Write=Y` with `Y >> X` | Many writers — invariants are diffuse |
| Test rows cluster in `<crate>::tests::common` | Existing test-fixtures module — good convergence target |

## Output format

Per symbol:

```
Symbol: <qualified_name>
Verdict: <test-only | production-only | mixed | over-tested>
Test rows: <n>; Other rows: <m>
Top consumers (Test): <list>
Top consumers (Other): <list>
Recommended action: <demote / cfg(test) / move-to-fixtures / keep>
```

## Limitations

- The Test/Other category is heuristic — modules under `tests/`, `*_test`,
  or `#[cfg(test)]` items count as Test. Integration tests outside
  conventional paths may misclassify.
- Read/Write sub-counts are not present on every payload shape; rely on
  Test/Other for the core verdict.
- `who_uses_summary` excludes import-only references — pair with
  `who_imports` if you suspect a symbol is imported but never used.

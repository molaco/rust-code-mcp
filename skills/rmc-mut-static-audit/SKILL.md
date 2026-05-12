---
name: rmc-mut-static-audit
description: Audit Rust global mutable state.
argument-hint: "[workspace-path]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust global mutable state audit

Type-aware audit of every local `static` item whose HIR type matches
`static mut` / `LazyLock<...>` / `OnceLock<...>` / `OnceCell<...>`.
Scope: workspace-wide.

For unsafe-block compliance audits (often paired with `static mut`), use
`rmc-unsafe-audit`. For the in-fn `Mutex` usage audit, use
`rmc-signature-search` (`has_param_type="Mutex"`).

## Scope — workspace-wide

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Pull every match

```
mut_static_audit(directory=...)
```

Returns:

```json
{
  "directory": "...",
  "finding_count": 5,
  "findings": [
    { "item": "<64-char-hex>",
      "qualified_name": "my_crate::CONFIG",
      "matched_pattern": "LazyLock<...>",
      "type_string": "LazyLock<Mutex<Foo>>",
      "file": "src/config.rs",
      "span": [200, 260] }
  ]
}
```

Sorted by `(qualified_name, matched_pattern)`. A single static matching
multiple patterns produces one finding per pattern. `type_string` is
post-processed: init-fn pointers and allocator parameters are dropped.

### Step 2. Per-pattern audit

Filter findings by `matched_pattern`:

- `static mut` — the riskiest; requires `unsafe` to access. FFI / legacy
  hot spot.
- `LazyLock<...>` — process-lifetime init; common, often legitimate.
- `OnceLock<...>` — write-once cells.
- `OnceCell<...>` — same shape, different crate.

### Step 3. Per-finding fan-in

For each finding:

```
who_uses(directory=..., target=<finding.qualified_name>)
who_uses_summary(directory=..., target=<finding.qualified_name>)
```

Quantifies how many sites depend on the global. High fan-in = removing
it requires touching many sites; the global is load-bearing.

### Step 4. Render context

```
read_file_content(file_path=<finding.file>)
```

at `span` widened by ~30 lines. Review init expression and surrounding
documentation.

## Recipes

### Recipe — "Hidden singleton inventory"

List every `LazyLock` / `OnceLock` / `OnceCell` finding. Top candidates
for "should this be DI'd instead of a global?" — singletons are easy to
ship and hard to test. Rank by `who_uses_summary.total` desc to find the
most consumed singletons (most painful to remove, highest leverage if
removed).

### Recipe — "static mut audit"

Filter to `matched_pattern="static mut"`. FFI / legacy compatibility
cases. Each warrants a SAFETY review — `static mut` access requires
`unsafe`. Cross-reference with `rmc-unsafe-audit` — many `static mut`
sites have a corresponding `unsafe { /* read STATIC_MUT */ }` block.

### Recipe — "Cross-pattern singletons"

A static of type `LazyLock<OnceCell<T>>` would match both patterns and
produce two findings for the same item. Group by `qualified_name` →
cross-pattern singletons (uncommon; usually intentional layered init).

## Decision frames

| Pattern | Likely verdict |
|---|---|
| `LazyLock<HashMap<K, V>>` for a constant lookup table | Process-lifetime constant — fine |
| `LazyLock<Mutex<State>>` for shared mutable state | Hidden singleton — DI candidate |
| `OnceLock<Sender<T>>` for a global channel | Often DI candidate (carries side effects) |
| `static mut COUNT: usize = 0` | FFI / legacy — review SAFETY pre-conditions |
| `OnceCell<Config>` populated at startup | Probably fine; init order matters |

## Pattern reference

| Audit | Invocation |
|---|---|
| All globals | `mut_static_audit(directory=...)` |
| Risky `static mut` only | filter to `matched_pattern="static mut"` |
| Singletons | filter to `matched_pattern ∈ {LazyLock<...>, OnceLock<...>, OnceCell<...>}` |
| Cross-reference fan-in | per-finding `who_uses_summary(target=qualified_name)` |
| Cross-reference unsafe | `unsafe_audit` with `static mut` finding's qualified name in the unsafe block |

## Output format

```
Workspace: <path>
Total findings: <n>
  static mut: <a>
  LazyLock<...>: <b>
  OnceLock<...>: <c>
  OnceCell<...>: <d>

Top by fan-in:
  my_crate::CONFIG (LazyLock<Mutex<Foo>>): <n> users
  ...

Recommended DI candidates: <list>
Recommended SAFETY review (static mut): <list>
```

## Limitations

- The `lazy_static!` macro is NOT detected. Its expansion produces a
  generated wrapper type whose name doesn't contain `LazyLock`. Use
  `items_with_attribute(crate_name=X, attribute_pattern="lazy_static")`
  or `search(keyword="lazy_static!")` to cover that case.
- `parking_lot::Mutex<T>` constructor calls inside a fn body are NOT
  scanned — only `static` items are checked. For in-fn `Mutex` usage,
  drop to `rmc-signature-search` with `has_param_type="Mutex"`.
- Type-string match is post-processed (init-fn pointers dropped) —
  comparing `type_string` literally across findings is reliable;
  comparing against ad-hoc strings outside the tool may diverge.

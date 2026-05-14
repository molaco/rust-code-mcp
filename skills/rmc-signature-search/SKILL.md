---
name: rmc-signature-search
description: Find Rust fns by signature shape.
argument-hint: "<filter-expression> [crate-name]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust signature-based fn discovery

Recorded `FunctionSignature` data per fn unlocks signature-shape filtering
at workspace scale: "every async fn returning `Result<_, MyError>`",
"every fn with ≥5 params", "every fn taking `&Path`". Scope: single crate
or per-fn signature inspection.

For per-fn forensics (callers, fan-in), use `rmc-symbol-forensics`. For
attribute-based discovery (`#[deprecated]`, `#[must_use]`), use
`rmc-attribute-audit`.

## Scope — single crate or per-fn signature inspection

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Per-fn signature

```
function_signature(directory=..., target=<crate>::Y)
```

Returns the recorded signature: `is_async`, `self_param`
(Owned / Ref / RefMut, or null for free / assoc fns without self),
`params` (each with `name`, `type_string`, `by_ref`, `mutability`),
`return_type`, `generics` (with declaration-site trait bounds).

Type strings come from RA's `HirDisplay` rendered against the function's
owning crate; allocator / hasher type parameters (`, Global>`,
`, RandomState>`, `, BuildHasherDefault<...>>`) and `LazyLock` /
`OnceLock` init-fn pointer parameters are stripped.

### Step 2. Crate-wide filtered enumeration

```
functions_with_filter(directory=..., krate=X,
                      min_param_count=<n>,
                      has_param_type=<substring>,
                      returns_type_pattern=<substring>,
                      is_async=<bool>,
                      self_kind=<"none"|"owned"|"ref"|"ref_mut">,
                      limit=50, offset=0,
                      summary=false)
```

Knobs:

- `min_param_count` — fns with at least N non-self params.
- `has_param_type` — case-sensitive substring against any param's
  stringified type (e.g. `"&Path"`, `"tokio::sync::Mutex"`).
- `returns_type_pattern` — case-sensitive substring against return type
  (e.g. `"Result<"` — note: substring, not regex).
- `is_async` — `true` for async-only / `false` for sync-only / omit for
  both.
- `self_kind` — `"none"` (free fns + assoc fns without self), `"owned"`
  (`self`), `"ref"` (`&self`), `"ref_mut"` (`&mut self`).
- `limit` (default 50), `offset` (default 0).
- `summary=true` drops the `signature` payload from each match — useful
  for lightweight enumeration when full signatures exceed MCP token
  budget.

Sorted by qualified name. Trait-impl method bodies are NOT included
(Layer 4 limitation — impl items aren't Item nodes).

### Step 3. Pagination

`total_match_count` returned per call. Compare to `offset + match_count`
to detect "more pages exist". Bump `offset` by `limit` until
`match_count < limit`.

## Recipes

### Recipe — "Migration helper"

```
functions_with_filter(krate=X, returns_type_pattern="Result<",
                      is_async=true, has_param_type="OldError")
```

Every async fn returning a Result that mentions the legacy error type.
Pair with `rmc-call-graph` to scope migration per fn.

### Recipe — "Builder pattern detection"

```
functions_with_filter(krate=X, self_kind="owned")
```

Consuming methods (`fn foo(self) -> Self`) are the builder-pattern
signature. Combine with `returns_type_pattern="Self"` for a tight builder
filter.

### Recipe — "Filesystem-touching surface"

```
functions_with_filter(krate=X, has_param_type="&Path")
```

Or `has_param_type="PathBuf"`. Pair with `who_uses_summary` per finding
to rank by fan-in — top filesystem-touching fns are the natural seam for
an injected `FileSystem` trait.

### Recipe — "Self-kind consistency"

For trait T's methods (from `module_tree`):

```
function_signature(target=<crate>::T::method)
```

per method. Compare `self_param` shape across the trait's method set —
inconsistent self-kind on a trait (some `&self`, some `&mut self`, some
owned `self`) is usually a smell.

For implementors:
`functions_with_filter(krate=<impl_crate>, self_kind="ref_mut")` and
check whether impl methods match the trait's declared self-kind.

### Recipe — "High-arity fns"

```
functions_with_filter(krate=X, min_param_count=5)
```

Refactor candidates — five-or-more params usually wants a struct-of-args,
builder, or splitting. Cross-reference with `rmc-complexity` for the
containing file — high-arity + high cyclomatic = top refactor priority.

## Decision frames

| Goal | Mode |
|---|---|
| Workspace inventory ("list every async Result fn") | `summary=true` (drops signature payload) |
| Single-fn analysis | `function_signature(target=Y)` (no filter needed) |
| Migration prep | `functions_with_filter(returns_type_pattern=<old type>, is_async=...)` |
| Refactor candidate detection | `functions_with_filter(min_param_count=5)` |
| Self-kind audit | `function_signature` per method, compare manually |

## Pattern reference

| Filter combo | Result |
|---|---|
| `min_param_count=5` | Refactor candidates |
| `has_param_type="&Path"` | I/O surface |
| `returns_type_pattern="Result<"` + `is_async=true` | Async fallible API |
| `self_kind="owned"` | Consuming / builder methods |
| `self_kind="none"` | Free fns + static assoc fns |
| `has_param_type="tokio::sync::Mutex"` | Async-locked critical sections |

## Output format

```
Filter: <expression>
Crate: <X>
Matches: <n> (total <total_match_count>; pages remaining: <bool>)

Per match:
  <crate>::Y
    is_async: <bool>, self: <kind>, params: <n>, returns: <T>
    notes: <hint - e.g. "builder candidate", "I/O seam">
```

## Limitations

- `has_param_type` and `returns_type_pattern` are substring matches on
  `HirDisplay` output, not type-aware. `Result<MyError>` and `MyError`
  both substring-match `"MyError"` — disambiguation is the caller's job.
- Default type parameters (e.g. `, Global>` from `Vec<T, Global>`) are
  trimmed but other defaults may still appear depending on RA's render.
- `impl Trait` signatures may differ slightly from source (RA renders the
  resolved trait obj, not the source `impl Trait` syntax).
- Trait-impl method bodies are NOT included — only free fns, inherent
  assoc fns, and trait declaration fns. To audit impl methods, walk
  `module_tree` filtered to impl Items.
- Where-clause bounds on generics are NOT included in `trait_bounds` —
  only declaration-site bounds (RA limitation).

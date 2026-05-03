# Extra MCP Tools — Phase Plan

Phase-by-phase plan for seven additional MCP tools that extend the workspace hypergraph's coverage. Skipping the call graph (already on the roadmap as Layer 5).

Ordered by leverage-per-line. Each phase is independently shippable.

---

## Phase 1 — `forbidden_dependency_check` (1 day, ~100 LOC)

The cheapest, highest-leverage tool. Pure filter over existing `crate_edges` data. **No schema bump, no extraction changes.**

DSL turns ad-hoc architectural rules into version-controlled CI gates.

### Steps

1. **Decide rule shape.** JSON:
   ```json
   {
     "rules": [
       {
         "consumer": "domain*",
         "producer": "tokio",
         "except": null,
         "severity": "error",
         "message": "Domain crates must not depend on async runtimes"
       }
     ]
   }
   ```
   Patterns are glob with `*`. Hand-roll the matcher (no new dep) — ~15 LOC.

2. **`src/graph/queries.rs`** — add `forbidden_dependency_check(rules: Vec<Rule>) -> Result<Vec<Violation>>`. Iterate `crate_edges`, for each `(consumer, producer)` pair test every rule. ~50 LOC.

3. **`src/tools/search_tool.rs`** — `ForbiddenDependencyCheckParams { directory: PathBuf, rules: Vec<Rule> }`.

4. **`src/tools/graph_tools.rs`** + **`search_tool_router.rs`** — wire the tool. ~30 LOC total.

5. **Tests** — 3 cases:
   - Simple match (rule fires on a real cross-crate edge).
   - Glob wildcard (`*` matches multiple crates).
   - `except` clause overrides a positive match.
   ```
   cargo test --lib -j 2 graph::queries::forbidden -- --test-threads=1
   ```

### Use cases unlocked

- "Domain crates must not import async runtimes"
- "Only adapter crates may use provider SDKs"
- "Core crates must remain framework-free"

---

## Phase 2 — `enum_variants` (4 hours, ~30 LOC)

Cheap structural extension. Currently `ModuleDefId::EnumVariantId(_)` is in the exclusion list at `bindings.rs::process_entry`. Flip it.

### Steps

1. **`src/graph/model.rs`** — add `ItemKind::EnumVariant` variant.

2. **`src/graph/bindings.rs`** — remove `EnumVariantId` from the `matches!` exclusion in `process_entry`. Handle it like a regular item with `parent_id = enum's NodeId`.

3. **`src/graph/storage.rs`** — bump `SCHEMA_VERSION`. Document the bump reason in the comment block.

4. **`src/graph/queries.rs`** — add `enum_variants(enum_id) -> Vec<Node>`. ~10 LOC; just walks `children_by_parent`.

5. **MCP wrapper** — small param struct + tool fn + route. ~25 LOC.

6. **Test** — pick a known enum (e.g. `BindingKind`), assert it has the expected 4 variants. Run with:
   ```
   cargo test --lib -j 2 graph::queries::enum_variants -- --test-threads=1
   ```

### Cost expectation

`coding-agent` items grow by ~300 (75 enums × ~4 variants avg). `file_search_mcp` grows by ~70.

### Use cases unlocked

- "God error enum" detection — count variants per error type.
- Enum-variant fan-in via `who_uses(SomeEnum::SomeVariant)`.

---

## Phase 3 — `item_attributes` (1.5 days, ~250 LOC)

Extract attributes once at build time, query forever. Schema bump.

Single tool unlocks `#[must_use]`, `#[non_exhaustive]`, derives, `#[inline]`, doc-presence audits.

### Steps

1. **Decide storage shape.** Two options:
   - **A.** `Node.attributes: Vec<String>` — simple, fits existing serialization.
   - **B.** New `attributes_by_target` sub-DB — cleaner separation, allows multi-attribute records.

   Recommend **A** — attributes are <10 per item typically, no scale concern.

2. **`src/graph/model.rs`** — add `attributes: Vec<String>` to `Node`. Mark `#[serde(default)]` for forward-compat.

3. **`src/graph/storage.rs`** — bump SCHEMA_VERSION.

4. **New file `src/graph/attributes.rs`** — `extract_attributes` pass run after bindings/impls. For each local Item, navigate to its declaration via `try_to_nav`, walk `ast::HasAttrs::attrs()`, format each as `"#[derive(Debug, Clone)]"` / `"#[must_use]"` / `"/// doc line"`. ~120 LOC.

5. **`src/graph/extract.rs`** — invoke `extract_attributes` after `extract_impl_items` and before `extract_usages`. Need to thread `vfs` through.

6. **`src/graph/queries.rs`**:
   - `item_attributes(target) -> Vec<String>` — direct lookup. ~15 LOC.
   - `items_with_attribute(crate, attr_pattern)` — for "find all `#[must_use]` items in foo crate." ~30 LOC.

7. **MCP tools** — wrap both queries.

8. **Tests** — synthetic fixture with `#[derive(Debug, Clone)]`, `#[must_use]`, `#[non_exhaustive]`, doc comments. Assert each surfaces correctly.
   ```
   cargo test --lib -j 2 graph::attributes -- --test-threads=1
   ```

### Use cases unlocked

- `#[must_use]` audit — pub fns returning `Result`/`Option` without it.
- `#[non_exhaustive]` audit — pub enums/structs that should likely have it.
- Derive standard traits — pub structs without `Debug`, error types without `Error`.
- Doc-comment presence on every pub item.
- `#[inline]` on large fns (combined with `function_signatures` for body size).

---

## Phase 4 — Three small wrappers (1 day, ~150 LOC total)

Pure queries over existing data. No extraction or schema changes.

### 4a. `pub_use_pub_type_audit(crate)`

For each `Item.TypeAlias` in the crate, check if there's a `Binding` with `is_explicit_pub_use && visible_name == alias.name && target == alias.target`. If yes, the `pub type` is functionally a re-export disguised as an alias — flag it.

~40 LOC in `queries.rs` + ~25 LOC MCP wiring.

### 4b. `re_export_chain(target)`

Find every `Binding` whose `target == target_id && is_explicit_pub_use`. For each, recurse on the binding's `from_module` to find its re-exports of the re-export. Bounded by `MAX_REEXPORT_HOPS = 8` (already defined for `lookup_by_qualified_name`).

~50 LOC + MCP wiring.

**Use case**: "Audit the public surface of `Token`" — show every place it's re-exported and the canonical declaration.

### 4c. `crate_dependency_metric()`

For each local crate, compute Robert Martin's instability metric:
- `efferent` (Ce) = outgoing edges from `crate_edges`
- `afferent` (Ca) = incoming edges
- `instability = Ce / (Ce + Ca)` (0 = max stable, 1 = max unstable)

Optionally also `abstractness = (traits + pub_type_aliases) / total_items`.

~60 LOC + MCP wiring.

**Use case**: single-number health metric for refactor decisions.

### Tests

One test per wrapper, all in `queries.rs::tests`:
```
cargo test --lib -j 2 graph::queries::pub_use_audit graph::queries::reexport_chain graph::queries::dependency_metric -- --test-threads=1
```

---

## Phase 5 — `function_signatures` (3-5 days, ~500 LOC)

The heavy one. Engages HIR's `Ty` system, which Layers 1-4 deliberately avoided. Schema bump.

Highest-leverage extraction — unlocks most of the param/borrow/async checks.

### Steps

1. **Decide stringification scheme.** Use `HirDisplay::display(db).to_string()` for canonical type strings. Document lifetime-handling caveats. Optional `verbosity: short | full` knob.

2. **`src/graph/model.rs`** — new types:
   ```rust
   pub struct FunctionSignature {
       pub params: Vec<Param>,
       pub return_type: String,
       pub is_async: bool,
       pub self_param: Option<SelfKind>,
       pub generics: Vec<GenericBound>,
   }
   pub struct Param { name, ty, by_ref, mutability }
   pub enum SelfKind { Owned, Ref, RefMut }
   pub struct GenericBound { name, bounds: Vec<String> }
   ```

3. **`src/graph/storage.rs`** — new `signatures_by_target` sub-DB (key=NodeId, val=FunctionSignature). Schema bump.

4. **New file `src/graph/signatures.rs`** — extract for each `Item.Fn` / `Item.Method` / `Item.AssocFunction`. ~200 LOC.

5. **`src/graph/extract.rs`** — invoke `extract_signatures` after impls, before usages.

6. **`src/graph/queries.rs`**:
   - `function_signature(target) -> Option<FunctionSignature>`
   - `functions_with_filter(crate, filter)` — for batch queries
   ~60 LOC.

7. **MCP tools** — `function_signatures(directory, filter?)`. Filter shape:
   ```rust
   FunctionFilter {
       min_param_count: Option<usize>,
       has_param_type: Option<String>,    // substring match
       returns_type_pattern: Option<String>,
       is_async: Option<bool>,
       self_kind: Option<SelfKind>,
   }
   ```

8. **Tests** — synthetic fixture covering: `&self`, `&mut self`, `self`, generic bounds, `async fn`, lifetimes, `Result<T, E>` return. ~100 LOC of tests.

### Risk areas

Type stringification edge cases on lifetimes, const generics, `impl Trait` returns. Build a "scary types" fixture early:
```rust
fn f<'a, const N: usize>(x: &'a [u8; N]) -> impl Iterator<Item = &'a u8>
```
and lock the expected output before extending coverage.

### Use cases unlocked

- "5+ parameter functions" — `len(params) >= 5` → flag.
- "Avoid boolean parameters" — find any param of type `bool`.
- `as_*` should borrow, `to_*` may allocate, `into_*` consumes — verify naming convention matches signature.
- "Borrow by default" — find pub fn taking `String`/`Vec<T>`/`PathBuf` where `&str`/`&[T]`/`&Path` would do.
- `&self` vs `&mut self` vs `self` distribution per type.
- `async fn` density per crate.

---

## Phase 6 — `unsafe_audit` (2 days, ~150 LOC, optional)

Requires AST traversal we don't currently do. Run lazily at query time, no schema impact.

### Steps

1. **New file `src/graph/unsafe_audit.rs`** — for each local crate, walk every file's syntax tree, find `ast::BlockExpr` nodes with `unsafe_token`. For each unsafe block:
   - Locate enclosing fn via `Semantics::scope_at_offset` + `containing_function()`.
   - Extract byte span (`SyntaxNode::text_range()`).
   - Count source lines.
   - Scan preceding 5 lines for `// SAFETY:` comment.

2. **Result struct**:
   ```rust
   pub struct UnsafeFinding {
       pub file: String,
       pub span: (u32, u32),
       pub line_count: u32,
       pub enclosing_function: Option<NodeId>,
       pub has_safety_comment: bool,
   }
   ```

3. **`src/graph/queries.rs`** — `unsafe_audit() -> Vec<UnsafeFinding>`. Live computation; cache nothing (rare query, full-workspace AST scan is acceptable).

4. **MCP wrapper.**

5. **Tests** — synthetic fixture with `unsafe { ... }` blocks both with and without `// SAFETY:`, verify detection.

### Use cases unlocked

- "Keep unsafe blocks small" — block size distribution per crate.
- "Put a `// SAFETY:` comment" — coverage check.
- Full unsafe inventory for safety-critical review.

---

## Phase 7 — `mut_static_audit` (1 day, ~100 LOC)

Two implementation paths:

### Path A — Heuristic (ship first)

Source-scan `Node.span` ranges of `Item.Static` items, regex-match for `static\s+mut\s+`, `LazyLock`, `OnceCell`, `OnceLock`, `lazy_static!`. Documented false-positive risk: a `static FOO: &str = "OnceCell"` would match.

### Path B — Type-aware (ship after Phase 5)

Once `function_signatures` extraction is in place, types of `Item.Static` items become available. Check the type name directly.

### Steps (Path A)

1. **`src/graph/queries.rs`** — `mut_static_audit() -> Vec<MutStaticFinding>`. For each Item with `item_kind == Some(ItemKind::Static)`, read source via `Node.file` + `Node.span`, scan for the patterns. ~50 LOC.

2. **Result struct**:
   ```rust
   pub struct MutStaticFinding {
       pub item: NodeId,
       pub qualified_name: String,
       pub matched_pattern: String,  // "static mut" | "LazyLock" | "OnceCell" | ...
       pub file: String,
       pub span: (u32, u32),
   }
   ```

3. **MCP wrapper.**

4. **Tests** — synthetic fixture with each pattern present and absent.

5. **Document** in the tool description that this is a heuristic and recommend the Phase 5–dependent version once available.

### Use cases unlocked

- "Avoid global mutable state."
- "Hidden singleton config, auth, clocks, RNGs" anti-pattern.

---

## Build / memory discipline (still in effect)

Every cargo command goes through:
```
nix develop ../nix-devshells#code --command cargo <subcmd> -j 2
```

- Use `--lib` test scope only — never `--tests` (compiles 20+ test binaries, OOMs).
- Test with `--test-threads=1`.
- Targeted test paths: `cargo test --lib graph::usages` not `cargo test`.

The OOM from `cargo build --tests` previously cost real time. Don't repeat.

---

## Total estimated effort

| Phase | Days | LOC | Schema bump? |
|-------|------|-----|--------------|
| 1. `forbidden_dependency_check` | 1 | 100 | no |
| 2. `enum_variants` | 0.5 | 30 | yes |
| 3. `item_attributes` | 1.5 | 250 | yes |
| 4. Three wrappers | 1 | 150 | no |
| 5. `function_signatures` | 3-5 | 500 | yes |
| 6. `unsafe_audit` | 2 | 150 | no |
| 7. `mut_static_audit` (Path A) | 1 | 100 | no |
| **Total** | **10-12** | **~1280** | 3 schema bumps |

**Cheap wins (Phases 1–4):** ~3-4 days, massive coverage gain.
**Heavy lift (Phase 5):** ~3-5 days alone.
**Optional (Phases 6–7):** ~3 days, defer until concrete need.

## Critical path

`1 → 2 → 3 → 4 → 5 → (6, 7 in either order)`

Phases 1, 2, 4 are independent and could be parallelized as separate worktrees. Sequential is simpler. Phase 7 Path B depends on Phase 5.

## If shipping only one

**Phase 1 (`forbidden_dependency_check`).** Highest leverage-per-line in the whole catalog. Turns ad-hoc architectural rules into version-controlled CI assertions.

---

## What's NOT in this plan (deliberately)

These belong in clippy / cargo-* / external tools, not the hypergraph:

| Guideline | Why it's not here |
|-----------|-------------------|
| `unwrap()` audits | clippy `unwrap_used` / `expect_used` |
| `.await` while holding locks | clippy `await_holding_lock` |
| `redundant_clone` | clippy + profiling |
| `manual_let_else` | clippy |
| Cargo.toml hygiene | `cargo-deny` / `cargo-audit` / `cargo-machete` |
| MSRV checks | `cargo-msrv` |
| Doc generation | `cargo doc` + `rustdoc::missing_docs_in_private_items` |
| Test discovery | `cargo test` / `cargo nextest` |
| Performance benchmarks | `criterion`, `flamegraph`, `perf` |
| Formal verification | Kani, Prusti, Creusot, Verus |

Don't try to build these in the hypergraph — clippy and ecosystem tools do them better, and they require parser/AST hooks the hypergraph doesn't have.

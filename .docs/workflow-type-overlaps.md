# Workflow — type overlaps and naming hygiene

Audit a Rust workspace for name collisions and namespace footguns: same type name in multiple crates, modules shadowing workspace crates, within-crate duplicate type names, and common fn names hinting at missing abstractions.

Two scopes:
1. **Workspace-wide** — every collision in the workspace at once.
2. **Single-crate** — the subset involving one specific crate, plus its internal hygiene.

The workflow distinguishes intentional design (wire vs domain types, etc.) from accidental migration debt or footguns.

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

If the schema bumped or sources changed, this rebuilds. Otherwise reuse is sub-second.

---

## Scope 1 — workspace-wide

### Step 1. Pull overlap data

```
overlaps(directory=...)
```

Returns four buckets:
- `cross_crate_type_collisions` — same type name in 2+ crates
- `module_shadows` — `mod X` matching a workspace crate name
- `within_crate_type_duplicates` — same name in different submodules of one crate
- `common_fn_names` — fn names appearing in 4+ crates

### Step 2. Cross-crate collisions: investigate each

For each entry, fetch usage on both qualified names in parallel:

```
who_uses_summary(directory=..., target=A_qualified_name)
who_uses_summary(directory=..., target=B_qualified_name)
```

Apply this decision matrix:

| Pattern | Verdict | Severity |
|---|---|---|
| Both used by the same consumer module | Likely accidental dupe or half-finished migration. The consumer is converting between them. | 🔴 High |
| Different consumer sets, no overlap | Independent concepts with shared name. Functional but ambiguous. | 🟢 Low |
| Different shapes (Struct vs Enum, fields differ) | Definitely independent. Pure naming collision. | 🟢 Low |
| One side is a re-export alias for the other | Verify it's intentional; if just legacy, drop the alias. | 🟡 Medium |
| Both sides have non-trivial unique consumers + same domain area | Genuine ambiguity — domain split (wire/domain types) or candidate for unification. | 🟡 Medium |

To dig deeper:

```
find_definition(name=A_unqualified_name)        → file:line for each definition
read_file_content(path=...)                     → inspect actual struct/enum bodies
get_similar_code(query=<one-of-the-bodies>)     → semantic neighbors (catches dupes get_overlaps misses)
```

### Step 3. Module shadows: real bug or footgun?

For each shadow `(crate=X, shadowed=Y)`:

Filter `crate_edges` for `consumer_crate=X, producer_crate=Y`:
- **Both shadow + actual dep on Y** → real bug risk. Inside X, references to `Y::...` resolve to the local `mod Y` not the workspace crate Y. Verify call sites with `read_file_content`.
- **Shadow only, no dep on Y** → footgun but functional. Anyone trying to add `use Y::...` inside X gets the local module silently.

Both warrant renaming the local module.

### Step 4. Within-crate duplicates: distinguish test fixtures from real dupes

Most within-crate duplicates are test fixtures replicated across test modules. Pattern recognition:
- Names like `Mock*`, `Fake*`, `Stub*`, `Test*`, `Recording*` → test-fixture duplicates.
- Located in modules ending in `tests`, `test`, `fixtures`, `common` → almost certainly fixtures.

For test fixtures:
- Factor into a shared `<crate>::tests::common::*` module.
- Mechanical refactor, big readability win, no product code change.

For non-fixture within-crate duplicates:

```
read_file_content(path=...)                     → inspect each definition
get_similar_code(query=<body>)                  → confirm semantic equivalence
```

Decide: merge to single canonical home (if equivalent), or document the intentional split.

### Step 5. Common fn names: check for missing abstractions

`common_fn_names` of `main` is expected (one per binary). Other entries warrant checking:
- `init`, `default`, `new`, `apply` in 4+ crates → probably normal Rust idioms, not actionable.
- More specific names appearing across many crates → possible missing trait. E.g., if 5 crates have `parse_config`, a `Config` trait might be earned.

### Step 6. Severity-ranked output

Produce a findings table:

```
🔴 High    — Same-name type used by the same consumer (migration debt)
🔴 High    — Module shadow + actual workspace-crate dep (real bug risk)
🟡 Medium  — Different-shape collisions in same domain area (structural ambiguity)
🟡 Medium  — Module shadow without dep (footgun)
🟢 Low     — Independent concepts with shared name (rename for clarity)
🟢 Low     — Test-fixture duplicates (mechanical refactor)
⚪ Info    — common_fn_names that confirm idiomatic Rust (no action)
```

---

## Scope 2 — single crate

### Step 1. Pull overlaps + crate context

```
overlaps(directory=...)                                  → workspace-wide overlap data
module_tree(directory=..., krate=X)                      → full crate structure
```

The `overlaps` call returns workspace data; filter relevant entries to those involving crate `X`.

### Step 2. Filter overlap entries to this crate

From `cross_crate_type_collisions`: keep entries where any location has `crate_name=X`.
From `module_shadows`: keep if `crate_name=X` (X is the shadowing crate) OR `shadowed_crate=X` (X is the shadowed crate).
From `within_crate_type_duplicates`: keep if `crate_name=X`.

### Step 3. Investigate cross-crate collisions involving X

For each filtered collision, run `who_uses_summary` on both qualified names. Apply the same decision matrix as Scope 1 Step 2:
- Same consumer for both → severity HIGH.
- Different shapes + different consumers → severity LOW (rename for clarity).

### Step 4. Investigate within-crate duplicates

Apply the same fixtures-vs-real-dupes pattern as Scope 1 Step 4. For X specifically:
- Walk `module_tree(X)` looking at the parent paths of each duplicate.
- Test fixtures cluster under `tests`, `unit`, `common` modules → mechanical refactor.
- Production duplicates are a deeper hygiene issue.

### Step 5. Module organization audit (single-crate hygiene)

Walk `module_tree(X)` looking for:
- **Namespace overload**: a module containing both production code AND test fixtures. Common smell: a module named `unit` or `test` containing a `*Display` or `*Presentation` type.
- **Unusual depths**: a `pub fn` at depth 5 might be a leak.
- **Inconsistent naming**: `tests` at one level, `unit` at another, `common` at a third.

### Step 6. Crate-internal collisions (subtler)

For each pair of types within `module_tree(X)` with the same `display_name`:
- If they're in `within_crate_type_duplicates`, that's already caught.
- If they're at different depths (one is a method, one is a top-level item), `module_tree`'s `kind` field disambiguates (`Item.Method` vs `Item.Struct`).
- A method named the same as a top-level type is rarely a problem, but worth glancing.

### Step 7. Method-level naming check (Layer 4)

For types with many methods, scan for inconsistent naming:
- Every constructor is `new`? Some `from`? Some `create`?
- Error types: `from_io`, `from_parse`, etc., consistent?

`module_tree` shows methods as children of types. Inconsistent patterns are subjective but worth noting.

### Step 8. Decision frames

| Finding | Action |
|---|---|
| Type collision with same-consumer migration debt involving X | Pick canonical home, delete duplicate (HIGH severity) |
| Module in X shadows a workspace crate | Rename the local module (MEDIUM-HIGH depending on if dep also exists) |
| Test fixtures duplicated in X | Factor into `X::tests::common::*` |
| Production duplicates in X | Read-and-merge or document split |
| Naming-only collision (different shapes) | Rename for clarity (LOW) |
| Namespace overload (test + prod in same module) | Split modules |

---

## Worked examples

### Workspace example (`coding-agent-bad`)
5 cross-crate collisions, 1 module shadow, 6 within-crate duplicates. Top severity: `AgentConfig` exists in both `agent::config` and `config` crates, both used by `coding-agent::compose` — half-finished migration. Plus `agent::config` shadows the workspace `config` crate (footgun). 4 of the 6 within-crate duplicates are test mocks (`MockProvider`, `MockRegistry`, `MockSessionStore`, `MockTool`). Common fn names: empty (good hygiene signal).

### Single-crate example (`tui` in `coding-agent-bad`)
1 within-crate duplicate (`TestEventSender` in `tui::unit::bridge_plugin` and `tui::unit::replay`, test fixtures), 1 cross-crate collision (`ToolName` — Enum in tui vs Struct in permissions, different shapes, low-severity rename), 1 namespace overload (`tui::unit` mixes test fixtures with `tui::unit::presentation::ToolName` which is production code). Total cleanup: factor TestEventSender, rename ToolName, split unit module. Few hours of work.

---

## Pattern reference

These are the regex-ish heuristics that distinguish intent from accident:

| If you see... | Probably... |
|---|---|
| Same name in domain + provider crates | Wire/domain split (intentional) |
| Same name in core + tests modules | Test fixture duplicate (mechanical refactor) |
| Same name in 5+ test modules of one crate | Recurring test fixture (factor to common) |
| Module name shadowing crate + dep edge to that crate | Real bug |
| Module name shadowing crate, no dep | Footgun (rename anyway) |
| Two types with same name, different shapes (Struct vs Enum) | Independent concepts (rename for clarity) |
| Two types with same name + same consumer module | Migration debt (HIGH) |
| Common fn name `main` in many crates | Expected (binaries) |
| Common fn name `init`, `default`, `new` | Idiomatic Rust (no action) |
| Other common fn name in 4+ crates | Possible missing trait abstraction |

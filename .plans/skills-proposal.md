# Proposal: one SKILL.md per workflow

**Inputs surveyed**
- `.docs/workflows.md` — topical intent map + tool cheat-sheet + recipe combinations
- `.docs/workflows-detailed.md` — 24 numbered workflows W1–W24 with steps, decision frames, worked examples (~2,400 lines)
- `.docs/workflow-imports-exports.md` — full detailed workflow for W7 (already in skill-template shape)
- `.docs/workflow-type-overlaps.md` — full detailed workflow for W9 (already in skill-template shape)

**Existing skills surveyed** (`~/.claude/skills/`): `rusty/SKILL.md`, `rust-workspace-symbols/SKILL.md`, `generate-docs/SKILL.md`. The user's skill format is YAML frontmatter (`name`, `description`, optional `argument-hint`, optional `allowed-tools`) followed by a markdown body. Skills surface in the `/`-picker and are auto-suggested to Claude through the `description` field.

---

## 1. Approach

Produce **24 SKILL.md files**, one per workflow W1–W24 in `workflows-detailed.md`. Each skill:

- Has a unique, kebab-case name prefixed `rust-mcp-` so they cluster together in the `/`-picker.
- Has a `description` field engineered around the phrasings a user is likely to type — that's what drives auto-invocation.
- Has a self-contained body (no external `Read` of `.docs/workflows-detailed.md` at runtime). The body is generated *from* the source workflow doc but doesn't depend on it after generation.
- Follows a fixed template (§3) so they read uniformly and can be regenerated cleanly if the source docs change.

W7 and W9 are special — they already have full standalone docs (`workflow-imports-exports.md`, `workflow-type-overlaps.md`). Their skills lift the body from those docs directly rather than the brief stubs inside `workflows-detailed.md`.

---

## 2. The 24 skills

| # | Skill name | Source workflow | One-line trigger summary |
|---|---|---|---|
| 1 | `rust-mcp-find-symbol` | W1 | Find a Rust symbol's qualified name from a string, file path, or vague description |
| 2 | `rust-mcp-workspace-overview` | W2 | First-look audit of an unfamiliar Rust workspace — shape, hygiene, hotspots |
| 3 | `rust-mcp-crate-audit` | W3 | Deep dive on one Rust crate — structure, public surface, dead pubs, complexity |
| 4 | `rust-mcp-module-audit` | W4 | Audit a single Rust module — imports, exports, re-exports, internal structure |
| 5 | `rust-mcp-symbol-forensics` | W5 | Everything about one Rust symbol — callers, importers, fan-in, test vs prod |
| 6 | `rust-mcp-trait-audit` | W6 | Audit a Rust trait — methods, dispatch sites, single-impl, sealing candidates |
| 7 | `rust-mcp-imports-exports` | W7 | Workspace-wide and per-crate import/export audit (uses workflow-imports-exports.md) |
| 8 | `rust-mcp-refactor-plan` | W8 | Plan a Rust refactor — delete/move/downgrade/dedupe/seal decisions with evidence |
| 9 | `rust-mcp-type-overlaps` | W9 | Type-name collisions, module shadows, within-crate duplicates (uses workflow-type-overlaps.md) |
| 10 | `rust-mcp-test-vs-prod` | W10 | Distinguish test-only from production code via Test/Other category split |
| 11 | `rust-mcp-method-api` | W11 | Audit a Rust type's method surface — fan-in per method, dead methods, naming consistency |
| 12 | `rust-mcp-api-surface` | W12 | Audit a crate's public API surface — declared vs effective, facade hygiene |
| 13 | `rust-mcp-semantic-overlaps` | W13 | Find duplicate logic via semantic similarity — clones, convergent enums, same-shape structs |
| 14 | `rust-mcp-call-graph` | W14 | Workspace-wide fn-level call graphs — callers, callees, recursive trees, blast radius |
| 15 | `rust-mcp-complexity` | W15 | Prioritize refactors by complexity × blast radius |
| 16 | `rust-mcp-snapshot-diff` | W16 | Compare two Rust workspace snapshots / branches — API surface, dead-pub, edge weights, complexity |
| 17 | `rust-mcp-architecture-rules` | W17 | Enforce architectural rules — DAG, layering, "no tokio in domain" — via forbidden_dependency_check |
| 18 | `rust-mcp-attribute-audit` | W18 | Audit Rust attributes — `#[deprecated]`, `#[must_use]`, `#[non_exhaustive]`, derives |
| 19 | `rust-mcp-signature-search` | W19 | Find Rust fns by signature shape — param types, return types, self-kind, async-ness |
| 20 | `rust-mcp-unsafe-audit` | W20 | Audit `unsafe { ... }` blocks — SAFETY-comment compliance, block size, blast radius |
| 21 | `rust-mcp-mut-static-audit` | W21 | Audit global mutable state — `static mut`, `LazyLock`, `OnceLock`, `OnceCell` |
| 22 | `rust-mcp-reexport-chain` | W22 | Trace `pub use` re-export chains; detect pub-type masquerading as re-export |
| 23 | `rust-mcp-dependency-metric` | W23 | Rank crates by Robert Martin instability / abstractness / fan-in / fan-out |
| 24 | `rust-mcp-enum-variants` | W24 | Inspect enum variants and per-variant fan-in; detect dead variants |

---

## 3. SKILL.md template

Every skill body uses this shape, lifted from `workflow-imports-exports.md` (which is already battle-tested as a workflow template):

```markdown
---
name: rust-mcp-<workflow-slug>
description: |
  <2-3 sentences engineered around the user phrasings that should trigger this
  skill. Lead with the verb of the task ("Audit...", "Find...", "Trace..."),
  list the concrete questions it answers, end with "Use when ..." plus 3-5
  example phrasings.>
argument-hint: "<one-line hint for what arguments to pass>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# <Title>

<1-paragraph framing — what this skill does, scope (workspace-wide /
single-crate / single-symbol), and what it does NOT cover (pointers to
sibling skills).>

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
[index_codebase(directory=<absolute-path>)  # only if semantic tools are used]
```

## Workflow

### Scope <name the scope variant if the workflow has them>

### Step 1. <verb phrase> (parallel)

```
<MCP tool call 1>
<MCP tool call 2>
...
```

### Step 2. <interpret outputs>

<decision-frame table copied verbatim from the source workflow doc>

### Step 3. ...

...

## Decision frames

| Finding | Action |
|---|---|
| ... | ... |

## Pattern reference

| If you see... | Means |
|---|---|
| ... | ... |

## Output format

Severity-ranked findings table:

```
🔴 High    — broken or contradictory state
🟡 Medium  — wasted surface or namespace overload
🟢 Low     — naming clarity, mechanical refactors
⚪ Info    — confirms healthy structure
```

A clean output is a markdown table per finding with: severity, location
(qualified name + file:span where available), what's wrong, recommended
action.

## Limitations

<copied from the source workflow doc>

## Worked example

<lifted from the source workflow doc when one exists>
```

Standardising on this template means every skill reads the same way and can be regenerated by a small script from the source doc if drift becomes a problem (§7).

---

## 4. Two concrete examples (full SKILL.md drafts)

### Example A — `rust-mcp-workspace-overview` (W2)

```markdown
---
name: rust-mcp-workspace-overview
description: |
  First-look audit of an unfamiliar Rust workspace using the rust-code-mcp
  hypergraph. Produces architecture shape (crate edges + dependency metric),
  hygiene snapshot (overlaps, dead pubs, unsafe blocks, mut statics), and
  complexity hotspots in one pass. Use when the user says "what is this
  codebase", "give me an overview", "explore this workspace", "I just inherited
  this repo", or before any deeper crate-level audit.
argument-hint: "[workspace-path]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust workspace overview

First-look recipe. Use when inheriting a codebase, comparing branches, or
starting any deeper audit. Scope: workspace-wide.

For single-crate audits, use `rust-mcp-crate-audit`. For single-module audits,
use `rust-mcp-module-audit`.

## Prerequisites

build_hypergraph(directory=<absolute-path>)
index_codebase(directory=<absolute-path>)   # required for semantic_overlaps

## Workflow

### Step 1. Foundation (parallel)

workspace_stats(directory=...)
crate_edges(directory=...)
crate_dependency_metric(directory=..., sort_by="instability", top_n=10)
dead_pub_report(directory=...)
overlaps(directory=...)

### Step 2. Read workspace_stats for shape

[... lifted from workflows-detailed.md W2 step 2 ...]

### Step 3. Read crate_edges for architectural shape

[... lifted W2 step 3 ...]

### Step 4. Read dead_pub_report for rot

[... lifted W2 step 4 ...]

### Step 5. Read overlaps for hygiene

[... lifted W2 step 5 ...]

### Step 6. Spot the gnarl

analyze_complexity(file_path=<hottest_file_per_crate>)

### Step 7. Output snapshot

[... lifted W2 step 7 ...]

### Step 8. Unsafe surface

unsafe_audit(directory=..., has_safety_comment=false)

### Step 9. Global mutable state

mut_static_audit(directory=...)

### Step 10. Literal duplicates

semantic_overlaps(directory=..., threshold=0.95)

### Step 11. Optional: architectural rules

forbidden_dependency_check(directory=..., rules=[...])

## Decision frames

| Finding | Action |
|---|---|
| pub_crate_share < 0.2 | Low encapsulation discipline — many leaked pubs |
| Cycle in crate_edges | Architectural smell — should be a DAG |
| Producer with 1000+ fan-in | "Universal types" crate — change at your peril |
| Dead-pub count > 50 in one crate | Vendored library OR genuine rot — verify before action |
| Undocumented unsafe block in hot path | Add SAFETY: comment or break apart |
| static mut found | Unsafe singleton — replace with OnceLock/LazyLock |

## Output format

🔴 High / 🟡 Medium / 🟢 Low / ⚪ Info severity-ranked findings table.

## Worked example (coding-agent-bad)

17 crates; pub_crate_share 0.07 (low); domain has 1441 fan-in; tools has
13 dead-pub `*Tool` types (trait-registry dispatch); AgentConfig duplicated
across agent::config and config crates with shared consumer = half-finished
migration (🔴 High).
```

### Example B — `rust-mcp-symbol-forensics` (W5)

```markdown
---
name: rust-mcp-symbol-forensics
description: |
  Deep dive on a single Rust symbol using the rust-code-mcp hypergraph. Returns
  declaration site, every importer, every non-import reference, Test/Other
  category split, cross-crate fan-in, and method-level fan-in (Layer 4). Use
  when the user asks "who uses X", "where is X called", "what would break if
  I change X", "show me references to X", "is X safe to delete".
argument-hint: "<qualified-symbol-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust symbol forensics

Single-symbol deep-dive. Works for structs, enums, traits, fns, methods,
consts, type aliases, assoc consts, assoc types. Scope: single symbol Y
(qualified name).

For trait-specific analysis (methods + dispatch), use `rust-mcp-trait-audit`.
For fn-level call graphs, use `rust-mcp-call-graph`. For refactor decisions,
use `rust-mcp-refactor-plan`.

## Prerequisites

build_hypergraph(directory=<absolute-path>)

## Workflow

### Step 1. Locate

find_definition(symbol_name=<short_name>)

### Step 2. Render declaration

read_file_content(file_path=<file>)   # widen by ~10 lines

### Step 3. Reverse lookups (parallel)

who_imports(directory=..., target=<qualified_name>)
who_uses(directory=..., target=<qualified_name>)
who_uses_summary(directory=..., target=<qualified_name>)

### Step 4. Render call sites with context

For each who_uses hit, read_file_content with slice [start - 200, end + 200].

### Step 5. RA cross-reference

find_references(symbol_name=<short_name>)

Different scope from who_uses — catches local var refs, lifetimes, macro
expansions that the hypergraph doesn't index. Use both when verifying
"is X really unused?".

### Step 6. Cross-crate fan-in summary

Group who_imports by consumer crate.

### Step 7. Method-level fan-in (Layer 4)

If Y is a type, run who_uses on Y::method for each method from module_tree.

## Decision frames

| Finding | Verdict |
|---|---|
| who_uses empty + who_imports empty + find_references empty | Safe to delete |
| who_uses empty + who_imports non-empty | Imported as generic bound; investigate |
| who_uses_summary 100% Test | Test fixture; demote or #[cfg(test)] |
| who_uses_summary 100% Other | Critical path; refactor with care |
| Single consumer module | Tightly coupled to one place; consider co-locating |
| Many consumer crates | Workspace-shared API; avoid breaking changes |

## Pattern reference

| Signal | Means |
|---|---|
| who_uses empty but find_references populated | Macro-introduced or cfg-gated context |
| Read >> Write | Read-mostly API — encapsulation healthy |
| Write-heavy | Diffuse invariants; many writers means brittle state |

## Limitations

- Method-level resolution requires Layer 4 (current).
- Trait dispatch through dyn T may miss some sites (resolver is type-based).
- Macro-expanded refs sometimes don't surface — fall back to find_references.
```

The remaining 22 skills follow the same shape with bodies pulled from the corresponding W-section.

---

## 5. Description field — the trigger surface

Skills are auto-picked from the `description` field. Each description follows this 3-part recipe:

```
<Verb-of-the-task> <object> using rust-code-mcp.
<Concrete questions answered, comma-separated.>
Use when <list of 3-5 example user phrasings>.
```

Example phrasings to mine for each skill:

| Skill | Sample phrasings to cover |
|---|---|
| `rust-mcp-find-symbol` | "where is X defined", "find the symbol that...", "what's the qualified name for..." |
| `rust-mcp-workspace-overview` | "what is this codebase", "give me an overview", "I just inherited this repo" |
| `rust-mcp-crate-audit` | "audit crate X", "dissect crate X", "what's in the X crate" |
| `rust-mcp-symbol-forensics` | "who uses X", "where is X called from", "what would break if I change X" |
| `rust-mcp-trait-audit` | "audit trait T", "find dead methods on T", "is T safe to seal" |
| `rust-mcp-refactor-plan` | "is X safe to delete", "should I downgrade X to pub(crate)", "should I move X" |
| `rust-mcp-architecture-rules` | "enforce X doesn't import Y", "DAG check", "no tokio in domain" |
| `rust-mcp-call-graph` | "who calls X", "what does X call", "blast radius of X" |
| `rust-mcp-unsafe-audit` | "audit unsafe blocks", "find undocumented unsafe", "SAFETY-comment compliance" |
| `rust-mcp-mut-static-audit` | "find global mutable state", "hidden singletons", "static mut audit" |
| `rust-mcp-attribute-audit` | "find deprecated items", "must_use compliance", "non_exhaustive audit" |
| `rust-mcp-signature-search` | "fns returning Result<X>", "methods on &Path", "consuming methods" |
| `rust-mcp-enum-variants` | "enum variant fan-in", "dead variants", "enum audit" |
| `rust-mcp-dependency-metric` | "most-depended-on crate", "instability ranking", "Robert Martin metric" |
| `rust-mcp-complexity` | "gnarly functions", "complex code", "refactor priority by complexity" |
| `rust-mcp-snapshot-diff` | "compare branches", "API surface diff", "before/after refactor verification" |
| `rust-mcp-test-vs-prod` | "test-only helpers", "fixture detection", "production-only methods" |
| `rust-mcp-method-api` | "type's full method surface", "dead methods on T", "method-naming consistency" |
| `rust-mcp-api-surface` | "audit public API", "facade hygiene", "declared vs effective surface" |
| `rust-mcp-semantic-overlaps` | "find duplicate logic", "literal clones", "convergent enum design" |
| `rust-mcp-reexport-chain` | "trace pub use chain", "facade chain decode", "pub type masquerade" |
| `rust-mcp-module-audit` | "audit module M", "what does M import/export" |
| `rust-mcp-imports-exports` | "cross-crate dependencies", "who imports from X", "dead facade" |
| `rust-mcp-type-overlaps` | "name collisions", "module shadows", "type duplicates" |

The test for each description: list 5 plausible user prompts; if the description doesn't cover ≥3 of them, expand.

---

## 6. Where they live

All 24 skills are MCP-driven — they call `mcp__rust-code-mcp__*` tools, not anything project-local. They work in any Rust workspace with the rust-code-mcp server attached. Therefore:

**Recommended: user-global at `~/.claude/skills/rust-mcp-*/SKILL.md`.**

Alternative: project-scope at `.claude/skills/rust-mcp-*/SKILL.md` if the user wants them only to fire in this repo. Easy to move later.

W7 and W9 skills also need a copy of their source docs (the long-form workflow files). Two options:

1. **Inline the body into SKILL.md.** Self-contained; survives moving the repo.
2. **Reference the source doc via `Read` at activation time.** Body stays short but ties the skill to a specific repo path.

Recommended: option 1 (inline) — keeps the skill portable and consistent with the other 22.

---

## 7. Generation strategy — how to actually produce 24 files

Writing 24 SKILL.md by hand invites drift and tedium. Two paths:

### Path A — manual, one-by-one (estimated 4-6 hours)

Walk W1 → W24, copy each section from `workflows-detailed.md` into a SKILL.md, hand-craft the `description` field, polish. Best quality, slowest. Worth it for the high-traffic skills (workspace-overview, crate-audit, symbol-forensics, refactor-plan); diminishing returns for the rarer ones.

### Path B — scripted from `workflows-detailed.md` (estimated 1-2 hours of script + 2 hours of polish)

Write a small Python or shell script that:

1. Parses `workflows-detailed.md` by `^## W<N> —` headings.
2. For each W-section, extracts the title, scope variants, prerequisites, steps, decision frames, pattern reference, worked example.
3. Emits `~/.claude/skills/rust-mcp-<slug>/SKILL.md` using a Jinja-style template matching §3.
4. The `description` field is hand-edited after generation (it's the one field that benefits from human polish — the body is mechanical).

W7 and W9 are special-cased to read from their dedicated docs.

Recommended: **Path B for the body, manual for the description fields.** Re-runnable when the source doc changes. The `name → workflow-id` mapping is in §2.

---

## 8. Rollout

1. **Build the generator + run it.** Produces 24 draft SKILL.md files with mechanical bodies and stub descriptions.
2. **Hand-tune descriptions in priority order.** Start with the 5 high-traffic ones (workspace-overview, crate-audit, symbol-forensics, refactor-plan, find-symbol). Skills with weak descriptions stay invisible to auto-invocation, but `/<name>` still works.
3. **Test each skill in isolation.** Invoke via `/rust-mcp-<name>` and confirm the body still makes sense out of context.
4. **Add an entry to memory** noting which skill maps to which workflow, in case the user asks "which one handles attribute audits" etc.

---

## 9. Open questions for the user

1. **User-global vs project-scope?** Recommended: `~/.claude/skills/`. Confirm.
2. **Generation strategy — Path A (manual) or Path B (scripted)?** Recommended: Path B for bodies, manual for descriptions. Confirm.
3. **Naming prefix — `rust-mcp-` or something shorter (`rmc-`, `mcp-`, no prefix)?** Recommended: `rust-mcp-` for clarity in the picker. Confirm.
4. **For W7 / W9 — inline the dedicated docs into SKILL.md, or reference them at runtime?** Recommended: inline. Confirm.
5. **Should sub-recipes (e.g. W8.12, W13.2, W17.1) get their own SKILL.md, or stay as named sections inside the parent skill?** Recommended: stay inside the parent — 24 skills is already a lot; sub-recipe-level is overkill. Confirm.

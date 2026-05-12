# Proposal: SKILL.md per workflow

**Inputs surveyed**
- `.docs/workflows.md` — 29 numbered topical sections (the "intent map"), tool cheat-sheet, and recipe combinations
- `.docs/workflows-detailed.md` — 24 numbered workflows W1–W24 with steps, decision frames, worked examples (~2,400 lines)
- `.docs/workflow-imports-exports.md` — full detailed workflow for W7 (already in skill-template shape)
- `.docs/workflow-type-overlaps.md` — full detailed workflow for W9 (already in skill-template shape)

**Existing skills surveyed** (`~/.claude/skills/`): `rusty/SKILL.md`, `rust-workspace-symbols/SKILL.md`, `generate-docs/SKILL.md`. The user's skill format is YAML frontmatter (`name`, `description`, optional `argument-hint`, optional `allowed-tools`) followed by a markdown body. Skills are invoked as `/<name>` or auto-suggested via the `description` field.

---

## 1. Core question — granularity

24 workflows is too many to expose as 24 top-level slash commands (`/w1`–`/w24`) — the skill picker would be noisy and discoverability poor. But the workflows are too heterogeneous to collapse into one mega-skill. Three viable options:

| Option | Skill count | Pros | Cons |
|---|---|---|---|
| **A. One skill per workflow** | 24 | Each skill has a crisp trigger | Skill list noise; many near-duplicates (W3/W4 audit, W5/W6 forensics) |
| **B. One skill per workflow family** | 5–7 | Manageable; matches user intent ("explore", "audit", "refactor") | Larger bodies; per-skill trigger less precise |
| **C. Hybrid — one parametric skill + a few high-value standalones** | 1 + 4 = **5** | Tightest list; one umbrella covers all 24; high-traffic workflows still get their own door | Umbrella must dispatch correctly; small upfront design cost |

**Recommendation: Option C.** Rationale:
- 80% of the 24 workflows are "investigation playbooks" you'd reach for by name: W2 (workspace overview), W3 (audit crate X), W5 (forensics on Y), W17 (rule enforcement), etc. They share a structure (prereq → parallel reads → decision frame → recipes). A single dispatcher skill — `/rust-mcp-workflow <id|keyword>` — loads the right section from `workflows-detailed.md` on demand. This keeps `workflows-detailed.md` as the canonical source and avoids 24 near-duplicate `SKILL.md` files that will drift.
- The remaining 20% are workflows you'd want to *trigger by description* — Claude should auto-pick them when the user describes a task. These deserve standalone skills with rich `description` fields.

Concretely, that's **1 dispatcher + 4 standalones = 5 new skills**.

---

## 2. The 5 proposed skills

### S1. `rust-mcp-workflow` (dispatcher / umbrella)

Single skill that fronts all 24 numbered workflows. Activated by id (`W3`) or keyword ("crate audit"). Loads the matching section from `workflows-detailed.md` plus the shared prerequisites block, then proceeds with the workflow.

```yaml
---
name: rust-mcp-workflow
description: |
  Run one of the documented rust-code-mcp investigation playbooks against a Rust
  workspace. Use when the user names a workflow (e.g. "run W3", "audit crate X",
  "do a workspace overview", "trait analysis on Foo") and there's a matching
  numbered workflow in .docs/workflows-detailed.md. Loads the relevant section
  on demand and orchestrates the hypergraph queries.
argument-hint: "<workflow-id-or-keyword> [target]"
allowed-tools: Read, Grep, Bash(rg:*), mcp__rust-code-mcp__*
---
```

Body sketch:
1. Parse `$ARGUMENTS` → resolve to a workflow id (`W1`–`W24`). Maintain an inline keyword map (e.g. "overview" → W2, "trait" → W6, "deprecated" → W18 R18.1).
2. Run the shared prereq (`build_hypergraph`) once.
3. `Read` the matching section from `.docs/workflows-detailed.md` (line ranges baked into a lookup table inside the skill body — see §4).
4. Execute the workflow's parallel-reads block. Pipe outputs through the decision-frame table from the doc.
5. Render the standard finding format (severity 🔴/🟡/🟢/⚪ as in `workflow-imports-exports.md`).

Why this scales: adding a 25th workflow = update the lookup table + extend the doc. No new skill file.

### S2. `rust-workspace-overview` (standalone — W2)

Standalone because "I just got dropped into this codebase, give me the lay of the land" is the highest-traffic entry point — users will describe it many ways, and a rich description field gives Claude a strong trigger.

```yaml
---
name: rust-workspace-overview
description: |
  First-look audit of an unfamiliar Rust workspace. Produces architecture shape
  (crate edges + dependency metric), hygiene snapshot (overlaps, dead pubs,
  unsafe, mut statics), and complexity hotspots in one pass. Use when the user
  says "what is this codebase", "give me an overview", "explore this workspace",
  "I just inherited this repo", or before any deeper audit.
argument-hint: "[workspace-path]"
allowed-tools: Read, mcp__rust-code-mcp__*
---
```

### S3. `rust-refactor-plan` (standalone — W8 + W13 + W15 + W16 cross-cutting)

Standalone because refactor planning crosses multiple workflows (8.1–8.13 recipes, plus complexity-driven prioritization W15, plus semantic-overlap dedupe W13, plus before/after verification W16). The user will say "should I delete X?", "is this safe to move?", "find duplicate logic" — one well-described skill catches all of these.

```yaml
---
name: rust-refactor-plan
description: |
  Plan a Rust refactor with structural evidence. Answers "is X safe to delete",
  "should this trait be sealed", "where should X live", "find dead pub items",
  "find duplicate logic worth extracting", "what's the blast radius if I change Y".
  Combines who_uses / who_imports / dead_pub_report / semantic_overlaps /
  analyze_complexity. Use whenever the user asks a delete / move / merge /
  downgrade question about a Rust symbol.
argument-hint: "<question> [target-symbol]"
allowed-tools: Read, mcp__rust-code-mcp__*
---
```

### S4. `rust-architecture-rules` (standalone — W17)

Standalone because `forbidden_dependency_check` is the one workflow that maps directly to CI usage (declarative rules → violations list). Users will reach for it as a separate gesture: "enforce that the domain crate doesn't import tokio".

```yaml
---
name: rust-architecture-rules
description: |
  Enforce architectural rules on a Rust workspace using forbidden_dependency_check.
  Use for DAG enforcement, layer audits (domain ↛ transport), "no tokio in domain
  crates", async-boundary checks. Returns concrete edge violations with sample
  symbols. Suitable for one-off audits or CI integration.
argument-hint: "[rules-file-or-inline]"
allowed-tools: Read, mcp__rust-code-mcp__*
---
```

### S5. `rust-symbol-forensics` (standalone — W5 + W6 + W14)

Standalone because "tell me everything about this symbol" is a frequent IDE-replacement gesture (goto-def + find-references + call graph + Test/Other breakdown all at once). Crosses W5 (symbol forensics), W6 (if it's a trait), and W14 (call graph if it's a fn).

```yaml
---
name: rust-symbol-forensics
description: |
  Deep dive on a single Rust symbol — declaration site, callers, importers,
  Test/Other category split, blast radius, method-level fan-in (for types),
  call graph (for fns), trait dispatch (for traits). Use when the user asks
  "who uses X", "where is X called from", "what would break if I change X",
  "show me references to X".
argument-hint: "<qualified-symbol-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---
```

---

## 3. SKILL.md template (shared structure)

Each skill body follows this shape — borrowed from the existing `workflow-imports-exports.md` template since it's already battle-tested:

```markdown
# <Title>

<1-paragraph framing — what this skill does, when to use, when NOT to use>

## Prerequisites

build_hypergraph(directory=<absolute-path>)
[index_codebase(...) if semantic tools are involved]

## Steps

### Step 1. <verb phrase> (parallel)
   <inline code block listing the parallel MCP calls>

### Step 2. <interpret outputs>
   <decision frame or table copied from .docs/workflows-detailed.md>

...

## Output format

Produce a severity-ranked findings table:
🔴 High — ...
🟡 Medium — ...
🟢 Low — ...
⚪ Info — ...

## Pattern reference

<if/then table from the source workflow doc>

## Limitations

<copied from the source workflow doc>
```

The dispatcher skill (S1) defers most of this to a dynamic `Read` of `workflows-detailed.md`. Standalones (S2–S5) inline the relevant subset so they don't re-read the doc on every invocation.

---

## 4. How S1 maps workflow-id → doc section

Bake a lookup table into `S1/SKILL.md` so the dispatcher doesn't have to re-scan `workflows-detailed.md` line numbers on every run:

```markdown
## Workflow → doc-section lookup

| Id | Section start (line) | Aliases |
|---|---|---|
| W1 | 33 | symbol-lookup, find-symbol, qualified-name |
| W2 | 104 | workspace-overview, what-is-this-codebase, inherited-repo |
| W3 | 261 | crate-audit, dissect-crate |
| W4 | 360 | module-audit |
| W5 | 426 | symbol-forensics, dissect-symbol |
| W6 | 516 | trait-analysis, trait-audit |
| W7 | 597 | imports-exports → .docs/workflow-imports-exports.md |
| W8 | 620 | refactor-plan, should-i-delete |
| W9 | 764 | type-overlaps → .docs/workflow-type-overlaps.md |
| W10 | 778 | test-vs-prod, fixture-detection |
| W11 | 838 | method-api, type-methods |
| W12 | 917 | api-surface, public-surface |
| W13 | 986 | semantic-similarity, dedupe, find-duplicates |
| W14 | 1171 | call-graph, who-calls, fn-callers |
| W15 | 1286 | complexity, gnarly-fns |
| W16 | 1358 | branch-diff, before-after |
| W17 | 1453 | architectural-rules, dag, forbidden-deps |
| W18 | 1576 | attributes, deprecation, must-use |
| W19 | 1685 | signature-search, fn-discovery |
| W20 | 1806 | unsafe-audit, safety-comments |
| W21 | 1911 | mut-statics, hidden-singletons |
| W22 | 2011 | reexport-chain, facade-trace |
| W23 | 2093 | dependency-metric, instability-rank |
| W24 | 2188 | enum-variants, variant-fan-in |
```

The dispatcher uses this to `Read(.docs/workflows-detailed.md, offset=<line>, limit=<next-line - offset>)` and only loads the relevant ~100 lines.

**Line-number fragility caveat:** every edit to `workflows-detailed.md` will shift these. Two mitigations:
1. The doc is stable — the 24 workflows are sealed; new ones append at the end.
2. Add a pre-commit hook (or a one-line `make` target) that regenerates the table from `grep -n "^## W"`. Keeps drift to zero.

---

## 5. Where to put them

| Skill | Scope | Location |
|---|---|---|
| S1 dispatcher | Project-only (depends on `.docs/workflows-detailed.md`) | `.claude/skills/rust-mcp-workflow/SKILL.md` |
| S2–S5 standalones | Either; recommend user-global since the MCP server is available across repos | `~/.claude/skills/<name>/SKILL.md` |

The dispatcher *must* be project-scoped because its body refs `.docs/workflows-detailed.md` by path. The standalones don't reference local files — they're driven by the MCP server, which is invoked via tool name — so they work in any Rust workspace.

If the user prefers a uniform location, put all 5 under `.claude/skills/` in this repo, accepting they only fire when invoked from this directory. Easy reversal.

---

## 6. The `description` field is the heart of each skill

Skills are chosen by Claude (or auto-picked) primarily from their `description`. The proposed descriptions in §2 are designed around **trigger phrasings** the user is likely to type — "what is this codebase", "who uses X", "is this safe to delete". This matters more than the body, which only loads after the skill is chosen.

When refining, the test is: list 5 plausible user prompts for the skill; if the description doesn't cover at least 3 of them, expand it.

---

## 7. Migration / rollout

A staged rollout keeps it cheap:

1. **Phase 1 — dispatcher only (S1).** One file. Covers all 24 workflows via the lookup table. Validates that `Read(offset, limit)` against the existing doc is fast enough and the output is usable.
2. **Phase 2 — promote S2 (workspace overview).** Highest-traffic entry point; the dispatcher will already work for it, but a standalone gives Claude a stronger trigger.
3. **Phase 3 — S3 / S4 / S5 as demand emerges.** Don't pre-build; let user friction drive which ones earn their own door.

This avoids the trap of writing 24 SKILL.md files up front and discovering 18 of them never get invoked.

---

## 8. Open questions for the user

1. **Project-scope vs user-scope?** Recommended: dispatcher in `.claude/skills/`, standalones in `~/.claude/skills/`. Confirm.
2. **Phase 1 only, or all 5?** Recommended: ship Phase 1 (S1 dispatcher) first; add standalones as needed.
3. **Doc-line-number maintenance?** Recommended: a tiny shell snippet in the dispatcher's body that re-runs the `grep -n "^## W"` if the table looks stale (or document the regen step in the skill's notes section). Confirm preference.
4. **Treatment of `workflow-imports-exports.md` and `workflow-type-overlaps.md`.** They're already in skill-template shape. The dispatcher should `Read` them directly for W7/W9 rather than the workflows-detailed.md stubs (which only point at them). The lookup table in §4 reflects this.
5. **Should the dispatcher *also* support recipe-level resolution** (e.g. `W8.12` → recipe 8.12 "dead facade re-exports") or only top-level workflow ids? Recommendation: yes, recipe-level support — the recipe index in `workflows-detailed.md` (lines 2334–2414) already gives a ready-made dispatch table for sub-recipes.

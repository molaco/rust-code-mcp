---
name: rmc-enum-variants
description: Inspect Rust enum variants.
argument-hint: "<enum-qualified-name>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust enum-variant inspection

Variants of an enum are first-class — list them, then per-variant fan-in
via `who_uses(target=Enum::Variant)`. Scope: single enum.

For workspace-wide variant-duplicate detection across enums, use
`rmc-semantic-overlaps`. For attribute audits like `#[non_exhaustive]`,
use `rmc-attribute-audit`.

## Scope — single enum

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
index_codebase(directory=<absolute-path>)   # only for Step 3 (semantic_overlaps)
```

## Workflow

### Step 1. Pull the variants

```
enum_variants(directory=..., target=<crate>::E)
```

One row per variant in source order with `display_name`,
`qualified_name`, `(file, span)`. Useful for auditing the variant set
without parsing the source manually.

### Step 2. Per-variant fan-in

For each variant:

```
who_uses(directory=..., target=<crate>::E::Variant)
who_uses_summary(directory=..., target=<crate>::E::Variant)
```

Every pattern-match / construction site for the variant. Sort by total →
"which states actually carry the load?".

### Step 3. Cross-reference with `semantic_overlaps`

```
semantic_overlaps(directory=..., item_kind="EnumVariant", threshold=0.95)
```

Variants whose source bytes hash identically (e.g. unit `Error` variant
duplicated across 6 different enums) cluster together. The signal that
the same logical state was modeled as separate variants on different
enums — convergent enum design.

## Recipes

### Recipe — "Variant fan-in"

For an enum E, compute fan-in for every variant. Heaviest-used variants
surface as the load-bearing states; rarely-used variants are candidates
for collapse / split into a different type.

### Recipe — "Dead variant detection"

Variants with empty `who_uses` are dead. The constructor never executes;
the pattern-match arm never matches. Either:

- The variant is reserved for future use (intentional; document with a
  comment).
- The variant is genuine dead state — remove (verify the enum isn't
  `#[non_exhaustive]`, which preserves the variant for downstream
  pattern matching even if no caller in this workspace uses it).

### Recipe — "Convergent enum design"

`semantic_overlaps(item_kind="EnumVariant", threshold=0.95)` clusters
variants whose source is identical. Each cluster is a candidate for
harmonization — extract a shared base, introduce a trait, or collapse
the convergent enums into one.

## Decision frames

| Finding | Action |
|---|---|
| Empty `who_uses` for a variant | Dead variant; remove (mind `#[non_exhaustive]`) |
| One variant carries 90% of fan-in | Other variants may be over-modeled — consider flattening |
| Variants are mostly unused unit variants | Collapse into a flag / single variant |
| Same variant duplicated across 3+ enums | Convergent design — harmonize |
| Variant with all-Test fan-in | Test-only state; gate or remove |

## Pattern reference

| Audit | Invocation |
|---|---|
| List variants | `enum_variants(target=E)` |
| Per-variant fan-in | `who_uses_summary(target=E::Variant)` per variant |
| Convergence | `semantic_overlaps(item_kind="EnumVariant", threshold=0.95)` |
| Cross-reference attributes | `item_attributes(target=E)` for `#[non_exhaustive]` etc. |

## Output format

```
Enum: <crate>::E
Variants: <n>
Per-variant fan-in (sorted desc):
  E::A — <n> uses (Test: <t>, Other: <o>)
  E::B — <n> uses
  E::C — 0 uses (dead?)

Attribute check: <#[non_exhaustive] present | absent>
Convergent variants (from semantic_overlaps): <list of clusters>

Recommendation: <keep | remove E::C | flatten | harmonize-with-other-enums>
```

## Limitations

- Discriminants (the explicit `= 5` part of `Variant = 5`) are present
  only when the source declared them — implicit discriminants aren't
  computed.
- Struct/tuple variant fields are NOT enumerated separately —
  `Variant { a: T, b: U }` returns one row for the variant; fields are
  not graph nodes. To inspect fields, drop to `read_file_content` at the
  variant's span.
- `who_uses(target=E::Variant)` resolves correctly for direct variant
  references, but pattern-matches that bind via `_` or `..` may not
  carry an explicit reference to the variant — the count is a lower
  bound.

---
name: rmc-attribute-audit
description: Audit Rust attributes and doc-comments.
argument-hint: "<attribute-pattern> [crate-name]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust attribute-driven audits

Per-Item attributes (and `///` doc-comment lines) are first-class graph
data. Two tools cover the per-item view and the workspace-wide search.

For inspecting one item's attributes plus its references, prefer
`rmc-symbol-forensics`. For enum variant fan-in cross-referenced with
`#[non_exhaustive]` audits, use `rmc-enum-variants`.

## Scope — single Item or workspace-wide

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
```

## Workflow

### Step 1. Per-item attribute fingerprint

```
item_attributes(directory=..., target=<crate>::Y)
```

Returns the trimmed source text of every `#[...]` attribute (e.g.
`#[derive(Debug, Clone)]`, `#[must_use]`, `#[non_exhaustive]`, `#[inline]`)
and every `///` doc-comment line in source order. Useful for rendering
context around an Item without reading the full file.

### Step 2. Workspace-wide attribute search

```
items_with_attribute(directory=..., crate_name=X, attribute_pattern="#[must_use]")
```

Anchored prefix match on each attribute string OR on the body of a `///`
doc-comment. Each result row carries `match_location: "attr"` or `"doc"`
so callers can filter visually. Case-sensitive.

`attribute_pattern` is a substring anchored as a prefix — e.g.
`"#[deprecated"` matches `#[deprecated]` and `#[deprecated(note = "...")]`,
but not `// #[deprecated]` in a comment block.

### Step 3. Combine with usage data

For each finding:

```
who_uses(directory=..., target=<finding.qualified_name>)
who_uses_summary(directory=..., target=<finding.qualified_name>)
```

Pairs attribute presence with consumption signal: "deprecated items still
being called", "must_use fns whose return is being ignored", etc.

## Recipes

### Recipe — "Deprecation rollout audit"

```
items_with_attribute(crate_name=X, attribute_pattern="#[deprecated")
```

For each finding, `who_uses_summary(target=<qualified_name>)` → rank by
remaining caller count. Non-zero callers = migration backlog. Zero
callers = safe to delete deprecated attribute and the item.

### Recipe — "Serialization surface inventory"

```
items_with_attribute(crate_name=X, attribute_pattern="#[derive(Serialize")
```

Surfaces every type participating in the wire format. Cross-reference
with `module_tree(krate=X)` to confirm visibility (pub Serialize struct =
wire-stable; pub(crate) Serialize struct may be incidental).

Note: `#[derive(Debug, Clone, Serialize)]` matches as ONE attribute string
(the derive list isn't split). `attribute_pattern="#[derive(Serialize"`
matches it; `attribute_pattern="#[derive(Clone)]"` will NOT match
`#[derive(Debug, Clone, Serialize)]` because of differing literal prefix.

### Recipe — "Must-use compliance"

```
items_with_attribute(crate_name=X, attribute_pattern="#[must_use]")
```

Returns every `#[must_use]` Item — the API contract list. Cross-reference
with `module_tree` to find pub fns/types that should carry `#[must_use]`
but don't (manual review — there's no anti-attribute audit).

### Recipe — "Forward-compat audit"

```
items_with_attribute(crate_name=X, attribute_pattern="#[non_exhaustive]")
```

Surfaces enums and structs that are evolution-safe (callers must use `_`
arms / non-positional construction). Combine with `rmc-enum-variants` to
predict downstream breakage when adding a new variant.

### Recipe — "Test-only fns"

```
items_with_attribute(crate_name=X, attribute_pattern="#[cfg(test)]")
```

Catches `#[cfg(test)] fn` declarations. Does NOT catch
`#[cfg(test)] mod tests { fn ... }` — module-gated test fns inherit the
gate from their parent module and don't carry the attribute themselves.
For module-level cfg-gating, walk `module_tree` and inspect parent
attributes manually.

## Decision frames

| Situation | Pattern shape |
|---|---|
| Match exact attribute | `"#[must_use]"` (anchored prefix; closing `]` makes the match strict) |
| Match attribute family | `"#[deprecated"` (no closing bracket; matches `#[deprecated]`, `#[deprecated(...)]`) |
| Match doc-comment substring | `"TODO"` against doc bodies (anchored at start of each `///` line body) |
| Match derive trait | `"#[derive(Serialize"` (matches any derive list with Serialize first; non-first position won't match) |

## Pattern reference

| Audit | Pattern |
|---|---|
| Deprecations | `"#[deprecated"` |
| Must-use | `"#[must_use]"` |
| Non-exhaustive | `"#[non_exhaustive]"` |
| Inline hints | `"#[inline"` |
| Test-only fns | `"#[cfg(test)]"` |
| Serializable types | `"#[derive(Serialize"` |
| Doc TODOs | `"TODO"` (matches `match_location: "doc"`) |

## Output format

```
Pattern: <attribute_pattern>
Scope: <crate or workspace>
Matches: <n> (attr: <a>, doc: <d>)

Per-finding:
  - <qualified_name> [<attr|doc>] — <attribute_string>
    Callers: <who_uses_summary count> (Test: <t>, Other: <o>)
    Action: <delete-attribute | migrate-callers | keep>
```

## Limitations

- Derive lists count as a single attribute string —
  `#[derive(Debug, Clone, Serialize)]` is one entry, not three. Substring
  match against the derive list works but isn't position-independent.
- Nested attributes (`#[serde(skip)]` inside `#[derive(...)]`) are NOT
  split; rendered as one string when they appear in source as one.
- Match is anchored prefix — substring-anywhere matching requires reading
  the full attribute list and filtering client-side.
- `#[cfg(test)]` on a `mod` is NOT inherited by child fns in the result
  list — those child fns won't carry the attribute on their own row.

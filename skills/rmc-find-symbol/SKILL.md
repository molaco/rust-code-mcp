---
name: rmc-find-symbol
description: Find a Rust symbol's qualified name.
argument-hint: "<symbol-name-or-fragment>"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust symbol lookup

Bridge skill. You can't call hypergraph queries like `who_uses` / `who_imports`
without a qualified name. This skill turns a string, a short name, a file
path, or a vague description into that qualified name, then promotes the
result into structural queries.

For deep analysis of a symbol once you have its qualified name, hand off to
`rmc-symbol-forensics`. For "I want to find duplicates / similar code", use
`rmc-semantic-overlaps`.

## Scope — single lookup (no scope variants)

## Prerequisites

```
build_hypergraph(directory=<absolute-path>)
index_codebase(directory=<absolute-path>)   # only if using search / get_similar_code
```

## Workflow

### Step 1. Pick the entry-point tool by what you have

| Starting point | Tool |
|---|---|
| Free-text fragment, log message, doc string | `search(keyword=<string>)` |
| A symbol name, no path | `find_definition(symbol_name=<short_name>)` |
| Vague description ("function that parses JSON") | `get_similar_code(query=<description>)` |
| File path, no symbol yet | `read_file_content(file_path=...)` |
| Crate name + general area | `module_tree(directory=..., krate=<crate>)` |

### Step 2. Confirm the location

`search` and `get_similar_code` may return multiple candidates. Use
`find_definition` on the most promising name to get a clean `file:line` for
each candidate:

```
find_definition(symbol_name=<candidate>)        → file:line list
read_file_content(file_path=<file>)             → confirm declaration
```

### Step 3. Derive the qualified name

The qualified name is `<crate>::<module_path>::<item_name>`. From the file
path, derive the module path:

- `crates/<crate>/src/<a>/<b>/foo.rs` → `<crate>::a::b::foo`
- `crates/<crate>/src/<a>/<b>/mod.rs` → `<crate>::a::b`
- `crates/<crate>/src/lib.rs`         → `<crate>` (root)

For inner items (methods, impl items), append `::<TypeName>::<method>`.

Confirm by walking `module_tree` for that crate to sufficient depth and
finding the item:

```
module_tree(directory=..., krate=<crate>, depth=4)
```

Match `display_name + parent_path` against the derived qualified name. If
`module_tree` shows it, the qualified name is correct.

### Step 4. Promote to structural queries

Once you have the qualified name, drop the RA-driven tools and use the
hypergraph:

```
who_imports(directory=..., target=<qualified_name>)
who_uses_summary(directory=..., target=<qualified_name>)
```

At this point hand off to `rmc-symbol-forensics` for the full deep-dive.

## Decision frames

| Situation | Tool to start with |
|---|---|
| You're sure the symbol exists, just need the path | `find_definition` |
| You don't know if anything matches | `search` |
| You want concept-level matches (rename-tolerant) | `get_similar_code` |
| You're poking at an unfamiliar crate | `module_tree` then walk |
| You have a file but no symbol | `read_file_content` first |

## Pattern reference

| If you see... | Means |
|---|---|
| `find_definition` returns multiple hits | Same name in multiple crates — likely a `cross_crate_type_collision` (see `rmc-type-overlaps`) |
| `find_definition` returns nothing but `search` finds string hits | Symbol may be macro-generated or named differently than the search query |
| `get_similar_code` returns clusters of similar fns | Possible refactor candidate (see `rmc-semantic-overlaps`) |
| `search` returns hits only in comments / docs | The symbol name lives only in prose — not a code identifier |
| Multiple candidates and you can't tell them apart | Use `who_uses_summary` on each — the right one will have non-empty fan-in matching your context |

## Output

Return the qualified name plus a one-line confirmation of how it was
resolved (which tool found it, which file confirmed it). If multiple
qualified names match, list them and recommend the one with the highest
fan-in from `who_uses_summary` unless the user's context disambiguates.

## Limitations

- `find_definition` is RA-driven — works on the in-memory analysis index;
  may return stale results if sources changed after the last RA load.
- `search` (BM25) catches comments and strings, not just identifiers — high
  recall, lower precision. Confirm with `find_definition`.
- `get_similar_code` is vector-based and chunk-level; returns approximate
  locations, not exact item declarations. Always confirm with `find_definition`.
- Qualified-name derivation assumes a standard `crates/<crate>/src/...` layout.
  Non-standard layouts require reading `Cargo.toml` to map crate name to path.

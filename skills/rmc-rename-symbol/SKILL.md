---
name: rmc-rename-symbol
description: Preview a Rust symbol rename — exact reference set & refactor probe.
argument-hint: "<symbol-name> [new-name]"
allowed-tools: Read, mcp__rust-code-mcp__*
---

# Rust symbol rename — preview & probe

`rename_symbol` is rust-analyzer's rename engine run dry. It returns the exact
set of byte-precise edits and file moves that *would* be applied if you
committed the rename. Nothing is written to disk.

That makes it useful for more than renaming. It is the highest-precision
single-symbol reference inventory rust-analyzer can produce, plus a safety
probe for whether a given refactor is even legal.

For "what is this symbol's qualified name?" → `rmc-find-symbol`. For broader
fan-in (importers, consumer crates, Test/Other breakdown) → `rmc-symbol-forensics`.
For composing this into a refactor decision → `rmc-refactor-plan`.

## Scope — single symbol, dry-run (no files modified)

## Prerequisites

```
# No hypergraph required. rust-analyzer must load the project successfully.
# `directory` must be the Cargo project root (or workspace root with the
# crate in scope).
```

## Tool behavior

```
rename_symbol(symbol_name=<exact_short_name>,
              new_name=<identifier>,
              directory=<absolute_path>,
              file_path=<optional_path>,
              line=<optional_1_based_line>,
              column=<optional_1_based_column>)
```

- Resolves the symbol by **exact** name match. Ambiguous names fail with a
  candidate list. Rerun with that candidate's `file_path`, `line`, and
  `column` to disambiguate.
- `file_path`, `line`, and `column` are optional but must be provided together.
  When present, the tool bypasses name search and asks rust-analyzer to rename
  the symbol at that concrete position.
- Returns:
  - `edits`: a list of `file:start_line:start_col-end_line:end_col → "new_text"`
    for every textual reference rust-analyzer would touch.
  - `file_moves`: any module file moves required (e.g. renaming `mod parser`
    may move `parser.rs` or `parser/mod.rs`).
- May refuse the rename: keywords, foreign-crate items with no sources,
  identifier conflicts, macro-defined names. The refusal reason is returned.

## Use cases (beyond actually renaming)

### Use 1 — Full exact-reference inventory (rename-to-self)

You want every place where `Foo` is referenced, including:

- Method call sites
- Trait impl headers (`impl Foo for ...`)
- Pattern destructuring sites
- `use` path components
- Doc-link references (`[Foo]`)
- Macro-expanded refs RA can trace

Run the rename with `new_name = symbol_name`. The edit set is the full
reference inventory at byte precision.

```
rename_symbol(symbol_name=Foo, new_name=Foo, directory=...)
```

This is *stricter* than `who_uses` (hypergraph-driven, structural) and
*narrower* than `find_references` (which can include comment/doc-string
matches). When the three disagree, the difference is the signal:

- `rename_symbol` has refs `who_uses` doesn't → those are macro-introduced.
- `find_references` has refs `rename_symbol` doesn't → those are comments / docs.

### Use 2 — Refactor legality probe

Before you commit to a rename:

```
rename_symbol(symbol_name=Foo, new_name=Foo_PROBE_NAME, directory=...)
```

If rust-analyzer refuses, the reason tells you whether the rename is even
possible. Common refusals:

- Symbol crosses into a foreign / no-source crate boundary.
- Conflicts with an existing symbol in scope.
- Symbol is a keyword or non-identifier token.
- Symbol is macro-defined; RA can't rewrite the macro output.

### Use 3 — Cross-crate blast-radius

Group `edits` by their crate path prefix (`crates/<x>/...`).

- All edits inside the defining crate → safe internal refactor.
- Edits across multiple crates → public-API surface change. Coordinate with
  consumers; consider a SemVer bump.

This is faster than running `who_imports` + `who_uses_summary` when you just
need a yes/no on "does this leak outside the crate?".

### Use 4 — Verify "dead" symbols before deletion

When `who_uses` + `find_references` look empty but you're unsure:

```
rename_symbol(symbol_name=Foo, new_name=Foo_dead_check, directory=...)
```

If the only edit is the definition site itself, the symbol really is
unreferenced. If RA returns more sites, you've caught references the other
tools missed — usually macro-introduced.

### Use 5 — Module rename: file-move preview

For module-level symbols, `file_moves` reveals the required filesystem
reorganization. Run the rename and inspect `file_moves` before doing anything
manually:

```
rename_symbol(symbol_name=parser, new_name=lexer, directory=...)
→ file_moves: [src/parser.rs → src/lexer.rs]
```

### Use 6 — Trait method dispatch tracking

Renaming a trait method gives you, in one edit set:

- Every `impl Trait for T { fn method... }` site
- Every call site that dispatches through the trait (including `dyn T` sites
  RA can resolve)

This is the only built-in way to enumerate trait dispatch points without
walking impls by hand.

### Use 7 — Pre/post refactor reference snapshot

Save the rename-to-self edit set as JSON before a refactor, re-run after,
diff:

- New entries = new references introduced.
- Missing entries = references removed.

Useful for verifying that a refactor touched only what was intended.

## Decision frames

| Result | Interpretation |
|---|---|
| `Ambiguous symbol` error with candidate list | Rerun with the candidate's `file_path`, `line`, and `column`. |
| `rust-analyzer rename refused: <reason>` | Refactor as proposed is illegal. Read the reason; the right move is often to rename a wrapper, not the inner item. |
| Edits = 1 (definition site only) | Symbol unreferenced → safe-to-delete candidate. Confirm with `who_uses` and `find_references`. |
| Edits all inside one crate | Internal refactor — go. |
| Edits span 2+ crates | Public API change — coordinate consumers / SemVer. |
| `file_moves` non-empty | Module rename — plan the filesystem reorganization. |
| Edit count >> `who_uses` count | Macro-expanded refs in the gap. Trust `rename_symbol`. |

## Pattern reference

| Signal | Means |
|---|---|
| Edits live entirely in `tests/` or under `#[cfg(test)]` | Test-only symbol; demote visibility or gate with `#[cfg(test)]`. |
| RA refuses with a "macro" reason | Symbol is generated; rename the macro / its input, not the expansion. |
| Edits cluster in one foreign crate | That crate is the primary consumer; consider co-locating the symbol there. |
| `rename_symbol` succeeds with 0 edits | Symbol exists but is structurally unreachable (e.g. behind a disabled `cfg`). |

## Output

When invoked, return:

```
Symbol: <name>
Resolution: <exact @ file:line | ambiguous: N candidates>
Edits: <n> across <m> files in <k> crates
File moves: <n>
Refactor legality: <ok | refused: <reason>>
```

If the user wants the rename applied, also dump:

```
Text edits:
  <file>:<l>:<c>-<l>:<c> → "<new_text>"
  ...

File system changes:
  <from> → (anchor: <a>) <to>
  ...
```

If invoked for use cases 1–7, frame the output around the question asked
(e.g. for Use 4, lead with "X is dead: only the definition site appears in
the rename edit set").

## Limitations

- Name-only symbol resolution is **exact short-name** match. Up to 50 fuzzy
  candidates are listed on bail. For real disambiguation, rerun with
  `file_path`, `line`, and `column`.
- Read-only by design — caller applies edits themselves.
- RA's reference detection covers macro-expanded refs it can resolve, but
  some `proc_macro`-introduced names are invisible.
- Renames that cross into foreign-crate sources are refused.
- Doc-string / comment occurrences of the name are *not* included
  (`find_references` covers those instead).

# Algorithm Plan

## Goal

Build a workspace-internal dependency-surface tool for Rust that answers queries about:

- `import`
- `export`
- `reexport`
- `usage`

at:

- symbol level
- module level
- crate level

This plan is for the current repo and assumes:

- the workspace is already loaded through the existing semantic layer
- `no_deps=true`
- workspace-local analysis only
- macros, unresolved imports, and block-local `use` statements are out of scope for v1

## Core Decision

Use three different rust-analyzer layers for three different relations:

- `imports` come from `hir_def::ItemScope`
- `exports` / `reexports` come from the same bindings plus visibility
- `usage` comes from definition-driven reference search (`Definition::usages`)

Do not try to derive all three from a single graph.

## Integration and Snapshot Lifecycle

The current public semantic API in this repo only exposes name search and name-based references.
This algorithm needs lower-level access to:

- `AnalysisHost`
- `RootDatabase`
- `Vfs`
- `Semantics`
- `crate_def_map`

So the new semantic module must use internal access to the loaded project snapshot, not the current name-based helper surface.

For correctness in v1:

- module/binding/export indexes are built per tool invocation
- usage caches are per tool invocation
- no long-lived dependency-surface cache survives across edits

Reason:

- the current semantic project cache is sticky and does not define edit invalidation semantics
- reusing graph/index state across edits would risk stale answers
- fresh semantic load is cheap enough for v1

## Definitions

### Binding

A binding is one visible name in one module scope.

```rust
struct Binding {
    from_module: ModuleId,
    from_crate: Crate,
    ns: Namespace, // Types | Values
    name: Name,    // visible name in this module
    target: ModuleDefId,
    vis: Visibility,
    provenance: Option<ImportOrExternCrate>,
    kind: BindingKind,
}
```

```rust
enum BindingKind {
    Declared,
    NamedImport,
    GlobImport,
    ExternCrateImport,
}
```

Notes:

- `provenance` must be kept, not just `kind`
- `ValuesItem` uses `ImportOrGlob`, while `TypesItem` uses `ImportOrExternCrate`
- normalize values into the common binding shape when collecting
- skip macros in v1

### Import Edge

An import edge is any binding with `kind != Declared`.

```rust
struct ImportEdge {
    binding_id: BindingId,
}
```

### Export Edge

A binding in module `M` is exported to consumer module `C` only if:

1. the module path to `M` is reachable from `C`
2. the binding itself is visible from `C`

```rust
struct ExportEdge {
    binding_id: BindingId,
    consumer_module: ModuleId,
    consumer_crate: Crate,
    kind: ExportKind,
}
```

```rust
enum ExportKind {
    Direct,
    Reexport,
}
```

Rules:

- `Declared` + visible => `Direct`
- imported + visible => `Reexport`

Notes:

- `consumer_crate` is denormalized from `consumer_module.krate(db)` for query speed
- export visibility is not just `binding.vis`; ancestor module reachability must also hold

### Usage Edge

Usage is not inferred from scope.

```rust
struct UsageSite {
    file_id: EditionedFileId,
    range: TextRange,
}

struct UsageEdge {
    target: Definition,
    consumer_module: ModuleId,
    consumer_crate: Crate,
    sites: Vec<UsageSite>,
}
```

Notes:

- one `UsageEdge` is aggregated per `(target, consumer_module)`
- `sites.len()` is the reference count for that consumer
- keep the full site list in the cache shape so later UI queries can show files/ranges without a cache redesign

## Algorithm

### Phase 1: Build the workspace module index

For each workspace crate:

1. get `crate_def_map(db, crate)`
2. iterate `def_map.modules()`
3. record:
   - module id
   - owning crate
   - module source file
   - child modules

Also build:

- `modules_by_crate: Map<Crate, Vec<ModuleId>>`
- `module_info: Map<ModuleId, ModuleInfo>`
- `reachable_consumers_by_module: Map<ModuleId, ConsumerSet>`

`reachable_consumers_by_module[M]` means workspace modules that can reach the module path to `M`.
This must account for ancestor-module visibility, not just the binding visibility on items inside `M`.

Compute it recursively:

- start from the parent module's reachable consumer set
- intersect with the consumer set allowed by `M`'s own module visibility
- root modules start from the query's candidate workspace module set

## Phase 2: Build the binding table

For each module scope:

1. iterate `scope.types()`
2. iterate `scope.values()`
3. convert each item into a canonical `Binding`

Collection rules:

- if `item.import == None`, binding kind is `Declared`
- if type namespace import is `Import(_)`, binding kind is `NamedImport`
- if type namespace import is `Glob(_)`, binding kind is `GlobImport`
- if type namespace import is `ExternCrate(_)`, binding kind is `ExternCrateImport`
- if value namespace import is `Import(_)`, binding kind is `NamedImport`
- if value namespace import is `Glob(_)`, binding kind is `GlobImport`
- if the collected target is `ModuleDefId::MacroId(_)`, skip it in v1

Do not deduplicate by just `(from_module, target)`.

Use a key like:

```rust
(from_module, ns, name, target, provenance_kind)
```

because aliases and namespace splits matter.

Build indexes:

- `bindings_by_module: Map<ModuleId, Vec<BindingId>>`
- `bindings_by_crate: Map<Crate, Vec<BindingId>>`
- `bindings_by_target: Map<ModuleDefId, Vec<BindingId>>`
- `bindings_by_target_module: Map<ModuleId, Vec<BindingId>>`
- `bindings_by_target_crate: Map<Crate, Vec<BindingId>>`
- `declared_by_module: Map<ModuleId, Vec<BindingId>>`
- `imports_by_module: Map<ModuleId, Vec<BindingId>>`
- `bindings_by_use_id: Map<UseId, Vec<BindingId>>`

Where:

- `declared_by_module` contains only `kind == Declared`
- `imports_by_module` contains only `kind != Declared`

### Phase 3: Import queries

Imports are direct lookups over `imports_by_module`.

For a module `M`, its imports are:

```rust
imports_by_module[M]
```

Rollups:

- module level: keep module edges
- crate level: group by `target.module(db)?.krate(db)`
- symbol level: group by `target`

### Phase 4: Export / reexport queries

Exports are visibility queries over bindings.

For each binding in module `M`:

1. determine the candidate consumer modules
2. require `consumer ∈ reachable_consumers_by_module[M]`
3. require `binding.vis.is_visible_from(db, consumer)`
4. classify:
   - `Declared` => direct export
   - imported => reexport

For v1, use full visibility semantics:

- `Visibility::Public`
- `Visibility::PubCrate`
- `Visibility::Module`

Do not collapse this to “public API only”.
Keep raw visibility on the result so callers can filter later.

Important:

- export/reexport results are not globally materialized as `(binding, consumer)` rows
- they are generated on demand from bindings plus consumer buckets
- this avoids quadratic memory growth for large workspaces with many `pub` bindings

### Phase 4a: Consumer bucketing optimization

Avoid:

```rust
bindings_of(M) * all_candidate_modules * is_visible_from(...)
```

Instead pre-bucket consumers:

- `Public` => all workspace modules
- `PubCrate(k)` => `modules_by_crate[k]`
- `Module(m, _)` => non-block descendants / reachable visibility bucket for `m`

So export enumeration becomes:

1. inspect binding visibility
2. fetch the correct consumer bucket
3. yield export/reexport results for those consumers

This is an optimization, not a semantic change.

Implementation note:

- an internal sentinel such as `AllWorkspaceModules` is acceptable for `Visibility::Public`
- externally, queries still receive normal expanded results

### Phase 5: Usage queries

Usage is computed lazily only when explicitly requested.

Canonical source:

- use `Definition::usages(sema)`
- do not treat `find_all_refs` as equivalent
- do not opt into self/declaration-like extras such as `include_self_refs()` in v1

For each selected target definition:

1. convert target to `Definition`
2. run `Definition::usages(sema).all()`
3. for each returned `FileReference`:
   - take its exact syntax/range
   - resolve the containing module from the reference node/range, e.g. `reference.name.syntax()` plus `sema.scope(...)` / `scope_at_offset(...)`
   - do not attribute by `file -> owning modules` alone
4. aggregate one `UsageEdge` per `(target, consumer_module)`
5. append `UsageSite { file_id, range }` for each hit

Cache:

- `usage_by_target: Map<Definition, Vec<UsageEdge>>`

Do not precompute usage for the entire workspace.

## Query Semantics

These must stay distinct.

### Module A -> rest of modules

This is three different queries:

1. `imports`
   - outgoing imported bindings from `A`
2. `exports` / `reexports`
   - bindings in `A` that are visible from other modules
3. `usage`
   - consumers of definitions owned/exported by `A`

Do not merge these into one undifferentiated result set.

### Crate X -> module Y

- imports:
  - bindings from modules in `X` whose targets are in `Y`
- exports:
  - bindings from modules in `X` visible from `Y`
- usage:
  - usage edges whose consumer module is `Y`

### Crate X -> crate Z

Same as above, rolled up by crate.

### Crate X -> all crates

- imports:
  - group `bindings_by_crate[X]` by target crate
- exports:
  - group visible bindings from `X` by consumer crate
- usage:
  - group cached usage edges by consumer crate

### Module A -> all crates

Same as above starting from `bindings_by_module[A]`.

### All crates -> modules {A, B, C}

- imports:
  - reverse filter by `target.module(db)` in `{A, B, C}`
- exports:
  - reverse filter by `consumer_module` in `{A, B, C}`
- usage:
  - reverse filter by `consumer_module` in `{A, B, C}`

## v1 Exclusions

Deliberately out of scope:

- macros in the binding/import/export graph
- unresolved imports
- block-local `use` statements from block `DefMap`s
- proc-macro and build-script expansion
- external dependency traversal beyond whatever target identity is already present in the loaded DB

## Practical Notes

- External imports may still resolve as targets even with `no_deps=true`, but those targets are not meant to be traversed as first-class producer scopes in v1.
- `Module::declarations()` is still useful for owned-definition views, but not sufficient for reexports.
- Keep provenance now so future features can map a binding back to its originating `UseId` / `UseTree`.
- `ItemScope::fully_resolve_import(...)` is not required for v1. Bindings keep the immediate target. If a later feature needs the origin behind chained reexports, resolve it from the kept provenance.
- usage attribution must be range-level, not file-level, because inline/nested modules can share one file.

## Summary

The final v1 algorithm is:

1. collect modules from `DefMap`
2. collect bindings from `ItemScope.types()` and `ItemScope.values()`
3. classify imported vs declared bindings
4. answer imports directly from imported bindings
5. answer exports/reexports from bindings plus visibility
6. answer usage lazily from reference search

That is the implementation baseline.

# Hypergraph Storage Schema

## Executive Summary

This document defines the persisted semantic hypergraph for workspace-local Rust
analysis in `rust-code-mcp`.

The graph is built once from a Cargo workspace loaded with `no_deps = true`,
stored as an immutable snapshot in a `heed` environment, and then reused by MCP
tools. The first tool on top of this storage is `build_hypergraph`. Follow-on
tools should read the stored graph instead of rebuilding semantic state for each
query.

This is not a serialized in-memory prototype graph. It is a persisted graph
store with:

- stable snapshot identity
- deterministic node and relation IDs
- typed secondary indexes
- immutable snapshots
- external stub nodes for non-local dependencies
- enough import/export provenance to support future tools

## Goals

- Build a workspace-wide semantic graph once and query it many times.
- Use `heed` as the primary store.
- Load Rust workspaces with `no_deps = true`.
- Support import, export, and re-export analysis without re-running expensive
  semantic extraction.
- Keep the graph reusable for future tools such as call graph, type graph,
  ownership graph, and dependency summaries.
- Preserve enough provenance to explain graph answers with file locations and
  binding details.

## Non-Goals

- Full dependency graph extraction for third-party crates.
- Lossless persistence of rust-analyzer internal IDs.
- In-place mutation of an existing snapshot.
- Fine-grained incremental graph updates in v1.
- SQL as the primary store.

## Terminology

- `workspace`: the Cargo workspace root resolved from the user-provided
  directory.
- `snapshot`: an immutable persisted graph build for one workspace fingerprint.
- `workspace_hash`: stable hash of the canonical workspace root path.
- `fingerprint`: stable hash of the Rust inputs that define the graph.
- `graph_id`: stable identifier for one snapshot.
- `local`: defined inside the loaded workspace.
- `external`: referenced from the workspace but not defined locally.

## Tool Surface

### `build_hypergraph`

Build or reuse the current snapshot for a workspace.

Input:

- `directory`
- `force_rebuild` optional

Output:

- `graph_id`
- `workspace_root`
- `fingerprint`
- `schema_version`
- `node_count`
- `relation_count`
- `file_count`
- `reused` boolean
- `snapshot_path`

### Initial read tools

These tools should accept either `graph_id` or `directory`. If `graph_id` is
omitted, they resolve the current snapshot for the workspace.

- `graph_info`
- `get_imports`
- `get_exports`
- `get_reexports`
- `traverse_hypergraph`

Future tools should use the same snapshot store rather than bypassing it.

## Snapshot Model

Snapshots are immutable. A build produces a new snapshot directory and then
atomically updates the workspace's current pointer.

### Filesystem layout

```text
<data_dir>/graphs/<workspace_hash>/
  CURRENT
  snapshots/
    <graph_id>/
      data.mdb
      lock.mdb
      manifest.json
```

### `CURRENT`

`CURRENT` is a small text file containing the active `graph_id`. It is updated
with atomic rename semantics after a successful build.

### `manifest.json`

`manifest.json` is a debug and operational artifact, not the primary store. It
contains:

- `graph_id`
- `workspace_root`
- `workspace_hash`
- `fingerprint`
- `schema_version`
- `created_at`
- `node_count`
- `relation_count`
- `file_count`

## Workspace Fingerprint

The snapshot fingerprint must change whenever the semantic graph should change.

Inputs:

- `Cargo.toml`
- `Cargo.lock` when present
- all `*.rs` files inside the workspace
- file metadata needed to distinguish renames and deletions

Excluded paths:

- `target/`
- `.git/`
- generated storage directories owned by this tool

Implementation note:

- The existing Merkle-based change detection logic in this repository is a good
  base to reuse for fingerprinting and rebuild decisions.

## Stable Identifier Strategy

Rust-analyzer IDs are process-local and snapshot-local. They must never be
persisted as the public graph identity.

All persisted IDs are deterministic 128-bit hashes encoded as fixed-width big
endian bytes for ordered scans.

### ID types

- `FileId`
- `NodeId`
- `RelationId`

### Hash inputs

All hash inputs use normalized UTF-8 text joined by `\n`.

Normalization rules:

- canonical workspace root path
- workspace-relative file paths with `/`
- crate names normalized exactly as resolved from Cargo metadata
- module paths joined with `::`
- visibility strings preserved exactly
- source ranges encoded as byte offsets

### `FileId`

Hash of:

- `workspace_hash`
- workspace-relative file path

### `NodeId`

Hash of:

- `workspace_hash`
- node kind
- owning crate identity
- semantic path
- source anchor when needed

Examples:

- crate node: manifest path + target kind + root file
- module node: crate identity + module path + defining file
- item node: crate identity + module path + namespace + item kind + item name +
  defining file + start byte + end byte
- external symbol node: external path string + namespace

### `RelationId`

Hash of:

- `workspace_hash`
- relation kind
- ordered endpoint descriptors
- binding name when applicable
- source file + source range when applicable

## Graph Shape

The persisted model is a hypergraph, not only a binary edge list.

Internally the store has:

- nodes
- relations
- relation endpoints
- binary adjacency projections for common traversals

This keeps n-ary relation support available without making routine queries slow.

## Node Schema

### Node kinds

- `workspace`
- `crate`
- `module`
- `item`
- `external_symbol`
- `unresolved_symbol`

### Item kinds

- `function`
- `struct`
- `enum`
- `trait`
- `type_alias`
- `const`
- `static`
- `macro`
- `enum_variant`
- `field`
- `assoc_function`
- `assoc_const`
- `assoc_type`

### Namespace kinds

- `type`
- `value`
- `macro`

### `NodeRecord`

```text
NodeRecord {
  id: NodeId,
  kind: NodeKind,
  display_name: String,
  qualified_name: String,
  crate_id: Option<NodeId>,
  module_id: Option<NodeId>,
  file_id: Option<FileId>,
  visibility: Option<String>,
  span: Option<TextSpan>,
  attrs: NodeAttrs,
}
```

### `TextSpan`

```text
TextSpan {
  start_byte: u32,
  end_byte: u32,
  start_line: u32,
  start_col: u32,
  end_line: u32,
  end_col: u32,
}
```

### `NodeAttrs`

```text
WorkspaceAttrs {
  canonical_root: String,
}

CrateAttrs {
  crate_name: String,
  manifest_path: String,
  root_file_id: FileId,
  target_kind: CrateTargetKind,
}

ModuleAttrs {
  module_path: String,
  root_file_id: FileId,
  is_inline: bool,
}

ItemAttrs {
  item_kind: ItemKind,
  namespace: NamespaceKind,
  signature: Option<String>,
}

ExternalSymbolAttrs {
  path: String,
  namespace: NamespaceKind,
  external_crate: Option<String>,
}

UnresolvedSymbolAttrs {
  path: String,
  namespace: NamespaceKind,
  reason: String,
}
```

## Relation Schema

### Relation kinds

Structural:

- `contains`

Import and export:

- `imports`
- `reexports`

Type and definition:

- `field_type`
- `variant_field_type`
- `param_type`
- `return_type`
- `type_alias_target`
- `const_type`
- `static_type`
- `implements_trait`
- `assoc_item`
- `supertrait`
- `inherent_method`
- `trait_impl_method`

Behavioral:

- `calls`

### Endpoint roles

- `container`
- `member`
- `scope`
- `target`
- `owner_type`
- `owner_trait`
- `method`
- `caller`
- `callee`

### `RelationRecord`

```text
RelationRecord {
  id: RelationId,
  kind: RelationKind,
  primary_file_id: Option<FileId>,
  primary_span: Option<TextSpan>,
  attrs: RelationAttrs,
}
```

### `RelationEndpointRecord`

```text
RelationEndpointRecord {
  relation_id: RelationId,
  role: EndpointRole,
  ordinal: u16,
  node_id: NodeId,
}
```

### `RelationAttrs`

Binary relations keep payload minimal. Import and re-export relations carry more
provenance.

```text
ImportAttrs {
  namespace: NamespaceKind,
  source_path: String,
  visible_name: String,
  original_name: Option<String>,
  alias: Option<String>,
  is_glob: bool,
  visibility: String,
  resolution: ResolutionKind,
}

CallAttrs {
  dispatch: DispatchKind,
}
```

`ResolutionKind`:

- `local`
- `external_stub`
- `unresolved`

`DispatchKind`:

- `direct`
- `method`

## Import and Export Semantics

This is the part that must be right up front.

### Imports

An `imports` relation means:

- a `module` node introduces a name into scope
- the relation records the visible binding name
- the relation target is either a local node, an external symbol stub, or an
  unresolved symbol stub

Required endpoints:

- `scope` -> module node
- `target` -> local or stub symbol node

Important rule:

- imports are recorded even when the target is not local

### Re-exports

A `reexports` relation means:

- a module publicly exposes a target through a binding site
- the visible exported name may differ from the original name

Required endpoints:

- `scope` -> module node
- `target` -> local or stub symbol node

Important rule:

- `pub use foo::bar as baz` is one `reexports` relation with
  `visible_name = "baz"` and `original_name = Some("bar")`

### Why bindings are relation payload, not nodes

For v1, bindings are stored as rich relation payload and indexed by scope and
visible name. This keeps the graph smaller while still supporting:

- `get_imports(module)`
- `get_exports(module)`
- `get_reexports(module)`
- `who imports name X from crate Y`

If later work needs binding-level nodes, they can be added without breaking the
primary node identity model.

## External and Unresolved Symbols

Because the workspace is loaded with `no_deps = true`, not every reference can
resolve to a local item.

The graph must create explicit stub nodes for non-local targets:

- `external_symbol` for dependency-facing paths such as `serde::Serialize`
- `unresolved_symbol` when syntax exists but the target cannot be resolved into
  either a local node or a stable external path

This is required to avoid losing edges at the workspace boundary.

## File Schema

`FileRecord` deduplicates file metadata and keeps line-oriented answers cheap.

```text
FileRecord {
  id: FileId,
  workspace_relative_path: String,
  canonical_path: String,
}
```

## Heed Database Catalog

One `heed` environment per snapshot. Each named database is versioned by the
snapshot schema, not independently.

### Metadata

- `meta_by_key`
  - key: `String`
  - value: versioned metadata record

Required keys:

- `schema_version`
- `graph_id`
- `workspace_root`
- `workspace_hash`
- `fingerprint`
- `created_at`
- `node_count`
- `relation_count`
- `file_count`

### Primary records

- `files_by_id`
  - key: `FileId`
  - value: `FileRecord`
- `nodes_by_id`
  - key: `NodeId`
  - value: `NodeRecord`
- `relations_by_id`
  - key: `RelationId`
  - value: `RelationRecord`
- `relation_endpoints_by_relation`
  - key: `(RelationId, EndpointRole, Ordinal)`
  - value: `NodeId`

### Adjacency projections

- `out_pairs`
  - key: `(SourceNodeId, RelationKind, TargetNodeId, RelationId)`
  - value: unit
- `in_pairs`
  - key: `(TargetNodeId, RelationKind, SourceNodeId, RelationId)`
  - value: unit
- `relations_by_scope`
  - key: `(ScopeNodeId, RelationKind, RelationId)`
  - value: unit

### Lookup indexes

- `file_id_by_path`
  - key: workspace-relative path
  - value: `FileId`
- `node_ids_by_qualified_name`
  - key: qualified name
  - value: duplicate-sorted `NodeId`
- `node_ids_by_crate`
  - key: crate name
  - value: duplicate-sorted `NodeId`
- `node_ids_by_module`
  - key: `(crate name, module path)`
  - value: duplicate-sorted `NodeId`
- `node_ids_by_file`
  - key: `FileId`
  - value: duplicate-sorted `NodeId`

### Import and export indexes

- `imports_by_scope_and_name`
  - key: `(ScopeNodeId, NamespaceKind, VisibleName)`
  - value: duplicate-sorted `RelationId`
- `reexports_by_scope_and_name`
  - key: `(ScopeNodeId, NamespaceKind, VisibleName)`
  - value: duplicate-sorted `RelationId`
- `imports_by_target`
  - key: `(TargetNodeId, ScopeNodeId, RelationId)`
  - value: unit
- `reexports_by_target`
  - key: `(TargetNodeId, ScopeNodeId, RelationId)`
  - value: unit

## Encoding Strategy

### Keys

Keys should be compact and prefix-friendly.

- fixed-width IDs are stored as big-endian bytes
- enums are stored as compact integer tags
- composite keys are packed in lexical order of query priority

### Values

Values should use versioned `bincode` records.

Reasoning:

- compact enough for LMDB
- faster than JSON
- easy to evolve with explicit schema versioning
- readable enough through debug export tools

## Build Pipeline

### Phase 1: resolve workspace

- canonicalize input directory
- resolve Cargo workspace root
- compute `workspace_hash`

### Phase 2: fingerprint

- compute workspace fingerprint
- reuse current snapshot if fingerprint and schema version match and
  `force_rebuild` is false

### Phase 3: extract facts

- load rust-analyzer workspace with `no_deps = true`
- enumerate local crates
- enumerate modules and local items
- capture source spans and visibility
- extract imports and re-exports
- extract type-use relations
- extract trait and impl relations
- extract call relations
- create stub nodes for external and unresolved targets

### Phase 4: persist snapshot

- create new snapshot directory
- open `heed` environment with generous map size
- write files, nodes, relations, endpoints, and indexes
- validate invariants
- fsync and commit
- write `manifest.json`
- atomically update `CURRENT`

## Query Patterns

### `graph_info`

Read only `meta_by_key`.

### `get_imports`

Resolve module scope node, scan `imports_by_scope_and_name`, then hydrate
relations and target nodes.

### `get_exports`

Resolve crate or module scope, walk:

- public `contains` edges for declarations
- `reexports_by_scope_and_name` for explicit public forwarding

### `get_reexports`

Scan `reexports_by_scope_and_name`.

### `traverse_hypergraph`

Use `out_pairs` and `in_pairs` for routine traversal. If a relation needs full
n-ary context, load `relation_endpoints_by_relation`.

## Validation Rules

The builder must fail the snapshot if any invariant is broken.

- every `file_id` referenced by a node or relation exists
- every endpoint node exists
- every relation has the required roles for its kind
- every local item belongs to exactly one crate
- every module except the workspace root has exactly one structural parent
- every indexed `RelationId` and `NodeId` resolves
- every import or re-export relation has `visible_name`, `namespace`, and
  `resolution`

## Concurrency Model

- one writer per workspace snapshot build
- many concurrent read transactions
- snapshots are immutable once committed
- readers must never observe a partially-built snapshot

This aligns with LMDB and `heed` well.

## Snapshot Retention

Keep the current snapshot plus a small number of recent older snapshots.

Suggested policy:

- keep latest successful snapshot
- keep previous `N` snapshots, default `N = 2`
- delete stale snapshots opportunistically after a successful pointer swap

## Versioning and Migration

`schema_version` is global for the whole snapshot store.

Rules:

- a schema version change creates a new snapshot
- no in-place migration is required in v1
- readers must reject snapshots with unsupported `schema_version`

## Initial Implementation Boundaries

The first implementation should include:

- workspace, crate, module, and item nodes
- external and unresolved symbol stubs
- `contains`
- `imports`
- `reexports`
- `field_type`
- `variant_field_type`
- `param_type`
- `return_type`
- `type_alias_target`
- `const_type`
- `static_type`
- `implements_trait`
- `assoc_item`
- `supertrait`
- `inherent_method`
- `trait_impl_method`
- `calls`

The first implementation should not block on:

- docstrings
- macro expansion detail
- ownership and borrow edges
- full generic constraint modeling
- incremental graph mutation

## Open Design Constraint

The graph must remain a reusable storage substrate, not a single-tool cache.

That means:

- import/export analysis cannot use one-off relation shapes
- relation payloads must preserve provenance
- node identity must survive implementation refactors
- future tools should be able to read this store without redefining the schema

If a shortcut makes `get_imports` easy but makes later graph tools harder, it is
the wrong shortcut.

# Hypergraph v2 — Implementation Overview

A workspace-internal Rust hypergraph for import / export / reexport / who-imports queries. Built HIR-first, persisted in heed with hash-only keys, exposed via MCP.

This doc is a per-layer summary of what's in the code and why. For the why-not-v1, see the analysis at the top of the conversation thread that produced this work.

## Architecture

```
                ┌─────────────────────────────────────────────┐
                │          MCP tools (Layer 7)                │
                │  build_hypergraph, get_imports,             │
                │  get_exports, get_reexports, who_imports,   │
                │  who_uses, dead_pub_in_crate,               │
                │  dead_pub_report                            │
                └────────────────────┬────────────────────────┘
                                     │
                ┌────────────────────▼────────────────────────┐
                │       Read-path queries (Layer 6)           │
                │   imports_of, exports_of, reexports_of,     │
                │   who_imports, usages_of, usages_in,        │
                │   dead_pub_in_crate, dead_pub_report,       │
                │   lookup_by_qualified_name                  │
                └────────────────────┬────────────────────────┘
                                     │
                ┌────────────────────▼────────────────────────┐
                │   Snapshot lifecycle + write (Layers 4–5)   │
                │   GraphPaths · GraphDatabases ·             │
                │   build_and_persist · open_current          │
                └────────────────────┬────────────────────────┘
                                     │
                ┌────────────────────▼────────────────────────┐
                │   Extraction passes (Layers 2–3, 8)         │
                │   Workspace/Crate/Module nodes ·            │
                │   ItemScope-driven Bindings + Items +       │
                │   ExternalSymbol stubs · Usage records      │
                └────────────────────┬────────────────────────┘
                                     │
                ┌────────────────────▼────────────────────────┐
                │   Workspace loader (Layer 1)                │
                │   ra_ap_load_cargo · no_deps=true ·         │
                │   workspace-member filter                   │
                └─────────────────────────────────────────────┘
```

Source layout:

```
src/graph/
  loader.rs       L1   workspace loader
  ids.rs          L2   NodeId, BindingId (32-byte SHA-256)
  model.rs        L2   Node, Binding, ExtractionModel
  extract.rs      L2,3 orchestrator: workspace/crate/module nodes
  bindings.rs     L3   ItemScope-driven binding pass + items + stubs
  storage.rs      L4   GraphPaths, GraphDatabases, fingerprint
  snapshot.rs     L5   build_and_persist, open_current, lifecycle
  queries.rs      L6   imports_of / exports_of / reexports_of / who_imports /
                       usages_of / usages_in / dead_pub_in_crate / dead_pub_report
  usages.rs       L8   Definition::usages extraction + Item file/span backfill
  mod.rs               public re-exports

src/tools/
  graph_tools.rs  L7   MCP handlers
  search_tool.rs  L7   param structs (BuildHypergraphParams, …)
  search_tool_router.rs  L7  #[tool]-annotated routes
```

## Design decisions (made once, never revisited)

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | NodeId = SHA-256 of `(workspace_hash, kind_label, crate, module_path, item_kind, item_name)` — no span | Stable across edits that don't rename or move the symbol; survives RA reloads (unlike `ModuleDefId`). |
| 2 | "Local crate" = workspace member from `ProjectWorkspace::cargo().packages()` filtered by `package.is_member` | Cleaner boundary than "any file under workspace_root"; avoids surprises with path-deps living above root. |
| 3 | External symbols become stub `ExternalSymbol` nodes | Whole point of the analysis is cross-crate "who imports X"; ~doubles node count on dep-heavy crates, acceptable. |
| 4 | Usage edges are extracted eagerly via `Definition::usages` and persisted alongside bindings (Layer 8) | The algorithm-plan originally proposed lazy in-process caching; in practice eager extraction adds ~1.3 ms / item (seconds on this repo, scales linearly), and persistence makes `who_uses` and `dead_pub_*` constant-time at query time. |

## Layer 1 — Workspace loader (`src/graph/loader.rs`)

Thin wrapper over `ra_ap_load_cargo::load_workspace`.

```rust
pub struct LoadedWorkspace {
    pub workspace_root: PathBuf,
    pub db: RootDatabase,
    pub vfs: Vfs,
    pub local_crates: Vec<Crate>,
}

pub fn load(directory: &Path) -> Result<LoadedWorkspace>
```

Steps:
1. Canonicalize `directory`, discover `Cargo.toml` via `ProjectManifest::discover_single`.
2. Build `CargoConfig { sysroot: None, no_deps: true }`. With both flags set, `Crate::all(db)` returns only workspace members and built-ins; we then filter out built-ins and keep only crates whose root file matches a known member target's root.
3. `load_workspace(workspace, &Default::default(), &load_config)` — returns `(RootDatabase, Vfs, _proc_macro)`. We pin `ProcMacroServerChoice::None` and `prefill_caches: true`.

Test: `loads_self_workspace` confirms `file_search_mcp` appears in `local_crates` when loading this repo.

## Layer 2 — In-memory model + stable IDs (`src/graph/ids.rs`, `model.rs`, `extract.rs`)

### IDs (`ids.rs`)

```rust
pub struct NodeId(pub [u8; 32]);     // SHA-256
pub struct BindingId(pub [u8; 32]);  // SHA-256

impl NodeId    { pub fn from_components(parts: &[&str]) -> Self }
impl BindingId { pub fn from_components(parts: &[&str]) -> Self }
pub fn workspace_hash(workspace_root: &Path) -> String
```

Construction uses an explicit `0u8` separator between parts to prevent `["a","bc"]` and `["ab","c"]` from colliding. Verified by `separator_prevents_collision`.

### Model (`model.rs`)

```rust
pub enum NodeKind { Workspace, Crate, Module, Item, ExternalSymbol }
pub enum ItemKind { Function, Struct, Enum, Union, Trait, TypeAlias,
                    Const, Static, AssocFunction, AssocConst, AssocType }
pub enum Namespace { Type, Value }
pub enum BindingKind { Declared, NamedImport, GlobImport, ExternCrateImport }

pub enum BindingVisibility {
    Public,
    Crate(NodeId),         // pub(crate) — points at the crate node
    RestrictedTo(NodeId),  // pub(in path::to::M) — points at M
    Private,               // unresolvable / private
}

pub struct Node      { id, kind, display_name, qualified_name,
                       crate_id, parent_id, item_kind, file, span, visibility }
pub struct Binding   { from_module, namespace, visible_name, target,
                       kind, visibility }
pub struct ExtractionModel { workspace_root, workspace_hash, workspace_id,
                             nodes, bindings, contains }
```

`BindingVisibility` is the load-bearing decision: it carries enough structured information that Layer 6's export query (`is_visible_from`) is a constant-time check, no HIR re-walk required.

### Orchestration (`extract.rs`)

`extract(loaded: &LoadedWorkspace) -> ExtractionModel`:

1. Insert one `Workspace` node.
2. For each local crate: insert `Crate` node, walk its `crate_def_map(db, krate.base())`, and for every non-block module insert a `Module` node + `Contains` edge to its parent (root module's parent is the crate).
3. Compute parent NodeIds on-the-fly via the same hashing recipe instead of relying on iteration order, since `def_map.modules()` is not guaranteed parent-first.
4. Hand off `(crate_node_for, crate_name_for, module_node_for)` to Layer 3.

Note: we work entirely at `ra_ap_hir_def::ModuleId` level here, not `ra_ap_hir::Module`, because the latter's `id` field is `pub(crate)` — we cannot bridge `Module → ModuleId` from outside the crate.

## Layer 3 — Bindings pass (`src/graph/bindings.rs`)

The HIR-driven heart of the system. For each local module's `ItemScope`, iterate every type-namespace and value-namespace entry:

```rust
for (name, Item { def, vis, import }) in scope.types() { … }
for (name, Item { def, vis, import }) in scope.values() { … }
```

Per entry:

1. **Skip v1 exclusions**: macros, builtins, enum variants, trait aliases.
2. **Resolve target**: look up `def_id` in `def_to_node`; if absent, find owner module via `HasModule::module(db)`. If owner is local, lazily create an `Item` node (and emit `Contains` if this is the declaring module). If owner is non-local, create an `ExternalSymbol` stub node.
3. **Classify provenance** from `Option<ImportOrExternCrate>` / `Option<ImportOrGlob>` into `BindingKind { Declared | NamedImport | GlobImport | ExternCrateImport }`.
4. **Encode visibility** from `ra_ap_hir_def::visibility::Visibility`:
   - `Public` → `BindingVisibility::Public`.
   - `PubCrate(crate)` → `BindingVisibility::Crate(crate_node)` (crate hash matches Layer 2's recipe).
   - `Module(module_id, _)` → `BindingVisibility::RestrictedTo(module_node)` if local, else `Private`.
5. **Push the `Binding` record**.

Why `ItemScope` and not `module.declarations()` like the failed try? Because `ItemScope` is the *resolved* view: each entry is `(visible_name → ModuleDefId)` with provenance attached. There is no string matching, no re-export chain to walk, no glob to expand by hand. RA already did all that.

## Layer 4 — Persistence schema (`src/graph/storage.rs`)

### Filesystem layout

```
<data_dir>/graphs/<workspace_hash>/
  CURRENT                 ← text file with active graph_id (atomically renamed)
  snapshots/
    <graph_id>/
      data.mdb            ← heed env
      lock.mdb
      manifest.json       ← debug/operational metadata
```

`<data_dir>` defaults to `directories::ProjectDirs("dev","rust-code-mcp","search")/graphs`; tests override via `BuildOptions::data_dir_override`.

### heed schema

Nine sub-databases, all keyed by 32-byte hashes (or fixed short strings). The hash-only-keyed discipline is the load-bearing change vs v1's `Str`-keyed `node_ids_by_qualified_name` which crashed on burn with `MDB_BAD_VALSIZE`.

| DB | Key | Value | Flags | Purpose |
|----|-----|-------|-------|---------|
| `nodes_by_id` | NodeId (32B) | bincode `Node` | — | Primary node store. |
| `bindings_by_id` | BindingId (32B) | bincode `Binding` | — | Primary binding store; dedupes via hash. |
| `bindings_by_from_module` | NodeId (32B) | BindingId (32B) | DUP_SORT | Index for `imports_of` / `exports_of`. |
| `bindings_by_target` | NodeId (32B) | BindingId (32B) | DUP_SORT | Index for `who_imports`. |
| `usages_by_id` | UsageId (32B) | bincode `Usage` | — | Primary usage store (Layer 8). |
| `usages_by_target` | NodeId (32B) | UsageId (32B) | DUP_SORT | Index for `usages_of` / `who_uses`. |
| `usages_by_consumer` | NodeId (32B) | UsageId (32B) | DUP_SORT | Index for `usages_in` (what does this module reference?). |
| `children_by_parent` | NodeId (32B) | NodeId (32B) | DUP_SORT | Hierarchy traversal. |
| `meta_by_key` | `Str` (≤16B) | raw bytes | — | Manifest mirror inside heed (graph_id, fingerprint, etc.). |

`BindingId = SHA-256(from_module_hex, namespace, visible_name, target_hex)` — same name → same id when other fields match, so glob+named overlap dedupes naturally.

Schema version is `SCHEMA_VERSION: u32 = 3`. v1 was the binding-only shape; v2 added the three `usages_*` sub-databases; v3 keeps the same layout but the extraction now backfills `Node.file` / `Node.span` for local Items via `Definition::try_to_nav` (see Layer 8). `graph_id_for` hashes `SCHEMA_VERSION`, so v1/v2/v3 graph_ids are disjoint and old snapshots stop being reused (they remain on disk until manual cleanup). Manifest reads validate the version.

### Fingerprint (`compute_fingerprint`)

SHA-256 over the sorted list of `(workspace-relative path, file SHA)` for every `.rs`, `Cargo.toml`, and `Cargo.lock` under the workspace, excluding `target/` and `.git/`. `graph_id_for(workspace_hash, fingerprint)` is the 16-byte hex prefix of `SHA-256(workspace_hash || fingerprint || schema_version)`.

## Layer 5 — Write path + lifecycle (`src/graph/snapshot.rs`)

Public entry:

```rust
pub fn build_and_persist(directory: &Path, options: BuildOptions) -> Result<BuildResult>
pub fn open_current(paths: &GraphPaths, env: GraphEnvOptions) -> Result<Option<OpenedSnapshot>>
pub fn open_specific(paths: &GraphPaths, graph_id: &str, env: GraphEnvOptions)
        -> Result<Option<OpenedSnapshot>>
```

`build_and_persist` is an idempotent reuse path — if `manifest.json` already exists for the computed `graph_id`, it returns `reused = true` without re-running extraction.

Build flow (when not reusing):

1. `loader::load(directory)` → `LoadedWorkspace`.
2. `compute_fingerprint(&workspace_root)` → `fingerprint`.
3. `graph_id_for(workspace_hash, fingerprint)` → `graph_id`.
4. Wipe stale `<snapshot_dir>` if any, create fresh, open heed env.
5. `extract(&loaded)` → `ExtractionModel` (Layers 2–3, 8). The Layer 8 usages pass runs in the same call: it backfills `Node.file` / `Node.span` for local Items and pushes `Usage` records onto the model.
6. `write_model(&env, …, &model, …)` — **single transaction**:
   - `nodes_by_id.put` for every node.
   - For every binding: derive `BindingId`, write to `bindings_by_id`, `bindings_by_from_module`, `bindings_by_target`.
   - For every usage: derive `UsageId`, write to `usages_by_id`, `usages_by_target`, `usages_by_consumer`.
   - For every `(parent, child)` in `model.contains`: write to `children_by_parent`.
   - Write meta keys.
   - `wtxn.commit()`.
7. Write `manifest.json`.
8. **Atomic publish**: write `CURRENT.tmp`, then `fs::rename` it onto `CURRENT`.

`OpenedSnapshot::open_specific` has one subtlety worth a comment in the code:

```rust
let rtxn = env.read_txn()?;
let dbs = GraphDatabases::open(&env, &rtxn)?...;
rtxn.commit()?;   // ← NOT drop()
```

Per the heed `open_database` doc: when a read txn opens dbs, the dbi handles must be **committed** back to the env. Dropping leaves them half-registered and the next `iter()` returns `EINVAL`. This bit us mid-implementation — easy to forget.

### Acceptance gate

`examples/graph_burn.rs` builds against an arbitrary workspace and prints counts. Verified results:

| Workspace | Nodes | Bindings | Time | v1 result |
|-----------|------:|---------:|------|-----------|
| this repo (1 crate, 89 files) | ~1.3k | ~3.8k | <1s | crashed late |
| chart-indicators (4 crates, 253 files) | 1505 | 4286 | 3.4s | worked |
| **burn (~150 crates)** | **6736** | **32536** | **1.2s** | **MDB_BAD_VALSIZE** |

## Layer 6 — Read path / queries (`src/graph/queries.rs`)

Methods on `OpenedSnapshot`. Every query is a direct LMDB lookup — no graph traversal — because the indexes are designed to serve exactly these questions.

```rust
impl OpenedSnapshot {
    pub fn lookup_by_qualified_name(&self, name: &str) -> Result<Option<(NodeId, Node)>>;
    pub fn node_by_id(&self, rtxn, id: NodeId) -> Result<Option<Node>>;
    pub fn imports_of(&self, module: NodeId) -> Result<Vec<Binding>>;
    pub fn exports_of(&self, module: NodeId, consumer: NodeId) -> Result<Vec<Binding>>;
    pub fn reexports_of(&self, module: NodeId, consumer: NodeId) -> Result<Vec<Binding>>;
    pub fn who_imports(&self, target: NodeId) -> Result<Vec<Binding>>;
    pub fn usages_of(&self, target: NodeId) -> Result<Vec<Usage>>;
    pub fn usages_in(&self, consumer_module: NodeId) -> Result<Vec<Usage>>;
    pub fn dead_pub_in_crate(&self, crate_id: NodeId) -> Result<Vec<DeadPubFinding>>;
    pub fn dead_pub_report(&self) -> Result<Vec<CrateDeadPub>>;
}
```

- `imports_of` walks `bindings_by_from_module.get_duplicates(module)` and resolves each `BindingId` to a `Binding`, filtering out `Declared`.
- `exports_of` walks the same index and runs `is_visible_from(binding.visibility, consumer_crate, consumer_ancestry)` per binding. Ancestry is precomputed once via `module_ancestors` (walking `Node.parent_id` upward).
- `reexports_of` = `exports_of` filtered to `kind != Declared`.
- `who_imports` walks `bindings_by_target.get_duplicates(target)`.
- `usages_of` walks `usages_by_target.get_duplicates(target)` and resolves each `UsageId` via `usages_by_id`. One row per concrete reference site.
- `usages_in` walks `usages_by_consumer.get_duplicates(consumer_module)` — the inverse view, useful to ask "what does this module reference?".
- `dead_pub_in_crate` scans `nodes_by_id` for `pub` Items in the crate, then per-Item checks `bindings_by_target` for any cross-crate importer and `usages_by_target` for any cross-crate user. Items with neither are reported as candidates for `pub(crate)` downgrade. Skips `Private`, `pub(crate)`, and `pub(in path)` items (already minimal). Known false positive: items only reachable through public signatures aren't named directly in caller code, so won't appear in `usages_by_target`.
- `dead_pub_report` runs `dead_pub_in_crate` once per local crate and returns the aggregate, sorted by crate name.

Visibility check (from the structured enum):

```rust
fn is_visible_from(vis, consumer_crate, consumer_ancestry) -> bool {
    Public          => true,
    Private         => false,
    Crate(c)        => consumer_crate == Some(c),
    RestrictedTo(a) => consumer_ancestry.contains(&a),
}
```

`lookup_by_qualified_name` is an `O(n)` scan over `nodes_by_id`. Sub-millisecond at burn scale (6.7k nodes); a hash-keyed secondary index can be added if MCP latency complaints appear.

Tests cover each query, including a discrimination test (`private_visibility_blocks_export`) that proves the structured visibility encoding works end-to-end.

## Layer 7 — MCP tools (`src/tools/graph_tools.rs`, `search_tool.rs`, `search_tool_router.rs`)

Eight tools, all keeping the same shape: open snapshot → resolve qualified names to NodeIds → run query → enrich bindings/usages/findings with target/from-module qualified names → JSON.

| Tool | Params | Calls |
|------|--------|-------|
| `build_hypergraph` | `directory`, `force_rebuild?` | `build_and_persist` |
| `get_imports` | `directory`, `module` | `imports_of` |
| `get_exports` | `directory`, `module`, `consumer` | `exports_of` |
| `get_reexports` | `directory`, `module`, `consumer` | `reexports_of` |
| `who_imports` | `directory`, `target` | `who_imports` |
| `who_uses` | `directory`, `target` | `usages_of` |
| `dead_pub_in_crate` | `directory`, `crate` | `dead_pub_in_crate` |
| `dead_pub_report` | `directory` | `dead_pub_report` |

The MCP layer never exposes `NodeId` — clients work in qualified names in and out. Bindings come back enriched:

```json
{
  "module": "file_search_mcp::graph",
  "bindings": [
    {
      "visible_name": "load",
      "namespace": "Type",
      "kind": "NamedImport",
      "visibility": "pub",
      "from_module": "file_search_mcp::graph",
      "target": "file_search_mcp::graph::loader::load",
      "target_kind": "Item.Function"
    }
  ]
}
```

Visibility labels reconstruct the human-readable form: `pub`, `pub(crate=my_crate)`, `pub(in my::sub::module)`, `private`.

End-to-end test (`mcp_round_trip_against_self`) drives `build_hypergraph → get_imports → who_imports` against this very repo and asserts the JSON content.

## Test surface

| File | Tests | Asserts |
|------|------:|---------|
| `loader.rs` | 1 | `file_search_mcp` shows up as a local crate. |
| `ids.rs` | 2 | Determinism + separator-collision safety. |
| `extract.rs` | 2 | Workspace/crate/module nodes; declared items + re-export bindings. |
| `snapshot.rs` | 1 | Build → reopen → reuse path → readback matches. |
| `queries.rs` | 11 | All binding queries + `lookup_by_qualified_name`; private-vis discrimination; `usages_of` / `usages_in`; `dead_pub_in_crate` shape. |
| `graph_tools.rs` | 3 | MCP round-trip; `get_exports` accepting a crate name as consumer; `who_uses` + `dead_pub_in_crate` JSON shape. |

## Cross-workspace resolution — the RA-version saga

We initially shipped with `no_deps: true` and `sysroot: None` because RA
0.0.313 had a known bug: it always appended `cargo metadata --lockfile-path
<copy>` to avoid mutating `Cargo.lock`, but the cargo in the project's
nightly toolchain (1.97.0-nightly) doesn't accept `--lockfile-path` for the
`metadata` subcommand. RA caught the error and silently fell back to
`--no-deps`, throwing away the resolve graph. Net effect:
`burn_core.dependencies()` returned only 5 sysroot crates and `who_imports`
never crossed crate boundaries.

**Fixed by bumping ra_ap_* from 0.0.313 → 0.0.330.** Newer RA introduced a
`LockfileUsage` enum that picks the right mechanism by toolchain version:

- cargo `< 1.82.0` — no lockfile copy
- cargo `[1.82.0, 1.95.0-beta)` — `--lockfile-path` CLI flag (the broken path)
- cargo `>= 1.95.0-beta` — `CARGO_RESOLVER_LOCKFILE_PATH` env var

Our cargo (1.97.0-nightly) lands in the third bucket, so the env-var path
runs and works correctly.

**Result on burn:** before/after for the canonical `who_imports` query:

|  | Before bump | After bump |
|---|---|---|
| Snapshot nodes | 6,736 | **32,378** |
| Snapshot bindings | 32,536 | **378,552** |
| `who_imports(canonical Tensor)` | 31 (all in burn_tensor) | **2,405 across 30+ crates** |
| burn_core ItemScope cross-crate entries | 0 | 475 |
| burn_core ItemScope `Tensor` entries | 0 | 18 |

Loader config (`src/graph/loader.rs`):

```rust
CargoConfig {
    sysroot: Some(RustLibSource::Discover),  // sysroot loaded → core/alloc/std resolve
    no_deps: false,                          // full resolve graph
    features: CargoFeatures::All,            // all cfg(feature) gated modules visible
    all_targets: true,                       // lib + bin + tests + examples
    set_test: true,                          // cfg(test) on
    ..Default::default()
}
```

**Cost:** loading burn now takes ~30–60s per fingerprint (one-time, snapshot
cached). Loading this very repo for unit tests went from ~1s to ~15s
(integration-flavored — the loader test really does load std + every dep
into RA). If unit-test perf becomes an issue, the next refactor is to use a
small synthetic-fixture workspace for tests rather than self-loading.

## Layer 8 — Usage extraction (`src/graph/usages.rs`)

**Goal.** Capture every non-`use` reference between local items so queries like `who_uses` and `dead_pub_*` reflect actual code usage, not just import topology. Bindings tell you "module M brings name X into scope"; usages tell you "byte range R inside module M' actually mentions item X".

**Algorithm.** Four-step pipeline, run once per local Item after Layer 3:

1. For every `(ModuleDefId, NodeId)` in the `def_to_node` map whose target node is a local `Item`, convert the def to a `Definition` (Function / Adt / Trait / TypeAlias / Const / Static).
2. Call `Definition::usages(&Semantics::new(db)).all()`. RA returns one entry per file, with a list of `FileReference { range, category, … }`.
3. For each reference, drop those whose `category` contains `ReferenceCategory::IMPORT` — those are already modeled as `Binding`s with `kind != Declared`. Then attribute the site to its enclosing module via `sema.scope_at_offset(syntax, range.start()).module()` (necessary because multiple inline `mod` blocks can share one file). Refs in dep-crate files canonicalize outside `workspace_root` and are filtered out by `module_node_for.get(consumer_module_id)` returning `None`.
4. Emit one `Usage { target, consumer_module, file, start, end, category }` record per surviving reference.

**Data.** `Usage` and `UsageCategory { Read, Write, Test, Other }` live in `model.rs`. `UsageCategory` mirrors `ReferenceCategory` with `IMPORT` stripped (it's the filter, never an output) and ties broken in `Write > Read > Test > Other` order. Persistence: three new sub-databases — `usages_by_id` (primary, `UsageId → Usage`), `usages_by_target` (`NodeId → UsageId`, `DUP_SORT`, drives `who_uses`), `usages_by_consumer` (`NodeId → UsageId`, `DUP_SORT`, drives `usages_in`). Adding these bumped `SCHEMA_VERSION` from 1 to 2.

**Side effect — file/span backfill.** Before walking references for an Item, the same pass calls `def.try_to_nav(&sema)` and copies the canonical declaration's `file_id` / `full_range` onto `Node.file` / `Node.span`. Cheap (single call), and makes `dead_pub_report` findings navigable. Errors and macro-only definitions silently fall through. The backfill itself doesn't change the schema layout, but it changes the *contents* of every snapshot (Node.file/span go from always-`None` to populated for local Items), which is why `SCHEMA_VERSION` bumped a second time from 2 to 3 — so v2 snapshots stop being reused and users see the new fields without remembering to `--force`.

**Cost.** Spike harness `examples/spike_usages.rs` measured ~1.3 ms / item on the `coding-agent` workspace (1087 items, 5.2k refs, ~1.4 s total extraction). Cold extraction on this repo and on `../coding-agent` lands in the seconds range. The dominant cost is `Definition::usages` itself; in principle it's parallelizable across items, but RA's `Semantics` isn't `Send`-friendly enough to do that today, so the loop is serial.

**Limitations.**

- All five reference patterns are now CAPTURED — generic bounds, const reads, macro expansions, method calls (`Foo::bar()`), and trait dispatch (`x.method()`). The first three landed with Layer 8; method calls and trait dispatch landed with Layer 9 once impl-block items and trait declaration items started being emitted as `Item` nodes (see Layer 9 below). Phase A1's synthetic-fixture suite (`src/graph/usages.rs::tests`) asserts all five.
- Trait *impl* method bodies (`impl T for Foo { fn m() { ... } }`) are deliberately not emitted as separate `Item` nodes. RA's `Definition::usages` resolves both `x.m()` dispatch and `Foo::m()` qualified calls back to the trait declaration's def, so the trait Item alone covers `who_uses` for trait dispatch — adding impl-body items would emit duplicates that just shadow the trait declaration. Inherent impls are extracted (one Item per inherent impl method).
- No call graph yet. Usages tell you which *module* references a target, but not which function inside that module. Going from "consumer module" to "consumer function" needs a second resolution pass we haven't written.

## MCP query tools shipped on top of the persisted graph

Of the eight Layer 7 tools, four are pure read queries that exercise the full stack:

- **`who_imports(directory, target)`** — every binding whose target is `target`. Reverse import lookup. Backed by `bindings_by_target`.
- **`who_uses(directory, target)`** — every non-import reference site that mentions `target`, returned as `(file, byte range, consumer_module, category)`. Backed by `usages_by_target`.
- **`dead_pub_in_crate(directory, crate)`** — `pub` items in `crate` with no cross-crate importer and no cross-crate user. Candidates for `pub(crate)`. Backed by per-Item `bindings_by_target` + `usages_by_target` scans.
- **`dead_pub_report(directory)`** — `dead_pub_in_crate` run across every local crate, sorted by crate name.

## Layer 9 — Impl-block and trait-declaration item extraction (`src/graph/impls.rs`)

**Goal.** Capture method-level references in `who_uses` and `dead_pub` so calls like `Foo::bar()`, `x.method()`, and `Trait::method` resolve to a real `Item` node instead of returning empty. Without this layer, the bindings pass only walks module-level `ItemScope`, which never enters `impl` blocks or trait bodies — so methods don't exist as graph targets and `Definition::usages` is never asked about them.

**Algorithm.** Runs after `extract_bindings` and before `extract_usages` in `extract.rs`. Re-uses the `def_to_node` map that bindings returns, extending it with new entries:

1. Build `adt_node_for: HashMap<AdtId, NodeId>` and `trait_node_for: HashMap<TraitId, NodeId>` from `def_to_node` (filter to the relevant `ModuleDefId` variants). These are the graph nodes already emitted as Items by Layer 3.
2. For every local crate, iterate `Impl::all_in_crate(db, krate)`. Skip impls with a trait (`impl T for Foo`) — those are deferred (see "Limitations" below). For each *inherent* impl, look up the self-type's `AdtId` in `adt_node_for`; skip if not local. Walk `impl_.items(db) → Vec<AssocItem>` and emit one Item node per `Function | Const | TypeAlias` with `parent_id = adt_node`.
3. For every local trait declaration, walk `trait.items(db)` and emit Items with `parent_id = trait_node`.
4. For each emitted Item, resolve its `Definition` so `try_to_nav(&sema)` gives a (file_id, full_range). Translate file_id to a workspace-relative path via `Vfs::file_path` + `strip_prefix(workspace_root)` — same helper `usages.rs` uses for the v3 location backfill. The byte offset of `full_range.start()` is mixed into the NodeId scheme as a disambiguator (multiple inherent impls of the same name on the same type don't collide because their declaration sites differ).
5. Insert into `def_to_node` so `extract_usages` picks the new defs up automatically.

**Data.** Three new `ItemKind` variants in `model.rs`:

- `Method` — fn inside an inherent `impl Foo { ... }` block OR a trait declaration. Both share this variant; the distinction is encoded by `parent_id` pointing at a struct/enum/union Item vs a trait Item.
- `AssocConst` — const inside an inherent impl or trait declaration.
- `AssocType` — type alias inside an inherent impl or trait declaration.

`Node.parent_id` semantics widen here: previously a parent was always a Workspace/Crate/Module Item; now Method/AssocConst/AssocType items can have an Item parent (the host type or trait). The doc comment on `Node.parent_id` reflects this.

**NodeId scheme.** `NodeId::from_components(&[workspace_hash, kind_label, crate, file_path, byte_offset, name])` where `kind_label ∈ {"method", "assoc_const", "assoc_type"}`. The byte offset is the disambiguator that lets two inherent impls of the same name on the same type produce distinct ids — important for ADTs that re-declare a method across different generic specializations.

**Schema.** `SCHEMA_VERSION = 5` (bumped from 4 in this layer). v4 snapshots stop being reused because `graph_id_for` mixes `SCHEMA_VERSION`.

**Cost.** Inherent-impl walking and trait-item iteration are both cheap by themselves. The real cost lands in Layer 8: `Definition::usages` now runs on roughly 2× more Items, so cold extraction time scales accordingly. On `coding-agent` the binding count grew from 10955 to ~14k Items, with usage extraction time growing proportionally.

**Limitations.**

- Trait impl method *bodies* aren't emitted as Items (Layer 4c, deferred). Calls like `<Foo as Trait>::method()` and `x.method()` resolve back to the trait declaration's def via RA's reference resolver, so a single trait Item suffices for `who_uses` — but a "list every implementation of this trait method" workflow still has no first-class node.
- Inherent impls of types declared in dep crates are skipped — the host ADT isn't in our local `def_to_node` map, so we can't attach the impl items anywhere meaningful in the graph.
- Anonymous consts (`impl Foo { const _: () = ...; }`) are skipped because they have no name to address them by.

## Layer 10 — Call graph (`src/graph/usages.rs`, `queries.rs`, `tools/graph_tools.rs`)

**Goal.** Function-level attribution on every Usage record, so queries can answer "which fn inside which module made this reference?" instead of just "which module?". Without this layer, `usages_in` and `who_uses` give a module-level view; users still need to grep inside the module to find the actual call site's enclosing function.

**Algorithm.** One extra `SemanticsScope::containing_function()` call per reference site, keyed back through `def_to_node`:

1. After computing `consumer_module` for a reference (existing Layer 8 logic), pick a syntax node *at* the reference offset — `syntax.token_at_offset(r.range.start()).parent()` — and pass that to a second `sema.scope_at_offset(...)` call. `scope_at_offset` walks the given node's ancestors via `find_container`, so the file-root scope used for module attribution always returns a module-level resolver where `containing_function` is `None`. The deeper node lets `find_container` reach `DefWithBodyId` (the enclosing fn) and the resolver then carries an `ExprScope`.
2. Call `scope.containing_function()`. RA returns `Option<Function>`; convert to `FunctionId` via `ra_ap_hir_def::FunctionId::try_from(f)` (skipping `BuiltinDeriveImplMethod` cases) and look up `def_to_node[ModuleDefId::FunctionId(id)]` to get the caller fn's `NodeId`.
3. Emit on the `Usage` record. `None` is preserved verbatim — no fallback to `expression_store_owner()` in v1.

**Data.** `Usage` gains `consumer_function: Option<NodeId>` (carries `#[serde(default)]` so future schema rolls don't need to touch every record). New `usages_by_consumer_function` sub-DB (DUP_SORT, `NodeId → UsageId`) mirrors `usages_by_consumer` and powers `calls_from`. Schema bumped to **v6** — bincode reads of v5 records reject the missing field as unexpected EOF, so the bump is required even with `#[serde(default)]`. v5/v6 graph_ids are disjoint via `graph_id_for` mixing `SCHEMA_VERSION`.

**Query layer.** Two new methods on `OpenedSnapshot`, both returning `Vec<EnrichedCallSite>` (`{caller_qualified_name, callee_qualified_name, file, start, end, category}`):

- `who_calls(target_fn)` — scans `usages_by_target`, filters to rows where `consumer_function.is_some()`, resolves caller names. References from non-fn scopes (const initializers, type alias bounds, enum variant discriminants) are excluded — see `who_uses` for those.
- `calls_from(caller_fn)` — scans the new `usages_by_consumer_function` sub-DB keyed by the caller fn, dereferences each `UsageId` from `usages_by_id`, resolves callee names.

**MCP tools.** `who_calls(directory, target)` and `calls_from(directory, caller)` ship via the same router shape as `who_uses`. Tool docstrings note both the non-fn-scope exclusion and the closures-attribute-to-parent-fn rule.

**Cost.** ~+5–10% on the usages pass — one extra `scope_at_offset` per reference plus a `try_from` and a hashmap lookup. On a `coding-agent`-sized snapshot the LMDB env grows by roughly +0.5 MB (the `usages_by_consumer_function` sub-DB is sparse: only refs inside fn bodies populate it). No measurable cold-rebuild regression beyond the schema-bump invalidation.

**Limitations.**

- **Const initializers / trait bounds / enum variant discriminants** give `consumer_function = None`. We don't fall back to `expression_store_owner()` to attribute these to the enclosing const/static/etc. in v1 — a follow-up could surface them as a `ConsumerOwner` enum but it'd touch every existing query.
- **Closures** attribute to the parent fn — RA's default for `SemanticsScope::containing_function`. Users expecting a separate "closure call site" entry will see the call attributed to the outer function instead.
- **No recursive `call_graph` tool yet** — this layer only ships the two direct queries. Recursive BFS callers (`call_graph(target, depth)`), within-crate filtering (`callers_in_crate`), and `recursive_callers_count` are deferred to a follow-up; they're additive on top of the new `usages_by_consumer_function` index.

## What's not done (deliberately, v1 scope)

- Trait impl method bodies (`impl T for Foo { fn m() {...} }`) as first-class `Item` nodes (Layer 4c). Adds duplicates for the trait-decl Item and was deferred in v1.
- Recursive call-graph tools — `call_graph(target, depth)` (BFS callers), `callers_in_crate(target, crate)`, `recursive_callers_count(target)`. The Layer 10 `usages_by_consumer_function` index already supports them; only the query/tool surface is missing.
- `ItemId` parity with `BindingId` (currently Items are addressed only via `NodeId`, and there's no separate per-item identity for use-cases like cross-snapshot diffing).
- Pagination on large query results (`who_imports`, `who_uses` on a hot symbol can return thousands of rows).
- Macros, block-local `use`, proc-macro / build-script expansion.

Each is additive — none requires reshaping any existing layer.

## How to extend

- **New node kind**: extend `NodeKind` and `model.rs`. Update `node_kind_label` in `graph_tools.rs`. No schema change needed.
- **New binding kind**: extend `BindingKind` and the classifier in `bindings.rs`. Add the label in `graph_tools.rs::binding_kind_label`. No schema change needed.
- **New index**: declare in `GraphDatabases::create` and `::open`, populate in `snapshot::write_model`, query in `queries.rs`. Bump `SCHEMA_VERSION` if existing snapshots can't be read with the new layout.
- **New MCP tool**: add a param struct in `search_tool.rs`, an `async fn` in `graph_tools.rs`, a `#[tool]` route in `search_tool_router.rs`, mention in the `instructions` string.

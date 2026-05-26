# Plan: `crate_skeleton` for the refactored workspace

Status: ready to implement after review. Written against
`/home/molaco/Documents/rust-code-mcp-refactor` on 2026-05-26.

This plan supersedes the single-crate `strip-code-plan.md` shape for this
refactored workspace. The current codebase is split into `rmc-graph` and
`rmc-server`, and the implementation should follow that boundary.

## Goal

Add an MCP tool named `crate_skeleton` that writes a stripped Rust facade tree
under the project root at `.skeleton/`. The generated files mirror the regular
codebase's Rust source layout: for a source file such as
`crates/rmc-graph/src/graph/model.rs`, the skeleton file is written to
`.skeleton/crates/rmc-graph/src/graph/model.rs`.

Each generated `.rs` file keeps the items that belong to the matching source
file: attributes, doc comments, type declarations, trait declarations,
functions, methods, associated items, const/static/type declarations, and
declared re-exports. Function bodies and value initializers are replaced with
placeholders.

The first implementation target is **mirrored file granularity only**:

```text
.skeleton/
  crates/
    rmc-graph/
      src/
        graph/
          model.rs
          snapshot.rs
          query/
            modules.rs
    rmc-server/
      src/
        tools/
          graph/
            core.rs
```

The MCP response is only a summary of what was written. It does not return the
full skeleton text inline.

## Current Codebase Findings

The repo already has most of the data needed, but not all of it in the form the
original plan assumed.

- `rmc-graph` owns snapshot data and graph-aware algorithms:
  - `crates/rmc-graph/src/graph/model.rs`
  - `crates/rmc-graph/src/graph/query/*`
  - `crates/rmc-graph/src/graph/codemap/*`
- `rmc-server` owns MCP params, tool registration, endpoint glue, and file IO:
  - `crates/rmc-server/src/tools/params/graph.rs`
  - `crates/rmc-server/src/tools/graph/*`
  - `crates/rmc-server/src/tools/router.rs`
- `Node.file` and `Node.span` are already backfilled for local items in
  `crates/rmc-graph/src/graph/usages.rs` before usages are extracted.
- Attributes and doc comments are already captured in `Node.attributes` by
  `crates/rmc-graph/src/graph/attributes.rs`.
- Function signatures and static metadata are already persisted:
  - `signatures_by_target`
  - `static_metadata_by_target`
- Enum variants are already Item nodes parented to the enum Item.
- Inherent associated items and trait declaration items are Item nodes parented
  to the host type/trait.
- Trait implementation bodies are deliberately **not** extracted by
  `crates/rmc-graph/src/graph/impls.rs`. A v1 skeleton must not promise real
  `impl Trait for Type` blocks.
- Module nodes currently do not carry visibility in `Node.visibility`.
  Module visibility must be recovered from declared `Binding` rows, the same
  way item visibility is recovered for `module_tree`.
- Type generics, where-clauses, struct fields, enum payload fields, type alias
  RHS, const types, ABI, `unsafe`, `const fn`, and other exact declaration
  syntax are **not** fully represented in the snapshot model.
- `compute_fingerprint` in `crates/rmc-graph/src/graph/storage.rs` and
  `codemap::newest_source_mtime` currently skip `target/` and `.git/`, but not
  `.skeleton/`. Since this tool writes generated `.rs` files under the
  workspace root, `.skeleton/` must be excluded from both walkers before the
  endpoint lands.

## Key Design Decision

Do **not** start with a schema bump.

Instead, implement v1 as a hybrid snapshot/source renderer:

1. Use the snapshot for selection, hierarchy, filtering, and stable ordering.
2. Use `Node.file` + `Node.span` to read the original declaration source.
3. Use `ra_ap_syntax` to strip bodies/initializers from the source slice.

This is a better fit for the current codebase than immediately adding
`StructFields`, `VariantPayload`, `ConstMetadata`, and type-generic records:

- It preserves type generics and where-clauses exactly.
- It preserves struct fields and enum payloads exactly.
- It preserves type alias RHS exactly.
- It avoids schema version churn for the first landing.
- `ra_ap_syntax` is already a workspace dependency of `rmc-graph`.
- The approach matches the existing codemap precedent of reading source files
  at query time for snippets and freshness diagnostics.

The tradeoff is that the renderer depends on source files still existing and
matching the snapshot. Handle that with a freshness diagnostic and graceful
fallback rendering.

## MCP-Verified Reuse Audit

Verified against the current codebase with the `rust-code-mcp-refactor` tools
(`build_hypergraph`, `module_tree`, `find_definition`, `search`,
`get_declared_reexports`, `get_exports`, and `get_similar_code`).

Reuse requirements:

- Reuse `OpenedSnapshot` and its existing read-side data/helpers:
  - `find_root_module_of`
  - `lookup_by_qualified_name`
  - `node`
  - `declared_reexports_of`
  - `bindings_for_from_module`
  - `bindings_for_target`
  - `children_by_parent`
- Move shared visibility logic instead of copying it. The current
  `query/modules.rs` helpers `declared_item_visibility_map` and
  `format_binding_visibility` should become shared graph-query helpers, and
  `module_tree` should call the shared helper too.
- Move shared crate-scope logic instead of copying it. The target-kind/vendor
  selection logic currently embedded in `query/overlaps.rs` should become a
  shared helper that both `overlaps_with_scope` and skeleton collection use.
  `exclude_vendor=true` maps to `OverlapScope::LocalNoVendor`; false maps to
  `OverlapScope::Local`.
- Reuse the codemap freshness infrastructure. Do not add a second
  `newest_source_mtime` walker. Instead, extend the shared workspace-walk
  exclusion rule so both `compute_fingerprint` and
  `codemap::newest_source_mtime` skip `.skeleton/`.
- Reuse server graph response helpers:
  - `open_workspace_snapshot`
  - `json_result`
  - `internal_error`
  - the existing `tokio::task::spawn_blocking` pattern from `core.rs` and
    `audits.rs`
- Reuse existing persisted surface data:
  - `Node.attributes`
  - `FunctionSignature`
  - `StaticMetadata`
  - `Binding` visibility/re-export information
- Do not load a full `LoadedWorkspace` or `Semantics` stack for v1 skeleton
  rendering. Existing audit tools use that for semantic resolution; skeleton
  only needs syntax-level body stripping from `Node.file` + `Node.span`.
- Keep `skeleton::source::SourceCache` local in v1 because existing caches are
  snippet-specific (`codemap::extract_snippet`) or audit-specific (`FileId`
  text caches). If a later feature needs the same parsed-source cache, extract
  it into a shared graph module then.

Generated-output hygiene:

- Add `/.skeleton/` to `.gitignore`.
- Add a shared workspace-walk exclusion helper for `target/`, `.git/`, and
  `.skeleton/`.
- Use that helper from `compute_fingerprint` and `codemap::newest_source_mtime`.
- Add tests proving `.skeleton/*.rs` does not change the graph fingerprint and
  does not trigger stale-source diagnostics.

## Tool Surface

MCP params in `crates/rmc-server/src/tools/params/graph.rs`:

```rust
pub(crate) struct CrateSkeletonParams {
    pub directory: String,
    #[serde(default)]
    pub crates: Option<Vec<String>>,
    #[serde(default)]
    pub include: Option<Vec<String>>,
    #[serde(default)]
    pub include_docs: Option<bool>,
    #[serde(default)]
    pub include_attrs: Option<bool>,
    #[serde(default)]
    pub include_impls: Option<bool>,
    #[serde(default)]
    pub include_reexports: Option<bool>,
    #[serde(default)]
    pub exclude_tests: Option<bool>,
    #[serde(default)]
    pub exclude_vendor: Option<bool>,
    #[serde(default)]
    pub clean: Option<bool>,
}
```

Defaults:

- `crates = None` means all selected local crates.
- `include = ["pub", "pub(crate)"]`.
- `include_docs = true`.
- `include_attrs = true`.
- `include_impls = true`.
- `include_reexports = true`.
- `exclude_tests = true`.
- `exclude_vendor = true`.
- `clean = true`, meaning the tool removes the existing `<directory>/.skeleton`
  tree before writing the fresh mirror. The implementation must only ever remove
  that exact hidden generated directory.

Supported `include` values:

- `"pub"`: only pure public items/modules.
- `"pub(crate)"`: crate-visible items/modules.
- `"restricted"`: `pub(super)` / `pub(in ...)` style restricted visibility.
- `"private"`: private module-local declarations.
- `"all"`: all visibility buckets.

Response:

```rust
pub(crate) struct CrateSkeletonResponse {
    pub skeleton_dir: String,
    pub snapshot_id: String,
    pub files_written: Vec<CrateSkeletonFileSummary>,
    pub total_files: usize,
    pub total_items: usize,
    pub total_bytes: usize,
    pub diagnostics: Vec<String>,
}

pub(crate) struct CrateSkeletonFileSummary {
    pub crate_name: String,
    pub source_path: String,
    pub skeleton_path: String,
    pub bytes: usize,
    pub items: usize,
}
```

## Target Module Layout

`rmc-graph`:

```text
crates/rmc-graph/src/graph/skeleton/
  mod.rs       # public graph API for skeleton rendering
  model.rs     # SkeletonOptions, SkeletonOutput, SkeletonFile, diagnostics
  collect.rs   # crate/module/item collection from OpenedSnapshot
  source.rs    # source file cache + ra_ap_syntax body stripping
  render.rs    # Rust facade renderer
```

`crates/rmc-graph/src/graph/mod.rs` re-exports only the stable public types and
entrypoint needed by `rmc-server`.

`rmc-server`:

```text
crates/rmc-server/src/tools/graph/skeleton.rs
```

`crates/rmc-server/src/tools/graph/mod.rs` adds `pub(super) mod skeleton;`.

## Rendering Model

The renderer should not produce one giant file. It should build a
skeleton-specific tree from `children_by_parent`, then bucket retained items by
their `Node.file` so each output file mirrors the source file that declared the
items.

Do not use `ModuleTreeNode` directly. It lacks NodeIds, files, spans, and module
visibility.

```rust
struct SkeletonTreeNode {
    id: NodeId,
    node: Node,
    visibility: Option<String>,
    modules: Vec<SkeletonTreeNode>,
    items: Vec<SkeletonItem>,
}

struct SkeletonItem {
    id: NodeId,
    node: Node,
    visibility: Option<String>,
    rendered_source: Option<String>,
}
```

Then project the retained tree into per-file buckets:

```rust
struct SkeletonSourceFile {
    crate_name: String,
    source_path: String,
    skeleton_path: String,
    modules: Vec<SkeletonModuleInFile>,
    items: Vec<SkeletonItem>,
    synthetic_impls: Vec<SkeletonImplBlock>,
    reexports: Vec<SkeletonReexport>,
}
```

Important traversal rule:

- Module nodes recurse into module children.
- Item nodes are rendered as items.
- Item children are not rendered as top-level module children.

That avoids duplicating enum variants and trait-associated items, because enum
source already contains variants and trait source already contains associated
items.

File layout rule:

- `Node.file = "crates/rmc-graph/src/graph/model.rs"` maps to
  `.skeleton/crates/rmc-graph/src/graph/model.rs`.
- Only Rust files represented by retained items/modules are generated.
- Non-Rust files are not copied.
- Deleted/stale skeleton files are removed when `clean=true`.
- Inline modules stay in the same mirrored file as their source.
- File modules produce normal skeleton content in their own mirrored `.rs`
  files.

For inherent associated items, `parent_id` points at the host type item. These
items are not inside the type declaration source, so render them in synthetic
inherent impl blocks when `include_impls=true`:

```rust
impl TypeName {
    pub fn method(&self) { /* ... */ }
}
```

Do not attempt to reconstruct original impl generics, where-clauses, trait
impls, negative impls, unsafe impls, or blanket impls in v1.

## Source Stripping Rules

Use `ra_ap_syntax` in `rmc-graph`, not `syn`, so no new dependency is needed.

For a node with `file` and `span`:

1. Read the workspace-relative source file from `snap.manifest.workspace_root`.
2. Parse the full file with `SourceFile::parse`.
3. Convert `Node.span` to `TextRange`.
4. Find the smallest descendant syntax node matching the expected item kind and
   covering the span. Exact equality is preferred; covering range is a fallback
   for attrs/doc full ranges.
5. Render the item source with replacements applied from the inside out.

Body/initializer replacements:

- `ast::Fn::body()`:
  - If present, replace the block range with `{ /* ... */ }`.
  - If absent and the fn has a semicolon, keep it as a declaration.
- `ast::Const::body()`:
  - Replace initializer expression with `/* ... */`.
  - Keep the declared type and semicolon.
- `ast::Static::body()`:
  - Replace initializer expression with `/* ... */`.
- Descendant functions inside trait bodies:
  - Strip default bodies.
  - Keep pure trait method declarations ending in `;`.
- Type aliases:
  - Keep exact RHS. The RHS is API surface and has no function body.
- Structs, enums, unions:
  - Keep exact source after attr/doc filtering. This preserves generics,
    fields, tuple payloads, discriminants, and where-clauses.

Attribute/doc filtering:

- Strip leading outer attrs and doc comments from the source slice.
- Re-emit entries from `Node.attributes` based on:
  - `include_docs`
  - `include_attrs`
- This avoids duplicating attrs and allows options to work consistently.

Fallback rendering:

- If source is missing, stale, unparsable, or range lookup fails, emit a
  synthetic declaration from snapshot data and add a diagnostic.
- Fallback functions use `FunctionSignature`.
- Fallback statics use `StaticMetadata`.
- Fallback type declarations use placeholders:
  - `pub struct Foo { /* fields unavailable */ }`
  - `pub enum Foo { /* variants unavailable */ }`
  - `pub type Foo = /* ... */;`

## Phase 0: Preflight

Purpose: confirm no local changes and preserve project rules.

Steps:

1. Check instructions:

   ```sh
   sed -n '1,180p' AGENTS.md
   ```

2. Check VCS with jujutsu first:

   ```sh
   jj status
   ```

3. Do not run `cargo fmt` or any formatting command.

4. All build/check commands must be wrapped:

   ```sh
   nix develop ../nix-devshells#cuda-code --command <command>
   ```

Exit gate:

- Working copy state is understood.
- No unrelated changes are touched.

## Phase 1: Graph Skeleton Core With Stub Output

Purpose: add the `rmc-graph` API and collector without source stripping, while
first making `.skeleton/` a proper generated directory that existing graph
infrastructure ignores.

Files:

- `.gitignore`
- `crates/rmc-graph/src/graph/storage.rs`
- `crates/rmc-graph/src/graph/codemap/build.rs`
- `crates/rmc-graph/src/graph/skeleton/mod.rs`
- `crates/rmc-graph/src/graph/skeleton/model.rs`
- `crates/rmc-graph/src/graph/skeleton/collect.rs`
- `crates/rmc-graph/src/graph/skeleton/render.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-graph/src/graph/query/shared.rs`
- `crates/rmc-graph/src/graph/query/modules.rs`
- `crates/rmc-graph/src/graph/query/overlaps.rs`

Implementation steps:

1. Add generated-directory hygiene before the endpoint exists:

   - Add `/.skeleton/` to `.gitignore`.
   - Add a shared workspace-walk exclusion helper in `storage.rs`, scoped for
     graph-internal reuse, covering `target/`, `.git/`, and `.skeleton/`.
   - Replace the ad hoc component filters in `compute_fingerprint` and
     `codemap::newest_source_mtime` with that helper.
   - Add `fingerprint_stable_when_skeleton_dir_grows`.
   - Add `newest_source_mtime_skips_skeleton_dir`.

   This avoids generated skeleton files becoming graph input on the next
   `build_hypergraph` run and avoids false stale-source warnings.

2. Add `mod skeleton;` to `graph/mod.rs`.

3. Add public graph-facing types:

   ```rust
   pub struct SkeletonOptions { ... }
   pub struct SkeletonOutput { ... }
   pub struct SkeletonFile { ... }
   pub struct SkeletonDiagnostic { ... }
   pub fn render_crate_skeletons(
       snap: &OpenedSnapshot,
       opts: &SkeletonOptions,
   ) -> anyhow::Result<SkeletonOutput>
   ```

4. Generalize the visibility helper currently private to `query/modules.rs`.
   Move the implementation; do not copy it.

   Current helper:

   - `declared_item_visibility_map`
   - `format_binding_visibility`

   New shared shape:

   ```rust
   pub(in crate::graph) fn declared_visibility_map(
       snap: &OpenedSnapshot,
       rtxn: &RoTxn<'_, heed::WithoutTls>,
       target_ids: &HashSet<NodeId>,
   ) -> Result<HashMap<NodeId, String>>
   ```

   It must work for both Item and Module targets. This is required because
   module `Node.visibility` is currently `None`.

5. Generalize crate selection currently embedded in `query/overlaps.rs`.

   Current logic:

   - crate target kind defaults from `Node.crate_target_kind`
   - local targets are `lib` and `bin`
   - vendor crates are detected from any local node whose file starts with
     `vendor/`
   - `OverlapScope::LocalNoVendor` excludes those vendor crates

   New shared shape:

   ```rust
   pub(in crate::graph) fn crate_scope_allows(
       scope: OverlapScope,
       crate_id: NodeId,
       crate_target_kind_for: &HashMap<NodeId, String>,
       vendor_crates: &HashSet<NodeId>,
   ) -> bool
   ```

   `overlaps_with_scope` and skeleton collection both use this helper so the
   definition of "local crate" cannot drift.

6. Collector logic:

   - Scan `nodes_by_id` for crate nodes.
   - Keep only local target kinds `lib` and `bin` by default.
   - If `exclude_vendor=true`, use the shared crate-scope helper with
     `OverlapScope::LocalNoVendor`.
   - If `exclude_vendor=false`, use the shared crate-scope helper with
     `OverlapScope::Local`.
   - Resolve the root module with `find_root_module_of`.
   - Walk `children_by_parent`.
   - Attach declared visibility for modules and items.
   - Apply visibility and test filters.
   - Prune empty modules after filtering.

7. Stub renderer:

   - Emit one `SkeletonFile` per mirrored source path, not one per crate.
   - Prefix each generated file with a short banner naming the source file,
     crate, snapshot, and active filters.
   - Emit module context only where needed to keep the file parseable.
   - Emit one placeholder comment per retained item in that source file:

     ```rust
     // item: rmc_graph::graph::model::Node [Struct]
     ```

8. Add unit tests for:

   - `include=["pub"]` excludes `pub(crate)` and private items.
   - `include=["all"]` keeps private items.
   - `exclude_tests=true` prunes `::tests::`.
   - Module visibility is recovered from bindings.
   - The shared crate-scope helper matches existing overlaps behavior for
     local, example, and vendor crates.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib
```

Optional focused tests:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
```

## Phase 2: Source-Slice Renderer

Purpose: replace placeholders with stripped Rust declarations using existing
source ranges.

Files:

- `crates/rmc-graph/src/graph/skeleton/source.rs`
- `crates/rmc-graph/src/graph/skeleton/render.rs`
- `crates/rmc-graph/src/graph/skeleton/model.rs`

Implementation steps:

1. Add `SourceCache`:

   - keyed by workspace-relative file path
   - stores source text and parsed `SourceFile`
   - records diagnostics for read/parse failures
   - uses `snap.manifest.workspace_root` + `Node.file`; do not add a second
     VFS or `LoadedWorkspace` path
   - remains skeleton-local in v1 because it stores parsed full files and AST
     lookup state, unlike the existing codemap snippet cache

2. Add range lookup:

   - Convert `Node.span` to `ra_ap_syntax::TextRange`.
   - Find the expected `ast` node type for the item kind.
   - Prefer exact range, then smallest covering range.

3. Add body stripping:

   - Apply replacements in descending range order.
   - Strip nested fn bodies inside trait declarations.
   - Strip const/static initializer expressions.

4. Add attr/doc filtering:

   - Remove leading outer attrs/docs from source item text.
   - Re-emit filtered `Node.attributes`.
   - Do not re-extract attrs from source unless `Node.attributes` is missing;
     the build-time `attributes.rs` pass is the source of truth.

5. Render module blocks:

   - Each mirrored file renders the items declared in the matching source
     file.
   - Inline modules render as `{vis} mod {name} { ... }` inside that same
     file.
   - File modules do not get wrapped in parent module blocks in their own file;
     their path already supplies the context.
   - Convert `visibility == "pub(self)"` to no prefix.
   - Keep `pub`, `pub(crate)`, and `pub(in path)` as emitted visibility.

6. Render direct module items:

   - Functions: stripped source.
   - Structs/enums/unions/traits/type aliases/consts/statics: stripped source.
   - Skip enum variant child nodes as standalone items.
   - Skip trait associated child nodes as standalone module items.

7. Render synthetic inherent impl blocks:

   - Group `Method`, `AssocConst`, and `AssocType` children by parent type.
   - Include only groups whose host type is retained by filters.
   - Sort methods by `(file, span.start, qualified_name)`.
   - Render:

     ```rust
     impl TypeName {
         ...
     }
     ```

   - Add a single comment before synthetic impls in each mirrored file:

     ```rust
     // inherent impl facades; original impl generics/where clauses are not reconstructed
     ```

8. Render declared re-exports:

   - For each module, call `declared_reexports_of`.
   - Write each rendered re-export to the mirrored file that owns the module.
   - Filter by re-export binding visibility.
   - Render simple canonical paths:

     ```rust
     pub use crate::graph::loader::load;
     pub(crate) use crate::foo::Bar as Baz;
     ```

   - Do not try to reproduce original use-tree grouping in v1.
   - Do not add a second exports/re-exports query path; use the existing
     `Binding` output and enrichment conventions where possible.

Tests:

- A function with a body renders with `{ /* ... */ }`.
- A trait method without a default body keeps `;`.
- A trait method with a default body gets `{ /* ... */ }`.
- A struct with generics and fields is preserved.
- An enum with tuple/record variants is preserved.
- A const/static initializer is replaced.
- Attribute/doc toggles work.
- Each generated mirrored file parses with `ra_ap_syntax::SourceFile::parse`.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-graph --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
```

## Phase 3: MCP Endpoint And File Writing

Purpose: expose the renderer as an MCP tool and write output files.

Files:

- `crates/rmc-server/src/tools/params/graph.rs`
- `crates/rmc-server/src/tools/graph/skeleton.rs`
- `crates/rmc-server/src/tools/graph/mod.rs`
- `crates/rmc-server/src/tools/router.rs`

Implementation steps:

1. Add `CrateSkeletonParams`.

2. Add endpoint module:

   ```rust
   pub(crate) async fn crate_skeleton(
       params: CrateSkeletonParams,
   ) -> Result<CallToolResult, McpError>
   ```

3. Endpoint behavior:

   - Open snapshot with `open_workspace_snapshot`.
   - Serialize responses with `json_result`.
   - Map graph errors with `internal_error` unless they are user parameter
     errors.
   - Canonicalize `directory`.
   - Set `skeleton_dir = <canonical directory>/.skeleton`.
   - If `clean=true`, remove only `skeleton_dir` after verifying the final path
     is exactly a `.skeleton` child of the workspace root.
   - Call `rmc_graph::graph::render_crate_skeletons`.
   - Create parent directories under `.skeleton` for every mirrored source
     path.
   - Write one file per `SkeletonFile`.
   - Return `CrateSkeletonResponse`.

4. Use `tokio::task::spawn_blocking` around the synchronous render + file IO.
   This follows the `build_hypergraph` and graph-audit endpoint patterns and
   avoids blocking the async runtime worker.

5. Add router method in `tools/router.rs` near the structure/surface tools:

   ```rust
   #[tool(description = "...")]
   async fn crate_skeleton(...)
   ```

6. Add endpoint tests in `crates/rmc-server/src/tools/graph/tests.rs`:

   - Build/open a snapshot.
   - Run the endpoint against a temp fixture workspace.
   - Assert response has at least one file.
   - Assert files are written under `<fixture>/.skeleton`.
   - Assert a generated file mirrors a real source-relative path.
   - Assert the file exists and contains a banner.
   - Assert `clean=true` removes stale files under `.skeleton`.
   - Assert a sibling directory with a similar name, such as
     `.skeleton-backup`, is never removed.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib crate_skeleton
```

## Phase 4: Docs And User-Facing Contract

Purpose: document the tool honestly and add it to the public tool list.

Files:

- `TOOLS.md`
- `README.md`

Implementation steps:

1. Add `crate_skeleton` to the TOOLS overview table under Graph: Structure or
   Graph: Signatures/API Surface.

2. Add a full section with:

   - parameters table
   - defaults
   - example invocation
   - response shape
   - notes and limitations

3. Add one README row/category mention.

Required limitation notes:

- Files are always written under `<workspace>/.skeleton/` with source-relative
  paths mirrored from the real codebase.
- `.skeleton/` is generated output: it is git-ignored and excluded from graph
  fingerprint/staleness source walks.
- Output is intended to be parseable Rust-like facade source, not type-checking
  source.
- Trait impl blocks are not reconstructed in v1.
- Synthetic inherent impl blocks do not preserve original impl generics or
  where-clauses.
- Re-export use-tree grouping is normalized, not source-exact.
- Output is selected from the snapshot but declaration text is read from source;
  stale snapshots can produce diagnostics.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check -p rmc-server --lib
```

## Phase 5: End-To-End Quality Pass

Purpose: harden parseability and deterministic output.

Implementation steps:

1. Add deterministic ordering assertions:

   - crates by crate name
   - modules by qualified name when no source span exists
   - items by `(file, span.start, qualified_name)`
   - synthetic impl members by `(file, span.start, qualified_name)`
   - re-exports by `(visibility, rendered_path, alias)`

2. Add parse checks:

   - Use `ra_ap_syntax::SourceFile::parse` on every generated mirrored file.
   - Assert no parse errors for a fixture/simple self-snapshot.

3. Add diagnostics checks:

   - Missing source file produces a diagnostic but does not abort the whole
     render.
   - Missing `Node.span` falls back to synthetic rendering.

4. Manual smoke:

   - Build current workspace snapshot.
   - Run `crate_skeleton`.
   - Inspect `.skeleton/crates/rmc-graph/src/graph/model.rs` and
     `.skeleton/crates/rmc-server/src/tools/router.rs`.

Validation:

```sh
nix develop ../nix-devshells#cuda-code --command cargo check --workspace --lib
```

If running tests for this feature:

```sh
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-graph --lib skeleton
nix develop ../nix-devshells#cuda-code --command cargo test -p rmc-server --lib crate_skeleton
```

## Phase 6: Optional Skeleton Manifest

Do this only after the mirrored `.skeleton/` tree is useful.

Goal:

```text
.skeleton/
  manifest.json
  crates/
    rmc-graph/
      src/
        graph/
          model.rs
```

Implementation notes:

- `manifest.json` records snapshot id, generation time, filters, source root,
  file count, item count, and diagnostics.
- Keep the MCP response shape as the authoritative immediate result; the
  manifest is for humans and later tools that read `.skeleton/` directly.
- Do not add non-Rust source copies.

Validation:

- Manifest paths match the generated file list.
- Re-running with identical snapshot/options produces stable generated Rust
  file bytes; only manifest generation timestamp is allowed to differ.

## Phase 7: Optional Pure-Snapshot Metadata

This is deferred. Add it only if query-time source IO/staleness becomes a real
problem.

Possible schema v13 additions:

```rust
pub struct DeclarationHeader {
    pub header: String,
    pub has_body: bool,
}

pub struct FieldRecord {
    pub name: Option<String>,
    pub ty: String,
    pub vis: Option<String>,
}

pub struct StructFields {
    pub shape: StructShape,
    pub fields: Vec<FieldRecord>,
}

pub enum StructShape {
    Unit,
    Tuple,
    Record,
}

pub struct VariantPayload {
    pub shape: StructShape,
    pub fields: Vec<FieldRecord>,
    pub discriminant: Option<String>,
}
```

Relevant RA HIR APIs verified locally in `ra_ap_hir 0.0.330`:

- `Struct::fields(db)`
- `Struct::kind(db)`
- `Union::fields(db)`
- `EnumVariant::fields(db)`
- `EnumVariant::kind(db)`
- `Field::name(db)`
- `Field::ty(db).to_type(db).display(db, dt)`
- `Field::visibility(db)`
- `Const::ty(db)`
- `Static::ty(db)`
- `TypeAlias::ty(db)`

This phase would touch:

- `crates/rmc-graph/src/graph/model.rs`
- new or extended extraction pass near `signatures.rs` / `statics.rs`
- `crates/rmc-graph/src/graph/storage.rs` with `SCHEMA_VERSION += 1`
- `crates/rmc-graph/src/graph/snapshot.rs`
- query accessors under `crates/rmc-graph/src/graph/query/*`

Do not mix this schema bump into the first MCP landing.

## Risk Register

1. **Module visibility is missing from Module nodes.**
   Mitigation: generalize declared visibility lookup from item-only to
   module/item targets.

2. **Trait impl blocks are absent by design.**
   Mitigation: do not render them in v1; document the limitation.

3. **Synthetic inherent impls lose original impl generics/where-clauses.**
   Mitigation: mark them as facades and keep the goal parseability, not
   typeability.

4. **Source can be newer than the snapshot.**
   Mitigation: reuse the codemap-style newest `.rs` mtime check and emit a
   diagnostic suggesting `build_hypergraph(force_rebuild=true)`.

5. **Generated `.skeleton/*.rs` files can perturb graph fingerprints or stale
   source diagnostics.**
   Mitigation: add a shared workspace-walk exclusion helper and make both
   `compute_fingerprint` and `codemap::newest_source_mtime` skip `.skeleton/`;
   add regression tests before the MCP endpoint writes files.

6. **Raw source attrs/docs conflict with option toggles.**
   Mitigation: strip leading attrs/docs and re-emit filtered `Node.attributes`.

7. **Generated output can be syntactically valid but semantically invalid.**
   Mitigation: document that the output is for API/context reading and parsing,
   not compiling.

## Suggested PR Slicing

1. `rmc-graph`: generated-dir hygiene + skeleton collector + stub renderer.
2. `rmc-graph`: source-slice stripping renderer.
3. `rmc-server`: MCP endpoint + file writing.
4. Docs/tests polish.
5. Optional `.skeleton/manifest.json`.
6. Optional schema v13 metadata.

Each PR should end with the relevant nix-wrapped `cargo check` command. Never
run `cargo fmt`.

## Change Impact Estimate

These are rough implementation-size estimates, not exact counts. They exist to
help review and PR slicing.

| Phase | New | Modified | Deleted | Prod LOC | Test LOC |
|---|---:|---:|---:|---:|---:|
| 0 Preflight | 0 | 0 | 0 | 0 | 0 |
| 1 Graph collector + stub renderer | 1 dir, 4 files | 7 files | 0 | ~800-1050 | ~320-520 |
| 2 Source stripping renderer | 1 file | 2-3 files | 0 | ~600-900 | ~350-550 |
| 3 MCP endpoint + `.skeleton/` writing | 1 file | 4 files | 0 | ~250-400 | ~150-250 |
| 4 Docs | 0 | 2 files | 0 | 0 | 0 |
| 5 Quality pass | 0 | 2-4 files | 0 | ~100-200 | ~250-450 |
| 6 Optional manifest | 0-1 file | 2-3 files | 0 | ~150-250 | ~100-200 |
| 7 Optional schema metadata | 1 file maybe | 5-7 files | 0 | ~600-1000 | ~300-600 |

### Phase 1 Impact

New directory:

- `crates/rmc-graph/src/graph/skeleton/`

New files/modules:

- `crates/rmc-graph/src/graph/skeleton/mod.rs`
- `crates/rmc-graph/src/graph/skeleton/model.rs`
- `crates/rmc-graph/src/graph/skeleton/collect.rs`
- `crates/rmc-graph/src/graph/skeleton/render.rs`

Modified files:

- `.gitignore`
- `crates/rmc-graph/src/graph/storage.rs`
- `crates/rmc-graph/src/graph/codemap/build.rs`
- `crates/rmc-graph/src/graph/mod.rs`
- `crates/rmc-graph/src/graph/query/shared.rs`
- `crates/rmc-graph/src/graph/query/modules.rs`
- `crates/rmc-graph/src/graph/query/overlaps.rs`

New types:

- `SkeletonOptions`
- `SkeletonOutput`
- `SkeletonFile`
- `SkeletonDiagnostic`
- `SkeletonTreeNode`
- `SkeletonItem`
- `SkeletonSourceFile`
- visibility/filter helper enums

New functions:

- `render_crate_skeletons`
- collector/tree-walk helpers
- visibility filter helpers
- stub file renderer
- generalized `declared_visibility_map`
- shared workspace-walk exclusion helper for `target/`, `.git/`, `.skeleton/`
- shared crate-scope/vendor helper reused by overlaps and skeleton collection

### Phase 2 Impact

New file/module:

- `crates/rmc-graph/src/graph/skeleton/source.rs`

Modified files:

- `crates/rmc-graph/src/graph/skeleton/render.rs`
- `crates/rmc-graph/src/graph/skeleton/model.rs`
- possibly `crates/rmc-graph/src/graph/skeleton/mod.rs`

New types:

- `SourceCache`
- `RenderedDecl`
- `Replacement`
- source/range diagnostic variants

New functions:

- source file loading/cache helpers
- syntax-node range lookup
- body/initializer stripping
- attr/doc filtering
- fallback declaration rendering
- synthetic inherent impl rendering
- re-export rendering

### Phase 3 Impact

New file/module:

- `crates/rmc-server/src/tools/graph/skeleton.rs`

Modified files:

- `crates/rmc-server/src/tools/params/graph.rs`
- `crates/rmc-server/src/tools/graph/mod.rs`
- `crates/rmc-server/src/tools/router.rs`
- `crates/rmc-server/src/tools/graph/tests.rs`

New types:

- `CrateSkeletonParams`
- `CrateSkeletonResponse`
- `CrateSkeletonFileSummary`

New functions/methods:

- `tools::graph::skeleton::crate_skeleton`
- router method `crate_skeleton`
- `.skeleton/` clean/write helpers

### Phase 4 Impact

Modified docs:

- `TOOLS.md`
- `README.md`

No production or test Rust LOC is expected unless examples are added.

### Phase 5 Impact

Likely modified files:

- `crates/rmc-graph/src/graph/skeleton/*`
- `crates/rmc-server/src/tools/graph/tests.rs`

Likely additions:

- deterministic ordering assertions
- parse checks for every generated mirrored file
- stale/missing source diagnostics
- self-workspace or fixture smoke tests

No new modules are expected.

### Phase 6 Impact

Possible new file/module:

- `crates/rmc-graph/src/graph/skeleton/manifest.rs`

Possible modified files:

- `crates/rmc-graph/src/graph/skeleton/model.rs`
- `crates/rmc-server/src/tools/graph/skeleton.rs`
- docs/tests

New types:

- `SkeletonManifest`
- `SkeletonManifestFile`

### Phase 7 Impact

Possible new file/module:

- `crates/rmc-graph/src/graph/declarations.rs`
- or `crates/rmc-graph/src/graph/field_types.rs`

Modified files:

- `crates/rmc-graph/src/graph/model.rs`
- `crates/rmc-graph/src/graph/extract.rs`
- `crates/rmc-graph/src/graph/storage.rs`
- `crates/rmc-graph/src/graph/snapshot.rs`
- query accessor module(s) under `crates/rmc-graph/src/graph/query/`
- `crates/rmc-graph/src/graph/mod.rs`
- tests

New persisted types:

- `DeclarationHeader`
- `FieldRecord`
- `StructFields`
- `StructShape`
- `VariantPayload`
- possible `ConstMetadata`
- possible `TypeAliasMetadata`

This is the expensive phase and should stay deferred unless source-based
rendering proves insufficient.
